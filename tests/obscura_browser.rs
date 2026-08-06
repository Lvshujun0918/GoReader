//! obscura 浏览器后端集成测试（GAP：obscura 反检测浏览器集成）：真实 spawn
//! `obscura serve --stealth` → CDP 连接 → 本地 mock 站点导航/求值/cookie/截图全链路。
//!
//! 无 obscura 可执行文件（READER_OBSCURA_BIN 未配置且 PATH 无 obscura）时自动跳过
//! ——不破坏无浏览器环境的常规测试。有二进制时实测：
//!   READER_OBSCURA_BIN=/path/to/obscura cargo test --test obscura_browser -- --nocapture
//!   （或把 obscura 放进 PATH 后直接 cargo test --test obscura_browser）

use std::io::{Read, Write};
use std::net::TcpListener;

use reader_dev::service::browser::Browser;

/// 本地 mock 站点（127.0.0.1 随机端口；obscura 默认禁私网导航——集成代码已带
/// --allow-private-network，此处正好验证该参数生效）
fn serve_mock_site() -> (u16, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock site");
    let port = listener.local_addr().expect("mock addr").port();
    let handle = std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            // 读完请求头即可回包（固定响应）
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let body = "<!DOCTYPE html><html><head><title>obscura-mock</title></head><body>\
                <h1 id=\"h\">hello obscura</h1>\
                <script>window.__x = 42;</script>\
                </body></html>";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    (port, handle)
}

#[test]
fn obscura_launch_navigate_evaluate_cookie_screenshot() {
    if !reader_dev::service::browser::is_browser_available() {
        println!("skipped: obscura 不可用（未配置 READER_OBSCURA_BIN/URL 且 PATH 无 obscura）");
        return;
    }
    let (port, _server) = serve_mock_site();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut b = Browser::launch().await.expect("obscura 启动失败");
        // ① 导航（mock 站点——验证 --allow-private-network 生效）
        b.navigate(&format!("http://127.0.0.1:{port}/"))
            .await
            .expect("导航失败");
        // ② 求值（页面脚本已执行）
        let title = b.evaluate("document.title").await.expect("求值 title");
        assert_eq!(
            title.as_str().unwrap_or(""),
            "obscura-mock",
            "document.title 应来自 mock 页"
        );
        let x = b.evaluate("window.__x").await.expect("求值 window.__x");
        // obscura 的 V8 数字一律序列化为浮点（42.0）——as_i64 对浮点返回 None，用 as_f64 断言
        assert_eq!(
            x.as_f64(),
            Some(42.0),
            "页面脚本应已执行（window.__x={x:?}）"
        );
        // ③ cookie 写入 + 读取（httpOnly 读取能力——登录流依赖）
        b.set_cookies(&[("k1".to_string(), "v1".to_string())], "127.0.0.1", false)
            .await
            .expect("set cookie");
        let cookie_str = Browser::cookies_to_string(&b.get_cookies().await.expect("get cookies"));
        assert!(
            cookie_str.contains("k1=v1"),
            "cookie 应包含 k1=v1（实际: {cookie_str}）"
        );
        // ④ 截图：obscura 无布局/绘制引擎——Page.captureScreenshot 返回明确错误
        //    （图片验证码截图流在 obscura 后端下不可用，调用方得到明确错误而非悬挂）
        let shot = b.screenshot_clip(0.0, 0.0, 100.0, 100.0).await;
        match &shot {
            Err(e) => {
                let msg = format!("{e:#}");
                assert!(
                    msg.contains("not supported") || msg.contains("不支持"),
                    "obscura 截图应返回明确的 not supported 错误（实际: {msg}）"
                );
                println!("obscura 截图限制（预期）: {msg}");
            }
            Ok(png) => {
                // 未来 obscura 支持绘制后此断言生效；当前版本不可能到达
                assert!(png.len() > 8 && png.starts_with(&[0x89, b'P', b'N', b'G']));
            }
        }
        println!(
            "obscura 集成实测通过：title={title} window.__x={x} cookies=[{cookie_str}] 截图={}",
            if shot.is_ok() {
                "ok"
            } else {
                "明确错误（not supported）"
            }
        );
    });
    // drop(b) → 杀 obscura serve 进程（Drop 清理）
}

/// READER_OBSCURA_URL 直连路径：spawn 一个独立 obscura serve（手动），经 URL 连接，
/// 验证 connect 路径（不 spawn 第二个进程、Drop 不杀外部进程）
#[test]
fn obscura_connect_external_url() {
    if !reader_dev::service::browser::is_browser_available() {
        println!("skipped: obscura 不可用");
        return;
    }
    use std::process::{Child, Command, Stdio};
    let exe = reader_dev::service::browser::discover_obscura_bin().expect("obscura bin");
    let port = 20000 + rand::random::<u16>() % 30000;
    let mut child: Child = Command::new(&exe)
        .args([
            "serve",
            "--port",
            &port.to_string(),
            "--stealth",
            "--allow-private-network",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn obscura");
    // 等待端口就绪（HTTP /json/version 探测）
    let rt = tokio::runtime::Runtime::new().unwrap();
    let ready = rt.block_on(async {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            if reqwest::get(format!("http://127.0.0.1:{port}/json/version"))
                .await
                .is_ok()
            {
                return true;
            }
            if std::time::Instant::now() > deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    });
    if !ready {
        let _ = child.kill();
        panic!("obscura serve 未就绪（端口 {port}）");
    }
    let r = rt.block_on(async {
        // http:// 端点 → connect 路径（自动补 /devtools/browser）
        let mut b = Browser::connect(&format!("http://127.0.0.1:{port}"))
            .await
            .expect("connect 失败");
        let _title = b.evaluate("document.title").await.expect("求值");
        // 直连路径 Drop 不应杀外部进程——显式验证进程仍存活
        drop(b);
        child.try_wait().expect("try_wait").is_none()
    });
    let _ = child.kill();
    let _ = child.wait();
    assert!(r, "connect 路径求值或进程存活校验失败");
    println!("obscura READER_OBSCURA_URL 直连路径实测通过（端口 {port}）");
}
