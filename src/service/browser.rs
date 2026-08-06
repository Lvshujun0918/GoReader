//! 浏览器自动化（CDP over WebSocket——轻量实现，复用 tokio-tungstenite）。
//! **唯一浏览器后端：obscura**（Rust headless 浏览器，stealth 构建含 BoringSSL
//! TLS 指纹模拟/反检测/追踪器拦截，CDP 兼容——puppeteer/playwright 可连；
//! https://github.com/h4ckf0r0day/obscura）。无 Chrome/Edge fallback。
//!
//! 用于书源登录（mode=browser）：滑块验证码自动拖拽（人类轨迹：贝塞尔曲线 + 随机噪声 +
//! 微停）、图片验证码截图（前端显示后回填）、登录表单自动填写、CDP 提取 cookie 存库；
//! CF 质询/Turnstile 求解（obscura 内置 stealth 指纹 + 本文件 STEALTH_JS 注入双保险）。
//!
//! 后端发现：`READER_OBSCURA_URL`（连接既有 obscura CDP 服务，不接管进程）→
//! `READER_OBSCURA_BIN`（可执行文件路径）→ 本程序同目录 → 系统 PATH；找到后
//! spawn `obscura serve --port <随机> --stealth`。找不到则功能禁用
//! （登录回退手动 Cookie 流程，接口报"未安装浏览器"）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, LazyLock};
use std::time::Duration;

use anyhow::{anyhow, Result};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

/// 滑块拖拽后的等待时间（验证结果判定）
pub const CAPTCHA_SETTLE_MS: u64 = 1800;
/// 单步 CDP 命令超时
const CDP_CMD_TIMEOUT: Duration = Duration::from_secs(20);
/// 浏览器启动超时
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(15);

// ==================== 浏览器发现（obscura——唯一后端） ====================

/// obscura 候选路径（`READER_OBSCURA_BIN` 显式指定优先，其次本程序同目录、系统
/// PATH 中的 obscura/obscura.exe）——纯函数，供测试。覆盖场景：Docker 镜像
/// /usr/local/bin 布局、Windows 手工解压目录、cargo install 等
pub fn obscura_bin_candidates() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("READER_OBSCURA_BIN") {
        let p = p.trim();
        if !p.is_empty() {
            v.push(PathBuf::from(p));
        }
    }
    // 本程序可执行文件同目录（如镜像内 /usr/local/bin/reader-dev + obscura 并列）
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            #[cfg(windows)]
            v.push(dir.join("obscura.exe"));
            #[cfg(not(windows))]
            v.push(dir.join("obscura"));
        }
    }
    // 系统 PATH 探测
    if let Ok(path) = std::env::var("PATH") {
        let sep = if cfg!(windows) { ';' } else { ':' };
        for dir in path.split(sep) {
            let dir = dir.trim();
            if dir.is_empty() {
                continue;
            }
            #[cfg(windows)]
            {
                v.push(PathBuf::from(dir).join("obscura.exe"));
                v.push(PathBuf::from(dir).join("obscura"));
            }
            #[cfg(not(windows))]
            v.push(PathBuf::from(dir).join("obscura"));
        }
    }
    v
}

/// 发现可用 obscura 可执行文件（第一个存在的路径）。未找到 → None（功能禁用）
pub fn discover_obscura_bin() -> Option<PathBuf> {
    obscura_bin_candidates().into_iter().find(|p| p.exists())
}

/// 浏览器是否可用（登录接口快速短路用）：`READER_OBSCURA_URL` 已配置 → true
/// （连接失败在 connect 时报错）；否则要求 obscura 可执行文件可发现
pub fn is_browser_available() -> bool {
    if let Ok(u) = std::env::var("READER_OBSCURA_URL") {
        if !u.trim().is_empty() {
            return true;
        }
    }
    discover_obscura_bin().is_some()
}

// ==================== CDP 客户端 ====================

type WsStream = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// CDP 浏览器会话（launch/connect → 命令 → drop 时杀进程；READER_OBSCURA_URL
/// 直连路径 child=None——不接管外部进程生命周期）
pub struct Browser {
    /// spawn 的 obscura 进程（READER_OBSCURA_URL 直连时为 None，Drop 不杀）
    child: Option<Child>,
    sink: futures::stream::SplitSink<WsStream, Message>,
    /// 待响应命令表（reader 任务按 id 路由回 oneshot）——Arc 共享，避免跨 await 持有非 Sync 的 Receiver
    pending: std::sync::Arc<
        std::sync::Mutex<HashMap<u64, tokio::sync::oneshot::Sender<Result<Value, String>>>>,
    >,
    next_id: u64,
    session_id: Option<String>,
    /// iframe 执行上下文缓存（executionContextCreated 事件——frameId → contextId，
    /// Turnstile 等 iframe 内 JS 执行用——obscura/Chrome 通用）
    frame_ctx: std::sync::Arc<std::sync::Mutex<HashMap<String, i64>>>,
    /// WS 关闭信号（P1）：Drop 时发送 → reader 任务退出循环 → stream（接收半）drop
    /// → WebSocket 关闭——不残留悬挂的 reader 任务/连接（连接泄漏防护）
    close_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Drop for Browser {
    fn drop(&mut self) {
        // P1：先发关闭信号（同步 send，不阻塞）——reader 任务收到后立即退出，
        // 接收半随之 drop，WS 对端观察到连接关闭；在途命令的 oneshot 随之断开
        // （发送端 drop → 接收端 RecvError，调用方立即得到"连接已关闭"错误）
        if let Some(tx) = self.close_tx.take() {
            let _ = tx.send(());
        }
        // obscura serve 单进程（--workers 1 默认）——kill 句柄即清理；无临时目录需回收
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// CDP 端点 URL 规范化：http(s):// → ws(s)://（无路径时补 /devtools/browser——
/// Playwright connectOverCDP 的 endpointURL 语义）；ws(s):// 原样返回。纯函数，供测试
fn normalize_cdp_url(url: &str) -> String {
    let url = url.trim();
    let (rest, secure) = if let Some(rest) = url.strip_prefix("https://") {
        (rest, true)
    } else if let Some(rest) = url.strip_prefix("http://") {
        (rest, false)
    } else {
        return url.to_string();
    };
    let rest = rest.trim_end_matches('/');
    let scheme = if secure { "wss://" } else { "ws://" };
    if rest.contains('/') {
        format!("{scheme}{rest}")
    } else {
        format!("{scheme}{rest}/devtools/browser")
    }
}

/// spawn `obscura serve --port <port> --stealth [--proxy <proxy>]` → 等待 stdout banner
/// （`CDP server: ws://127.0.0.1:{port}/devtools/browser`——serve 的 --quiet 只关日志，
/// banner 无条件打印）→ 连接 → 会话初始化。任何失败均杀进程并返回错误
/// （launch_with 换随机端口重试）。
///
/// 参数说明：obscura 为纯 headless 引擎（**无 --headless 参数**——headless 是其固有
/// 形态）；`--stealth` 启用反检测 + BoringSSL TLS 指纹模拟（stealth 构建；lean 构建
/// 传该参数仅打警告、其余功能正常）；**不传 `--allow-private-network`**（P1：obscura
/// 默认禁 RFC1918 内网导航——SSRF 面收窄；书源/登录 URL 在交给浏览器前另行公网校验）；
/// `--proxy` 透传书源级 proxyUrl / 环境 READER_OBSCURA_PROXY（socks5://host:port 等——
/// 69shuba 等对数据中心 IP 风控的站点可经住宅/本地代理出口）。
/// obscura serve 命令构造（纯函数，供测试断言参数）：
/// `serve --port <port> --stealth [--proxy <proxy>]`
async fn spawn_serve_and_connect(
    exe: &std::path::Path,
    port: u16,
    proxy: Option<&str>,
) -> Result<Browser> {
    let mut child = obscura_serve_command(exe, port, proxy)
        .spawn()
        .map_err(|e| anyhow!("启动 obscura 失败（{}）: {e}", exe.display()))?;
    // 读 stdout banner（banner 先于监听就绪打印——连接阶段有重试）
    let stdout = child.stdout.take().expect("stdout piped");
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let ws_url = std::thread::scope(|_| -> Result<String> {
        let deadline = std::time::Instant::now() + LAUNCH_TIMEOUT;
        loop {
            if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
                if let Some(idx) = line.find("CDP server: ws://") {
                    let url = line[idx + "CDP server: ".len()..].trim().to_string();
                    if url.starts_with("ws://") {
                        return Ok(url);
                    }
                }
            }
            if std::time::Instant::now() > deadline {
                return Err(anyhow!("obscura 启动超时（15s）——未获取到 CDP 地址"));
            }
            // 提前退出（端口占用/动态库缺失等）→ 非零退出码即失败
            if let Ok(Some(status)) = child.try_wait() {
                if !status.success() {
                    return Err(anyhow!(
                        "obscura 进程启动失败（{status}）——端口 {port} 可能被占用"
                    ));
                }
            }
        }
    });
    let ws_url = match ws_url {
        Ok(u) => u,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
    };
    // banner 打印先于监听就绪——短重试连接（最多 10s；进程提前退出即失败）
    let mut ws = None;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while ws.is_none() {
        match tokio_tungstenite::connect_async(ws_url.clone()).await {
            Ok(x) => ws = Some(x),
            Err(e) => {
                let exited = child.try_wait().ok().flatten();
                if std::time::Instant::now() > deadline || exited.is_some() {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(anyhow!("obscura CDP 连接失败: {e}"));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
    let mut browser = match init_session(ws.expect("connected").0).await {
        Ok(b) => b,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
    };
    browser.child = Some(child);
    Ok(browser)
}

/// 构造 obscura serve 命令（spawn_serve_and_connect 用；独立函数便于单测断言参数）：
/// `serve --port <port> --stealth [--proxy <proxy>]`——
/// P1：不传 `--allow-private-network`（obscura 默认禁 RFC1918 内网导航）；
/// proxy 为空/纯空白时不附加 --proxy 参数
fn obscura_serve_command(exe: &std::path::Path, port: u16, proxy: Option<&str>) -> Command {
    let mut cmd = Command::new(exe);
    cmd.arg("serve")
        .arg("--port")
        .arg(port.to_string())
        .arg("--stealth");
    if let Some(p) = proxy.filter(|p| !p.trim().is_empty()) {
        cmd.arg("--proxy").arg(p.trim());
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());
    cmd
}

/// 连接建立后的会话初始化（target 创建/附加、域启用、stealth 注入）——spawn 与
/// READER_OBSCURA_URL 直连两条路径共用
async fn init_session(ws: WsStream) -> Result<Browser> {
    let (sink, stream) = ws.split();
    // reader 任务：按 id 路由响应到对应 oneshot（events 忽略）
    let pending: std::sync::Arc<
        std::sync::Mutex<HashMap<u64, tokio::sync::oneshot::Sender<Result<Value, String>>>>,
    > = std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()));
    let pending_task = std::sync::Arc::clone(&pending);
    let frame_ctx: std::sync::Arc<std::sync::Mutex<HashMap<String, i64>>> =
        std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()));
    let frame_ctx_task = std::sync::Arc::clone(&frame_ctx);
    let (close_tx, close_rx) = tokio::sync::oneshot::channel();
    // reader 任务：按 id 路由响应到对应 oneshot（events 忽略）；
    // P1：close_rx 收到关闭信号（Browser Drop）→ 退出循环（连接泄漏防护）
    spawn_reader_task(stream, pending_task, frame_ctx_task, close_rx);

    let mut browser = Browser {
        child: None,
        sink,
        pending,
        next_id: 0,
        session_id: None,
        frame_ctx,
        close_tx: Some(close_tx),
    };
    // 创建并附加页面 target（flatten 后命令需带 sessionId——obscura CDP 支持
    // Target.createTarget/attachToTarget + sessionId 路由，puppeteer 同款协议）
    let target_id = browser
        .command("Target.createTarget", json!({ "url": "about:blank" }))
        .await?
        .get("targetId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("CDP 创建页面失败"))?
        .to_string();
    let session_id = browser
        .command(
            "Target.attachToTarget",
            json!({ "targetId": target_id, "flatten": true }),
        )
        .await?
        .get("sessionId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("CDP 附加页面失败"))?
        .to_string();
    browser.session_id = Some(session_id);
    let _ = browser.command("Page.enable", json!({})).await;
    let _ = browser.command("Network.enable", json!({})).await;
    let _ = browser.command("Runtime.enable", json!({})).await;
    // stealth 注入（obscura 内置 stealth 之外的第二层）：每次新文档加载前执行
    // （webdriver 清除、plugins/vendor/languages/hardwareConcurrency/chrome.*/outer
    // 尺寸/WebGL 厂商模拟——见 STEALTH_JS，puppeteer-extra-plugin-stealth 清单翻译）。
    // 测试钩子 READER_CDP_NO_STEALTH=1 可跳过注入（过率对比实验用）。
    let stealth_enabled = std::env::var("READER_CDP_NO_STEALTH")
        .map(|v| v.trim() != "1")
        .unwrap_or(true);
    if stealth_enabled {
        let _ = browser
            .command(
                "Page.addScriptToEvaluateOnNewDocument",
                json!({ "source": STEALTH_JS }),
            )
            .await;
    }
    // Turnstile render 参数捕获 hook（sitekey/widgetId 提取 + execute 重试用）：
    // api.js 加载前注入，拦截 window.turnstile 赋值并包装 render（功能性注入，
    // 与 stealth 开关无关）
    let _ = browser
        .command(
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": TURNSTILE_HOOK_JS }),
        )
        .await;
    Ok(browser)
}

/// CDP reader 任务：按 id 路由响应到对应 oneshot（events 忽略——
/// Runtime.executionContextCreated 缓存 iframe 上下文）。
/// P1 WS 连接泄漏防护：`close_rx` 收到关闭信号（Browser Drop）→ 立即退出循环，
/// stream（接收半）随之 drop → WebSocket 关闭；退出前清空 pending（在途命令的
/// oneshot 发送端 drop → 调用方立即收到"连接已关闭"错误）。
fn spawn_reader_task(
    stream: futures::stream::SplitStream<WsStream>,
    pending_task: std::sync::Arc<
        std::sync::Mutex<HashMap<u64, tokio::sync::oneshot::Sender<Result<Value, String>>>>,
    >,
    frame_ctx_task: std::sync::Arc<std::sync::Mutex<HashMap<String, i64>>>,
    mut close_rx: tokio::sync::oneshot::Receiver<()>,
) {
    tokio::spawn(async move {
        let mut stream = stream;
        loop {
            tokio::select! {
                msg = stream.next() => {
                    let Some(msg) = msg else { break };
                    let Ok(msg) = msg else { break };
                    let text = match msg {
                        Message::Text(t) => t.to_string(),
                        Message::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
                        _ => continue,
                    };
                    let Ok(v) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    // 事件：Runtime.executionContextCreated —— 缓存 iframe 执行上下文（frameId→contextId）
                    if v.get("id").is_none() {
                        if v.get("method").and_then(|m| m.as_str())
                            == Some("Runtime.executionContextCreated")
                        {
                            if let Some(ctx) = v.get("params").and_then(|p| p.get("context")) {
                                let frame_id = ctx
                                    .get("auxData")
                                    .and_then(|a| a.get("frameId"))
                                    .and_then(|f| f.as_str());
                                let ctx_id = ctx.get("id").and_then(|i| i.as_i64());
                                if let (Some(fid), Some(cid)) = (frame_id, ctx_id) {
                                    if let Ok(mut map) = frame_ctx_task.lock() {
                                        map.insert(fid.to_string(), cid);
                                    }
                                }
                            }
                        }
                        continue;
                    }
                    let Some(id) = v.get("id").and_then(|i| i.as_u64()) else {
                        continue;
                    };
                    if let Some(tx) = pending_task
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&id)
                    {
                        let result = match v.get("error") {
                            Some(err) => Err(err.to_string()),
                            None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                        };
                        let _ = tx.send(result);
                    }
                }
                // P1：Browser Drop 关闭信号——立即退出（不再持有 WS 接收半）
                _ = &mut close_rx => break,
            }
        }
        // 在途命令立即失败（oneshot 发送端 drop）
        pending_task
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    });
}

impl Browser {
    /// 启动浏览器（obscura 唯一后端）：`READER_OBSCURA_URL` 已配置 → 连接既有 CDP
    /// 服务（不 spawn、不接管进程）；否则发现 obscura 可执行文件并 spawn
    /// `obscura serve --port <随机> --stealth`。未配置/不可用 → Err（提示手动 Cookie 流程）
    pub async fn launch() -> Result<Browser> {
        // 代理（环境变量 READER_OBSCURA_PROXY——socks5://127.0.0.1:1080 等；
        // 书源级 proxyUrl 由 solve_captcha/solve_cf_challenge 的 proxy 参数覆盖）
        let proxy = std::env::var("READER_OBSCURA_PROXY")
            .ok()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty());
        Browser::launch_proxy(proxy).await
    }

    /// 带代理启动（书源级 proxyUrl 路径）：
    /// `READER_OBSCURA_URL` 已配置 → 连接既有 CDP 服务（代理不适用——外部服务
    /// 的出口由该服务自身配置）；否则发现 obscura 可执行文件并 spawn
    /// `obscura serve --port <随机> --stealth [--proxy <proxy>]`。
    pub async fn launch_proxy(proxy: Option<String>) -> Result<Browser> {
        // ① READER_OBSCURA_URL：连接既有 obscura CDP 服务
        if let Ok(url) = std::env::var("READER_OBSCURA_URL") {
            let url = url.trim();
            if !url.is_empty() {
                return Browser::connect(url).await;
            }
        }
        // ② spawn obscura serve（stealth 构建）
        let exe = discover_obscura_bin().ok_or_else(|| {
            anyhow!(
                "未安装 obscura 浏览器（唯一浏览器后端）——请下载 stealth 构建并设置 READER_OBSCURA_BIN（或配置 READER_OBSCURA_URL 连接既有 CDP 服务）；未配置时无法使用浏览器自动登录，请在书源设置中粘贴 Cookie"
            )
        })?;
        Browser::launch_with_proxy(exe, proxy).await
    }

    /// 连接既有 obscura CDP 服务（`READER_OBSCURA_URL` 路径；不接管进程生命周期——
    /// Drop 不杀进程）。URL 支持 ws:// 直连或 http://（Playwright connectOverCDP
    /// 风格，自动补 /devtools/browser）
    pub async fn connect(url: &str) -> Result<Browser> {
        let ws_url = normalize_cdp_url(url);
        // 注意必须走 &str/String 路径：tungstenite 0.24 的 `http::Request` 转换是
        // 恒等（不会补全握手头），只有 Uri/str 路径才会填充 Host/Connection/Upgrade/
        // Sec-WebSocket-Key 等头；否则 DevTools 会回 400
        let (ws, _resp) = tokio_tungstenite::connect_async(ws_url.clone())
            .await
            .map_err(|e| anyhow!("obscura CDP 连接失败（{ws_url}）: {e}"))?;
        init_session(ws).await
    }

    /// 用指定 obscura 可执行文件启动（spawn `serve --port <随机> --stealth`；
    /// 端口冲突等启动失败自动换随机端口重试，最多 3 次）
    pub async fn launch_with(exe: PathBuf) -> Result<Browser> {
        Browser::launch_with_proxy(exe, None).await
    }

    /// 用指定 obscura 可执行文件 + 代理启动（proxy 非空时 spawn 加 `--proxy <proxy>`）
    pub async fn launch_with_proxy(exe: PathBuf, proxy: Option<String>) -> Result<Browser> {
        if !exe.exists() {
            return Err(anyhow!(
                "obscura 可执行文件不存在（{}）——无法使用浏览器自动登录",
                exe.display()
            ));
        }
        let mut last_err: Option<anyhow::Error> = None;
        for _ in 0..3 {
            // 随机端口（20000-49999）——与已有服务/其他实例冲突时 obscura 退出，
            // 换端口重试（概率极低，防御性处理）
            let port = 20000 + rand::random::<u16>() % 30000;
            match spawn_serve_and_connect(&exe, port, proxy.as_deref()).await {
                Ok(b) => return Ok(b),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("obscura 启动失败（多次尝试）")))
    }

    /// 发送 CDP 命令并等待响应（带超时）
    pub async fn command(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(sid) = &self.session_id {
            msg["sessionId"] = json!(sid);
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, tx);
        self.sink
            .send(Message::Text(msg.to_string()))
            .await
            .map_err(|e| anyhow!("CDP 发送失败: {e}"))?;
        // 等待 reader 任务路由回响应（oneshot Receiver 为 Send，可安全跨 await）
        match tokio::time::timeout(CDP_CMD_TIMEOUT, rx).await {
            Ok(Ok(result)) => result.map_err(|e| anyhow!("CDP {method} 错误: {e}")),
            Ok(Err(_)) => Err(anyhow!("CDP 连接关闭（{method}）")),
            Err(_) => {
                self.pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                Err(anyhow!("CDP 命令超时（{method}）"))
            }
        }
    }

    /// Runtime.evaluate（returnByValue，awaitPromise）→ 返回值。
    /// 兼容两种返回形状：Chrome/Edge = `{result: RemoteObject, exceptionDetails}`；
    /// obscura = RemoteObject **直接**作为命令 result（无内层包装——obscura
    /// runtime.rs evaluate 返回 `{"result": remote_object_from_info(&info)}`）
    pub async fn evaluate(&mut self, expression: &str) -> Result<Value> {
        let r = self
            .command(
                "Runtime.evaluate",
                json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }),
            )
            .await?;
        // 异常时 result 里带 exceptionDetails，value 缺失（Chrome 形状；obscura
        // 的 JS 异常返回 type=undefined 的 RemoteObject——value 为 Null）
        if r.get("exceptionDetails").is_some() {
            return Err(anyhow!(
                "页面 JS 异常: {}",
                r.get("exceptionDetails").unwrap_or(&Value::Null)
            ));
        }
        // 形状判别：命令 result 含 "type" 键 → 本身即 RemoteObject（obscura）；
        // 否则取内层 result（Chrome）
        let remote = if r.get("type").is_some() {
            &r
        } else {
            r.get("result").unwrap_or(&Value::Null)
        };
        Ok(remote.get("value").cloned().unwrap_or(Value::Null))
    }

    /// iframe 内 JS 执行：按 frame src 子串找 frame → 用缓存的 contextId 执行。
    /// Turnstile 勾选框在 challenges.cloudflare.com iframe 内部——主页面 JS/合成事件
    /// 无法穿透跨源 iframe，obscura 又不投递 CDP 坐标事件——必须 frame 内执行。
    pub async fn evaluate_in_frame(&mut self, src_hint: &str, expression: &str) -> Result<Value> {
        // 1. 找 frame
        let tree = self.command("Page.getFrameTree", json!({})).await?;
        let mut frame_id: Option<String> = None;
        let mut stack = vec![tree.get("frameTree").cloned().unwrap_or(Value::Null)];
        while let Some(node) = stack.pop() {
            if node.is_null() {
                continue;
            }
            let f = node.get("frame").cloned().unwrap_or(Value::Null);
            let url = f.get("url").and_then(|u| u.as_str()).unwrap_or("");
            let id = f.get("id").and_then(|i| i.as_str()).unwrap_or("");
            if url.contains(src_hint) {
                frame_id = Some(id.to_string());
                break;
            }
            if let Some(children) = node.get("childFrames").and_then(|c| c.as_array()) {
                for c in children.iter().rev() {
                    stack.push(c.clone());
                }
            }
        }
        let Some(fid) = frame_id else {
            return Err(anyhow!("未找到包含 {src_hint} 的 iframe"));
        };
        // 2. 用缓存的 contextId 执行
        let ctx_id = self
            .frame_ctx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&fid)
            .cloned();
        let ctx = match ctx_id {
            Some(c) => c,
            None => {
                // context 事件未到——等 500ms 重试一次
                tokio::time::sleep(Duration::from_millis(500)).await;
                self.frame_ctx
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&fid)
                    .cloned()
                    .ok_or_else(|| anyhow!("iframe 执行上下文未就绪（{src_hint}）"))?
            }
        };
        let r = self
            .command(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": true,
                    "awaitPromise": true,
                    "contextId": ctx,
                }),
            )
            .await?;
        if r.get("exceptionDetails").is_some() {
            return Err(anyhow!(
                "iframe JS 异常: {}",
                r.get("exceptionDetails").unwrap_or(&Value::Null)
            ));
        }
        let remote = if r.get("type").is_some() {
            &r
        } else {
            r.get("result").unwrap_or(&Value::Null)
        };
        Ok(remote.get("value").cloned().unwrap_or(Value::Null))
    }

    /// 等待 document.readyState == complete（超时 20s）
    pub async fn wait_ready(&mut self, timeout: Duration) -> Result<()> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if std::time::Instant::now() > deadline {
                return Err(anyhow!("页面加载超时"));
            }
            let state = self
                .evaluate("document.readyState")
                .await?
                .as_str()
                .unwrap_or("")
                .to_string();
            if state == "complete" {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    /// 导航并等待加载完成
    pub async fn navigate(&mut self, url: &str) -> Result<()> {
        self.command("Page.navigate", json!({ "url": url })).await?;
        let _ = self.wait_ready(Duration::from_secs(20)).await;
        tokio::time::sleep(Duration::from_millis(600)).await;
        Ok(())
    }

    /// 注入 cookie（name=value 对；domain 为 host，secure 按页面 scheme 决定）
    pub async fn set_cookies(
        &mut self,
        pairs: &[(String, String)],
        host: &str,
        secure: bool,
    ) -> Result<()> {
        if pairs.is_empty() {
            return Ok(());
        }
        let cookies: Vec<Value> = pairs
            .iter()
            .map(|(name, value)| {
                json!({
                    "name": name,
                    "value": value,
                    "domain": host,
                    "path": "/",
                    "httpOnly": true,
                    "secure": secure,
                    "sameSite": "Lax",
                    "expires": -1,
                })
            })
            .collect();
        self.command("Network.setCookies", json!({ "cookies": cookies }))
            .await?;
        Ok(())
    }

    /// Storage.getCookies → cookie 数组（含 httpOnly）
    pub async fn get_cookies(&mut self) -> Result<Vec<Value>> {
        let r = self.command("Storage.getCookies", json!({})).await?;
        Ok(r.get("cookies")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default())
    }

    /// cookie 数组 → "a=1; b=2"（按 name 排序，顺序稳定）
    pub fn cookies_to_string(cookies: &[Value]) -> String {
        let mut pairs: Vec<(String, String)> = cookies
            .iter()
            .filter_map(|c| {
                let name = c.get("name").and_then(|v| v.as_str())?.to_string();
                let value = c
                    .get("value")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Some((name, value))
            })
            .collect();
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        pairs
            .into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// 鼠标拖拽（滑块：按下 → 贝塞尔轨迹移动（随机噪声+微停）→ 释放）。
    /// **obscura 不向页面投递 CDP Input.dispatchMouseEvent**（坐标事件不触发页面
    /// 监听器——实测零事件），故改用 JS 合成事件（new MouseEvent + dispatchEvent，
    /// clientX/clientY 直达监听器）：对 DOM 监听型滑块（含 mock）有效；要求
    /// isTrusted 事件的真实滑块（geetest 等）在 obscura 下不可自动拖拽（明确限制）。
    /// 事件双投递：elementFromPoint 命中元素 + document（委托式与元素式监听器均触发）。
    pub async fn mouse_drag(&mut self, x1: f64, y1: f64, x2: f64, y2: f64) -> Result<()> {
        // 人类轨迹：三次贝塞尔 + 随机噪声 + 随机步数（与 Input 路径同参）
        let steps = 28 + rand::random::<u64>() % 25;
        let ctrl1 = (
            x1 + (x2 - x1) * 0.4 + rand::random::<f64>() * 20.0 - 10.0,
            y1,
        );
        let ctrl2 = (
            x1 + (x2 - x1) * 0.6 + rand::random::<f64>() * 20.0 - 10.0,
            y2,
        );
        let mut pts: Vec<(f64, f64)> = Vec::with_capacity(steps as usize + 2);
        pts.push((x1, y1));
        for i in 1..=steps {
            let t = i as f64 / steps as f64;
            let inv = 1.0 - t;
            // 三次贝塞尔
            let x = inv * inv * inv * x1
                + 3.0 * inv * inv * t * ctrl1.0
                + 3.0 * inv * t * t * ctrl2.0
                + t * t * t * x2;
            let y = inv * inv * inv * y1
                + 3.0 * inv * inv * t * ctrl1.1
                + 3.0 * inv * t * t * ctrl2.1
                + t * t * t * y2;
            // 随机噪声（±2px）
            pts.push((
                x + rand::random::<f64>() * 4.0 - 2.0,
                y + rand::random::<f64>() * 4.0 - 2.0,
            ));
        }
        pts.push((x2, y2));
        let js = format!(
            r#"(function(pts){{
  function ev(t, x, y) {{ return new MouseEvent(t, {{ bubbles: true, cancelable: true, clientX: x, clientY: y, button: 0 }}); }}
  var start = pts[0], end = pts[pts.length - 1];
  var target = document.elementFromPoint(start[0], start[1]) || document.body;
  target.dispatchEvent(ev('mousedown', start[0], start[1]));
  document.dispatchEvent(ev('mousedown', start[0], start[1]));
  for (var i = 1; i < pts.length; i++) {{
    document.dispatchEvent(ev('mousemove', pts[i][0], pts[i][1]));
  }}
  document.dispatchEvent(ev('mouseup', end[0], end[1]));
  return true;
}})({})"#,
            serde_json::to_string(&pts).map_err(|e| anyhow!("轨迹序列化失败: {e}"))?
        );
        let _ = self.evaluate(&js).await?;
        Ok(())
    }

    /// 元素区域截图（PNG 字节；图片验证码发给前端显示）
    pub async fn screenshot_clip(&mut self, x: f64, y: f64, w: f64, h: f64) -> Result<Vec<u8>> {
        let r = self
            .command(
                "Page.captureScreenshot",
                json!({
                    "format": "png",
                    "clip": { "x": x, "y": y, "width": w, "height": h, "scale": 1 },
                    "captureBeyondViewport": false,
                }),
            )
            .await?;
        let data = r
            .get("data")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("截图失败"))?;
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| anyhow!("截图 base64 解码失败: {e}"))
    }
}

// ==================== CF 质询求解（进程内浏览器 CDP；FlareSolverr 免容器替代） ====================

/// CF 质询求解结果
#[derive(Debug, Clone)]
pub struct CfSolution {
    /// 求解完成后目标页最终 HTML（document.documentElement.outerHTML）
    pub html: String,
    /// 求解后浏览器内该站点全部 cookie（name, value——含 cf_clearance；按 name 排序去重）
    pub cookies: Vec<(String, String)>,
    /// 浏览器真实 UA（与 cf_clearance 绑定：后续抓取需带同一 UA）
    pub user_agent: String,
    /// Turnstile 求解得到的 cf-turnstile-response token（非 Turnstile 质询为 None）
    pub turnstile_token: Option<String>,
    /// Turnstile sitekey（检测时从页面 data-sitekey / iframe src query / api.js 脚本
    /// URL / window.turnstile.render 调用参数提取——日志/调试用；非 Turnstile 质询为 None）
    pub turnstile_sitekey: Option<String>,
}

/// CF 质询状态检测 JS（质询等待循环每 500ms 求值一次）——challenge=true 表示仍在质询页
pub const CF_CHALLENGE_STATE_JS: &str = r#"
(function(){
  try {
    var features = document.querySelector('#challenge-form, [id^="cf-chl-"], [class*="cf-chl"], iframe[src*="challenges.cloudflare"], iframe[src*="challenge-platform"]');
    var t = (document.title || '').toLowerCase();
    var hasTitle = t.indexOf('just a moment') >= 0;
    return {
      challenge: !!(features || hasTitle),
      ready: document.readyState,
      url: location.href,
      bodyChildren: document.body ? document.body.children.length : 0
    };
  } catch (e) { return { challenge: true, ready: 'error', url: '', bodyChildren: 0 }; }
})()
"#;

/// stealth 注入 JS（puppeteer-extra-plugin-stealth 清单翻译）——每次新文档加载前执行：
/// ① navigator.webdriver 清除（自动化最显著指纹）；② plugins 模拟（headless 常为空，
/// 真实 Chrome 有 5 个 PDF 插件）；③ vendor/languages/hardwareConcurrency 模拟；
/// ④ chrome.app/csi/loadTimes/runtime 存在性模拟；⑤ outer 尺寸固定（headless 默认 0）；
/// ⑥ WebGL 渲染商模拟（UNMASKED_VENDOR_WEBGL——headless SwiftShader 指纹）
pub const STEALTH_JS: &str = r#"
(() => {
  try {
    // ① webdriver 标志清除
    Object.defineProperty(navigator, 'webdriver', { get: () => undefined });
    // ② plugins 模拟（headless 下 plugins 常为空数组——真实 Chrome 有 5 个 PDF 插件）
    if (navigator.plugins.length === 0) {
      var names = ['PDF Viewer', 'Chrome PDF Viewer', 'Chromium PDF Viewer', 'Microsoft Edge PDF Viewer', 'WebKit built-in PDF'];
      var plugins = names.map(function (name) {
        var p = { name: name, filename: name + '.dll', description: name, length: 1,
                  item: function () { return null; }, namedItem: function () { return null; }, refresh: function () {} };
        p[0] = { type: 'application/pdf', suffixes: 'pdf', description: 'Portable Document Format' };
        return p;
      });
      Object.defineProperty(navigator, 'plugins', { get: function () { return plugins; } });
    }
    // ③ vendor / languages / hardwareConcurrency
    Object.defineProperty(navigator, 'vendor', { get: function () { return 'Google Inc.'; } });
    Object.defineProperty(navigator, 'languages', { get: function () { return ['zh-CN', 'zh']; } });
    Object.defineProperty(navigator, 'hardwareConcurrency', { get: function () { return 8; } });
    // ④ chrome 对象（app/csi/loadTimes/runtime 存在性——裸 headless 环境可能缺失）
    if (!window.chrome) { window.chrome = {}; }
    if (!window.chrome.runtime) { window.chrome.runtime = {}; }
    if (!window.chrome.app) { window.chrome.app = {}; }
    if (!window.chrome.csi) {
      window.chrome.csi = function () { return { startE: 0, onloadT: 0, pageT: 0, tran: 0 }; };
    }
    if (!window.chrome.loadTimes) {
      window.chrome.loadTimes = function () {
        return { commitLoadTime: 0, firstPaintAfterLoadTime: 0, requestTime: 0, startLoadTime: 0,
                 wasFetchedViaSpdy: true, wasNpnNegotiated: true, wasAlternateProtocolAvailable: true };
      };
    }
    // ⑤ 窗口 outer 尺寸固定（headless 默认 0 是常见指纹）
    if (window.outerWidth === 0 || window.outerHeight === 0) {
      Object.defineProperty(window, 'outerWidth', { get: function () { return 1280; } });
      Object.defineProperty(window, 'outerHeight', { get: function () { return 800; } });
    }
    // ⑥ WebGL 渲染商模拟（UNMASKED_VENDOR_WEBGL——headless SwiftShader 指纹）
    if (window.WebGLRenderingContext) {
      var origGetExt = WebGLRenderingContext.prototype.getExtension;
      WebGLRenderingContext.prototype.getExtension = function (name) {
        var ext = origGetExt.call(this, name);
        if (name === 'WEBGL_debug_renderer_info' && ext) {
          try {
            Object.defineProperty(ext, 'UNMASKED_VENDOR_WEBGL', { get: function () { return 'Google Inc. (Intel)'; } });
            Object.defineProperty(ext, 'UNMASKED_RENDERER_WEBGL', { get: function () { return 'ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0, D3D11)'; } });
          } catch (e) {}
        }
        return ext;
      };
    }
  } catch (e) {}
})();
"#;

/// Turnstile 质询检测 JS：页面含 iframe[src*=challenges.cloudflare.com]（widget 内嵌 iframe）
/// 或 .cf-turnstile 容器或 [name=cf-turnstile-response] 隐藏 input 或 turnstile 脚本标签
/// 或 title 含 Turnstile/Verifying → turnstile=true（附各特征标志，供点击/超时策略选择）
pub const TURNSTILE_DETECT_JS: &str = r#"
(function(){
  try {
    var iframe = document.querySelector('iframe[src*="challenges.cloudflare.com"]');
    // 只有 src 明确含 turnstile 的 iframe 才算 Turnstile widget——
    // CF JS 质询页同样内嵌 challenges.cloudflare.com iframe（challenge-platform），误判会走错分支
    var iframeIsTurnstile = !!(iframe && /turnstile/i.test(iframe.src || ''));
    var container = document.querySelector('.cf-turnstile');
    var input = document.querySelector('[name="cf-turnstile-response"]');
    var script = document.querySelector('script[src*="challenges.cloudflare.com/turnstile"], script[src*="turnstile/api.js"]');
    var t = (document.title || '').toLowerCase();
    var hasTitle = t.indexOf('turnstile') >= 0 || t.indexOf('verifying') >= 0;
    return {
      turnstile: !!(iframeIsTurnstile || container || input || script || hasTitle),
      hasContainer: !!container,
      hasInput: !!input,
      hasTitle: hasTitle,
      iframeIsTurnstile: iframeIsTurnstile
    };
  } catch (e) { return { turnstile: false, hasContainer: false, hasInput: false, hasTitle: false, iframeIsTurnstile: false }; }
})()
"#;

/// Turnstile 点击 JS：① .cf-turnstile 容器 element.click()（页面级回调——mock 等依赖
/// click 事件的 widget）；② 同时返回 challenges.cloudflare.com iframe 的 bounding box
/// 坐标（滚动到可视区后），由 CDP Input.dispatchMouseEvent 派发真实点击——真实 Turnstile
/// 勾选发生在 iframe 内部，element.click() 无法穿透，坐标点击可直达。
pub const TURNSTILE_CLICK_JS: &str = r#"
(function(){
  try {
    var out = { ok: false, reason: 'no-element' };
    var el = document.querySelector('.cf-turnstile');
    if (el) {
      try { el.click(); out = { ok: true, how: 'container' }; } catch (e) {}
    }
    var f = document.querySelector('iframe[src*="challenges.cloudflare.com"]');
    if (f) {
      try { f.scrollIntoView({ block: 'center' }); } catch (e) {}
      var r = f.getBoundingClientRect();
      out = { ok: true, how: 'iframe', x: r.x + Math.min(28, r.width * 0.18), y: r.y + Math.min(32, r.height * 0.35), w: r.width, h: r.height };
    }
    return out;
  } catch (e) { return { ok: false, reason: 'exception' }; }
})()
"#;

/// Turnstile token 读取 JS：`[name=cf-turnstile-response]` 隐藏 input 的 value（等价于
/// `document.querySelector('[name=cf-turnstile-response]')?.value`——不用可选链以兼容
/// boa 冒烟解析）；widget API 兜底（真实站点页面未必有该 input——turnstile.getResponse()
/// 等效）。
pub const TURNSTILE_TOKEN_JS: &str = r#"
(function(){
  try {
    var el = document.querySelector('[name="cf-turnstile-response"]');
    if (el && el.value) { return el.value; }
    if (window.turnstile && typeof window.turnstile.getResponse === 'function') {
      var t = window.turnstile.getResponse();
      if (t) { return t; }
    }
    return '';
  } catch (e) { return ''; }
})()
"#;

/// Turnstile render 参数捕获 hook（每个新文档加载前注入）：在 api.js 加载前用
/// defineProperty 拦截 `window.turnstile` 赋值并包装 `render`——把每次 render 的
/// 调用参数（widgetId/容器 + options，含 sitekey）存到 `window.__readerTurnstileCaptured`。
/// 用途：① sitekey 提取（render 参数来源）；② `turnstile.execute(widgetId)` 重试
/// （widget 暴露 render/execute 时，execute 需同一 widgetId/容器）。
/// 页面自身对 window.turnstile 的后续写入不受影响（configurable setter，赋值仍生效）。
pub const TURNSTILE_HOOK_JS: &str = r#"
(function(){
  try {
    var real = undefined;
    var captured = [];
    Object.defineProperty(window, 'turnstile', {
      configurable: true,
      get: function() { return real; },
      set: function(v) {
        if (v && typeof v.render === 'function' && !v.__readerHooked) {
          try {
            v.__readerHooked = true;
            var orig = v.render;
            v.render = function() {
              try { captured.push(Array.prototype.slice.call(arguments)); } catch (e) {}
              return orig.apply(this, arguments);
            };
          } catch (e) {}
        }
        real = v;
      }
    });
    window.__readerTurnstileCaptured = captured;
  } catch (e) {}
})();
"#;

/// Turnstile sitekey 提取 JS（检测命中时求值——日志/调试用）：依次尝试
/// ① .cf-turnstile 容器 data-sitekey 属性；② challenges.cloudflare.com iframe src 的
/// sitekey query 参数；③ turnstile/api.js 脚本 URL 的 sitekey query 参数（手动加载形式）；
/// ④ window.turnstile.render 调用参数（TURNSTILE_HOOK_JS 捕获的 options.sitekey）——
/// 同时返回 widgetId（render 首参，execute 重试用）。未命中返回空串。
pub const TURNSTILE_SITEKEY_JS: &str = r#"
(function(){
  try {
    var out = { sitekey: '', widgetId: '', how: '' };
    // ① data-sitekey 属性（页面容器）
    var el = document.querySelector('.cf-turnstile[data-sitekey]') || document.querySelector('[data-sitekey]');
    if (el) {
      var k = el.getAttribute('data-sitekey');
      if (k) { out.sitekey = k; out.how = 'data-sitekey'; }
    }
    // ② iframe src query（widget iframe 带 sitekey 参数）
    if (!out.sitekey) {
      var f = document.querySelector('iframe[src*="challenges.cloudflare.com"]');
      if (f) {
        var m = (f.src || '').match(/[?&]sitekey=([^&]+)/);
        if (m && m[1]) { out.sitekey = decodeURIComponent(m[1]); out.how = 'iframe-src'; }
      }
    }
    // ③ turnstile/api.js 脚本 URL query（手动加载形式）
    if (!out.sitekey) {
      var s = document.querySelector('script[src*="turnstile/api.js"], script[src*="challenges.cloudflare.com/turnstile"]');
      if (s) {
        var m = (s.src || '').match(/[?&]sitekey=([^&]+)/);
        if (m && m[1]) { out.sitekey = decodeURIComponent(m[1]); out.how = 'script-src'; }
      }
    }
    // ④ window.turnstile.render 调用参数（hook 捕获）
    if (!out.sitekey && window.__readerTurnstileCaptured && window.__readerTurnstileCaptured.length) {
      for (var i = 0; i < window.__readerTurnstileCaptured.length; i++) {
        var args = window.__readerTurnstileCaptured[i];
        var opts = args && args[1] ? args[1] : {};
        if (opts.sitekey) {
          out.sitekey = String(opts.sitekey);
          out.widgetId = args[0] != null ? String(args[0]) : '';
          out.how = 'render-args';
          break;
        }
      }
    }
    return out;
  } catch (e) { return { sitekey: '', widgetId: '', how: 'exception' }; }
})()
"#;

/// Turnstile 程序化执行 JS（点击后无 token 的策略升级①）：直接调
/// `window.turnstile.execute(widgetId | 容器 | 无参)`（v0 API——widget 暴露
/// render/execute 的 response 回调时可用）。**故意不 await**：obscura 只同步执行
/// CDP 驱动的 JS（定时器/微任务不可靠），execute 的 Promise 在后台跑，token 由
/// 既有轮询（TURNSTILE_TOKEN_JS 的 getResponse）兜底收集；catch 吞掉拒绝避免
/// 未处理 Promise 异常。返回实际使用的执行目标（日志用）。
pub const TURNSTILE_EXECUTE_JS: &str = r#"
(function(){
  try {
    if (!window.turnstile || typeof window.turnstile.execute !== 'function') return '';
    var wid = '';
    if (window.__readerTurnstileCaptured && window.__readerTurnstileCaptured.length) {
      var a0 = window.__readerTurnstileCaptured[0];
      if (a0 && a0[0] != null) wid = String(a0[0]);
    }
    var container = document.querySelector('.cf-turnstile');
    var p;
    if (wid) { p = window.turnstile.execute(wid); }
    else if (container) { p = window.turnstile.execute(container); }
    else { p = window.turnstile.execute(); }
    if (p && typeof p.catch === 'function') { try { p.catch(function(){}); } catch (e) {} }
    return wid || (container ? 'container' : 'all');
  } catch (e) { return 'error'; }
})()
"#;

/// Turnstile iframe 内勾选框点击 JS（checkbox 变体选择器扩展——策略升级②用，
/// 首次点击同样复用）：`.ctp-checkbox` → `input[type=checkbox]` → `[role=checkbox]`
/// → `.ctp-checkbox-label` → `label`（命中 label 时先点其内第一元素再点 label 本身
/// ——视觉勾选框常是 label 的首个子元素）。真实 Turnstile 勾选在 iframe 内部，
/// 跨源 iframe 需 frame 上下文执行。
pub const TURNSTILE_FRAME_CLICK_JS: &str = r#"
(function(){
  try {
    var el = document.querySelector('.ctp-checkbox')
      || document.querySelector('input[type="checkbox"]')
      || document.querySelector('[role="checkbox"]')
      || document.querySelector('.ctp-checkbox-label')
      || document.querySelector('label');
    if (!el) return false;
    if (el.tagName === 'LABEL') {
      var first = el.firstElementChild;
      if (first) { try { first.click(); } catch (e) {} }
    }
    el.click();
    return true;
  } catch (e) { return false; }
})()
"#;

/// 不支持的验证码类型检测 JS（reCAPTCHA：g-recaptcha/recaptcha/api.js；
/// hCaptcha：h-captcha/hcaptcha.com）——命中即返回明确错误（不自动求解）
pub const UNSUPPORTED_CAPTCHA_DETECT_JS: &str = r#"
(function(){
  try {
    var recaptcha = document.querySelector('.g-recaptcha, iframe[src*="recaptcha"], script[src*="recaptcha/api.js"], [class*="g-recaptcha"]');
    var hcaptcha = document.querySelector('[class*="h-captcha"], iframe[src*="hcaptcha"], script[src*="hcaptcha"]');
    return { recaptcha: !!recaptcha, hcaptcha: !!hcaptcha };
  } catch (e) { return { recaptcha: false, hcaptcha: false }; }
})()
"#;

/// Turnstile token 轮询间隔（任务要求每 800ms）
const TURNSTILE_POLL_MS: u64 = 800;
/// Turnstile token 轮询上限（任务要求最多 45s——Turnstile 慢网络下 30s 偏紧；
/// 仅对真 Turnstile widget 生效；经典 CF 质询误命中 iframe 特征时不受此限，
/// 仍按调用方 max_wait_ms）
const TURNSTILE_MAX_WAIT_MS: u64 = 45_000;
/// Turnstile 点击后无 token 的策略升级阈值（按 800ms 轮询次数计）：
/// - 4 次（约 3.2s）：程序化 `window.turnstile.execute()`（widget 暴露 render/execute 时）
/// - 8 次（约 6.4s）：iframe 内不同元素点击（checkbox 变体选择器扩展）
/// - 20 次（约 16s）：完整重点击一轮（容器 + iframe + 坐标）
const TURNSTILE_ESCALATE_EXECUTE: u32 = 4;
const TURNSTILE_ESCALATE_CLICK2: u32 = 8;
const TURNSTILE_ESCALATE_CLICK3: u32 = 20;

/// 会话浏览器闲置回收时限（最后一次使用后 TTL 内无新请求 → 杀进程释放资源）
const CF_SESSION_IDLE_TTL: Duration = Duration::from_secs(300);

/// 全局 CF 质询求解会话：惰性启动（首次 CF 命中时 launch）、并发互斥（每用户会话锁排队）、
/// 超时/异常自动重启（出错即弃用实例，下次调用重新 launch）。
/// proxy：会话建立时的浏览器代理（书源级 proxyUrl / READER_OBSCURA_PROXY）——
/// 后续请求 proxy 变化时弃用会话重启（代理是浏览器进程级参数，无法热切换）
struct CfSession {
    browser: Browser,
    last_used: std::time::Instant,
    proxy: Option<String>,
}

/// 会话条目：`inner` 为**会话级互斥锁**（P1 全局锁优化——不同用户命名空间的求解
/// 并行执行互不阻塞，同一 ns 串行）。条目经 Arc 共享：从 map 移除后，在途求解仍
/// 持有 Arc 正常完成（下次求解重建条目）。
struct CfSessionEntry {
    /// None = 尚未启动/已弃用（下次求解重新 launch）
    inner: tokio::sync::Mutex<Option<CfSession>>,
}

impl CfSessionEntry {
    fn new() -> Self {
        CfSessionEntry {
            inner: tokio::sync::Mutex::new(None),
        }
    }
}

/// 按用户命名空间隔离的浏览器会话池（安全：同一实例多用户共享一个浏览器实例会
/// 泄漏登录态 cookie——A 用户的 cf_clearance/登录 cookie 残留在浏览器，B 用户的
/// 质询求解会带着 A 的 cookie 请求。每 ns 独立实例（独立 user-data-dir），
/// 求解前还清空浏览器 cookie 再注入本用户 cookie（双保险）。
/// **P1 锁粒度优化**：全局 Mutex → RwLock<条目表> + 每条目会话锁（tokio Mutex）——
/// 挑战求解期间只持有本 ns 的会话锁，不同 ns 的求解并行；锁内无全局等待。
static CF_SESSION: LazyLock<
    tokio::sync::RwLock<std::collections::HashMap<String, Arc<CfSessionEntry>>>,
> = LazyLock::new(|| tokio::sync::RwLock::new(std::collections::HashMap::new()));

/// 取（无则创建）用户命名空间的会话条目。读锁快路径；未命中才写锁插入。
async fn cf_session_entry(ns: &str) -> Arc<CfSessionEntry> {
    if let Some(entry) = CF_SESSION.read().await.get(ns).cloned() {
        return entry;
    }
    let mut map = CF_SESSION.write().await;
    map.entry(ns.to_string())
        .or_insert_with(|| Arc::new(CfSessionEntry::new()))
        .clone()
}

/// 闲置回收：每次求解成功后挂一个定时任务——TTL 内无新使用则弃用会话（Drop 杀进程+清目录）。
/// 并发触发多个无害（幂等：last_used 刷新后条件不满足即跳过）。
/// 正在求解中的条目（会话锁被占用）`try_lock` 失败 → 跳过不回收。
fn spawn_cf_session_reaper() {
    tokio::spawn(async {
        tokio::time::sleep(CF_SESSION_IDLE_TTL).await;
        reap_idle_cf_sessions().await;
    });
}

/// 回收闲置超时（[`CF_SESSION_IDLE_TTL`] 内无使用）的会话条目。独立函数便于单测。
/// 锁序安全：try_lock 不等待——先读锁扫描（try_lock 每条目），再写锁二次确认删除，
/// 持会话锁期间绝不获取 map 写锁（无死锁环）。
async fn reap_idle_cf_sessions() {
    let mut stale: Vec<String> = Vec::new();
    {
        let map = CF_SESSION.read().await;
        for (ns, entry) in map.iter() {
            if let Ok(guard) = entry.inner.try_lock() {
                let idle = guard
                    .as_ref()
                    .map(|s| s.last_used.elapsed() >= CF_SESSION_IDLE_TTL)
                    .unwrap_or(true);
                if idle {
                    stale.push(ns.clone());
                }
            }
        }
    }
    if stale.is_empty() {
        return;
    }
    // 二次确认（写锁下 try_lock——避免误回收刚被复用的条目）
    let mut map = CF_SESSION.write().await;
    for ns in stale {
        let mut remove_it = false;
        if let Some(entry) = map.get(&ns) {
            if let Ok(guard) = entry.inner.try_lock() {
                let idle = guard
                    .as_ref()
                    .map(|s| s.last_used.elapsed() >= CF_SESSION_IDLE_TTL)
                    .unwrap_or(true);
                if idle {
                    remove_it = true;
                }
            }
        }
        if remove_it {
            map.remove(&ns); // Drop 条目 → 杀进程+清目录
        }
    }
}

/// 显式关闭 CF 求解会话（集成测试/优雅停机用；幂等）
pub async fn shutdown_cf_session() {
    let mut map = CF_SESSION.write().await;
    map.clear();
}

/// 解 CF 质询（进程内浏览器 CDP；会话级浏览器实例——惰性启动/互斥/异常自动重启）。
/// CF 专用入口（不含滑块分支——登录页滑块走 solve_captcha 或登录流程）。
///
/// proxy：书源级代理（None = 不指定，回退环境变量 READER_OBSCURA_PROXY）。
///
/// 流程：启动/复用会话浏览器（独立 user-data-dir，退出自动清理）→ Network.setCookies
/// 注入 cookies → Page.navigate → 质询等待循环（每 500ms 求值 document：challenge 特征
/// （#challenge-form/#cf-chl-*/iframe[src*=challenges.cloudflare]/title=="Just a moment"）
/// 消失或 URL 变化到目标页；Turnstile 分支：点击容器 + 每 800ms 轮询 token（最多 45s，
/// 点击后无 token 自动策略升级：window.turnstile.execute → iframe 内不同元素点击 →
/// 完整重点击；失败整体换新浏览器上下文重试一次）→ 提取最终 HTML → Storage.getCookies
/// （该站点全部，含 cf_clearance）→ {html, cookies, userAgent, turnstile_token, sitekey}。
/// 超时/浏览器不可用返回明确错误。
///
/// 服务端静默语义：全程 headless（--headless=new），不弹任何窗口/不等待用户——
/// 求解失败返回明确错误，由调用方（书源 JS 等）自行兜底。
pub async fn solve_cf_challenge(
    ns: &str,
    url: &str,
    cookies: &[(String, String)],
    max_wait_ms: u64,
    proxy: Option<&str>,
) -> Result<CfSolution> {
    solve_captcha_inner(ns, url, cookies, max_wait_ms, false, proxy).await
}

/// 统一验证码求解入口（服务端静默 headless——不弹浏览器给用户）：一个函数覆盖全部验证码
/// 类型——内部按检测分派：
/// - CF JS 质询（challenge-platform/#challenge-form/"Just a moment"）→ 等待循环（JS 自解）
/// - Turnstile（.cf-turnstile/[name=cf-turnstile-response]/challenges.cloudflare.com iframe）
///   → 点击容器（element.click + iframe 内 checkbox 变体点击）→ 每 800ms 轮询 token
///   （最多 45s；无 token 自动策略升级：execute/换元素点击/重点击）
/// - 登录页滑块（DETECT_CAPTCHA_JS kind=slider）→ 贝塞尔轨迹拖拽（人类轨迹，与登录流程一致）
/// - reCAPTCHA/hCaptcha → 明确错误（不支持自动求解）
/// 会话管理/超时语义与 solve_cf_challenge 一致。proxy：书源级代理（None = 环境变量
/// READER_OBSCURA_PROXY）。书源 JS 的 java.startBrowserAwait shim 应路由到此入口
/// （成功返回 body/cookies，失败返回明确错误）。
pub async fn solve_captcha(
    ns: &str,
    url: &str,
    cookies: &[(String, String)],
    max_wait_ms: u64,
    proxy: Option<&str>,
) -> Result<CfSolution> {
    solve_captcha_inner(ns, url, cookies, max_wait_ms, true, proxy).await
}

/// 统一求解内部实现（include_slider：solve_captcha 启用滑块分派，CF 专用入口不启用）。
/// 求解链（GAP 175）：内置浏览器 CDP → camoufox（HTTP 后端 scripts/camoufox_solver.py）
/// → 仍失败才报错（合并错误）；`READER_CAMOUFOX_FIRST=1` 时 camoufox 优先。
/// proxy：书源级代理（None = 不指定，回退环境变量 READER_OBSCURA_PROXY）。
/// 失败重试：Turnstile/CF 求解失败会**换全新浏览器上下文重试一次（总 2 次）**——
/// 风控页对同一浏览器指纹/上下文可能持续拒绝，新实例（新 TLS 指纹会话/新页面）
/// 有第二机会；仍失败才进 camoufox 兜底。
async fn solve_captcha_inner(
    ns: &str,
    url: &str,
    cookies: &[(String, String)],
    max_wait_ms: u64,
    include_slider: bool,
    proxy: Option<&str>,
) -> Result<CfSolution> {
    // ① 目标公网校验（P1 SSRF：书源/登录 URL 校验后才允许浏览器导航——DNS 解析后
    //    拒绝私网/回环/链路本地；obscura 侧已默认禁 RFC1918（去 --allow-private-
    //    network），此处为进程内双保险——camoufox/FlareSolverr 路径同样生效）
    crate::service::crawler::validate_public_target(url).await?;

    // ② camoufox 优先模式（READER_CAMOUFOX_FIRST=1）：先试 HTTP 后端，失败转 CDP
    let camo_err = if crate::service::camoufox::first_mode() {
        match crate::service::camoufox::solve(url, cookies, max_wait_ms).await {
            Ok(sol) => return Ok(sol),
            Err(e) => {
                tracing::warn!("camoufox 优先求解失败（转内置浏览器 CDP）: {e:#}");
                Some(e)
            }
        }
    } else {
        None
    };

    // ③ 会话条目（每用户命名空间独立条目 + 会话级锁——P1 全局锁优化：不同 ns 的
    //    求解并行、互不阻塞；同一 ns 串行）。条目本身先取（map 锁只用于取/建条目，
    //    不跨求解持有）
    let entry = cf_session_entry(ns).await;
    let mut guard = entry.inner.lock().await;
    // ④ 求解尝试循环（最多 2 次——失败换新浏览器上下文重试）：
    //    惰性启动 / 复用（每用户命名空间独立浏览器实例——防跨用户 cookie 泄漏）；
    //    proxy 变化 → 弃用旧实例（代理是进程级参数，无法热切换）
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        let proxy_changed = guard
            .as_ref()
            .map(|s| s.proxy.as_deref() != proxy)
            .unwrap_or(false);
        if proxy_changed {
            tracing::info!(
                "书源代理变化（{} → {:?}）——弃用既有浏览器会话",
                guard
                    .as_ref()
                    .and_then(|s| s.proxy.as_deref())
                    .unwrap_or("(直连)"),
                proxy
            );
            *guard = None; // Drop 旧实例（杀进程）
        }
        if guard.is_none() {
            let browser = match Browser::launch_proxy(proxy.map(String::from)).await {
                Ok(b) => b,
                Err(launch_err) => {
                    let cdp_err = anyhow!("CF 质询需浏览器环境：{launch_err:#}");
                    drop(guard);
                    // 无内置浏览器 → camoufox 兜底（默认启用；仍失败合并错误）
                    return finish_with_fallback(url, cookies, max_wait_ms, &cdp_err, camo_err)
                        .await;
                }
            };
            *guard = Some(CfSession {
                browser,
                last_used: std::time::Instant::now(),
                proxy: proxy.map(String::from),
            });
        }
        let session = guard.as_mut().expect("just launched");
        session.last_used = std::time::Instant::now();
        let result = solve_with(
            &mut session.browser,
            url,
            cookies,
            max_wait_ms,
            include_slider,
        )
        .await;
        match result {
            Ok(sol) => {
                spawn_cf_session_reaper();
                return Ok(sol);
            }
            Err(e) => {
                // 超时/异常 → 弃用该用户实例（Drop 杀进程 + 清 user-data-dir）；
                // 条目保留（None 状态），下次求解重新 launch
                *guard = None;
                if attempts >= 2 {
                    drop(guard);
                    // ⑤ CDP 两次尝试均失败 → camoufox 兜底（仍失败才报错）
                    return finish_with_fallback(url, cookies, max_wait_ms, &e, camo_err).await;
                }
                tracing::warn!(
                    "CF/Turnstile 求解失败（第 {attempts} 次）——更换全新浏览器上下文重试: {e:#}"
                );
                // 继续循环：重新 launch 全新浏览器实例
            }
        }
    }
}

/// CDP 失败后的统一收尾：camoufox 兜底；已优先尝试过 camoufox（并失败）则不再重复调用，
/// 直接合并错误返回。
async fn finish_with_fallback(
    url: &str,
    cookies: &[(String, String)],
    max_wait_ms: u64,
    cdp_err: &anyhow::Error,
    camo_err: Option<anyhow::Error>,
) -> Result<CfSolution> {
    if let Some(prev) = camo_err {
        return Err(anyhow!(
            "内置浏览器求解失败: {cdp_err:#}；camoufox 优先尝试失败: {prev:#}"
        ));
    }
    crate::service::camoufox::fallback(url, cookies, max_wait_ms, cdp_err).await
}

/// 在会话浏览器当前页面执行 JS（求解完成后继续操作页面——如提交表单/页内 fetch，
/// 69shuba 搜索场景：同源自动携带 cf_clearance）。无会话（未求解过）→ 错误。
/// P1：只持本 ns 的会话锁（不阻塞其他 ns）。
pub async fn evaluate_in_session(ns: &str, expression: &str) -> Result<Value> {
    let entry = cf_session_entry(ns).await;
    let mut guard = entry.inner.lock().await;
    let Some(session) = guard.as_mut() else {
        return Err(anyhow!(
            "无浏览器会话——请先调用 solve_cf_challenge/solve_captcha"
        ));
    };
    session.last_used = std::time::Instant::now();
    session.browser.evaluate(expression).await
}

/// 单次求解（浏览器实例已由会话就绪）
async fn solve_with(
    browser: &mut Browser,
    url: &str,
    cookies: &[(String, String)],
    max_wait_ms: u64,
    include_slider: bool,
) -> Result<CfSolution> {
    let parsed = url::Url::parse(url).map_err(|e| anyhow!("URL 解析失败（{url}）: {e}"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow!("URL 无主机名（{url}）"))?
        .to_string();
    let secure = parsed.scheme() == "https";
    let initial_url = url.to_string();

    // ① 注入用户 cookie（会话连续性：cf_clearance 等登录态随请求携带）
    browser.set_cookies(cookies, &host, secure).await?;

    // ② 导航（navigate 内部已等 readyState==complete）
    browser.navigate(url).await?;

    // ③ 质询等待循环（统一分派）：每 500ms 求值 document——challenge 特征消失 或 URL
    //    变化到目标页；Turnstile 分支：检测 → 点击容器 → 每 800ms 轮询 token；
    //    滑块分支（solve_captcha 入口）：检测到即拖拽；reCAPTCHA/hCaptcha → 明确错误。
    let deadline = std::time::Instant::now() + Duration::from_millis(max_wait_ms);
    // Turnstile token 轮询上限（任务要求最多 45s——Turnstile 慢网络下 30s 偏紧；
    // 仅对真 Turnstile widget 生效；经典 CF 质询误命中 iframe 特征时不受此限，
    // 仍按 max_wait_ms）
    let turnstile_deadline =
        std::time::Instant::now() + Duration::from_millis(max_wait_ms.min(TURNSTILE_MAX_WAIT_MS));
    let mut turnstile_mode = false;
    let mut turnstile_widget = false; // 页面确有 Turnstile widget（容器/input/标题/turnstile iframe）
    let mut turnstile_clicked = false;
    let mut turnstile_token: Option<String> = None;
    let mut turnstile_sitekey: Option<String> = None;
    // 点击后无 token 的策略升级状态（换求解手段，不干等）：
    // executed=已调 window.turnstile.execute；polls=点击后的轮询次数；
    // click_round=iframe 内换元素点击轮次（0→1 变体选择器；1→2 完整重点击）
    let mut turnstile_executed = false;
    let mut turnstile_polls: u32 = 0;
    let mut turnstile_click_round: u32 = 0;
    let mut slider_dragged = false;
    let mut saw_classic_challenge = false; // 经典 CF 质询特征曾出现（误判 Turnstile 时据此退出）
    loop {
        let now = std::time::Instant::now();
        let turnstile_timeout = turnstile_mode && turnstile_widget && now >= turnstile_deadline;
        if now >= deadline || turnstile_timeout {
            if turnstile_mode && turnstile_widget {
                return Err(anyhow!(
                    "Turnstile 验证超时（{}s）：{url}——未获取到 cf-turnstile-response token（可能需要人工验证）",
                    TURNSTILE_MAX_WAIT_MS / 1000
                ));
            }
            return Err(anyhow!(
                "CF 质询求解超时（{}s）：{url}——页面仍停留在质询页（challenge 特征未消失）",
                max_wait_ms / 1000
            ));
        }

        // ① 不支持的验证码类型（reCAPTCHA/hCaptcha）——明确错误（不自动求解）
        if let Ok(u) = browser.evaluate(UNSUPPORTED_CAPTCHA_DETECT_JS).await {
            if u.get("recaptcha")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                return Err(anyhow!(
                    "该验证码类型不支持（reCAPTCHA）——请手动完成验证或更换书源"
                ));
            }
            if u.get("hcaptcha").and_then(|v| v.as_bool()).unwrap_or(false) {
                return Err(anyhow!(
                    "该验证码类型不支持（hCaptcha）——请手动完成验证或更换书源"
                ));
            }
        }

        // ② Turnstile 检测（每次迭代刷新——widget 可能延迟渲染；script 标签先命中、容器后出现）
        //    注意：turnstile_widget 只看页面级特征（.cf-turnstile 容器 / 隐藏 input / 标题）
        //    ——iframe[src*=challenges.cloudflare.com] 单独命中不算 widget（经典 CF 质询页
        //    也内嵌该 iframe，误判会触发 45s token 轮询上限并破坏经典质询等待循环）
        if !turnstile_mode {
            if let Ok(d) = browser.evaluate(TURNSTILE_DETECT_JS).await {
                let ts = d
                    .get("turnstile")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if ts {
                    turnstile_mode = true;
                    turnstile_widget = d
                        .get("hasContainer")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                        || d.get("hasInput").and_then(|v| v.as_bool()).unwrap_or(false)
                        || d.get("hasTitle").and_then(|v| v.as_bool()).unwrap_or(false);
                    // sitekey 提取（页面 data-sitekey / iframe src / api.js URL /
                    // turnstile.render 参数——日志/调试用，存 CfSolution）
                    if let Ok(sk) = browser.evaluate(TURNSTILE_SITEKEY_JS).await {
                        let sk = sk
                            .get("sitekey")
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                            .map(String::from);
                        if sk.is_some() {
                            turnstile_sitekey = sk;
                        }
                    }
                    tracing::warn!(
                        "Turnstile 检测命中 {url}: container={} input={} title={} iframeTs={} sitekey={:?}",
                        d.get("hasContainer")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                        d.get("hasInput").and_then(|v| v.as_bool()).unwrap_or(false),
                        d.get("hasTitle").and_then(|v| v.as_bool()).unwrap_or(false),
                        d.get("iframeIsTurnstile")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                        turnstile_sitekey,
                    );
                }
            }
        } else if !turnstile_widget {
            // widget 标志升级（script 标签先命中、容器后渲染）
            if let Ok(d) = browser.evaluate(TURNSTILE_DETECT_JS).await {
                if d.get("hasContainer")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                    || d.get("hasInput").and_then(|v| v.as_bool()).unwrap_or(false)
                    || d.get("hasTitle").and_then(|v| v.as_bool()).unwrap_or(false)
                {
                    turnstile_widget = true;
                    // 容器/input 出现后再补提取一次 sitekey（脚本标签先命中时容器未渲染）
                    if turnstile_sitekey.is_none() {
                        if let Ok(sk) = browser.evaluate(TURNSTILE_SITEKEY_JS).await {
                            let sk = sk
                                .get("sitekey")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty())
                                .map(String::from);
                            if sk.is_some() {
                                turnstile_sitekey = sk;
                            }
                        }
                    }
                }
            }
        }

        // ③ Turnstile 流程：点击容器 → 轮询 token（每 800ms）——token 非空即通过；
        //    点击后无 token 自动策略升级（① window.turnstile.execute 程序化触发；
        //    ② iframe 内不同元素点击（checkbox 变体选择器）；③ 完整重点击一轮）
        if turnstile_mode {
            if !turnstile_clicked {
                if click_turnstile(browser).await? {
                    turnstile_clicked = true;
                }
                tokio::time::sleep(Duration::from_millis(TURNSTILE_POLL_MS)).await;
                continue;
            }
            turnstile_polls += 1;
            if let Ok(v) = browser.evaluate(TURNSTILE_TOKEN_JS).await {
                if let Some(s) = v.as_str().filter(|s| !s.is_empty()) {
                    turnstile_token = Some(s.to_string());
                    break;
                }
            }
            // 升级①：点击后约 3.2s 无 token → 直接调 window.turnstile.execute
            //（widget 暴露 render/execute 的 response 回调时；不 await——obscura 定时器
            //  不可靠，token 由轮询 getResponse 兜底收集）
            if turnstile_polls >= TURNSTILE_ESCALATE_EXECUTE && !turnstile_executed {
                turnstile_executed = true;
                if let Ok(how) = browser.evaluate(TURNSTILE_EXECUTE_JS).await {
                    tracing::info!(
                        "Turnstile 点击后无 token——程序化 execute 触发（target={:?}）",
                        how.as_str().unwrap_or("")
                    );
                }
                tokio::time::sleep(Duration::from_millis(TURNSTILE_POLL_MS)).await;
                continue;
            }
            // 升级②：点击后约 6.4s 仍无 token → iframe 内不同元素点击
            //（checkbox 变体选择器扩展：.ctp-checkbox/input[type=checkbox]/[role=checkbox]/label 内第一元素）
            if turnstile_polls >= TURNSTILE_ESCALATE_CLICK2 && turnstile_click_round == 0 {
                turnstile_click_round = 1;
                if let Ok(v) = browser
                    .evaluate_in_frame("challenges.cloudflare.com", TURNSTILE_FRAME_CLICK_JS)
                    .await
                {
                    if v.as_bool().unwrap_or(false) {
                        tracing::info!(
                            "Turnstile 无 token——iframe 内 checkbox 变体元素点击（轮 1）"
                        );
                        tokio::time::sleep(Duration::from_millis(TURNSTILE_POLL_MS)).await;
                        continue;
                    }
                }
            }
            // 升级③：点击后约 16s 仍无 token → 完整重点击一轮（容器 + iframe + 坐标）
            if turnstile_polls >= TURNSTILE_ESCALATE_CLICK3 && turnstile_click_round == 1 {
                turnstile_click_round = 2;
                if click_turnstile(browser).await? {
                    tracing::info!("Turnstile 无 token——完整重点击一轮（轮 2）");
                    tokio::time::sleep(Duration::from_millis(TURNSTILE_POLL_MS)).await;
                    continue;
                }
            }
            // 退出：URL 变化（表单提交/跳转）；或非 widget 命中（经典质询误判）且质询已清除
            if let Ok(state) = browser.evaluate(CF_CHALLENGE_STATE_JS).await {
                let challenge = state
                    .get("challenge")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let cur_url = state
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let ready = state
                    .get("ready")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let body_children = state.get("bodyChildren").and_then(num_u64).unwrap_or(0);
                if challenge {
                    saw_classic_challenge = true;
                }
                let url_changed = !cur_url.is_empty() && cur_url != initial_url;
                let page_loaded = ready == "complete" || (ready != "loading" && body_children > 0);
                if turnstile_widget {
                    // 仅 URL 规范化（http→https/trailing slash）不视为通过——需质询特征
                    // 同时消失（表单提交跳转到目标页）
                    if url_changed && !challenge {
                        break;
                    }
                } else if (!challenge && url_changed)
                    || (saw_classic_challenge && !challenge && page_loaded)
                {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(TURNSTILE_POLL_MS)).await;
            continue;
        }

        // ④ 滑块（统一入口分派——登录页滑块自动拖拽；CF 专用入口不启用）
        if include_slider && !slider_dragged {
            if let Ok(det) = browser.evaluate(DETECT_CAPTCHA_JS).await {
                if !det.is_null() && det.get("kind").and_then(|v| v.as_str()) == Some("slider") {
                    let bx = det.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let by = det.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let bw = det.get("w").and_then(|v| v.as_f64()).unwrap_or(40.0);
                    let track_w = det.get("trackW").and_then(|v| v.as_f64()).unwrap_or(300.0);
                    let start_x = bx + bw / 2.0;
                    let start_y = by + 12.0;
                    // 目标距离随机化（轨道 55%~90%），避免固定轨迹被风控（与登录流程一致）
                    let dist = (track_w - bw) * (0.55 + rand::random::<f64>() * 0.35);
                    let end_x = bx + dist;
                    let end_y = start_y + rand::random::<f64>() * 4.0 - 2.0;
                    browser.mouse_drag(start_x, start_y, end_x, end_y).await?;
                    slider_dragged = true;
                    tokio::time::sleep(Duration::from_millis(CAPTCHA_SETTLE_MS)).await;
                    continue;
                }
            }
        }

        // ⑤ 经典 CF 质询等待（非 Turnstile 页）：每 500ms 求值 document——challenge 特征
        //    消失 或 URL 变化到目标页
        match browser.evaluate(CF_CHALLENGE_STATE_JS).await {
            Ok(state) => {
                let challenge = state
                    .get("challenge")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let ready = state
                    .get("ready")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let cur_url = state
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let body_children = state.get("bodyChildren").and_then(num_u64).unwrap_or(0);
                let url_changed = !cur_url.is_empty() && cur_url != initial_url;
                let page_loaded = ready == "complete" || (ready != "loading" && body_children > 0);
                if !challenge && (page_loaded || url_changed) {
                    break;
                }
            }
            Err(_) => { /* 导航中执行上下文切换——忽略，继续等待 */ }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // ④ 稳定等待 + 提取最终 HTML / 全部 cookie（含 cf_clearance）/ 浏览器 UA
    tokio::time::sleep(Duration::from_millis(800)).await;
    let html = browser
        .evaluate("document.documentElement.outerHTML")
        .await?
        .as_str()
        .unwrap_or("")
        .to_string();
    let user_agent = browser
        .evaluate("navigator.userAgent")
        .await?
        .as_str()
        .unwrap_or("")
        .to_string();
    let mut cookies_out: Vec<(String, String)> = browser
        .get_cookies()
        .await?
        .into_iter()
        .filter(|c| {
            let domain = c.get("domain").and_then(|v| v.as_str()).unwrap_or("");
            cookie_domain_matches(domain, &host)
        })
        .filter_map(|c| {
            let name = c.get("name")?.as_str()?.to_string();
            let value = c
                .get("value")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some((name, value))
        })
        .collect();
    cookies_out.sort();
    cookies_out.dedup();
    Ok(CfSolution {
        html,
        cookies: cookies_out,
        user_agent,
        turnstile_token,
        turnstile_sitekey,
    })
}

/// 点击 Turnstile widget：容器 element.click()（页面回调）＋ iframe 中心坐标真实点击
/// （CDP Input.dispatchMouseEvent——穿透 iframe 直达勾选框；真实 Turnstile 勾选在
/// iframe 内部，element.click() 无法穿透）。返回是否已执行点击；iframe 尚未布局
/// （0 尺寸）→ false（下次迭代重试）。
async fn click_turnstile(browser: &mut Browser) -> Result<bool> {
    let r = match browser.evaluate(TURNSTILE_CLICK_JS).await {
        Ok(r) => r,
        Err(_) => return Ok(false), // 导航中执行上下文切换——下次迭代重试
    };
    let how = r.get("how").and_then(|v| v.as_str()).unwrap_or("");
    if how != "iframe" {
        return Ok(how == "container");
    }
    let x = r.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let y = r.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let w = r.get("w").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let h = r.get("h").and_then(|v| v.as_f64()).unwrap_or(0.0);
    if w < 2.0 || h < 2.0 {
        return Ok(false); // widget iframe 尚未布局——下次迭代重试
    }
    // obscura 不投递 CDP 坐标事件——优先 iframe 内 JS 点击勾选框（跨源 iframe 需 frame
    // 上下文；checkbox 变体选择器扩展：.ctp-checkbox/input[type=checkbox]/[role=checkbox]/label）
    if let Ok(v) = browser
        .evaluate_in_frame("challenges.cloudflare.com", TURNSTILE_FRAME_CLICK_JS)
        .await
    {
        if v.as_bool().unwrap_or(false) {
            return Ok(true);
        }
    }
    browser
        .command(
            "Input.dispatchMouseEvent",
            json!({ "type": "mouseMoved", "x": x, "y": y, "button": "none" }),
        )
        .await?;
    browser
        .command(
            "Input.dispatchMouseEvent",
            json!({ "type": "mousePressed", "x": x, "y": y, "button": "left", "clickCount": 1 }),
        )
        .await?;
    browser
        .command(
            "Input.dispatchMouseEvent",
            json!({ "type": "mouseReleased", "x": x, "y": y, "button": "left", "clickCount": 1 }),
        )
        .await?;
    Ok(true)
}

/// 不支持的验证码类型检测（HTML 特征字符串；与 UNSUPPORTED_CAPTCHA_DETECT_JS 镜像——
/// 供单测断言/预检）：reCAPTCHA（g-recaptcha/recaptcha/api.js）→ Some("reCAPTCHA")；
/// hCaptcha（h-captcha/hcaptcha.com）→ Some("hCaptcha")；未命中 → None
pub fn unsupported_captcha_kind(body: &str) -> Option<&'static str> {
    let b = body.to_lowercase();
    if b.contains("g-recaptcha") || b.contains("recaptcha/api.js") {
        return Some("reCAPTCHA");
    }
    if b.contains("h-captcha") || b.contains("hcaptcha.com") || b.contains("/hcaptcha") {
        return Some("hCaptcha");
    }
    None
}

/// 从 HTML 提取 Turnstile sitekey（纯 Rust 解析——预检/日志用，与浏览器内
/// TURNSTILE_SITEKEY_JS 镜像）：依次尝试 ① data-sitekey 属性；② iframe src 的
/// sitekey query 参数；③ turnstile/api.js 脚本 URL 的 sitekey query 参数。
/// 未命中 → None。大小写不敏感（HTML 属性名不区分大小写）。
pub fn extract_turnstile_sitekey(html: &str) -> Option<String> {
    let lower = html.to_lowercase();
    // ① data-sitekey 属性（.cf-turnstile 容器或任意元素）
    for attr in ["data-sitekey=", "data-sitekey = "] {
        let mut from = 0usize;
        while let Some(idx) = lower[from..].find(attr) {
            let start = from + idx + attr.len();
            let rest = &html[start..];
            if let Some(v) = rest
                .strip_prefix('"')
                .and_then(|r| r.split('"').next())
                .or_else(|| rest.strip_prefix('\'').and_then(|r| r.split('\'').next()))
            {
                let v = v.trim();
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
            from = start + 1;
        }
    }
    // ② iframe src / ③ 脚本 src 的 sitekey query 参数（URL 编码值原样返回）
    for (open, close) in [("<iframe", ">"), ("<script", ">")] {
        let mut from = 0usize;
        while let Some(idx) = lower[from..].find(open) {
            let tag_start = from + idx;
            let Some(tag_end) = lower[tag_start..].find(close) else {
                break;
            };
            let tag = &html[tag_start..tag_start + tag_end];
            let tag_lower = tag.to_lowercase();
            let is_turnstile = tag_lower.contains("challenges.cloudflare.com")
                || tag_lower.contains("turnstile/api.js");
            if is_turnstile {
                if let Some(sk) = url_query_param(tag, "sitekey") {
                    return Some(sk);
                }
            }
            from = tag_start + tag_end + 1;
        }
    }
    None
}

/// 从 URL/标签文本提取 query 参数值（简单解析——无 URL 解析依赖；未命中 → None）
fn url_query_param(text: &str, key: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let mut from = 0usize;
    while let Some(idx) = lower[from..].find(&format!("{key}=")) {
        let start = from + idx + key.len() + 1;
        let rest = &text[start..];
        let v: String = rest
            .chars()
            .take_while(|c| *c != '&' && *c != '\"' && *c != '\'' && !c.is_whitespace())
            .collect();
        if !v.is_empty() {
            return Some(v);
        }
        from = start + 1;
    }
    None
}

/// cookie domain 是否匹配目标主机（含父域 `.example.com` 形式；裸后缀 com 等不匹配）
fn cookie_domain_matches(domain: &str, host: &str) -> bool {
    let d = domain.trim_start_matches('.');
    if d.is_empty() {
        return false;
    }
    host == d || (d.contains('.') && host.ends_with(&format!(".{d}")))
}

/// 数值提取（兼容两种 JSON 数字表示）：obscura 的 V8 数字一律序列化为浮点
/// （`Number(42.0)`）——`as_u64()` 对浮点返回 None；整数由 Chrome 路径产生。
/// 先试整数，再试浮点截断
fn num_u64(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_f64().map(|f| f as u64))
}

/// 页面验证码检测 JS（DOM 启发式）——返回 {kind, ...} 或 null
pub const DETECT_CAPTCHA_JS: &str = r#"
(function(){
  try {
  function visible(el){
    if(!el) return false;
    var r = el.getBoundingClientRect();
    return r.width > 2 && r.height > 2 && r.top < innerHeight && r.left < innerWidth;
  }
  // 图片验证码（img 特征：src/id/class/alt 含 captcha/vcode/verify/code/yzm/验证码）
  var imgs = document.querySelectorAll('img');
  for (var i = 0; i < imgs.length; i++) {
    var im = imgs[i];
    var ctx = ((im.src||'') + ' ' + (im.id||'') + ' ' + (im.className||'') + ' ' + (im.alt||'')).toLowerCase();
    if (/captcha|vcode|verify|yzm|checkcode|验证码|randimg|kaptcha/.test(ctx) && visible(im)) {
      var r = im.getBoundingClientRect();
      return {kind:'image', x:r.x, y:r.y, w:r.width, h:r.height, src:im.src};
    }
  }
  // 滑块（常见类名；取按钮 + 轨道容器）
  var sliderSels = ['.geetest_slider_button','.geetest_slider','.slide-verify','.slider-verify','.captcha-slider',
    '[class*="geetest"]','[class*="slide-verify"]','#nc_1_n1z','.nc_iconfont','.btn_slide','.drag-slider',
    '.verify-slider','[class*="jigsaw"]','[class*="slider-btn"]','[class*="slider-button"]','[class*="captcha-slider"]'];
  for (var i = 0; i < sliderSels.length; i++) {
    var el = document.querySelector(sliderSels[i]);
    if (visible(el)) {
      var r = el.getBoundingClientRect();
      // 轨道：按钮的祖先里最宽的那个（含 slider/geetest/captcha 类）
      var track = el, tr = r;
      var p = el.parentElement;
      while (p) {
        var pr = p.getBoundingClientRect();
        var pc = ((p.className||'') + ' ' + (p.id||'')).toLowerCase();
        if (pr.width > tr.width + 20 && /slider|geetest|captcha|nc_|verify|drag/.test(pc)) { track = p; tr = pr; }
        p = p.parentElement;
      }
      return {kind:'slider', x:r.x, y:r.y, w:r.width, h:r.height,
              trackX:tr.x, trackY:tr.y, trackW:tr.width, trackH:tr.height};
    }
  }
  // 点选（无法自动识别目标点——检测后返回 kind=click 由调用方决定降级）
  var clickSels = ['[class*="click-verify"]','[class*="clickCaptcha"]','[class*="tcaptcha"]','[class*="verify-point"]','[class*="points-verify"]'];
  for (var i = 0; i < clickSels.length; i++) {
    var el = document.querySelector(clickSels[i]);
    if (visible(el)) {
      var r = el.getBoundingClientRect();
      return {kind:'click', x:r.x, y:r.y, w:r.width, h:r.height};
    }
  }
  return null;
  } catch(e) { return null; }
})()
"#;

/// 登录表单填写 JS（原生 setter 触发 input/change 事件——Vue/React 表单可识别）
pub const FILL_FORM_JS: &str = r#"
(function(){
  function setVal(el, v){
    var proto = el.tagName === 'TEXTAREA' ? window.HTMLTextAreaElement.prototype : window.HTMLInputElement.prototype;
    var setter = Object.getOwnPropertyDescriptor(proto, 'value').set;
    setter.call(el, v);
    el.dispatchEvent(new Event('input', {bubbles:true}));
    el.dispatchEvent(new Event('change', {bubbles:true}));
  }
  var pw = document.querySelector('input[type="password"]');
  if (!pw) return {ok:false, reason:'no-password-input'};
  setVal(pw, 'PASSWORD');
  // 用户名：优先 user 相关 name/id，其次表单内第一个可见文本输入框
  var user = document.querySelector('input[name*="user" i], input[id*="user" i], input[name*="name" i], input[placeholder*="用户" i], input[placeholder*="账号" i]');
  if (!user) {
    var inputs = document.querySelectorAll('input');
    for (var i = 0; i < inputs.length; i++) {
      var it = inputs[i];
      if (it === pw) continue;
      var t = (it.type||'text').toLowerCase();
      if (t === 'text' || t === 'email' || t === '' || t === 'tel' || t === 'number') {
        var r = it.getBoundingClientRect();
        if (r.width > 2 && r.height > 2) { user = it; break; }
      }
    }
  }
  if (user) setVal(user, 'USERNAME');
  return {ok:true, filled:!!user};
  } catch(e) { return {ok:false, reason:'exception'}; }
})()
"#;

/// 表单提交 JS（优先 submit 按钮点击，其次 form.requestSubmit，最后 form.submit）
pub const SUBMIT_FORM_JS: &str = r#"
(function(){
  try {
  var btn = document.querySelector('button[type="submit"], input[type="submit"], button.btn-primary, button.btn, form button');
  if (btn) { btn.click(); return {ok:true, how:'click'}; }
  var form = document.querySelector('form');
  if (form) {
    if (form.requestSubmit) { form.requestSubmit(); return {ok:true, how:'requestSubmit'}; }
    form.submit(); return {ok:true, how:'submit'};
  }
  return {ok:false, reason:'no-form'};
  } catch(e) { return {ok:false, reason:'exception'}; }
})()
"#;

/// 验证码输入框填写 JS（调用前替换 'CAPTCHA' 占位符）
pub const FILL_CAPTCHA_JS: &str = r#"
(function(){
  try {
  var el = document.querySelector('input[name*="captcha" i], input[id*="captcha" i], input[placeholder*="验证码" i], input[placeholder*="captcha" i], input[type="text"][name*="code" i]');
  if (!el) return {ok:false, reason:'no-captcha-input'};
  var setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
  setter.call(el, 'CAPTCHA');
  el.dispatchEvent(new Event('input', {bubbles:true}));
  el.dispatchEvent(new Event('change', {bubbles:true}));
  return {ok:true};
  } catch(e) { return {ok:false, reason:'exception'}; }
})()
"#;

// ==================== 测试 ====================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_obscura_bin_candidates_env_first() {
        // 环境变量优先
        std::env::set_var("READER_OBSCURA_BIN", "C:/fake/obscura.exe");
        let c = obscura_bin_candidates();
        assert_eq!(c[0], PathBuf::from("C:/fake/obscura.exe"));
        std::env::remove_var("READER_OBSCURA_BIN");
    }

    #[test]
    fn test_normalize_cdp_url() {
        // http(s):// 端点（Playwright connectOverCDP 风格）→ ws(s):// + /devtools/browser
        assert_eq!(
            normalize_cdp_url("http://127.0.0.1:9222"),
            "ws://127.0.0.1:9222/devtools/browser"
        );
        assert_eq!(
            normalize_cdp_url("http://127.0.0.1:9222/"),
            "ws://127.0.0.1:9222/devtools/browser"
        );
        assert_eq!(
            normalize_cdp_url("https://obscura.example:9443"),
            "wss://obscura.example:9443/devtools/browser"
        );
        // 已带路径 → 仅换 scheme
        assert_eq!(
            normalize_cdp_url("http://127.0.0.1:9222/devtools/browser"),
            "ws://127.0.0.1:9222/devtools/browser"
        );
        // ws(s):// 直连 → 原样返回（含首尾空白清理）
        assert_eq!(
            normalize_cdp_url("ws://127.0.0.1:9222/devtools/browser"),
            "ws://127.0.0.1:9222/devtools/browser"
        );
        assert_eq!(
            normalize_cdp_url("  wss://h:1/devtools/browser  "),
            "wss://h:1/devtools/browser"
        );
    }

    #[test]
    fn test_launch_with_missing_exe_fails() {
        // 降级路径：浏览器不可用 → 明确错误（不 panic、不启动任何进程）
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(Browser::launch_with(PathBuf::from(
            "C:/definitely/not/exists.exe",
        )));
        let err = r.err().expect("启动不存在的浏览器应失败");
        assert!(err.to_string().contains("浏览器"));
    }

    #[test]
    fn test_cookies_to_string_stable_order() {
        let cookies = vec![
            json!({"name": "b", "value": "2"}),
            json!({"name": "a", "value": "1"}),
            json!({"name": "c", "value": ""}),
        ];
        assert_eq!(Browser::cookies_to_string(&cookies), "a=1; b=2; c=");
    }

    #[test]
    fn test_detect_captcha_js_shape() {
        // JS 常量完整性（语法冒烟：能被 boa 解析执行——不依赖浏览器）
        let vars = std::collections::HashMap::new();
        let r = crate::parser::js::eval_js(DETECT_CAPTCHA_JS, &vars);
        assert!(r.is_ok(), "检测 JS 应可执行（无 DOM 时返回 null/空）");
    }

    #[test]
    fn test_cf_challenge_state_js_shape() {
        // 冒烟：JS 常量可被 boa 解析执行（无 DOM 时返回 challenge=true 状态对象）
        let vars = std::collections::HashMap::new();
        let r = crate::parser::js::eval_js(CF_CHALLENGE_STATE_JS, &vars);
        assert!(r.is_ok(), "质询状态 JS 应可执行");
    }

    #[test]
    fn test_turnstile_js_constants_shape() {
        // 冒烟：Turnstile 检测/点击/token 读取/sitekey 提取/execute 重试/iframe 点击/
        // render 捕获 hook/stealth 注入 JS 均可被 boa 解析执行
        // （无 DOM 环境——检测返回 false、点击返回 no-element、token 返回空串）
        let vars = std::collections::HashMap::new();
        for (name, js) in [
            ("TURNSTILE_DETECT_JS", TURNSTILE_DETECT_JS),
            ("TURNSTILE_CLICK_JS", TURNSTILE_CLICK_JS),
            ("TURNSTILE_TOKEN_JS", TURNSTILE_TOKEN_JS),
            ("TURNSTILE_HOOK_JS", TURNSTILE_HOOK_JS),
            ("TURNSTILE_SITEKEY_JS", TURNSTILE_SITEKEY_JS),
            ("TURNSTILE_EXECUTE_JS", TURNSTILE_EXECUTE_JS),
            ("TURNSTILE_FRAME_CLICK_JS", TURNSTILE_FRAME_CLICK_JS),
            (
                "UNSUPPORTED_CAPTCHA_DETECT_JS",
                UNSUPPORTED_CAPTCHA_DETECT_JS,
            ),
            ("STEALTH_JS", STEALTH_JS),
        ] {
            let r = crate::parser::js::eval_js(js, &vars);
            assert!(r.is_ok(), "{name} 应可被 boa 解析执行");
        }
    }

    #[test]
    fn test_extract_turnstile_sitekey() {
        // ① data-sitekey 属性（.cf-turnstile 容器）
        assert_eq!(
            extract_turnstile_sitekey(
                r#"<div class="cf-turnstile" data-sitekey="0x4AAAAAAA-mockkey"></div>"#
            )
            .as_deref(),
            Some("0x4AAAAAAA-mockkey")
        );
        // 属性名大小写不敏感 + 单引号
        assert_eq!(
            extract_turnstile_sitekey(r#"<DIV DATA-SITEKEY='0x4BBBBB'>"#).as_deref(),
            Some("0x4BBBBB")
        );
        // ② iframe src query
        assert_eq!(
            extract_turnstile_sitekey(
                r#"<iframe src="https://challenges.cloudflare.com/cdn-cgi/challenge-platform/h/b/orchestrate/turnstile/v1?x=1&sitekey=0x4CCCCC&y=2">"#
            )
            .as_deref(),
            Some("0x4CCCCC")
        );
        // ③ turnstile/api.js 脚本 URL query（手动加载形式）
        assert_eq!(
            extract_turnstile_sitekey(
                r#"<script src="https://challenges.cloudflare.com/turnstile/v0/api.js?sitekey=0x4DDDDD" async defer></script>"#
            )
            .as_deref(),
            Some("0x4DDDDD")
        );
        // 非 turnstile iframe 的 sitekey 不命中（只认 challenges.cloudflare.com）
        assert_eq!(
            extract_turnstile_sitekey(r#"<iframe src="https://other.com/x?sitekey=0x9999">"#),
            None
        );
        // 未命中 → None
        assert_eq!(extract_turnstile_sitekey("<html>hello</html>"), None);
        assert_eq!(extract_turnstile_sitekey(""), None);
        // 空属性值 → 继续找下一来源
        assert_eq!(
            extract_turnstile_sitekey(
                r#"<div data-sitekey=""></div><iframe src="https://challenges.cloudflare.com/turnstile/v1?sitekey=0x4EEEEE">"#
            )
            .as_deref(),
            Some("0x4EEEEE")
        );
    }

    #[test]
    fn test_obscura_serve_command_proxy_arg() {
        // 无代理：不带 --proxy；P1：不传 --allow-private-network（obscura 默认禁 RFC1918）
        let cmd = obscura_serve_command(std::path::Path::new("obscura"), 12345, None);
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["serve", "--port", "12345", "--stealth"]);
        assert!(
            !args.iter().any(|a| a.contains("allow-private-network")),
            "不应传 --allow-private-network（P1 内网导航收紧）: {args:?}"
        );
        // 有代理：--proxy 紧跟代理地址
        let cmd = obscura_serve_command(
            std::path::Path::new("obscura"),
            12345,
            Some("socks5://127.0.0.1:1080"),
        );
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec![
                "serve",
                "--port",
                "12345",
                "--stealth",
                "--proxy",
                "socks5://127.0.0.1:1080",
            ]
        );
        // 空/纯空白代理 → 不附加（等价无代理）
        let cmd = obscura_serve_command(std::path::Path::new("obscura"), 1, Some("  "));
        let n = cmd.get_args().count();
        assert_eq!(n, 4, "空白代理不应附加 --proxy（实际参数数 {n}）");
    }

    #[test]
    fn test_unsupported_captcha_kind() {
        // reCAPTCHA：g-recaptcha 容器 / recaptcha/api.js 脚本
        assert_eq!(
            unsupported_captcha_kind("<div class=\"g-recaptcha\" data-sitekey=\"x\"></div>"),
            Some("reCAPTCHA")
        );
        assert_eq!(
            unsupported_captcha_kind(
                "<script src=\"https://www.google.com/recaptcha/api.js\"></script>"
            ),
            Some("reCAPTCHA")
        );
        // hCaptcha：h-captcha 容器 / hcaptcha.com iframe
        assert_eq!(
            unsupported_captcha_kind("<div class=\"h-captcha\" data-sitekey=\"x\"></div>"),
            Some("hCaptcha")
        );
        assert_eq!(
            unsupported_captcha_kind("<iframe src=\"https://hcaptcha.com/\"></iframe>"),
            Some("hCaptcha")
        );
        // 大小写不敏感
        assert_eq!(
            unsupported_captcha_kind("<DIV CLASS=\"G-RECAPTCHA\">"),
            Some("reCAPTCHA")
        );
        // 未命中（Turnstile/普通页）→ None
        assert_eq!(
            unsupported_captcha_kind("<div class=\"cf-turnstile\"></div>"),
            None
        );
        assert_eq!(unsupported_captcha_kind("<html>hello</html>"), None);
        assert_eq!(unsupported_captcha_kind(""), None);
    }

    #[test]
    fn test_cookie_domain_matches() {
        // 精确主机
        assert!(cookie_domain_matches("a.com", "a.com"));
        // 父域（点前缀）
        assert!(cookie_domain_matches(".a.com", "a.com"));
        assert!(cookie_domain_matches(".a.com", "www.a.com"));
        // 不匹配
        assert!(!cookie_domain_matches("b.com", "a.com"));
        assert!(!cookie_domain_matches("", "a.com"));
        assert!(!cookie_domain_matches("com", "a.com")); // 裸后缀不匹配
        assert!(!cookie_domain_matches(".com", "a.com"));
        assert!(!cookie_domain_matches("a.com.evil.com", "a.com"));
    }

    // ---- P1：CF 会话锁粒度（每用户会话锁）+ WS 连接泄漏 + 内网导航校验 ----

    /// 微型假 CDP 服务端：接受一个 WS 连接；对任意命令回 `{"id":..,"result":..}`；
    /// createTarget/attachToTarget 返回必要字段；客户端断开后 notify（供断言用）
    async fn serve_fake_cdp(closed: std::sync::Arc<tokio::sync::Notify>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
                return;
            };
            let (mut sink, mut recv) = ws.split();
            loop {
                tokio::select! {
                    msg = recv.next() => {
                        match msg {
                            Some(Ok(Message::Text(t))) => {
                                let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
                                let id = v.get("id").cloned().unwrap_or(Value::Null);
                                let method = v
                                    .get("method")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("");
                                let mut result = json!({});
                                if method == "Target.createTarget" {
                                    result = json!({ "targetId": "t1" });
                                } else if method == "Target.attachToTarget" {
                                    result = json!({ "sessionId": "s1" });
                                }
                                let _ = sink
                                    .send(Message::Text(
                                        json!({ "id": id, "result": result }).to_string(),
                                    ))
                                    .await;
                            }
                            // Close/Err/None：客户端已断开
                            _ => break,
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(20)) => break,
                }
            }
            closed.notify_waiters();
        });
        format!("ws://{addr}/devtools/browser")
    }

    /// P1 WS 连接泄漏：Browser Drop → 关闭信号 → reader 任务退出 → WS 关闭
    /// （服务端在超时内观察到连接断开）——不残留悬挂 reader 任务/连接
    #[tokio::test]
    async fn test_browser_drop_closes_ws() {
        std::env::set_var("READER_CDP_NO_STEALTH", "1");
        let closed = std::sync::Arc::new(tokio::sync::Notify::new());
        let url = serve_fake_cdp(closed.clone()).await;
        let browser = Browser::connect(&url).await.expect("假 CDP 连接应成功");
        drop(browser);
        tokio::time::timeout(Duration::from_secs(5), closed.notified())
            .await
            .expect("Drop 后服务端应在 5s 内观察到连接关闭");
        std::env::remove_var("READER_CDP_NO_STEALTH");
    }

    /// P1 CF 全局锁优化：锁粒度降到会话级——同一 ns 串行（第二个求解等待），
    /// 不同 ns 并行（互不阻塞）；条目复用/清空语义
    #[tokio::test]
    async fn test_cf_session_per_ns_lock_isolation() {
        shutdown_cf_session().await; // 清场（幂等）
                                     // 同 ns 复用同一条目；不同 ns 独立条目
        let a1 = cf_session_entry("ns-a").await;
        let a2 = cf_session_entry("ns-a").await;
        let b = cf_session_entry("ns-b").await;
        assert!(Arc::ptr_eq(&a1, &a2), "同 ns 应复用同一条目");
        assert!(!Arc::ptr_eq(&a1, &b), "不同 ns 应独立条目");
        // shutdown → 清空 → 重新取是新条目
        shutdown_cf_session().await;
        let a3 = cf_session_entry("ns-a").await;
        assert!(!Arc::ptr_eq(&a1, &a3), "清空后应重建条目");
        // 同 ns 串行：持锁期间第二个获取者阻塞
        let held = a3.inner.lock().await;
        let waiter = tokio::spawn(async move {
            let binding = cf_session_entry("ns-a").await;
            let _g = binding.inner.lock().await;
            true
        });
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert!(
            !waiter.is_finished(),
            "同 ns 第二个求解应等待会话锁（串行）"
        );
        drop(held);
        assert!(tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("释放后等待者应完成")
            .unwrap());
        // 不同 ns 互不阻塞：ns-a 持锁期间 ns-b 立即可取（短超时——若阻塞会超时）
        let held_a = a3.inner.lock().await;
        let got_b = tokio::time::timeout(Duration::from_millis(300), b.inner.lock()).await;
        assert!(got_b.is_ok(), "不同 ns 不应互相阻塞");
        drop(held_a);
        shutdown_cf_session().await; // 清场
    }

    /// P1 CF 会话回收：闲置（无会话/超时）条目被回收；新鲜条目保留；
    /// 正在求解（会话锁被占用）的条目跳过
    #[tokio::test]
    async fn test_reap_idle_cf_sessions() {
        shutdown_cf_session().await; // 清场
        let stale_entry = Arc::new(CfSessionEntry::new()); // 无会话（None）→ 闲置
        let fresh_entry = Arc::new(CfSessionEntry::new());
        // fresh：真实（假 CDP）浏览器会话，last_used = 现在
        let closed = std::sync::Arc::new(tokio::sync::Notify::new());
        let url = serve_fake_cdp(closed).await;
        let browser = Browser::connect(&url).await.expect("假 CDP 连接应成功");
        *fresh_entry.inner.lock().await = Some(CfSession {
            browser,
            last_used: std::time::Instant::now(),
            proxy: None,
        });
        {
            let mut map = CF_SESSION.write().await;
            map.insert("stale-ns".to_string(), stale_entry.clone());
            map.insert("fresh-ns".to_string(), fresh_entry.clone());
        }
        reap_idle_cf_sessions().await;
        {
            let map = CF_SESSION.read().await;
            assert!(!map.contains_key("stale-ns"), "闲置（无会话）条目应被回收");
            assert!(map.contains_key("fresh-ns"), "新鲜条目应保留");
        }
        // 正在求解（锁被占用）的闲置条目跳过
        let busy_entry = Arc::new(CfSessionEntry::new());
        let busy_guard = busy_entry.inner.lock().await;
        {
            let mut map = CF_SESSION.write().await;
            map.insert("busy-ns".to_string(), busy_entry.clone());
        }
        reap_idle_cf_sessions().await;
        {
            let map = CF_SESSION.read().await;
            assert!(map.contains_key("busy-ns"), "求解中的条目不应被回收");
        }
        drop(busy_guard);
        shutdown_cf_session().await; // 清场（杀假浏览器进程句柄——child=None 仅关 WS）
    }

    /// P1 SSRF：solve_captcha_inner 入口拒绝私网/回环目标（书源 URL 校验后才允许
    /// 浏览器导航）——camoufox 优先路径同样被拦截（校验在最前）
    #[tokio::test]
    async fn test_solve_rejects_private_url() {
        let _g = crate::service::crawler::ssrf_allow_private_guard(false);
        // 禁用 camoufox 兜底，确保不发起任何外部调用即被拦截
        std::env::set_var("READER_CAMOUFOX_DISABLE", "1");
        let err = solve_cf_challenge("default", "http://127.0.0.1:1/", &[], 5_000, None)
            .await
            .unwrap_err();
        std::env::remove_var("READER_CAMOUFOX_DISABLE");
        assert!(
            err.to_string().contains("已拦截"),
            "求解入口应拦截私网目标: {err}"
        );
        // 公网 URL 不在此处拦截（继续走求解链——浏览器不可用报浏览器错误）
        std::env::set_var("READER_CAMOUFOX_DISABLE", "1");
        let err2 = solve_cf_challenge("default", "https://cf.example.com/x", &[], 5_000, None)
            .await
            .unwrap_err();
        std::env::remove_var("READER_CAMOUFOX_DISABLE");
        assert!(
            !err2.to_string().contains("已拦截"),
            "公网 URL 不应被 SSRF 拦截: {err2}"
        );
    }
}

/// 编译期断言：Browser 必须 Send（axum Handler 要求 future Send）
#[allow(dead_code)]
const _: () = {
    fn assert_send<T: Send>() {}
    fn check() {
        assert_send::<Browser>();
    }
};

#[cfg(test)]
mod send_tests {
    use super::*;

    /// 定位非 Send 类型：tokio::spawn 要求 future Send
    #[tokio::test]
    async fn test_launch_future_is_send() {
        let h = tokio::spawn(async {
            let _ = Browser::launch_with(PathBuf::from("C:/nope.exe")).await;
        });
        let _ = h.await;
    }
}
