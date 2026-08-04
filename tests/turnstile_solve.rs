//! Turnstile 验证码求解集成测试（scripts/mock-turnstile-site.py + 本机 Edge/Chrome）
//!
//! 覆盖：Turnstile 特征检测 → 点击 .cf-turnstile 容器 → 轮询 [name=cf-turnstile-response]
//!      → token 非空即通过 → cf_clearance cookie 提取 → 用户 cookie 保留/按 name 合并 →
//!      turnstile_token 随 cookie 串存库（按用户，http_get 全链路）。
//! 前置：本机安装 Edge/Chrome 且 PATH 有 python；任一缺失 → 跳过（打印原因，不失败）。

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// python 启动方式：直接可执行 或 经 cmd /C（Windows 下 pyenv 等 shim 是脚本/.bat，
/// Rust Command 无法直接 spawn——需 cmd 解释）
fn python_cmd() -> Option<Vec<&'static str>> {
    // ① 直接可执行
    for c in ["python", "python3", "py"] {
        if Command::new(c)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return Some(vec![c]);
        }
    }
    // ② 经 cmd /C（pyenv-win shim：python 是 POSIX 脚本 / python.bat）
    if Command::new("cmd")
        .args(["/C", "python --version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return Some(vec!["cmd", "/C", "python"]);
    }
    None
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind 0")
        .local_addr()
        .expect("addr")
        .port()
}

fn start_mock(port: u16) -> Child {
    let launcher = python_cmd().expect("python 已探测");
    let mut cmd = Command::new(launcher[0]);
    for a in &launcher[1..] {
        cmd.arg(a);
    }
    cmd.arg("scripts/mock-turnstile-site.py")
        .arg("--port")
        .arg(port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.spawn().expect("启动 mock-turnstile-site.py")
}

/// 杀 mock 进程树（cmd /C 包装下 child.kill 只杀 cmd，python 会残留——用 taskkill /T）
fn kill_mock(child: &mut Child) {
    #[cfg(windows)]
    {
        if child.try_wait().map(|s| s.is_some()).unwrap_or(false) {
            return;
        }
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &child.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

async fn wait_mock_ready(port: u16) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if reqwest::get(format!("http://127.0.0.1:{port}/status"))
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

/// mock 站点直连应命中 CF 特征检测（503 + Turnstile 特征）——证明 mock 与检测逻辑匹配
#[tokio::test]
async fn mock_serves_turnstile_challenge_features() {
    let port = free_port();
    let mut child = start_mock(port);
    if !wait_mock_ready(port).await {
        kill_mock(&mut child);
        panic!("mock-turnstile-site.py 未在 10s 内就绪");
    }
    let r = reqwest::get(format!("http://127.0.0.1:{port}/"))
        .await
        .expect("GET /");
    let status = r.status().as_u16();
    let body = r.text().await.expect("body");
    kill_mock(&mut child);
    assert_eq!(status, 503, "Turnstile 页应为 503");
    assert!(
        reader_dev::service::crawler::is_cloudflare_challenge(status, &body),
        "直连响应应命中 CF 特征检测（Turnstile 特征）"
    );
    assert!(body.contains("cf-turnstile"), "应含 .cf-turnstile 容器");
    assert!(
        body.contains("name=\"cf-turnstile-response\""),
        "应含隐藏 input[name=cf-turnstile-response]"
    );
    assert!(
        body.contains("challenges.cloudflare.com/turnstile"),
        "应含 turnstile 脚本标签"
    );
}

/// 全流程：solve_cf_challenge 的 Turnstile 分支（本机浏览器）——
/// 检测 → 点击容器 → 轮询 token（≤30s）→ token 非空即通过 → HTML/cookie/token 返回
#[tokio::test]
async fn turnstile_challenge_solve_end_to_end() {
    if !reader_dev::service::browser::is_browser_available() {
        eprintln!("SKIP: 本机未检测到 Edge/Chrome——跳过 Turnstile 求解集成测试");
        return;
    }
    if python_cmd().is_none() {
        eprintln!("SKIP: 未找到 python——跳过 Turnstile 求解集成测试");
        return;
    }
    let port = free_port();
    let mut child = start_mock(port);
    if !wait_mock_ready(port).await {
        kill_mock(&mut child);
        panic!("mock-turnstile-site.py 未在 10s 内就绪");
    }

    // 用户既有 cookie（会话连续性：求解后仍保留并合并）
    let user_cookies = vec![("sid".to_string(), "abc123".to_string())];
    let result = reader_dev::service::browser::solve_cf_challenge(
        &format!("http://127.0.0.1:{port}/"),
        &user_cookies,
        30_000,
    )
    .await;

    // 收尾：关闭会话浏览器 + mock 进程
    reader_dev::service::browser::shutdown_cf_session().await;
    kill_mock(&mut child);

    let sol = result.expect("solve_cf_challenge 应成功（mock 点击后 1.5s 出 token）");

    // ① Turnstile token：非空且为 mock 格式（点击容器 → 轮询 [name=cf-turnstile-response]）
    let token = sol.turnstile_token.as_deref().expect("应取得 turnstile token");
    assert!(
        token.starts_with("mock-turnstile-"),
        "token 应为 mock-turnstile-*（实际: {token}）"
    );
    // ② 求解后 HTML = 真实内容页（token 回显 + cookie 回显——证明点击触发回调）
    assert!(sol.html.contains("真实内容"), "应已切换为真实内容页");
    assert!(
        sol.html.contains("mock-turnstile-"),
        "页面应回显 token（实际: {}）",
        &sol.html[sol.html.len().saturating_sub(200)..]
    );
    assert!(
        sol.html.contains("cf_clearance=mock-"),
        "页面应回显 cookie（证明点击后写 cookie）"
    );
    // ③ cf_clearance cookie（模拟）
    let cf = sol
        .cookies
        .iter()
        .find(|(n, _)| n == "cf_clearance")
        .expect("应取得 cf_clearance");
    assert!(cf.1.starts_with("mock-"), "cf_clearance 值应为 mock-*（实际: {}）", cf.1);
    // ④ 用户 cookie 保留
    assert!(
        sol.cookies.iter().any(|(n, v)| n == "sid" && v == "abc123"),
        "用户 cookie 应保留"
    );
    // ⑤ UA 非空
    assert!(!sol.user_agent.is_empty(), "应返回浏览器 UA");
}

/// 全链路：http_get → CF 特征检测（Turnstile）→ 内置浏览器求解（含 Turnstile 分支）→
/// 响应正文 + cookies 按 name 合并存库（按用户）→ turnstile_token 随 cookie 串存库
#[tokio::test]
async fn http_get_solves_turnstile_and_stores_per_user() {
    if !reader_dev::service::browser::is_browser_available() {
        eprintln!("SKIP: 本机未检测到 Edge/Chrome——跳过 Turnstile 全链路集成测试");
        return;
    }
    if python_cmd().is_none() {
        eprintln!("SKIP: 未找到 python——跳过 Turnstile 全链路集成测试");
        return;
    }
    std::env::remove_var("FLARESOLVERR_URL"); // 强制走内置浏览器路径

    // 临时库（按用户存储）
    let dir = std::env::temp_dir().join(format!("reader-turnstile-it-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut config = reader_dev::AppConfig::from_env();
    config.work_dir = dir.to_string_lossy().into_owned();
    let storage = reader_dev::storage::init(&config).await.expect("storage init");
    reader_dev::service::crawler::register_cookie_storage(storage.clone());

    let port = free_port();
    let mut child = start_mock(port);
    if !wait_mock_ready(port).await {
        kill_mock(&mut child);
        panic!("mock-turnstile-site.py 未在 10s 内就绪");
    }
    let url = format!("http://127.0.0.1:{port}/");

    // 预置用户 cookie（模拟已有登录态）——求解后应与 cf_clearance 按 name 合并存库
    storage.set_cookie("default", &url, "sid=abc123").await.expect("预置 cookie");

    let result =
        reader_dev::service::crawler::http_get("default", &url, &std::collections::HashMap::new(), 30).await;

    // 先取回数据再收尾（避免断言失败时浏览器/mock 残留）
    let resp = result.expect("http_get 应经内置浏览器解 Turnstile 质询成功");
    let stored = storage.get_cookie_by_base("default", &url).await.expect("读库");
    let session = storage.get_source_session("default", &url).await.expect("读会话");
    reader_dev::service::browser::shutdown_cf_session().await;
    kill_mock(&mut child);
    reader_dev::service::crawler::clear_cookie_storage();
    storage.pool.close().await;
    let _ = std::fs::remove_dir_all(&dir);

    // 响应 = 真实内容（200）
    assert_eq!(resp.status, 200);
    assert!(resp.body.contains("真实内容"), "响应正文应为真实内容");
    assert!(
        resp.body.contains("mock-turnstile-"),
        "响应正文应含 turnstile token 回显"
    );
    // 存库：turnstile_token 伪 cookie + cf_clearance + 用户 sid 按 name 合并
    let stored = stored.expect("应按用户存下 cookie");
    assert!(
        stored.contains("cf_turnstile_token=mock-turnstile-"),
        "库中应含 cf_turnstile_token（实际: {stored}）"
    );
    assert!(
        stored.contains("cf_clearance=mock-"),
        "库中应含 cf_clearance（实际: {stored}）"
    );
    assert!(stored.contains("sid=abc123"), "库中应保留用户 sid（实际: {stored}）");
    // UA 记录（与 cf_clearance 绑定）
    let (_, ua) = session.expect("应有会话行");
    assert!(!ua.is_empty(), "应记录浏览器 UA");
}
