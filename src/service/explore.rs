//! 发现/探索（ruleExplore）：exploreUrl 集合 + 书单解析
//!
//! 对齐 legacy WebBook.exploreBook：URL 列表 → 抓取 → ruleExplore 字段 → SearchBook

use anyhow::Result;

use crate::model::BookSource;
use crate::service::crawler;
use crate::service::search::SearchBook;

/// 解析 exploreUrl 集合（legacy：多行 URL，{{page}} 分页）
pub fn parse_explore_urls(explore_url: &str) -> Vec<String> {
    explore_url
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.to_string())
        .collect()
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
