//! Headless 浏览器自动化（CDP over WebSocket——轻量实现，复用 tokio-tungstenite）
//!
//! 用于书源登录（mode=browser）：滑块验证码自动拖拽（人类轨迹：贝塞尔曲线 + 随机噪声 +
//! 微停）、图片验证码截图（前端显示后回填）、登录表单自动填写、CDP 提取 cookie 存库。
//!
//! 浏览器发现：`READER_CHROME_PATH` 优先 → Windows 自动检测 Edge/Chrome 常见路径 →
//! Linux 检测 chromium/chromium-browser/google-chrome；找不到则功能禁用
//! （登录回退手动 Cookie 流程，接口报"未安装浏览器"）。
//!
//! 说明：任务原始方案为 chromiumoxide crate；其依赖（websocket 0.27 等）编译重且存在
//! 工具链兼容风险，故采用**同协议（CDP）的轻量实现**（仅用已在本项目编译的
//! tokio-tungstenite），功能等价（导航/求值/鼠标拖拽/截图/cookie），编译开销近似为零。

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
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

// ==================== 浏览器发现 ====================

/// 候选浏览器路径/命令（环境变量优先，其次平台常见路径）——纯函数，供测试
pub fn browser_candidates() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("READER_CHROME_PATH") {
        let p = p.trim();
        if !p.is_empty() {
            v.push(PathBuf::from(p));
        }
    }
    #[cfg(windows)]
    {
        for c in [
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
            r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        ] {
            v.push(PathBuf::from(c));
        }
    }
    #[cfg(not(windows))]
    {
        for name in ["chromium", "chromium-browser", "google-chrome", "google-chrome-stable"] {
            v.push(PathBuf::from(name));
        }
    }
    v
}

/// 发现可用浏览器（第一个存在的路径；Windows 下命令名也可用 where 探测——
/// 简化：仅接受存在的文件路径）。未找到 → None（功能禁用）
pub fn discover_browser() -> Option<PathBuf> {
    browser_candidates().into_iter().find(|p| p.exists())
}

/// 浏览器是否可用（登录接口快速短路用）
pub fn is_browser_available() -> bool {
    discover_browser().is_some()
}

// ==================== CDP 客户端 ====================

type WsStream = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// CDP 浏览器会话（launch → 命令 → drop 时杀进程）
pub struct Browser {
    child: Option<Child>,
    user_data_dir: PathBuf,
    sink: futures::stream::SplitSink<WsStream, Message>,
    /// 待响应命令表（reader 任务按 id 路由回 oneshot）——Arc 共享，避免跨 await 持有非 Sync 的 Receiver
    pending: std::sync::Arc<std::sync::Mutex<HashMap<u64, tokio::sync::oneshot::Sender<Result<Value, String>>>>>,
    next_id: u64,
    session_id: Option<String>,
}

impl Drop for Browser {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.user_data_dir);
    }
}

impl Browser {
    /// 启动浏览器（自动发现；未安装 → Err（提示手动 Cookie 流程））
    pub async fn launch() -> Result<Browser> {
        let exe = discover_browser()
            .ok_or_else(|| anyhow!("未安装浏览器（Chrome/Edge）——无法使用浏览器自动登录，请在书源设置中粘贴 Cookie"))?;
        Browser::launch_with(exe).await
    }

    /// 用指定可执行文件启动（测试降级路径用）
    pub async fn launch_with(exe: PathBuf) -> Result<Browser> {
        if !exe.exists() {
            return Err(anyhow!("浏览器可执行文件不存在（{}）——无法使用浏览器自动登录", exe.display()));
        }
        // 独立 user-data-dir（避免与用户浏览器冲突；退出时清理）
        let user_data_dir = std::env::temp_dir().join(format!(
            "reader-cdp-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let mut cmd = Command::new(&exe);
        cmd.args([
            "--headless=new",
            "--remote-debugging-port=0",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-gpu",
            "--disable-extensions",
            "--disable-dev-shm-usage",
            "--remote-allow-origins=*",
            "--window-size=1280,900",
        ])
        .arg(format!("--user-data-dir={}", user_data_dir.display()))
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
        let spawned = cmd.spawn().map_err(|e| anyhow!("启动浏览器失败（{}）: {e}", exe.display()))?;
        let mut child = Some(spawned);

        // 从 stderr 解析 DevTools ws 地址（--remote-debugging-port=0 自动选端口）
        let stderr = child.as_mut().expect("stderr piped").stderr.take().expect("stderr piped");
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stderr).lines() {
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
                    if let Some(idx) = line.find("DevTools listening on ws://") {
                        let url = line[idx + "DevTools listening on ".len()..].trim().to_string();
                        if url.starts_with("ws://") {
                            return Ok(url);
                        }
                    }
                }
                if std::time::Instant::now() > deadline {
                    return Err(anyhow!("浏览器启动超时（15s）"));
                }
                if let Ok(Some(status)) = child.as_mut().expect("child").try_wait() {
                    return Err(anyhow!("浏览器进程提前退出（{status}）"));
                }
            }
        });
        let ws_url = ws_url?;

        // CDP 连接（浏览器级）
        let request = http::Request::builder()
            .uri(&ws_url)
            .body(())
            .map_err(|e| anyhow!("构造 CDP 连接失败: {e}"))?;
        let (ws, _resp) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| anyhow!("CDP 连接失败: {e}"))?;
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
            child,
            user_data_dir,
            sink,
            pending,
            next_id: 0,
            session_id: None,
        };
        // 创建并附加页面 target（flatten 后命令需带 sessionId）
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
        Ok(browser)
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
            .send(Message::Text(msg.to_string().into()))
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
            return Err(anyhow!("页面 JS 异常: {}", r.get("exceptionDetails").unwrap_or(&Value::Null)));
        }
        Ok(r.get("result").and_then(|v| v.get("value")).cloned().unwrap_or(Value::Null))
    }

    /// 等待 document.readyState == complete（超时 20s）
    pub async fn wait_ready(&mut self, timeout: Duration) -> Result<()> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if std::time::Instant::now() > deadline {
                return Err(anyhow!("页面加载超时"));
            }
            let state = self.evaluate("document.readyState").await?.as_str().unwrap_or("").to_string();
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
    pub async fn set_cookies(&mut self, pairs: &[(String, String)], host: &str, secure: bool) -> Result<()> {
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
        self.command("Network.setCookies", json!({ "cookies": cookies })).await?;
        Ok(())
    }

    /// Storage.getCookies → cookie 数组（含 httpOnly）
    pub async fn get_cookies(&mut self) -> Result<Vec<Value>> {
        let r = self.command("Storage.getCookies", json!({})).await?;
        Ok(r.get("cookies").and_then(|c| c.as_array()).cloned().unwrap_or_default())
    }

    /// cookie 数组 → "a=1; b=2"（按 name 排序，顺序稳定）
    pub fn cookies_to_string(cookies: &[Value]) -> String {
        let mut pairs: Vec<(String, String)> = cookies
            .iter()
            .filter_map(|c| {
                let name = c.get("name").and_then(|v| v.as_str())?.to_string();
                let value = c.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string();
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
        let ctrl1 = (x1 + (x2 - x1) * 0.4 + rand::random::<f64>() * 20.0 - 10.0, y1);
        let ctrl2 = (x1 + (x2 - x1) * 0.6 + rand::random::<f64>() * 20.0 - 10.0, y2);
        for i in 1..=steps {
            let t = i as f64 / steps as f64;
            let inv = 1.0 - t;
            // 三次贝塞尔
            let x = inv * inv * inv * x1 + 3.0 * inv * inv * t * ctrl1.0 + 3.0 * inv * t * t * ctrl2.0 + t * t * t * x2;
            let y = inv * inv * inv * y1 + 3.0 * inv * inv * t * ctrl1.1 + 3.0 * inv * t * t * ctrl2.1 + t * t * t * y2;
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
        let data = r.get("data").and_then(|v| v.as_str()).ok_or_else(|| anyhow!("截图失败"))?;
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| anyhow!("截图 base64 解码失败: {e}"))
    }
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
    fn test_browser_candidates_env_first() {
        // 环境变量优先
        std::env::set_var("READER_CHROME_PATH", "C:/fake/edge.exe");
        let c = browser_candidates();
        assert_eq!(c[0], PathBuf::from("C:/fake/edge.exe"));
        std::env::remove_var("READER_CHROME_PATH");
    }

    #[test]
    fn test_launch_with_missing_exe_fails() {
        // 降级路径：浏览器不可用 → 明确错误（不 panic、不启动任何进程）
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(Browser::launch_with(PathBuf::from("C:/definitely/not/exists.exe")));
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
