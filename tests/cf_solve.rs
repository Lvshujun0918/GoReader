//! CF 质询求解集成测试（scripts/mock-cf-site.py + 本机 Edge/Chrome）
//!
//! 覆盖：CF 特征检测 → 质询等待循环 → 求解后 HTML 提取 → cf_clearance cookie 获取 →
//!      用户 cookie 保留/按 name 合并 → 按用户存库（http_get 全链路）。
//! 前置：本机安装 Edge/Chrome 且 PATH 有 python；任一缺失 → 跳过（打印原因，不失败）。
//! P1：应用已不再传 --allow-private-network——mock 类测试需自行以
//! `obscura serve --allow-private-network` 启动并经 READER_OBSCURA_URL 连接；
//! 爬虫/求解入口的 SSRF 校验由 common::PrivateNetGuard 放行（仅测试进程）。

mod common;

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// 串行化使用全局 COOKIE_STORAGE 注册的测试（register_cookie_storage 是进程级全局——
/// 并行测试互相覆盖会导致存库断言竞态；非存储测试不受影响）
static STORAGE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    cmd.arg("scripts/mock-cf-site.py")
        .arg("--port")
        .arg(port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.spawn().expect("启动 mock-cf-site.py")
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

/// mock 站点直连应命中 CF 特征检测（503 + 质询页特征）——证明 mock 与检测逻辑匹配
#[tokio::test]
async fn mock_serves_cloudflare_challenge_features() {
    let port = free_port();
    let mut child = start_mock(port);
    if !wait_mock_ready(port).await {
        kill_mock(&mut child);
        panic!("mock-cf-site.py 未在 10s 内就绪");
    }
    let r = reqwest::get(format!("http://127.0.0.1:{port}/"))
        .await
        .expect("GET /");
    let status = r.status().as_u16();
    let body = r.text().await.expect("body");
    kill_mock(&mut child);
    assert_eq!(status, 503, "质询页应为 503");
    assert!(
        reader_dev::service::crawler::is_cloudflare_challenge(status, &body),
        "直连响应应命中 CF 特征检测"
    );
    assert!(body.contains("challenge-form"));
    assert!(body.to_lowercase().contains("just a moment"));
}

/// 全流程：solve_cf_challenge（本机浏览器）——检测→等待→求解→HTML→cf_clearance→合并
#[tokio::test]
async fn cf_challenge_solve_end_to_end() {
    if !reader_dev::service::browser::is_browser_available() {
        eprintln!("SKIP: obscura 浏览器不可用——跳过 CF 求解集成测试");
        return;
    }
    if python_cmd().is_none() {
        eprintln!("SKIP: 未找到 python——跳过 CF 求解集成测试");
        return;
    }
    let port = free_port();
    let mut child = start_mock(port);
    if !wait_mock_ready(port).await {
        kill_mock(&mut child);
        panic!("mock-cf-site.py 未在 10s 内就绪");
    }

    // 用户既有 cookie（会话连续性：求解后仍保留并合并）
    let user_cookies = vec![("sid".to_string(), "abc123".to_string())];
    // P1 SSRF：mock 绑定 127.0.0.1——持测试守卫放行
    let _ssrf = common::PrivateNetGuard::on();
    let result = reader_dev::service::browser::solve_cf_challenge(
        "default",
        &format!("http://127.0.0.1:{port}/"),
        &user_cookies,
        30_000,
        None,
    )
    .await;

    // 收尾：关闭会话浏览器 + mock 进程
    reader_dev::service::browser::shutdown_cf_session().await;
    kill_mock(&mut child);

    let sol = result.expect("solve_cf_challenge 应成功（mock 2s 内完成质询）");

    // ① 求解后 HTML = 真实内容页（质询特征消失）
    assert!(sol.html.contains("真实内容"), "应已跳转到真实内容页");
    assert!(
        sol.html.contains("CF_OK"),
        "服务端应收到 cf_clearance（证明 cookie 已随跳转携带）"
    );
    assert!(
        !sol.html.to_lowercase().contains("just a moment"),
        "不应再是质询页"
    );
    // ② cf_clearance cookie（模拟）
    let cf = sol
        .cookies
        .iter()
        .find(|(n, _)| n == "cf_clearance")
        .expect("应取得 cf_clearance");
    assert!(
        cf.1.starts_with("mock-"),
        "cf_clearance 值应为 mock-*（实际: {}）",
        cf.1
    );
    // ③ 用户 cookie 保留
    assert!(
        sol.cookies.iter().any(|(n, v)| n == "sid" && v == "abc123"),
        "用户 cookie 应保留"
    );
    // ④ UA 非空
    assert!(!sol.user_agent.is_empty(), "应返回浏览器 UA");
    // ⑤ 合并语义（按 name：cf_clearance 新覆盖、sid 用户保留）
    let fs: Vec<reader_dev::service::crawler::FsCookie> = sol
        .cookies
        .iter()
        .map(|(n, v)| reader_dev::service::crawler::FsCookie {
            name: n.clone(),
            value: v.clone(),
            domain: None,
            path: None,
        })
        .collect();
    let merged = reader_dev::service::crawler::merge_fs_cookies("sid=abc123", &fs);
    assert!(
        merged.contains("cf_clearance=mock-"),
        "合并后应含 cf_clearance=mock-*（实际: {merged}）"
    );
    assert!(merged.contains("sid=abc123"), "合并后应保留用户 sid");
}

/// 全链路：http_get → CF 检测 → 内置浏览器求解 → 响应正文 + cookies 按 name 合并存库（按用户）
#[tokio::test]
async fn http_get_solves_cf_and_stores_per_user() {
    let _guard = STORAGE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // P1 SSRF：mock 绑定 127.0.0.1——持测试守卫放行（http_get 入口公网校验）
    let _ssrf = common::PrivateNetGuard::on();
    if !reader_dev::service::browser::is_browser_available() {
        eprintln!("SKIP: obscura 浏览器不可用——跳过 CF 全链路集成测试");
        return;
    }
    if python_cmd().is_none() {
        eprintln!("SKIP: 未找到 python——跳过 CF 全链路集成测试");
        return;
    }
    std::env::remove_var("FLARESOLVERR_URL"); // 强制走内置浏览器路径

    // 临时库（按用户存储）
    let dir = std::env::temp_dir().join(format!("reader-cf-it-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut config = reader_dev::AppConfig::from_env();
    config.work_dir = dir.to_string_lossy().into_owned();
    let storage = reader_dev::storage::init(&config)
        .await
        .expect("storage init");
    reader_dev::service::crawler::register_cookie_storage(storage.clone());

    let port = free_port();
    let mut child = start_mock(port);
    if !wait_mock_ready(port).await {
        kill_mock(&mut child);
        panic!("mock-cf-site.py 未在 10s 内就绪");
    }
    let url = format!("http://127.0.0.1:{port}/");

    // 预置用户 cookie（模拟已有登录态）——求解后应与 cf_clearance 按 name 合并存库
    storage
        .set_cookie("default", &url, "sid=abc123")
        .await
        .expect("预置 cookie");

    let result = reader_dev::service::crawler::http_get(
        "default",
        &url,
        &std::collections::HashMap::new(),
        30,
        None,
    )
    .await;

    // 先取回数据再收尾（避免断言失败时浏览器/mock 残留）
    let resp = result.expect("http_get 应经内置浏览器解 CF 质询成功");
    let stored = storage
        .get_cookie_by_base("default", &url)
        .await
        .expect("读库");
    let session = storage
        .get_source_session("default", &url)
        .await
        .expect("读会话");
    reader_dev::service::browser::shutdown_cf_session().await;
    kill_mock(&mut child);
    reader_dev::service::crawler::clear_cookie_storage();
    storage.pool.close().await;
    let _ = std::fs::remove_dir_all(&dir);

    // 响应 = 真实内容（200）
    assert_eq!(resp.status, 200);
    assert!(resp.body.contains("真实内容"), "响应正文应为真实内容");
    assert!(resp.body.contains("CF_OK"), "服务端应收到 cf_clearance");
    // 存库：cf_clearance + 用户 sid 按 name 合并
    let stored = stored.expect("应按用户存下 cookie");
    assert!(
        stored.contains("cf_clearance=mock-"),
        "库中应含 cf_clearance（实际: {stored}）"
    );
    assert!(
        stored.contains("sid=abc123"),
        "库中应保留用户 sid（实际: {stored}）"
    );
    // UA 记录（与 cf_clearance 绑定）
    let (_, ua) = session.expect("应有会话行");
    assert!(!ua.is_empty(), "应记录浏览器 UA");
}

/// POST 重试链路（关键修复）：POST /search → 质询页 → 内置浏览器求解（cookie 合并存库）
/// → **重试原 POST 请求**（新 cookie）→ 真实搜索结果（而非浏览器 GET 首页兜底）
#[tokio::test]
async fn http_post_retries_after_solve_and_gets_search_results() {
    let _guard = STORAGE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // P1 SSRF：mock 绑定 127.0.0.1——持测试守卫放行（http_post 入口公网校验）
    let _ssrf = common::PrivateNetGuard::on();
    if !reader_dev::service::browser::is_browser_available() {
        eprintln!("SKIP: obscura 浏览器不可用——跳过 POST 重试集成测试");
        return;
    }
    if python_cmd().is_none() {
        eprintln!("SKIP: 未找到 python——跳过 POST 重试集成测试");
        return;
    }
    std::env::remove_var("FLARESOLVERR_URL"); // 强制走内置浏览器路径

    // 临时库（按用户存储）
    let dir = std::env::temp_dir().join(format!("reader-cf-post-it-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut config = reader_dev::AppConfig::from_env();
    config.work_dir = dir.to_string_lossy().into_owned();
    let storage = reader_dev::storage::init(&config)
        .await
        .expect("storage init");
    reader_dev::service::crawler::register_cookie_storage(storage.clone());

    let port = free_port();
    let mut child = start_mock(port);
    if !wait_mock_ready(port).await {
        kill_mock(&mut child);
        panic!("mock-cf-site.py 未在 10s 内就绪");
    }
    let url = format!("http://127.0.0.1:{port}/search");

    let result = reader_dev::service::crawler::http_post(
        "default",
        &url,
        &std::collections::HashMap::new(),
        30,
        Some("searchkey=诡秘"),
        None,
        None,
    )
    .await;

    // 先取回数据再收尾（避免断言失败时浏览器/mock 残留）
    let resp = result.expect("http_post 应经质询求解 + 重试成功");
    let stored = storage
        .get_cookie_by_base("default", &url)
        .await
        .expect("读库");
    reader_dev::service::browser::shutdown_cf_session().await;
    kill_mock(&mut child);
    reader_dev::service::crawler::clear_cookie_storage();
    storage.pool.close().await;
    let _ = std::fs::remove_dir_all(&dir);

    // ① 重试 POST 拿到真实搜索结果（SEARCH_OK + 关键词回显——证明是重试结果而非
    //    浏览器 GET 首页兜底）
    assert_eq!(resp.status, 200, "重试 POST 应返回 200");
    assert!(
        resp.body.contains("SEARCH_OK"),
        "应返回搜索结果页（实际: {}）",
        &resp.body[..resp.body.len().min(200)]
    );
    assert!(
        resp.body.contains("诡秘"),
        "应回显搜索关键词（实际: {}）",
        &resp.body[..resp.body.len().min(200)]
    );
    // ② 库中已合并 cf_clearance（重试凭它通过）
    let stored = stored.expect("应按用户存下 cookie");
    assert!(
        stored.contains("cf_clearance=mock-"),
        "库中应含 cf_clearance（实际: {stored}）"
    );
}
