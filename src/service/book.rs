//! 书籍链路：详情（ruleBookInfo）/ 目录（ruleToc）/ 正文（ruleContent）
//!
//! 对齐 legacy WebBook：getBookInfo / getChapterList / getBookContent
//! v1：CSS/JSONPath/JS（简单）规则；多页目录/正文（nextTocUrl/nextContentUrl）循环支持

use anyhow::Result;
use serde::Deserialize;

use crate::model::book_chapter::{BookChapter, BookInfo};
use crate::model::BookSource;
use crate::parser::css_chain::css_chain;
use crate::parser::rule::{apply, parse_rule, RuleKind};
use crate::service::crawler;
use crate::service::search::{expand_embedded, field, opt_field};

/// ruleBookInfo 结构（legacy BookInfoRule）
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BookInfoRule {
    pub name: Option<String>,
    pub author: Option<String>,
    pub kind: Option<String>,
    pub intro: Option<String>,
    pub cover_url: Option<String>,
    pub toc_url: Option<String>,
    pub word_count: Option<String>,
    pub last_chapter: Option<String>,
    pub init: Option<String>,
}

/// ruleToc 结构（legacy TocRule）
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TocRule {
    pub chapter_list: Option<String>,
    pub chapter_name: Option<String>,
    pub chapter_url: Option<String>,
    pub chapter_vip: Option<String>,
    pub update_time: Option<String>,
    pub next_toc_url: Option<String>,
    pub chapter_type: Option<String>,
    pub init: Option<String>,
}

/// ruleContent 结构（legacy ContentRule）
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ContentRule {
    pub content: Option<String>,
    pub next_content_url: Option<String>,
    pub source_regex: Option<String>,
    pub replace_regex: Option<String>,
    pub init: Option<String>,
}

/// 抓取（复用搜索的 URL 附加参数处理）
pub async fn fetch_url(url: &str, source: &BookSource) -> Result<crawler::FetchResponse> {
    let headers = source.header.as_deref().map(crawler::parse_header).unwrap_or_default();
    crawler::fetch(url, &headers, 15, "GET", None, None).await
}

/// 详情解析（ruleBookInfo 字段应用于详情页 HTML）
pub fn analyze_book_info(html: &str, base_url: &str, source: &BookSource, book_url: &str) -> BookInfo {
    let rule: BookInfoRule = source
        .rule_book_info
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    // tocUrl 规则可能是 URL 拼接（如 "$.book_id\n@js:..."）——v1 支持直接路径/URL
    let toc_url = rule
        .toc_url
        .as_deref()
        .map(|r| expand_embedded(r, html))
        .filter(|r| !r.is_empty())
        .map(|r| to_abs(&r, base_url));

    BookInfo {
        name: field(html, rule.name.as_deref(), ""),
        author: field(html, rule.author.as_deref(), ""),
        kind: opt_field(html, rule.kind.as_deref()),
        intro: opt_field(html, rule.intro.as_deref()),
        cover_url: opt_field(html, rule.cover_url.as_deref()),
        toc_url,
        word_count: opt_field(html, rule.word_count.as_deref()),
        latest_chapter_title: opt_field(html, rule.last_chapter.as_deref()),
        book_url: book_url.to_string(),
        origin: source.book_source_url.clone(),
        origin_name: source.book_source_name.clone(),
        language: None,
        publisher: None,
        published_at: None,
    }
}

/// 目录解析（ruleToc：chapterList 定位 + 字段规则；多页 nextTocUrl 循环）
pub async fn analyze_toc(
    toc_url: &str,
    source: &BookSource,
    max_pages: usize,
) -> Result<Vec<BookChapter>> {
    let mut all: Vec<BookChapter> = Vec::new();
    let mut current_url = toc_url.to_string();

    for _page in 0..max_pages {
        let resp = fetch_url(&current_url, source).await?;
        let base = resp.url.clone();
        let rule: TocRule = source
            .rule_toc
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let Some(list_rule) = rule.chapter_list.clone() else {
            break;
        };

        let parsed = parse_rule(&list_rule);
        let items: Vec<String> = match parsed.kind {
            RuleKind::Css => css_chain(&list_rule, &resp.body),
            RuleKind::JsonPath | RuleKind::Regex => apply(&list_rule, &resp.body),
            _ => vec![],
        };

        let start_index = all.len() as i64;
        for (i, item) in items.iter().enumerate() {
            let title = field(item, rule.chapter_name.as_deref(), "");
            let url = rule
                .chapter_url
                .as_deref()
                .map(|r| field(item, Some(r), ""))
                .unwrap_or_default();
            if title.is_empty() && url.is_empty() {
                continue;
            }
            let url = to_abs(&url, &base);
            let is_volume = title.starts_with("卷") || title.contains("【卷");
            all.push(BookChapter {
                title,
                url,
                is_volume,
                index: start_index + i as i64,
            });
        }

        // 多页目录
        let next = rule
            .next_toc_url
            .as_deref()
            .map(|r| field(&resp.body, Some(r), ""))
            .unwrap_or_default();
        if next.is_empty() {
            break;
        }
        current_url = to_abs(&next, &base);
    }

    Ok(all)
}

/// 正文解析（ruleContent：content 字段 + sourceRegex 清洗 + 多页）
pub async fn analyze_content(
    chapter_url: &str,
    source: &BookSource,
    max_pages: usize,
) -> Result<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut current_url = chapter_url.to_string();

    for _page in 0..max_pages {
        let resp = fetch_url(&current_url, source).await?;
        let base = resp.url.clone();
        let content = analyze_content_from(&resp.body, source);
        if !content.is_empty() {
            parts.push(content);
        }

        let rule: ContentRule = source
            .rule_content
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let next = rule
            .next_content_url
            .as_deref()
            .map(|r| field(&resp.body, Some(r), ""))
            .unwrap_or_default();
        if next.is_empty() {
            break;
        }
        current_url = to_abs(&next, &base);
    }

    Ok(parts.join("\n"))
}

/// 单页正文解析（纯函数，可测）
pub fn analyze_content_from(html: &str, source: &BookSource) -> String {
    let rule: ContentRule = source
        .rule_content
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let Some(content_rule) = rule.content.clone() else {
        return String::new();
    };
    let mut content = field(html, Some(&content_rule), "");
    // sourceRegex 清洗（legacy：正则移除干扰内容）
    if let Some(sr) = &rule.source_regex {
        if !sr.is_empty() {
            if let Ok(re) = regex::Regex::new(sr) {
                content = re.replace_all(&content, "").to_string();
            }
        }
    }
    // replaceRegex 替换
    if let Some(rr) = &rule.replace_regex {
        if let Some((old, new)) = rr.split_once("##") {
            if let Ok(re) = regex::Regex::new(old.trim()) {
                content = re.replace_all(&content, new.trim()).to_string();
            }
        }
    }
    content
}

/// 相对 URL → 绝对
fn to_abs(url: &str, base: &str) -> String {
    crate::service::search::to_absolute(url, base)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_source() -> BookSource {
        BookSource {
            book_source_url: "http://127.0.0.1:9999".into(),
            book_source_name: "测试源".into(),
            rule_book_info: Some(serde_json::json!({
                "name": "h1.bookname@text", "author": "p.author@text",
                "intro": "div.intro@text", "coverUrl": "img.cover@src",
                "tocUrl": "/toc"
            })),
            rule_toc: Some(serde_json::json!({
                "chapterList": "ul.chapters@li",
                "chapterName": "a@text", "chapterUrl": "a@href"
            })),
            rule_content: Some(serde_json::json!({
                "content": "div.content@text"
            })),
            ..Default::default()
        }
    }

    #[test]
    fn test_analyze_info() {
        let html = r#"<h1 class="bookname">测试书</h1><p class="author">作者X</p>
            <div class="intro">简介内容</div><img class="cover" src="/cover.jpg">"#;
        let info = analyze_book_info(html, "http://127.0.0.1:9999/book/1", &test_source(), "http://127.0.0.1:9999/book/1");
        assert_eq!(info.name, "测试书");
        assert_eq!(info.author, "作者X");
        assert_eq!(info.intro.as_deref(), Some("简介内容"));
        assert_eq!(info.cover_url.as_deref(), Some("/cover.jpg"));
        assert_eq!(info.toc_url.as_deref(), Some("http://127.0.0.1:9999/toc"));
    }

    #[test]
    fn test_analyze_content_from() {
        let html = r#"<html><div class="content">第一章正文内容测试。</div><script>干扰</script></html>"#;
        let content = analyze_content_from(html, &test_source());
        assert_eq!(content, "第一章正文内容测试。");
    }

    #[test]
    fn test_analyze_content_replace() {
        let mut src = test_source();
        src.rule_content = Some(serde_json::json!({
            "content": "div.content@text",
            "replaceRegex": "\\s+## "
        }));
        let html = r#"<div class="content">多   个  空格</div>"#;
        let content = analyze_content_from(html, &src);
        assert_eq!(content, "多个空格");
    }
}
