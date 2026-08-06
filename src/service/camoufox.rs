//! camoufox 求解后端（GAP 175）：HTTP 调用 `scripts/camoufox_solver.py`（独立 Python
//! 服务，默认端口 8196）——camoufox（Playwright 封装，Firefox 内核 + 真实指纹预设：
//! navigator/screen/WebGL/字体/canvas 噪声）替代手搓 stealth 解 Cloudflare 强质询
//! （如 69shuba managed challenge——内置 CDP headless Chrome 无法通过环境校验时）。
//!
//! 求解链（browser.rs solve_captcha_inner 统一接入，crawler 与书源 JS 桥共用）：
//! 内置浏览器 CDP → camoufox（HTTP 后端）→ 仍失败才报错（合并错误）。
//!
//! 环境变量：
//! - `READER_CAMOUFOX_URL`：服务地址（默认 http://127.0.0.1:8196）
//! - `READER_CAMOUFOX_DISABLE=1`：禁用 camoufox 兜底
//! - `READER_CAMOUFOX_FIRST=1`：camoufox 优先（CDP 前先试——配置启用场景）
//! - `READER_CAMOUFOX_UA`：求解用 UA（默认 Chrome/131 Windows——与 CDP 路径一致；
//!   69shuba 等站点有 UA 门禁，Firefox UA 会被 "请使用新版本的Google Chrome" 拦截）
//!
//! 求解成功后的 cookie 合并/存库/UA 记录复用 crawler::store_solution_session
//! （与 CDP 路径同构——本模块返回与 browser::CfSolution 相同结构）。

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::service::browser::CfSolution;

/// 默认求解 UA：Chrome/131 Windows——与内置 CDP 路径（browser.rs --user-agent）一致；
/// 69shuba 实测：camoufox 默认 Firefox wire UA 会命中站点 UA 门禁，Chrome wire UA 直过。
const DEFAULT_SOLVE_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// 求解用 UA：`READER_CAMOUFOX_UA` 显式配置优先，默认 Chrome/131 Windows
pub fn solve_ua() -> String {
    std::env::var("READER_CAMOUFOX_UA")
        .map(|v| v.trim().to_string())
        .into_iter()
        .find(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_SOLVE_UA.to_string())
}

/// camoufox 服务地址：`READER_CAMOUFOX_URL`（默认 http://127.0.0.1:8196，尾斜杠去除）
pub fn server_url() -> String {
    std::env::var("READER_CAMOUFOX_URL")
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .into_iter()
        .find(|v| !v.is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:8196".to_string())
}

/// 是否启用 camoufox（`READER_CAMOUFOX_DISABLE=1` 关闭；默认启用——CDP 失败后兜底）
pub fn enabled() -> bool {
    std::env::var("READER_CAMOUFOX_DISABLE")
        .map(|v| v.trim() != "1")
        .unwrap_or(true)
}

/// camoufox 优先模式（`READER_CAMOUFOX_FIRST=1`——CDP 前先试 camoufox）
pub fn first_mode() -> bool {
    enabled()
        && std::env::var("READER_CAMOUFOX_FIRST")
            .map(|v| v.trim() == "1")
            .unwrap_or(false)
}

/// POST /solve 调用 camoufox 服务求解质询。
///
/// 请求：`{url, cookies:[{name,value}], maxWaitMs}`；
/// 响应：`{html, cookies:[{name,value}], userAgent, turnstileToken, diagnostics}`；
/// 失败响应：HTTP 200 + `{error, diagnostics}`（服务端承载错误，统一 JSON 解析）。
pub async fn solve(
    url: &str,
    cookies: &[(String, String)],
    max_wait_ms: u64,
) -> Result<CfSolution> {
    if !enabled() {
        return Err(anyhow!("camoufox 已禁用（READER_CAMOUFOX_DISABLE=1）"));
    }
    let base = server_url();
    let payload = json!({
        "url": url,
        "cookies": cookies.iter().map(|(n, v)| json!({"name": n, "value": v})).collect::<Vec<_>>(),
        "maxWaitMs": max_wait_ms,
        "userAgent": solve_ua(),
    });
    // 超时：求解上限 + 20s 余量（导航/提取），封顶 120s——服务不可达时连接拒绝立即返回
    let timeout = std::time::Duration::from_secs(max_wait_ms.saturating_add(20).min(120));
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| anyhow!("camoufox HTTP 客户端构造失败: {e}"))?;
    let resp = client
        .post(format!("{base}/solve"))
        .json(&payload)
        .send()
        .await
        .map_err(|e| {
            anyhow!(
                "camoufox 服务不可达（{base}）: {e}——启动方式：python scripts/camoufox_solver.py（或设置 READER_CAMOUFOX_URL）"
            )
        })?;
    let v: Value = resp
        .json()
        .await
        .map_err(|e| anyhow!("camoufox 响应解析失败: {e}"))?;
    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        let diag = v.get("diagnostics").cloned().unwrap_or(Value::Null);
        return Err(anyhow!(
            "camoufox 求解失败: {err}{}",
            if diag.is_object() {
                format!("（诊断: {diag}）")
            } else {
                String::new()
            }
        ));
    }
    let html = v
        .get("html")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    if html.is_empty() {
        return Err(anyhow!("camoufox 响应缺少 html 字段"));
    }
    let cookies_out: Vec<(String, String)> = v
        .get("cookies")
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    Some((
                        c.get("name")?.as_str()?.to_string(),
                        c.get("value")?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(CfSolution {
        html,
        cookies: cookies_out,
        user_agent: v
            .get("userAgent")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        turnstile_token: v
            .get("turnstileToken")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty()),
    })
}

/// CDP 求解失败后的 camoufox 兜底入口：成功 → Ok；失败 → 合并错误（CDP + camoufox）
pub async fn fallback(
    url: &str,
    cookies: &[(String, String)],
    max_wait_ms: u64,
    cdp_err: &anyhow::Error,
) -> Result<CfSolution> {
    match solve(url, cookies, max_wait_ms).await {
        Ok(sol) => Ok(sol),
        Err(cf_err) => Err(anyhow!("内置浏览器求解失败: {cdp_err:#}；{cf_err:#}")),
    }
}

/// 健康检查（GET /health → 200 + ok:true）——集成测试/探活用
pub async fn health() -> Result<bool> {
    let base = server_url();
    let resp = reqwest::Client::new()
        .get(format!("{base}/health"))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| anyhow!("camoufox 服务不可达（{base}）: {e}"))?;
    let v: Value = resp
        .json()
        .await
        .map_err(|e| anyhow!("camoufox /health 响应解析失败: {e}"))?;
    Ok(v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 环境变量测试串行锁（READER_CAMOUFOX_* 全局共享——并行跑会互相踩踏：
    /// test_fallback_combines_errors_when_disabled 设置 DISABLE=1 时
    /// test_enabled_flags 的 remove_var 断言会间歇性失败）
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_server_url_default_and_env() {
        let _g = ENV_LOCK.lock().unwrap();
        // 默认地址
        std::env::remove_var("READER_CAMOUFOX_URL");
        assert_eq!(server_url(), "http://127.0.0.1:8196");
        // 显式配置（尾斜杠去除）
        std::env::set_var("READER_CAMOUFOX_URL", "http://127.0.0.1:9999/");
        assert_eq!(server_url(), "http://127.0.0.1:9999");
        std::env::remove_var("READER_CAMOUFOX_URL");
    }

    #[test]
    fn test_enabled_flags() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("READER_CAMOUFOX_DISABLE");
        std::env::remove_var("READER_CAMOUFOX_FIRST");
        assert!(enabled());
        assert!(!first_mode());
        std::env::set_var("READER_CAMOUFOX_DISABLE", "1");
        assert!(!enabled());
        assert!(!first_mode(), "禁用时优先模式不生效");
        std::env::remove_var("READER_CAMOUFOX_DISABLE");
        std::env::set_var("READER_CAMOUFOX_FIRST", "1");
        assert!(first_mode());
        std::env::remove_var("READER_CAMOUFOX_FIRST");
    }

    #[test]
    fn test_solve_ua_default_and_env() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("READER_CAMOUFOX_UA");
        let d = solve_ua();
        assert!(d.contains("Chrome/"), "默认 UA 应为 Chrome: {d}");
        std::env::set_var(
            "READER_CAMOUFOX_UA",
            "Mozilla/5.0 (X11; Linux x86_64) Firefox/143.0",
        );
        assert_eq!(solve_ua(), "Mozilla/5.0 (X11; Linux x86_64) Firefox/143.0");
        std::env::set_var("READER_CAMOUFOX_UA", "  ");
        assert!(solve_ua().contains("Chrome/"), "空白 env 回退默认");
        std::env::remove_var("READER_CAMOUFOX_UA");
    }

    #[test]
    fn test_fallback_combines_errors_when_disabled() {
        let _g = ENV_LOCK.lock().unwrap();
        // camoufox 禁用时 fallback 直接透传 CDP 错误（合并语义）
        std::env::set_var("READER_CAMOUFOX_DISABLE", "1");
        let cdp = anyhow!("内置浏览器超时");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(fallback("https://a.test/", &[], 1000, &cdp));
        std::env::remove_var("READER_CAMOUFOX_DISABLE");
        let err = r.expect_err("禁用时应失败");
        assert!(err.to_string().contains("内置浏览器求解失败"));
        assert!(err.to_string().contains("内置浏览器超时"));
    }
}
