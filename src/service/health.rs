//! 书源健康检测（getInvalidBookSources）：并发 HEAD/首页检测，轻量超时 8s

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::model::BookSource;
use crate::service::crawler;

/// 单书源健康检测：HEAD 优先（405/501 或失败回退 GET 首页）
/// 返回 (是否可用, 说明)
pub async fn check_source(_ns: &str, source: &BookSource) -> (bool, String) {
    let base = source
        .book_source_url
        .split("##")
        .next()
        .unwrap_or("")
        .trim();
    if base.is_empty() {
        return (false, "书源地址为空".to_string());
    }
    let headers = source
        .header
        .as_deref()
        .map(crawler::parse_header)
        .unwrap_or_default();
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent("Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Mobile Safari/537.36")
        .build()
    {
        Ok(c) => c,
        Err(e) => return (false, format!("客户端构建失败: {e}")),
    };
    let req_headers = to_reqwest_headers(&headers);

    // HEAD 优先（轻量）
    let head = client.head(base).headers(req_headers.clone()).send().await;
    if let Ok(resp) = head {
        let status = resp.status().as_u16();
        if status == 405 || status == 501 {
            // 服务器不支持 HEAD → GET 兜底
        } else if status < 400 {
            return (true, format!("HEAD {status}"));
        } else {
            // 4xx/5xx：不视为“网络失效”，但按不可用处理（GET 兜底验证首页）
            let get = client.get(base).headers(req_headers.clone()).send().await;
            return match get {
                Ok(r) if r.status().as_u16() < 400 => {
                    (true, format!("GET {}", r.status().as_u16()))
                }
                Ok(r) => (false, format!("HTTP {}", r.status().as_u16())),
                Err(e) => (false, format!("连接失败: {e}")),
            };
        }
    }
    // GET 兜底
    match client.get(base).headers(req_headers).send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            if status < 400 {
                (true, format!("GET {status}"))
            } else {
                (false, format!("HTTP {status}"))
            }
        }
        Err(e) => (false, format!("连接失败: {e}")),
    }
}

/// 并发检测全部书源（并发上限 8）；返回不可用列表 [(书源, 原因)]
pub async fn find_invalid(ns: &str, sources: &[BookSource]) -> Vec<(BookSource, String)> {
    let semaphore = Arc::new(tokio::sync::Semaphore::new(8));
    let mut handles = Vec::with_capacity(sources.len());
    for source in sources {
        let sem = semaphore.clone();
        let ns = ns.to_string();
        let source = source.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await;
            check_source(&ns, &source).await
        }));
    }
    let mut invalid = Vec::new();
    for (source, h) in sources.iter().zip(handles) {
        if let Ok((ok, reason)) = h.await {
            if !ok {
                invalid.push((source.clone(), reason));
            }
        }
    }
    invalid
}

/// HashMap<String,String> → http::HeaderMap（非法头名/值跳过）
fn to_reqwest_headers(headers: &HashMap<String, String>) -> http::HeaderMap {
    let mut hm = http::HeaderMap::new();
    for (k, v) in headers {
        if let (Ok(k), Ok(v)) = (
            http::header::HeaderName::try_from(k.as_str()),
            http::header::HeaderValue::try_from(v.as_str()),
        ) {
            hm.insert(k, v);
        }
    }
    hm
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_reqwest_headers_skips_invalid() {
        let mut map = HashMap::new();
        map.insert("User-Agent".to_string(), "UA1".to_string());
        map.insert("bad header name".to_string(), "x".to_string());
        let hm = to_reqwest_headers(&map);
        assert_eq!(hm.len(), 1);
        assert_eq!(hm.get("user-agent").unwrap(), "UA1");
    }

    #[tokio::test]
    async fn test_check_source_empty_url() {
        let src = BookSource::default();
        let (ok, reason) = check_source("default", &src).await;
        assert!(!ok);
        assert!(reason.contains("书源地址为空"));
    }

    #[tokio::test]
    async fn test_check_source_unreachable() {
        // 127.0.0.1:1 连接拒绝 → 不可用（快速失败）
        let src = BookSource {
            book_source_url: "http://127.0.0.1:1".into(),
            book_source_name: "坏源".into(),
            ..Default::default()
        };
        let (ok, reason) = check_source("default", &src).await;
        assert!(!ok, "不可达应判定不可用: {reason}");
    }
}
