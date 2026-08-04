//! HTTP 抓取客户端（reqwest，书源抓取）
//!
//! - `http_get`/`http_post`：书源抓取入口——按用户命名空间 + 请求 URL 的 baseUrl
//!   从 book_source_cookies 表读取书源 cookie 自动附加（登录态独立于系统用户）；
//!   响应命中 Cloudflare 质询特征时自动转 FlareSolverr 解（见 `flaresolverr` 模块说明）。
//! - `fetch`/`fetch_get`：原始抓取（不带 cookie/FS 逻辑），供 RSS/TTS 等非书源场景。

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

/// 内置浏览器 CF 质询求解的质询等待循环上限（任务要求最多 30s）
pub const CF_SOLVE_MAX_WAIT_MS: u64 = 30_000;

/// 抓取响应
pub struct FetchResponse {
    pub body: String,
    pub url: String,
    /// 响应头（键小写；Set-Cookie 可能有多个同名项）
    pub headers: Vec<(String, String)>,
    /// HTTP 状态码
    pub status: u16,
}

/// 按 charset 解码字节（GB2312/GBK/UTF-8 等，encoding_rs）
pub fn decode_bytes(bytes: &[u8], charset: Option<&str>) -> String {
    let charset = charset.unwrap_or("utf-8");
    let encoding = encoding_rs::Encoding::for_label(charset.as_bytes())
        .unwrap_or(encoding_rs::UTF_8);
    let (text, _, _) = encoding.decode(bytes);
    text.into_owned()
}

/// 抓取（GET/POST，支持 header JSON；charset 指定时转码）
pub async fn fetch(
    url: &str,
    headers: &HashMap<String, String>,
    timeout_secs: u64,
    method: &str,
    body: Option<&str>,
    charset: Option<&str>,
) -> Result<FetchResponse> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .user_agent("Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Mobile Safari/537.36")
        .build()?;

    let method = if method.eq_ignore_ascii_case("POST") {
        reqwest::Method::POST
    } else {
        reqwest::Method::GET
    };
    let mut req = client.request(method, url);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    if let Some(b) = body {
        req = req.body(b.to_string());
    }
    let resp = req.send().await?;
    let status = resp.status().as_u16();
    let final_url = resp.url().to_string();
    let resp_headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_lowercase(), v.to_str().unwrap_or_default().to_string()))
        .collect();
    let bytes = resp.bytes().await?;
    let body = decode_bytes(&bytes, charset);
    Ok(FetchResponse { body, url: final_url, headers: resp_headers, status })
}

/// 兼容旧签名（GET）
pub async fn fetch_get(url: &str, headers: &HashMap<String, String>, timeout_secs: u64) -> Result<FetchResponse> {
    fetch(url, headers, timeout_secs, "GET", None, None).await
}

// ==================== 书源 cookie（按用户隔离） ====================

/// 书源 cookie 存取：由 router 启动时注册（底层 Storage；None = 未注册，不附加 cookie）。
/// 全局注册（对齐 js.rs SOURCE_VARS 模式）：Storage 为连接池句柄，Clone 廉价。
static COOKIE_STORAGE: LazyLock<Mutex<Option<crate::storage::Storage>>> =
    LazyLock::new(|| Mutex::new(None));

/// 注册书源 cookie 存储（router 初始化时调用一次）
pub fn register_cookie_storage(storage: crate::storage::Storage) {
    *COOKIE_STORAGE.lock().unwrap_or_else(|e| e.into_inner()) = Some(storage);
}

/// 测试用：清空注册（回到无 cookie 状态）
pub fn clear_cookie_storage() {
    *COOKIE_STORAGE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// 取请求 URL 的 baseUrl（scheme://host[:port]）——书源 cookie 匹配键
pub fn base_url_of(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    let port = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
    Some(format!("{}://{host}{port}", parsed.scheme()))
}

/// 按命名空间 + 请求 URL 查书源 cookie（无注册/未命中 → None）
pub async fn cookie_for(ns: &str, url: &str) -> Option<String> {
    let base = base_url_of(url)?;
    let storage = COOKIE_STORAGE.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
    storage.get_cookie_by_base(ns, &base).await.ok().flatten()
}

/// 按命名空间 + 请求 URL 查书源登录态（cookie + user_agent）
pub async fn session_for(ns: &str, url: &str) -> Option<(String, String)> {
    let base = base_url_of(url)?;
    let storage = COOKIE_STORAGE.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT source_url, cookie, user_agent FROM book_source_cookies WHERE user_namespace = ?1",
    )
    .bind(ns)
    .fetch_all(&storage.pool)
    .await
    .ok()?;
    let target = crate::storage::normalize_base(&base)?;
    for (source_url, cookie, ua) in rows {
        // `##` 后缀：主地址/备用地址任一段命中即可（与 book_sources 语义一致）
        let any_match = source_url.split("##").any(|part| crate::storage::normalize_base(part) == Some(target.clone()));
        if any_match {
            return Some((cookie, ua));
        }
    }
    None
}

// ==================== 书源抓取（带 cookie + Cloudflare 质询绕过） ====================

/// 书源 GET（自动附加书源 cookie；CF 质询自动转 FlareSolverr）
pub async fn http_get(
    ns: &str,
    url: &str,
    headers: &HashMap<String, String>,
    timeout_secs: u64,
) -> Result<FetchResponse> {
    http_fetch(ns, url, headers, timeout_secs, "GET", None, None).await
}

/// 书源 POST（自动附加书源 cookie；CF 质询自动转 FlareSolverr）
pub async fn http_post(
    ns: &str,
    url: &str,
    headers: &HashMap<String, String>,
    timeout_secs: u64,
    body: Option<&str>,
    charset: Option<&str>,
) -> Result<FetchResponse> {
    http_fetch(ns, url, headers, timeout_secs, "POST", body, charset).await
}

/// 书源抓取统一入口：cookie 注入 → 直连 → CF 质询检测 → FlareSolverr 兜底
async fn http_fetch(
    ns: &str,
    url: &str,
    headers: &HashMap<String, String>,
    timeout_secs: u64,
    method: &str,
    body: Option<&str>,
    charset: Option<&str>,
) -> Result<FetchResponse> {
    // ① 书源 cookie + 记录的 UA（FlareSolverr 返回的 UA 绑定 cookie——部分站点校验 UA 一致性）
    let (cookie, stored_ua) = session_for(ns, url).await.unwrap_or_default();
    let mut req_headers = headers.clone();
    if !cookie.is_empty() {
        req_headers.insert("Cookie".to_string(), cookie.clone());
    }
    if !stored_ua.is_empty() && !req_headers.contains_key("User-Agent") && !req_headers.contains_key("user-agent") {
        req_headers.insert("User-Agent".to_string(), stored_ua);
    }

    // ② 直连
    let resp = fetch(url, &req_headers, timeout_secs, method, body, charset).await?;

    // ③ CF 质询检测（503 + 特征 HTML）→ 解质询降级链：FlareSolverr（配置了 URL）→
    //    内置浏览器（进程内 CDP，见 solve_cf_builtin）
    if is_cloudflare_challenge(resp.status, &resp.body) {
        if let Some(fs) = flaresolverr_request(url, &cookie, method, body, timeout_secs).await? {
            // FS 解成功：cookie 与用户原 cookie 按 name 合并后存库（按用户）+ UA 记录；
            // 响应用 FS 返回的最终 URL/状态（CF 重定向后）
            let fs_pairs: Vec<(String, String)> =
                fs.cookies.iter().map(|c| (c.name.clone(), c.value.clone())).collect();
            store_solution_session(ns, url, &cookie, &fs_pairs, &fs.user_agent).await;
            return Ok(FetchResponse {
                body: fs.response,
                url: if fs.url.is_empty() { url.to_string() } else { fs.url },
                headers: Vec::new(),
                status: fs.status,
            });
        }
        // 未配置 FLARESOLVERR_URL → 内置浏览器求解（进程内 CDP，不依赖外部容器）
        return solve_cf_builtin(ns, url, &cookie).await;
    }
    Ok(resp)
}

/// 请求 URL → 书源 source_url（cookie 存储键；按 base 匹配）。
/// 无既有 cookie 行时回退用请求 baseUrl 作为键（get_cookie_by_base 按 base 命中，不影响查找）。
async fn resolve_source_url(ns: &str, url: &str) -> Option<String> {
    let base = base_url_of(url)?;
    let Some(storage) = COOKIE_STORAGE.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return Some(base);
    };
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT source_url FROM book_source_cookies WHERE user_namespace = ?1",
    )
    .bind(ns)
    .fetch_all(&storage.pool)
    .await
    .ok()?;
    let target = crate::storage::normalize_base(&base)?;
    rows.into_iter()
        .find(|su| su.split("##").any(|part| crate::storage::normalize_base(part) == Some(target.clone())))
        .or(Some(base))
}

// ==================== Cloudflare 质询检测 ====================

/// Cloudflare 质询特征检测（503 + HTML 特征；未命中返回 false——零开销直连）
pub fn is_cloudflare_challenge(status: u16, body: &str) -> bool {
    if status != 503 {
        return false;
    }
    let body = body.to_lowercase();
    [
        "cf-browser-gesture",
        "challenge-platform",
        "__cf_chl",
        "cf-chl-",
        "just a moment",
        "cf_chl_opt",
        "challenge-running",
    ]
    .iter()
    .any(|m| body.contains(m))
}

// ==================== FlareSolverr（CF 质询解） ====================

/// FS 返回的 cookie（数组项：name/value/domain/path/...）
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct FsCookie {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
}

/// FS 解结果
pub struct FsSolution {
    pub response: String,
    pub cookies: Vec<FsCookie>,
    pub user_agent: String,
    /// FS 返回的最终 URL（CF 重定向后；空则回退请求 URL）
    pub url: String,
    /// FS 返回的最终 HTTP 状态（缺省 200）
    pub status: u16,
}

/// FlareSolverr 请求配置（环境变量 FLARESOLVERR_URL，默认空 = 禁用）
pub fn flaresolverr_base() -> Option<String> {
    let v = std::env::var("FLARESOLVERR_URL").unwrap_or_default().trim().to_string();
    if v.is_empty() {
        None
    } else {
        Some(v.trim_end_matches('/').to_string())
    }
}

/// 请求 FlareSolverr：`POST {base}/v1`（cmd=request.get，带书源 cookie 数组保持会话连续性）。
/// - 未配置 FLARESOLVERR_URL → Ok(None)（降级直连结果）
/// - FS 错误/超时（60s）→ Err（明确报错，含 FS 地址提示）
pub async fn flaresolverr_request(
    url: &str,
    cookie: &str,
    method: &str,
    body: Option<&str>,
    _timeout_secs: u64,
) -> Result<Option<FsSolution>> {
    let Some(base) = flaresolverr_base() else {
        return Ok(None);
    };
    // 用户 cookie（"a=1; b=2"）→ FS cookies 数组（name/value/domain/path）
    let cookies: Vec<serde_json::Value> = parse_cookie_string(cookie)
        .into_iter()
        .map(|(name, value)| {
            serde_json::json!({ "name": name, "value": value })
        })
        .collect();
    let mut payload = serde_json::json!({
        "cmd": "request.get",
        "url": url,
        "maxTimeout": 60000,
        "cookies": cookies,
    });
    if method.eq_ignore_ascii_case("POST") {
        payload["cmd"] = serde_json::json!("request.post");
        if let Some(b) = body {
            payload["postData"] = serde_json::json!(b);
        }
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?;
    let resp = client
        .post(format!("{base}/v1"))
        .json(&payload)
        .send()
        .await
        .map_err(|e| anyhow!("FlareSolverr 请求失败（{base}）: {e}"))?;
    let status = resp.status().as_u16();
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| anyhow!("FlareSolverr 响应解析失败（{base}）: {e}"))?;
    if json.get("status").and_then(|s| s.as_str()) != Some("ok") {
        let msg = json
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("未知错误");
        return Err(anyhow!("FlareSolverr 解质询失败（{base}，HTTP {status}）: {msg}"));
    }
    let solution = json.get("solution").cloned().unwrap_or_default();
    let response = solution
        .get("response")
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .to_string();
    let user_agent = solution
        .get("userAgent")
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    let final_url = solution
        .get("url")
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    let status = solution
        .get("status")
        .and_then(|s| s.as_u64())
        .unwrap_or(200)
        .min(u16::MAX as u64) as u16;
    let cookies: Vec<FsCookie> = serde_json::from_value(solution.get("cookies").cloned().unwrap_or_default())
        .unwrap_or_default();
    Ok(Some(FsSolution { response, cookies, user_agent, url: final_url, status }))
}

// ==================== 内置浏览器 CF 质询求解（进程内 CDP） ====================

/// 浏览器可用性探测（CF 内置求解前置检查；测试钩子可强制覆盖）
fn cf_browser_available() -> bool {
    #[cfg(test)]
    {
        use std::sync::atomic::Ordering;
        match CF_BROWSER_AVAIL_OVERRIDE.load(Ordering::Relaxed) {
            1 => return true,
            -1 => return false,
            _ => {}
        }
    }
    crate::service::browser::is_browser_available()
}

/// 测试钩子：强制浏览器可用性（Some(true)/Some(false) 强制；None 恢复自动探测）
#[cfg(test)]
pub(crate) fn force_cf_browser_available(v: Option<bool>) {
    use std::sync::atomic::Ordering;
    CF_BROWSER_AVAIL_OVERRIDE.store(
        match v {
            Some(true) => 1,
            Some(false) => -1,
            None => 0,
        },
        Ordering::Relaxed,
    );
}

#[cfg(test)]
static CF_BROWSER_AVAIL_OVERRIDE: std::sync::atomic::AtomicI8 = std::sync::atomic::AtomicI8::new(0);

/// 内置浏览器 CF 质询求解（FLARESOLVERR_URL 未配置时的降级路径）：
/// - 浏览器不可用 → 明确错误（提示安装 Edge/Chrome 或配置 FLARESOLVERR_URL）
/// - 成功：solution.html 作为响应正文；cookies 与用户 cookie 按 name 合并后存库（按用户）
///   + 浏览器 UA 记录（与 cf_clearance 绑定）
async fn solve_cf_builtin(ns: &str, url: &str, user_cookie: &str) -> Result<FetchResponse> {
    if !cf_browser_available() {
        return Err(anyhow!("CF 质询需浏览器环境：安装 Edge/Chrome 或配置 FLARESOLVERR_URL"));
    }
    let cookies = parse_cookie_string(user_cookie);
    let solution = crate::service::browser::solve_cf_challenge(url, &cookies, CF_SOLVE_MAX_WAIT_MS)
        .await
        .map_err(|e| anyhow!("内置浏览器解 CF 质询失败（{url}）: {e:#}"))?;
    store_solution_session(ns, url, user_cookie, &solution.cookies, &solution.user_agent).await;
    Ok(FetchResponse {
        body: solution.html,
        url: url.to_string(),
        headers: Vec::new(),
        status: 200,
    })
}

/// 解质询成功后持久化（按用户）：cookies 与用户原 cookie 按 name 合并存库 + UA 记录。
/// 两者都为空时跳过（无新信息）；存储失败仅告警（不影响本次响应）。
async fn store_solution_session(
    ns: &str,
    url: &str,
    user_cookie: &str,
    solution_cookies: &[(String, String)],
    user_agent: &str,
) {
    if solution_cookies.is_empty() && user_agent.is_empty() {
        return;
    }
    let storage_opt: Option<crate::storage::Storage> =
        COOKIE_STORAGE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(storage) = storage_opt else { return };
    let Some(su) = resolve_source_url(ns, url).await else { return };
    let fs_cookies: Vec<FsCookie> = solution_cookies
        .iter()
        .map(|(n, v)| FsCookie { name: n.clone(), value: v.clone(), domain: None, path: None })
        .collect();
    let merged = merge_fs_cookies(user_cookie, &fs_cookies);
    if !merged.is_empty() {
        let _ = storage.set_cookie(ns, &su, &merged).await;
    }
    // UA 与库中不同则一并记录（部分站点 UA 绑定 cookie）
    if !user_agent.is_empty() {
        let need_update = match storage.get_source_session(ns, &su).await {
            Ok(Some((_, old_ua))) => old_ua != user_agent,
            _ => true,
        };
        if need_update {
            let _ = storage.set_cookie_user_agent(ns, &su, user_agent).await;
        }
    }
}

// ==================== cookie 工具（合并策略见下） ====================

/// 解析 "a=1; b=2" cookie 串 → (name, value) 对（跳过空/损坏项）
pub fn parse_cookie_string(cookie: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for part in cookie.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((k, v)) = part.split_once('=') {
            let k = k.trim();
            let v = v.trim();
            if !k.is_empty() && !v.is_empty() {
                out.push((k.to_string(), v.to_string()));
            }
        }
    }
    out
}

/// 合并 FlareSolverr cookie 与用户原 cookie（**按 name 合并**）：
/// - 同名：FS 值覆盖用户值（cf_clearance 等质询 cookie 以 FS 为准）
/// - 不同名：保留用户值
/// - 顺序：按用户原 cookie 顺序为基底，FS 新增 name 依次追加（顺序稳定）
/// - 序列化 "a=1; b=2; cf_clearance=..." 存库（按用户）
pub fn merge_fs_cookies(user_cookie: &str, fs_cookies: &[FsCookie]) -> String {
    let mut order: Vec<String> = Vec::new();
    let mut map: HashMap<String, String> = HashMap::new();
    for (k, v) in parse_cookie_string(user_cookie) {
        if !map.contains_key(&k) {
            order.push(k.clone());
        }
        map.insert(k, v);
    }
    for c in fs_cookies {
        if c.name.is_empty() {
            continue;
        }
        if !map.contains_key(&c.name) {
            order.push(c.name.clone());
        }
        map.insert(c.name.clone(), c.value.clone());
    }
    order
        .into_iter()
        .filter_map(|k| map.get(&k).map(|v| format!("{k}={v}")))
        .collect::<Vec<_>>()
        .join("; ")
}

/// 解析书源 header 字段（legacy：JSON 字符串或 key=value 行）
pub fn parse_header(header: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let header = header.trim();
    if header.is_empty() {
        return map;
    }
    // 尝试 JSON（兼容单引号 JSON：'key': 'value' → 标准 JSON）
    if header.starts_with('{') {
        let normalized = if header.contains('\'') && !header.contains('"') {
            header.replace('\'', "\"")
        } else {
            header.to_string()
        };
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&normalized) {
            if let Some(obj) = v.as_object() {
                for (k, val) in obj {
                    if let Some(s) = val.as_str() {
                        map.insert(k.clone(), s.to_string());
                    } else {
                        map.insert(k.clone(), val.to_string());
                    }
                }
                return map;
            }
        }
    }
    // key=value 行
    for line in header.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base_url_of() {
        assert_eq!(base_url_of("https://a.com/book/1?x=2").as_deref(), Some("https://a.com"));
        assert_eq!(base_url_of("https://a.com:8443/x").as_deref(), Some("https://a.com:8443"));
        assert_eq!(base_url_of("http://a.com").as_deref(), Some("http://a.com"));
        assert_eq!(base_url_of("not a url"), None);
    }

    #[test]
    fn test_cookie_for_unregistered_is_none() {
        clear_cookie_storage();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(cookie_for("default", "https://a.com/x"));
        assert!(r.is_none());
    }

    #[test]
    fn test_is_cloudflare_challenge() {
        // 503 + 特征 HTML → true
        assert!(is_cloudflare_challenge(
            503,
            "<html>Just a moment...<script src=\"/cdn-cgi/challenge-platform/h/g/orchestrate/jsch/v1\"></script>"
        ));
        assert!(is_cloudflare_challenge(503, "__cf_chl_opt_tKb6Qe=...; cf-browser-gesture"));
        // 非 503 → false（即使含特征）
        assert!(!is_cloudflare_challenge(200, "Just a moment"));
        // 503 无特征 → false（零开销直连路径）
        assert!(!is_cloudflare_challenge(503, "<html>maintenance</html>"));
        assert!(!is_cloudflare_challenge(404, "challenge-platform"));
    }

    #[test]
    fn test_parse_cookie_string() {
        assert_eq!(parse_cookie_string("a=1; b=2"), vec![("a".into(), "1".into()), ("b".into(), "2".into())]);
        assert_eq!(parse_cookie_string("a=1;; b="), vec![("a".into(), "1".into())]);
        assert_eq!(parse_cookie_string(""), Vec::<(String, String)>::new());
    }

    /// 合并策略：同名 FS 覆盖、不同名保留用户值、顺序稳定（用户序为基底 + FS 新名追加）
    #[test]
    fn test_merge_fs_cookies() {
        let user = "sid=abc; theme=dark";
        let fs = vec![
            FsCookie { name: "cf_clearance".into(), value: "xyz".into(), domain: None, path: None },
            FsCookie { name: "theme".into(), value: "light".into(), domain: None, path: None },
        ];
        let merged = merge_fs_cookies(user, &fs);
        assert_eq!(merged, "sid=abc; theme=light; cf_clearance=xyz");
    }

    #[test]
    fn test_merge_fs_cookies_empty_user() {
        let fs = vec![
            FsCookie { name: "cf_clearance".into(), value: "xyz".into(), domain: None, path: None },
        ];
        assert_eq!(merge_fs_cookies("", &fs), "cf_clearance=xyz");
        assert_eq!(merge_fs_cookies("a=1", &[]), "a=1");
        assert_eq!(merge_fs_cookies("", &[]), "");
    }

    #[test]
    fn test_flaresolverr_disabled_by_default() {
        // 未配置 FLARESOLVERR_URL → Ok(None)（降级直连，不影响现有路径）
        std::env::remove_var("FLARESOLVERR_URL");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(flaresolverr_request("https://a.com", "a=1", "GET", None, 15));
        assert!(r.is_ok());
        assert!(r.unwrap().is_none());
    }

    /// 浏览器不可用分支单测：solve_cf_builtin 返回明确错误（不启动浏览器、不发请求）
    #[test]
    fn test_cf_builtin_browser_unavailable_returns_clear_error() {
        force_cf_browser_available(Some(false));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(solve_cf_builtin("default", "https://cf.example.com/book/1", "sid=abc"));
        force_cf_browser_available(None);
        let err = r.err().expect("浏览器不可用应返回错误");
        assert!(
            err.to_string().contains("CF 质询需浏览器环境"),
            "错误应提示浏览器环境: {err}"
        );
        assert!(
            err.to_string().contains("FLARESOLVERR_URL"),
            "错误应提示可配置 FLARESOLVERR_URL: {err}"
        );
    }
}
