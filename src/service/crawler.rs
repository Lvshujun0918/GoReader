//! HTTP 抓取客户端（reqwest，书源抓取）

use anyhow::Result;
use std::collections::HashMap;
use std::time::Duration;

/// 抓取响应
pub struct FetchResponse {
    pub body: String,
    pub url: String,
}

/// 简单抓取（GET，支持 header JSON）
pub async fn fetch(
    url: &str,
    headers: &HashMap<String, String>,
    timeout_secs: u64,
) -> Result<FetchResponse> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .user_agent("Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Mobile Safari/537.36")
        .build()?;

    let mut req = client.get(url);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    let resp = req.send().await?;
    let final_url = resp.url().to_string();
    let body = resp.text().await?;
    Ok(FetchResponse { body, url: final_url })
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
