//! 发现/探索（ruleExplore）：exploreUrl 集合 + 书单解析
//!
//! 对齐 legacy WebBook.exploreBook：URL 列表 → 抓取 → ruleExplore 字段 → SearchBook

use anyhow::Result;

use crate::model::BookSource;
use crate::service::crawler;
use crate::service::search::SearchBook;

/// 探索条目（title + url）
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExploreEntry {
    pub title: String,
    pub url: String,
}

/// 解析 exploreUrl（legado 语义）：
/// - `@js:代码`：执行 JS（返回 JSON.stringify([{title,url},...])）→ 解析条目
/// - 普通多行 URL：每行一个条目（title 从 URL 尾部提取）
pub fn parse_explore_entries(explore_url: &str) -> Vec<ExploreEntry> {
    let mut entries = Vec::new();
    for line in explore_url.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(code) = line.strip_prefix("@js:") {
            if let Ok(result) = crate::parser::js::eval_js(code, &Default::default()) {
                if let Ok(list) = serde_json::from_str::<Vec<serde_json::Value>>(&result) {
                    for item in list {
                        let title = item
                            .get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let url = item.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        if !url.is_empty() {
                            entries.push(ExploreEntry { title, url });
                        }
                    }
                    continue;
                }
            }
        }
        // 普通 URL 行：title 从尾部提取
        let title = url_title(&line);
        entries.push(ExploreEntry {
            title,
            url: line.to_string(),
        });
    }
    entries
}

/// 从 URL 提取分类名（尾部路径段/查询参数，解码）
fn url_title(url: &str) -> String {
    let cleaned = url.split(['?', '&', '#']).next().unwrap_or(url);
    let seg = cleaned
        .trim_end_matches('/')
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or(cleaned);
    let decoded = percent_decode(seg);
    if !decoded.is_empty() && decoded != "/" {
        return decoded;
    }
    // 查询参数 name/type/id
    for param in ["name", "type", "id"] {
        for pair in url.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if k == param && !v.is_empty() {
                    return percent_decode(v);
                }
            }
        }
    }
    url.to_string()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 单页发现：抓取 + 解析（复用搜索的 SearchRule 语义）
pub async fn explore_url(
    url: &str,
    source: &BookSource,
) -> Result<Vec<SearchBook>> {
    // URL 模板（{{page}}）
    let url = url.replace("{{page}}", "1").replace("{page}", "1");
    let headers = source.header.as_deref().map(crawler::parse_header).unwrap_or_default();
    let resp = crawler::fetch(&url, &headers, 15, "GET", None, None).await?;

    let rule: crate::service::search::SearchRule = match &source.rule_explore {
        Some(v) => serde_json::from_value(v.clone()).unwrap_or_default(),
        None => return Ok(vec![]),
    };
    let Some(book_list_rule) = rule.book_list.clone() else {
        return Ok(vec![]);
    };
    let books = crate::service::search::analyze_book_list_for_explore(
        &resp.body,
        &resp.url,
        source,
        &rule,
        &book_list_rule,
    );
    Ok(books)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_urls() {
        let urls = "https://a.com/list\n#注释\nhttps://b.com/{{page}}\n";
        let parsed = parse_explore_urls(urls);
        assert_eq!(parsed.len(), 2);
        assert!(parsed[1].contains("{{page}}"));
    }
}
