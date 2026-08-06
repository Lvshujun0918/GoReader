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
use std::sync::{mpsc, LazyLock};
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
    pending: std::sync::Arc<std::sync::Mutex<HashMap<u64, tokio::sync::oneshot::Sender<Result<Value, String>>>>>,
    next_id: u64,
    session_id: Option<String>,
}

impl Drop for Browser {
    fn drop(&mut self) {
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

/// spawn `obscura serve --port <port> --stealth` → 等待 stdout banner
/// （`CDP server: ws://127.0.0.1:{port}/devtools/browser`——serve 的 --quiet 只关日志，
/// banner 无条件打印）→ 连接 → 会话初始化。任何失败均杀进程并返回错误
/// （launch_with 换随机端口重试）。
///
/// 参数说明：obscura 为纯 headless 引擎（**无 --headless 参数**——headless 是其固有
/// 形态）；`--stealth` 启用反检测 + BoringSSL TLS 指纹模拟（stealth 构建；lean 构建
/// 传该参数仅打警告、其余功能正常）；`--allow-private-network` 放开本地/内网导航
/// （obscura 默认禁 RFC1918——与旧 Chrome 路径行为一致，SSRF 面持平）
async fn spawn_serve_and_connect(exe: &std::path::Path, port: u16) -> Result<Browser> {
    let mut cmd = Command::new(exe);
    cmd.arg("serve")
        .arg("--port")
        .arg(port.to_string())
        .arg("--stealth")
        .arg("--allow-private-network")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd
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
                    return Err(anyhow!("obscura 进程启动失败（{status}）——端口 {port} 可能被占用"));
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

/// 连接建立后的会话初始化（target 创建/附加、域启用、stealth 注入）——spawn 与
/// READER_OBSCURA_URL 直连两条路径共用
async fn init_session(ws: WsStream) -> Result<Browser> {
    let (sink, stream) = ws.split();
    // reader 任务：按 id 路由响应到对应 oneshot（events 忽略）
    let pending: std::sync::Arc<std::sync::Mutex<HashMap<u64, tokio::sync::oneshot::Sender<Result<Value, String>>>>> =
        std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()));
    let pending_task = std::sync::Arc::clone(&pending);
    tokio::spawn(async move {
        let mut stream = stream;
        while let Some(msg) = stream.next().await {
            let Ok(msg) = msg else { break };
            let text = match msg {
                Message::Text(t) => t.to_string(),
                Message::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
                _ => continue,
            };
            let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
            let Some(id) = v.get("id").and_then(|i| i.as_u64()) else { continue };
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
    });

    let mut browser = Browser {
        child: None,
        sink,
        pending,
        next_id: 0,
        session_id: None,
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
        .command("Target.attachToTarget", json!({ "targetId": target_id, "flatten": true }))
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
    Ok(browser)
}

impl Browser {
    /// 启动浏览器（obscura 唯一后端）：`READER_OBSCURA_URL` 已配置 → 连接既有 CDP
    /// 服务（不 spawn、不接管进程）；否则发现 obscura 可执行文件并 spawn
    /// `obscura serve --port <随机> --stealth`。未配置/不可用 → Err（提示手动 Cookie 流程）
    pub async fn launch() -> Result<Browser> {
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
        Browser::launch_with(exe).await
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
        if !exe.exists() {
            return Err(anyhow!("obscura 可执行文件不存在（{}）——无法使用浏览器自动登录", exe.display()));
        }
        let mut last_err: Option<anyhow::Error> = None;
        for _ in 0..3 {
            // 随机端口（20000-49999）——与已有服务/其他实例冲突时 obscura 退出，
            // 换端口重试（概率极低，防御性处理）
            let port = 20000 + rand::random::<u16>() % 30000;
            match spawn_serve_and_connect(&exe, port).await {
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

    /// Runtime.evaluate（returnByValue，awaitPromise）→ 返回值
    pub async fn evaluate(&mut self, expression: &str) -> Result<Value> {
        let r = self
            .command(
                "Runtime.evaluate",
                json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }),
            )
            .await?;
        // 异常时 result 里带 exceptionDetails，value 缺失
        if r.get("exceptionDetails").is_some() {
            return Err(anyhow!(
                "页面 JS 异常: {}",
                r.get("exceptionDetails").unwrap_or(&Value::Null)
            ));
        }
        Ok(r.get("result")
            .and_then(|v| v.get("value"))
            .cloned()
            .unwrap_or(Value::Null))
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

    /// 鼠标拖拽（滑块：按下 → 贝塞尔轨迹移动（随机噪声+微停）→ 释放）
    pub async fn mouse_drag(&mut self, x1: f64, y1: f64, x2: f64, y2: f64) -> Result<()> {
        self.command(
            "Input.dispatchMouseEvent",
            json!({ "type": "mousePressed", "x": x1, "y": y1, "button": "left", "clickCount": 1 }),
        )
        .await?;
        // 人类轨迹：三次贝塞尔 + 随机噪声 + 随机步数/微停
        let steps = 28 + rand::random::<u64>() % 25;
        let ctrl1 = (
            x1 + (x2 - x1) * 0.4 + rand::random::<f64>() * 20.0 - 10.0,
            y1,
        );
        let ctrl2 = (
            x1 + (x2 - x1) * 0.6 + rand::random::<f64>() * 20.0 - 10.0,
            y2,
        );
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
            // 随机噪声（±2px）+ 微停（6-28ms）
            let nx = x + rand::random::<f64>() * 4.0 - 2.0;
            let ny = y + rand::random::<f64>() * 4.0 - 2.0;
            self.command(
                "Input.dispatchMouseEvent",
                json!({ "type": "mouseMoved", "x": nx, "y": ny, "button": "none" }),
            )
            .await?;
            tokio::time::sleep(Duration::from_millis(6 + rand::random::<u64>() % 22)).await;
        }
        self.command(
            "Input.dispatchMouseEvent",
            json!({ "type": "mouseReleased", "x": x2, "y": y2, "button": "left", "clickCount": 1 }),
        )
        .await?;
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
/// Turnstile token 轮询上限（任务要求最多 30s——仅对真 Turnstile widget 生效；
/// 经典 CF 质询误命中 iframe 特征时不受此限，仍按调用方 max_wait_ms）
const TURNSTILE_MAX_WAIT_MS: u64 = 30_000;

/// 会话浏览器闲置回收时限（最后一次使用后 TTL 内无新请求 → 杀进程释放资源）
const CF_SESSION_IDLE_TTL: Duration = Duration::from_secs(300);

/// 全局 CF 质询求解会话：惰性启动（首次 CF 命中时 launch）、并发互斥（tokio Mutex 排队）、
/// 超时/异常自动重启（出错即弃用实例，下次调用重新 launch）
struct CfSession {
    browser: Browser,
    last_used: std::time::Instant,
}

/// 按用户命名空间隔离的浏览器会话池（安全：同一实例多用户共享一个浏览器实例会
/// 泄漏登录态 cookie——A 用户的 cf_clearance/登录 cookie 残留在浏览器，B 用户的
/// 质询求解会带着 A 的 cookie 请求。每 ns 独立实例（独立 user-data-dir），
/// 求解前还清空浏览器 cookie 再注入本用户 cookie（双保险）。
static CF_SESSION: LazyLock<tokio::sync::Mutex<std::collections::HashMap<String, CfSession>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(std::collections::HashMap::new()));

/// 闲置回收：每次求解成功后挂一个定时任务——TTL 内无新使用则弃用会话（Drop 杀进程+清目录）。
/// 并发触发多个无害（幂等：last_used 刷新后条件不满足即跳过）
fn spawn_cf_session_reaper() {
    tokio::spawn(async {
        tokio::time::sleep(CF_SESSION_IDLE_TTL).await;
        let mut guard = CF_SESSION.lock().await;
        guard.retain(|_ns, s| s.last_used.elapsed() < CF_SESSION_IDLE_TTL); // Drop 过期 → 杀进程+清目录
    });
}

/// 显式关闭 CF 求解会话（集成测试/优雅停机用；幂等）
pub async fn shutdown_cf_session() {
    let mut guard = CF_SESSION.lock().await;
    guard.clear();
}

/// 解 CF 质询（进程内浏览器 CDP；会话级浏览器实例——惰性启动/互斥/异常自动重启）。
/// CF 专用入口（不含滑块分支——登录页滑块走 solve_captcha 或登录流程）。
///
/// 流程：启动/复用会话浏览器（独立 user-data-dir，退出自动清理）→ Network.setCookies
/// 注入 cookies → Page.navigate → 质询等待循环（每 500ms 求值 document：challenge 特征
/// （#challenge-form/#cf-chl-*/iframe[src*=challenges.cloudflare]/title=="Just a moment"）
/// 消失或 URL 变化到目标页；Turnstile 分支：点击容器 + 每 800ms 轮询 token（最多 30s）
/// → 提取最终 HTML → Storage.getCookies（该站点全部，含 cf_clearance）→ {html, cookies,
/// userAgent, turnstile_token}。超时/浏览器不可用返回明确错误。
///
/// 服务端静默语义：全程 headless（--headless=new），不弹任何窗口/不等待用户——
/// 求解失败返回明确错误，由调用方（书源 JS 等）自行兜底。
pub async fn solve_cf_challenge(
    ns: &str,
    url: &str,
    cookies: &[(String, String)],
    max_wait_ms: u64,
) -> Result<CfSolution> {
    solve_captcha_inner(ns, url, cookies, max_wait_ms, false).await
}

/// 统一验证码求解入口（服务端静默 headless——不弹浏览器给用户）：一个函数覆盖全部验证码
/// 类型——内部按检测分派：
/// - CF JS 质询（challenge-platform/#challenge-form/"Just a moment"）→ 等待循环（JS 自解）
/// - Turnstile（.cf-turnstile/[name=cf-turnstile-response]/challenges.cloudflare.com iframe）
///   → 点击容器（element.click + iframe 中心坐标）→ 每 800ms 轮询 token（最多 30s）
/// - 登录页滑块（DETECT_CAPTCHA_JS kind=slider）→ 贝塞尔轨迹拖拽（人类轨迹，与登录流程一致）
/// - reCAPTCHA/hCaptcha → 明确错误（不支持自动求解）
/// 会话管理/超时语义与 solve_cf_challenge 一致。书源 JS 的 java.startBrowserAwait shim
/// 应路由到此入口（成功返回 body/cookies，失败返回明确错误）。
pub async fn solve_captcha(
    ns: &str,
    url: &str,
    cookies: &[(String, String)],
    max_wait_ms: u64,
) -> Result<CfSolution> {
    solve_captcha_inner(ns, url, cookies, max_wait_ms, true).await
}

/// 统一求解内部实现（include_slider：solve_captcha 启用滑块分派，CF 专用入口不启用）。
/// 求解链（GAP 175）：内置浏览器 CDP → camoufox（HTTP 后端 scripts/camoufox_solver.py）
/// → 仍失败才报错（合并错误）；`READER_CAMOUFOX_FIRST=1` 时 camoufox 优先。
async fn solve_captcha_inner(
    ns: &str,
    url: &str,
    cookies: &[(String, String)],
    max_wait_ms: u64,
    include_slider: bool,
) -> Result<CfSolution> {
    // ① camoufox 优先模式（READER_CAMOUFOX_FIRST=1）：先试 HTTP 后端，失败转 CDP
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

    let mut guard = CF_SESSION.lock().await;
    // 惰性启动 / 复用（每用户命名空间独立浏览器实例——防跨用户 cookie 泄漏）
    if !guard.contains_key(ns) {
        let browser = match Browser::launch().await {
            Ok(b) => b,
            Err(launch_err) => {
                let cdp_err = anyhow!("CF 质询需浏览器环境：{launch_err:#}");
                drop(guard);
                // 无内置浏览器 → camoufox 兜底（默认启用；仍失败合并错误）
                return finish_with_fallback(url, cookies, max_wait_ms, &cdp_err, camo_err).await;
            }
        };
        guard.insert(
            ns.to_string(),
            CfSession {
                browser,
                last_used: std::time::Instant::now(),
            },
        );
    }
    let result = {
        let session = guard.get_mut(ns).expect("just initialized");
        session.last_used = std::time::Instant::now();
        solve_with(
            &mut session.browser,
            url,
            cookies,
            max_wait_ms,
            include_slider,
        )
        .await
    };
    match result {
        Ok(sol) => {
            spawn_cf_session_reaper();
            Ok(sol)
        }
        Err(e) => {
            // 超时/异常 → 弃用该用户实例（Drop 杀进程 + 清 user-data-dir），下次自动重启
            guard.remove(ns);
            drop(guard);
            // ② CDP 失败 → camoufox 兜底（仍失败才报错）
            finish_with_fallback(url, cookies, max_wait_ms, &e, camo_err).await
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
pub async fn evaluate_in_session(ns: &str, expression: &str) -> Result<Value> {
    let mut guard = CF_SESSION.lock().await;
    let Some(session) = guard.get_mut(ns) else {
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
    // Turnstile token 轮询上限（任务要求最多 30s——仅对真 Turnstile widget 生效；
    // 经典 CF 质询误命中 iframe 特征时不受此限，仍按 max_wait_ms）
    let turnstile_deadline =
        std::time::Instant::now() + Duration::from_millis(max_wait_ms.min(TURNSTILE_MAX_WAIT_MS));
    let mut turnstile_mode = false;
    let mut turnstile_widget = false; // 页面确有 Turnstile widget（容器/input/标题/turnstile iframe）
    let mut turnstile_clicked = false;
    let mut turnstile_token: Option<String> = None;
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
        //    也内嵌该 iframe，误判会触发 30s token 轮询上限并破坏经典质询等待循环）
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
                    tracing::warn!(
                        "Turnstile 检测命中 {url}: container={} input={} title={} iframeTs={}",
                        d.get("hasContainer")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                        d.get("hasInput").and_then(|v| v.as_bool()).unwrap_or(false),
                        d.get("hasTitle").and_then(|v| v.as_bool()).unwrap_or(false),
                        d.get("iframeIsTurnstile")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
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
                }
            }
        }

        // ③ Turnstile 流程：点击容器 → 轮询 token（每 800ms）——token 非空即通过
        if turnstile_mode {
            if !turnstile_clicked {
                if click_turnstile(browser).await? {
                    turnstile_clicked = true;
                }
                tokio::time::sleep(Duration::from_millis(TURNSTILE_POLL_MS)).await;
                continue;
            }
            if let Ok(v) = browser.evaluate(TURNSTILE_TOKEN_JS).await {
                if let Some(s) = v.as_str().filter(|s| !s.is_empty()) {
                    turnstile_token = Some(s.to_string());
                    break;
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
                let body_children = state
                    .get("bodyChildren")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
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
                let body_children = state
                    .get("bodyChildren")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
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

/// cookie domain 是否匹配目标主机（含父域 `.example.com` 形式；裸后缀 com 等不匹配）
fn cookie_domain_matches(domain: &str, host: &str) -> bool {
    let d = domain.trim_start_matches('.');
    if d.is_empty() {
        return false;
    }
    host == d || (d.contains('.') && host.ends_with(&format!(".{d}")))
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
        assert_eq!(normalize_cdp_url("http://127.0.0.1:9222"), "ws://127.0.0.1:9222/devtools/browser");
        assert_eq!(normalize_cdp_url("http://127.0.0.1:9222/"), "ws://127.0.0.1:9222/devtools/browser");
        assert_eq!(normalize_cdp_url("https://obscura.example:9443"), "wss://obscura.example:9443/devtools/browser");
        // 已带路径 → 仅换 scheme
        assert_eq!(normalize_cdp_url("http://127.0.0.1:9222/devtools/browser"), "ws://127.0.0.1:9222/devtools/browser");
        // ws(s):// 直连 → 原样返回（含首尾空白清理）
        assert_eq!(normalize_cdp_url("ws://127.0.0.1:9222/devtools/browser"), "ws://127.0.0.1:9222/devtools/browser");
        assert_eq!(normalize_cdp_url("  wss://h:1/devtools/browser  "), "wss://h:1/devtools/browser");
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
        // 冒烟：Turnstile 检测/点击/token 读取/stealth 注入 JS 均可被 boa 解析执行
        // （无 DOM 环境——检测返回 false、点击返回 no-element、token 返回空串）
        let vars = std::collections::HashMap::new();
        for (name, js) in [
            ("TURNSTILE_DETECT_JS", TURNSTILE_DETECT_JS),
            ("TURNSTILE_CLICK_JS", TURNSTILE_CLICK_JS),
            ("TURNSTILE_TOKEN_JS", TURNSTILE_TOKEN_JS),
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
