//! 69shuba 质询对比探针（GAP 175）：内置浏览器 CDP（obscura stealth 后端）vs
//! camoufox 求解后端——对比「质询页是否出现 input（cf-turnstile-response）」。
//!
//! 运行（真实网络 + 本机浏览器；默认 cargo test 不执行）：
//!   cargo test --test 69shuba_probe -- --ignored --nocapture
//!
//! 前置：camoufox_solver.py 服务已启动（python scripts/camoufox_solver.py）——
//! 探针的 camoufox 分支经 READER_CAMOUFOX_URL 调用该 HTTP 服务。

use std::time::Duration;

/// 探针 1：内置浏览器 CDP（obscura stealth 构建）解 69shuba——
/// 记录质询页是否出现 input（[name=cf-turnstile-response]）/ Turnstile widget /
/// challenge 特征是否消失
#[test]
#[ignore = "真实网络 + 浏览器——手动实验用"]
fn probe_cdp_on_69shuba() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let r = rt.block_on(reader_dev::service::browser::solve_cf_challenge(
        "probe",
        "https://www.69shuba.com/",
        &[],
        45_000,
        None,
    ));
    match r {
        Ok(sol) => {
            let h = &sol.html;
            println!("=== CDP（当前实现）69shuba 求解结果 ===");
            println!("html_len={}", h.len());
            println!("title 含 Just a moment: {}", h.contains("Just a moment"));
            println!(
                "含 challenge-form/cf-chl: {}",
                h.contains("challenge-form")
                    || h.contains("cf-chl-")
                    || h.contains("challenge-platform")
            );
            println!(
                "含 [name=cf-turnstile-response] input: {}",
                h.contains("cf-turnstile-response")
            );
            println!("含 .cf-turnstile 容器: {}", h.contains("cf-turnstile"));
            println!(
                "含 challenges.cloudflare.com iframe: {}",
                h.contains("challenges.cloudflare.com")
            );
            println!("cookies: {:?}", sol.cookies);
            println!("ua: {}", sol.user_agent);
        }
        Err(e) => println!("=== CDP（当前实现）69shuba 求解失败 ===\n{e:#}"),
    }
}

/// 探针 2：camoufox 求解后端（HTTP 服务）解 69shuba——同一判定
#[test]
#[ignore = "真实网络 + camoufox 服务——手动实验用"]
fn probe_camoufox_on_69shuba() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let r = rt.block_on(reader_dev::service::camoufox::solve(
        "https://www.69shuba.com/",
        &[],
        60_000,
    ));
    match r {
        Ok(sol) => {
            let h = &sol.html;
            println!("=== camoufox（新后端）69shuba 求解结果 ===");
            println!("html_len={}", h.len());
            println!("title 含 Just a moment: {}", h.contains("Just a moment"));
            println!(
                "含 challenge-form/cf-chl: {}",
                h.contains("challenge-form")
                    || h.contains("cf-chl-")
                    || h.contains("challenge-platform")
            );
            println!(
                "含 [name=cf-turnstile-response] input: {}",
                h.contains("cf-turnstile-response")
            );
            println!("含 .cf-turnstile 容器: {}", h.contains("cf-turnstile"));
            println!(
                "含 challenges.cloudflare.com iframe: {}",
                h.contains("challenges.cloudflare.com")
            );
            println!("cookies: {:?}", sol.cookies);
            println!("ua: {}", sol.user_agent);
            println!("turnstile_token: {:?}", sol.turnstile_token);
        }
        Err(e) => println!("=== camoufox 69shuba 求解失败 ===\n{e:#}"),
    }
}

/// 探针 3：camoufox 健康检查（服务是否在跑）
#[test]
#[ignore = "依赖 camoufox 服务——手动实验用"]
fn probe_camoufox_health() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    match rt.block_on(reader_dev::service::camoufox::health()) {
        Ok(true) => println!("camoufox 服务健康"),
        Ok(false) => println!("camoufox 服务响应异常"),
        Err(e) => println!("camoufox 服务不可达: {e:#}"),
    }
    // 连接级超时探测（服务未启动时错误应明确且快速）
    let start = std::time::Instant::now();
    std::env::set_var("READER_CAMOUFOX_URL", "http://127.0.0.1:9");
    let r = rt.block_on(reader_dev::service::camoufox::health());
    println!(
        "不可达探测耗时 {:?}: {}",
        start.elapsed(),
        match r {
            Ok(_) => "意外 ok".to_string(),
            Err(e) => format!("{e:#}"),
        }
    );
    std::env::remove_var("READER_CAMOUFOX_URL");
    let _ = Duration::from_secs(0); // 保持 Duration 引用（计时展示用）
}
