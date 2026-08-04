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

/// 抓取（复用搜索的 URL 附加参数处理；自动带书源 cookie——按用户命名空间）
pub async fn fetch_url(ns: &str, url: &str, source: &BookSource) -> Result<crawler::FetchResponse> {
    let headers = source.header.as_deref().map(crawler::parse_header).unwrap_or_default();
    crawler::http_get(ns, url, &headers, 15).await
}

/// ruleRelated 结构（GAP 17b：相关推荐——字段与 ruleExplore 一致：bookList + 字段规则）
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RelatedRule {
    pub book_list: Option<String>,
    pub name: Option<String>,
    pub author: Option<String>,
    pub book_url: Option<String>,
    pub cover_url: Option<String>,
}

/// 相关推荐解析（GAP 17b）：ruleRelated 应用详情页 HTML，同 ruleExplore 风格
/// （bookList CSS 链式 + 字段规则）→ [{name, author, bookUrl, coverUrl}]
pub fn analyze_related_books(html: &str, base_url: &str, source: &BookSource) -> Vec<crate::model::book_chapter::RelatedBook> {
    let rule: RelatedRule = source
        .rule_related
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let Some(book_list_rule) = rule.book_list.clone() else {
        return vec![];
    };
    // 复用 ruleExplore 书单解析（SearchRule 字段名与 RelatedRule 一致）
    let search_rule = crate::service::search::SearchRule {
        book_list: rule.book_list,
        name: rule.name,
        author: rule.author,
        book_url: rule.book_url,
        cover_url: rule.cover_url,
        ..Default::default()
    };
    crate::service::search::analyze_book_list_for_explore(html, base_url, source, &search_rule, &book_list_rule)
        .into_iter()
        .map(|b| crate::model::book_chapter::RelatedBook {
            name: b.name,
            author: b.author,
            book_url: b.book_url,
            cover_url: b.cover_url,
        })
        .collect()
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
        related_books: analyze_related_books(html, base_url, source),
    }
}

/// 目录解析（ruleToc：chapterList 定位 + 字段规则；多页 nextTocUrl 循环）
pub async fn analyze_toc(
    ns: &str,
    toc_url: &str,
    source: &BookSource,
    max_pages: usize,
) -> Result<Vec<BookChapter>> {
    let mut all: Vec<BookChapter> = Vec::new();
    let mut current_url = toc_url.to_string();

    for _page in 0..max_pages {
        let resp = fetch_url(ns, &current_url, source).await?;
        let base = resp.url.clone();
        let rule: TocRule = source
            .rule_toc
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let Some(list_rule) = rule.chapter_list.clone() else {
            break;
        };

        let items = toc_items(&list_rule, &resp.body);
        let start_index = all.len() as i64;
        all.extend(chapters_from_items(&items, &rule, &base, start_index));

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

/// 单页目录解析（ruleToc 应用一次——getChapterListByRule 调试接口复用）
pub async fn parse_toc_page(
    ns: &str,
    url: &str,
    source: &BookSource,
) -> Result<Vec<BookChapter>> {
    let resp = fetch_url(ns, url, source).await?;
    let base = resp.url.clone();
    let rule: TocRule = source
        .rule_toc
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let Some(list_rule) = rule.chapter_list.clone() else {
        return Ok(vec![]);
    };
    let items = toc_items(&list_rule, &resp.body);
    Ok(chapters_from_items(&items, &rule, &base, 0))
}

/// chapterList 规则 → 章节上下文列表（CSS/JSONPath/Regex/JS 全类型）
pub(crate) fn toc_items(list_rule: &str, body: &str) -> Vec<String> {
    let parsed = parse_rule(list_rule);
    let mut items: Vec<String> = match parsed.kind {
        RuleKind::Css => css_chain(list_rule, body),
        RuleKind::JsonPath | RuleKind::Regex => apply(list_rule, body),
        RuleKind::Js => js_chapter_items(list_rule, body),
        _ => vec![],
    };
    // <js> 包裹形式（parse_rule 不识别为 Js）——兜底
    if items.is_empty()
        && (list_rule.contains("<js>") || list_rule.trim_start().starts_with("@js:"))
    {
        items = js_chapter_items(list_rule, body);
    }
    items
}

/// JS chapterList（<js> 或 @js:——eval 返回章节对象数组）→ 每项 JSON 文本
/// （数组经递归 JSON 转换——避免 ToString 的 "[object Object]" 使解析为空）
fn js_chapter_items(rule: &str, body: &str) -> Vec<String> {
    let code = if rule.trim_start().starts_with("@js:") {
        rule.trim_start()[4..].to_string()
    } else if let Some(start) = rule.find("<js>") {
        let rest = &rule[start + 4..];
        let end = rest.find("</js>").unwrap_or(rest.len());
        rest[..end].to_string()
    } else {
        return vec![];
    };
    let mut vars = std::collections::HashMap::new();
    vars.insert("result".to_string(), body.to_string());
    vars.insert("key".to_string(), String::new());
    vars.insert("page".to_string(), "1".to_string());
    let Ok(result) = crate::parser::js::eval_js_json(&code, &vars) else {
        return vec![];
    };
    match result {
        serde_json::Value::Array(list) => list
            .iter()
            .map(|item| match item {
                serde_json::Value::Object(_) => item.to_string(),
                serde_json::Value::String(s) => s.clone(),
                _ => String::new(),
            })
            .filter(|s| !s.is_empty())
            .collect(),
        serde_json::Value::Object(_) => vec![result.to_string()],
        _ => vec![],
    }
}

/// 章节上下文列表 → 章节（字段规则应用 + 相对 URL 转绝对）
fn chapters_from_items(
    items: &[String],
    rule: &TocRule,
    base: &str,
    start_index: i64,
) -> Vec<BookChapter> {
    items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| {
            let title = field(item, rule.chapter_name.as_deref(), "");
            let url = rule
                .chapter_url
                .as_deref()
                .map(|r| field(item, Some(r), ""))
                .unwrap_or_default();
            if title.is_empty() && url.is_empty() {
                return None;
            }
            let url = to_abs(&url, base);
            let is_volume = title.starts_with("卷") || title.contains("【卷");
            Some(BookChapter {
                title,
                url,
                is_volume,
                index: start_index + i as i64,
            })
        })
        .collect()
}

/// 正文解析（ruleContent：content 字段 + sourceRegex 清洗 + 多页）
pub async fn analyze_content(
    ns: &str,
    chapter_url: &str,
    source: &BookSource,
    max_pages: usize,
) -> Result<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut current_url = chapter_url.to_string();

    for _page in 0..max_pages {
        let resp = fetch_url(ns, &current_url, source).await?;
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
    // sourceRegex 清洗（legacy：正则移除干扰内容；GAP 153：lookbehind 经 fancy-regex）
    if let Some(sr) = &rule.source_regex {
        if !sr.is_empty() {
            match crate::util::regex::Regex::new(sr) {
                Ok(re) => content = re.replace_all(&content, "").to_string(),
                Err(e) => tracing::warn!("sourceRegex 编译失败（跳过清洗）: {e}"),
            }
        }
    }
    // replaceRegex 替换
    if let Some(rr) = &rule.replace_regex {
        if let Some((old, new)) = rr.split_once("##") {
            match crate::util::regex::Regex::new(old.trim()) {
                Ok(re) => content = re.replace_all(&content, new.trim()).to_string(),
                Err(e) => tracing::warn!("replaceRegex 编译失败（跳过替换）: {e}"),
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

    /// chapterList JS 规则（JSON.parse(result).data 数组）→ 章节上下文列表
    #[test]
    fn test_toc_items_js_array() {
        let body = r#"{"data":[{"title":"第一章","href":"/c/1"},{"title":"第二章","href":"/c/2"}]}"#;
        let items = toc_items("@js:JSON.parse(result).data", body);
        assert_eq!(items.len(), 2, "JS chapterList 应解析出 2 项");
        assert!(items[0].contains("第一章"));
        assert!(items[0].contains("/c/1"));
    }

    /// chapterList JS 数组字面量 + 字段规则 → 章节（title/url 绝对化/index）
    #[test]
    fn test_toc_js_full_pipeline() {
        let rule = TocRule {
            chapter_list: Some("@js:[{t:'章A',u:'/x/1'},{t:'章B',u:'/x/2'}]".into()),
            chapter_name: Some("$.t".into()),
            chapter_url: Some("$.u".into()),
            ..Default::default()
        };
        let items = toc_items(rule.chapter_list.as_deref().unwrap(), "{}");
        assert_eq!(items.len(), 2);
        let chapters = chapters_from_items(&items, &rule, "https://src.test", 5);
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].title, "章A");
        assert_eq!(chapters[0].url, "https://src.test/x/1");
        assert_eq!(chapters[0].index, 5);
        assert_eq!(chapters[1].index, 6);
    }

    /// <js> 包裹 chapterList 兜底
    #[test]
    fn test_toc_items_js_html_wrapped() {
        let body = r#"{"data":[{"title":"包章","url":"/b/1"}]}"#;
        let items = toc_items("<js>JSON.parse(result).data</js>", body);
        assert_eq!(items.len(), 1);
        assert!(items[0].contains("包章"));
    }

    /// GAP 17b：ruleRelated CSS 链式解析 → relatedBooks
    #[test]
    fn test_analyze_related_books() {
        let mut src = test_source();
        src.rule_related = Some(serde_json::json!({
            "bookList": "ul.related@li",
            "name": "a.bookname@text",
            "author": "span.author@text",
            "bookUrl": "a@href",
            "coverUrl": "img@src"
        }));
        let html = r#"<ul class="related">
            <li><a class="bookname" href="/r/1">推荐书1</a><span class="author">作者甲</span><img src="/c1.jpg"></li>
            <li><a class="bookname" href="/r/2">推荐书2</a><span class="author">作者乙</span><img src="/c2.jpg"></li>
        </ul>"#;
        let related = analyze_related_books(html, "http://127.0.0.1:9999/book/1", &src);
        assert_eq!(related.len(), 2);
        assert_eq!(related[0].name, "推荐书1");
        assert_eq!(related[0].author, "作者甲");
        assert_eq!(related[0].book_url, "http://127.0.0.1:9999/r/1");
        // coverUrl 经 field_url 绝对化（与 ruleExplore 书单一致）
        assert_eq!(related[0].cover_url.as_deref(), Some("http://127.0.0.1:9999/c1.jpg"));
        assert_eq!(related[1].name, "推荐书2");
        // 无 ruleRelated / 无 bookList → 空
        assert!(analyze_related_books(html, "http://x", &test_source()).is_empty());
        let mut src2 = test_source();
        src2.rule_related = Some(serde_json::json!({"name": "a@text"}));
        assert!(analyze_related_books(html, "http://x", &src2).is_empty());
    }

    /// GAP 17b：getBookInfo 完整链路——analyze_book_info 返回 relatedBooks
    #[test]
    fn test_analyze_info_includes_related_books() {
        let mut src = test_source();
        src.rule_related = Some(serde_json::json!({
            "bookList": "ul.related@li",
            "name": "a@text",
            "bookUrl": "a@href"
        }));
        let html = r#"<h1 class="bookname">测试书</h1><p class="author">作者X</p>
            <ul class="related"><li><a href="/r/9">推荐书9</a></li></ul>"#;
        let info = analyze_book_info(html, "http://127.0.0.1:9999/book/1", &src, "http://127.0.0.1:9999/book/1");
        assert_eq!(info.related_books.len(), 1);
        assert_eq!(info.related_books[0].name, "推荐书9");
        assert_eq!(info.related_books[0].book_url, "http://127.0.0.1:9999/r/9");
    }

    /// GAP 153：sourceRegex/replaceRegex 支持 lookbehind（regex crate 不支持）
    #[test]
    fn test_analyze_content_lookbehind_regex() {
        let mut src = test_source();
        src.rule_content = Some(serde_json::json!({
            "content": "div.content@text",
            "sourceRegex": "(?<=广告：)\\S+",
            "replaceRegex": "(?<=第).+(?=章)##X"
        }));
        let html = r#"<div class="content">正文：第一章 测试内容 广告：烦人</div>"#;
        let content = analyze_content_from(html, &src);
        // sourceRegex（lookbehind）移除 "烦人"；replaceRegex（lookbehind+lookahead）把 "一章 " 替换为 X
        assert_eq!(content, "正文：第X章 测试内容 广告：");
    }
}
