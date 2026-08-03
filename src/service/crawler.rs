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

    // ③ CF 质询检测（503 + 特征 HTML）→ FlareSolverr 解
    if is_cloudflare_challenge(resp.status, &resp.body) {
        if let Some(fs) = flaresolverr_request(url, &cookie, method, body, timeout_secs).await? {
            // FS 返回 cookie 与用户原 cookie 按 name 合并后存库（按用户）
            if !fs.cookies.is_empty() || !fs.user_agent.is_empty() {
                let storage_opt: Option<crate::storage::Storage> =
                    COOKIE_STORAGE.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if let Some(su) = resolve_source_url(ns, url).await {
                    if let Some(storage) = storage_opt {
                        let merged = merge_fs_cookies(&cookie, &fs.cookies);
                        if !merged.is_empty() {
                            let _ = storage.set_cookie(ns, &su, &merged).await;
                        }
                        // UA 与库中不同则一并记录（部分站点 UA 绑定 cookie）
                        if !fs.user_agent.is_empty() {
                            let need_update = match storage.get_source_session(ns, &su).await {
                                Ok(Some((_, old_ua))) => old_ua != fs.user_agent,
                                _ => true,
                            };
                            if need_update {
                                let _ = storage.set_cookie_user_agent(ns, &su, &fs.user_agent).await;
                            }
                        }
                    }
                }
            }
            return Ok(FetchResponse {
                body: fs.response,
                url: url.to_string(),
                headers: Vec::new(),
                status: 200,
            });
        }
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
    let cookies: Vec<FsCookie> = serde_json::from_value(solution.get("cookies").cloned().unwrap_or_default())
        .unwrap_or_default();
    Ok(Some(FsSolution { response, cookies, user_agent }))
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
}
