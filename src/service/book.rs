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
    let headers = source
        .header
        .as_deref()
        .map(crawler::parse_header)
        .unwrap_or_default();
    crawler::http_get(ns, url, &headers, 15, source.proxy_url.as_deref()).await
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
pub fn analyze_related_books(
    html: &str,
    base_url: &str,
    source: &BookSource,
) -> Vec<crate::model::book_chapter::RelatedBook> {
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
    crate::service::search::analyze_book_list_for_explore(
        html,
        base_url,
        source,
        &search_rule,
        &book_list_rule,
    )
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
pub fn analyze_book_info(
    html: &str,
    base_url: &str,
    source: &BookSource,
    book_url: &str,
) -> BookInfo {
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
        book_type: source.book_source_type,
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
pub async fn parse_toc_page(ns: &str, url: &str, source: &BookSource) -> Result<Vec<BookChapter>> {
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

/// 音频 URL → contentType（m3u8 走 HLS，其余按扩展名映射；未知默认 audio/mpeg）
///
/// 非文本正文返回契约（legacy getBookContent，由 router 直接以 JSON 构造，
/// P3-A：原 MediaContent 枚举从未被构造 → 已删除，此处保留 contentType 映射）：
/// - 音频书（book_type=1）：`{audioUrl, contentType}`
/// - 漫画书（book_type=2）：`{images: [url, ...]}`（章节 = 图片列表）
/// - 文件书（book_type=3）：`{downloadUrl}`
/// - 视频书（book_type=4）：`{videoUrl}`
pub fn audio_content_type(url: &str) -> &'static str {
    let path = url.split('?').next().unwrap_or(url).to_lowercase();
    if path.ends_with(".m3u8") {
        "application/vnd.apple.mpegurl"
    } else if path.ends_with(".mp3") {
        "audio/mpeg"
    } else if path.ends_with(".m4a") || path.ends_with(".aac") {
        "audio/mp4"
    } else if path.ends_with(".ogg") || path.ends_with(".oga") {
        "audio/ogg"
    } else if path.ends_with(".wav") {
        "audio/wav"
    } else if path.ends_with(".flac") {
        "audio/flac"
    } else if path.ends_with(".opus") {
        "audio/opus"
    } else if path.ends_with(".mp4") || path.ends_with(".m4v") {
        "audio/mp4"
    } else {
        "audio/mpeg"
    }
}

/// 规则结果 → URL 列表（兼容三种形态：普通 URL 文本 / JSON 字符串数组
/// （@js: 返回数组经 js_result_to_string 序列化）/ JSON 对象数组）
fn collect_urls(value: &str, out: &mut Vec<String>) {
    let t = value.trim();
    if t.is_empty() {
        return;
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
        match v {
            serde_json::Value::String(s) => {
                let s = s.trim().to_string();
                if !s.is_empty() {
                    out.push(s);
                }
            }
            serde_json::Value::Array(arr) => {
                for item in arr {
                    match item {
                        serde_json::Value::String(s) => {
                            let s = s.trim().to_string();
                            if !s.is_empty() {
                                out.push(s);
                            }
                        }
                        // 对象数组（如 [{url: "..."}]）：取 url/src/href 字段
                        serde_json::Value::Object(map) => {
                            for k in ["url", "src", "href"] {
                                if let Some(serde_json::Value::String(s)) = map.get(k) {
                                    let s = s.trim().to_string();
                                    if !s.is_empty() {
                                        out.push(s);
                                        break;
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => out.push(t.to_string()),
        }
    } else {
        out.push(t.to_string());
    }
}

/// 媒体 URL 提取（音频/视频/文件书共用）：ruleContent.content 规则应用到章节页 → URL；
/// 规则缺失或提取为空 → 章节 URL 本身（音频书章节 URL 常即音频流 URL 直链）。
pub async fn analyze_media_url(ns: &str, chapter_url: &str, source: &BookSource) -> Result<String> {
    let rule: ContentRule = source
        .rule_content
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let Some(content_rule) = rule.content.clone() else {
        return Ok(chapter_url.to_string());
    };
    if content_rule.trim().is_empty() {
        return Ok(chapter_url.to_string());
    }
    let resp = fetch_url(ns, chapter_url, source).await?;
    let base = resp.url.clone();
    // 规则结果可能含多值（CSS 命中多个/JSON 数组）——取首个 URL
    let mut urls: Vec<String> = Vec::new();
    for v in apply(&content_rule, &resp.body) {
        collect_urls(&v, &mut urls);
        if !urls.is_empty() {
            break;
        }
    }
    let Some(mut url) = urls.into_iter().next() else {
        return Ok(chapter_url.to_string());
    };
    url = to_abs(&url, &base);
    Ok(url)
}

/// 漫画书图片列表提取（ruleContent.content 规则 → 图片 URL 列表）：
/// - CSS/JSONPath/Regex 规则：全部命中值均为图片 URL
/// - @js:/<js> 规则：结果可为 URL 字符串或字符串数组（JSON 序列化形态）
/// - 规则缺失：章节 URL 本身为图片直链时直接返回
pub async fn analyze_comic_images(
    ns: &str,
    chapter_url: &str,
    source: &BookSource,
) -> Result<Vec<String>> {
    let rule: ContentRule = source
        .rule_content
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let Some(content_rule) = rule.content.clone() else {
        // 无规则：章节 URL 即图片直链（部分图源章节 URL 直接指向图片）
        if looks_like_image_url(chapter_url) {
            return Ok(vec![chapter_url.to_string()]);
        }
        return Ok(vec![]);
    };
    if content_rule.trim().is_empty() {
        return Ok(vec![]);
    }
    let resp = fetch_url(ns, chapter_url, source).await?;
    let base = resp.url.clone();
    let mut urls: Vec<String> = Vec::new();
    for v in apply(&content_rule, &resp.body) {
        collect_urls(&v, &mut urls);
    }
    // 绝对化 + 去重保序
    let mut seen = std::collections::HashSet::new();
    let mut images: Vec<String> = Vec::new();
    for u in urls {
        let abs = to_abs(&u, &base);
        if seen.insert(abs.clone()) {
            images.push(abs);
        }
    }
    Ok(images)
}

/// 是否为图片直链（按扩展名判断，忽略查询串）
fn looks_like_image_url(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url).to_lowercase();
    [
        ".jpg", ".jpeg", ".png", ".gif", ".webp", ".bmp", ".avif", ".svg",
    ]
    .iter()
    .any(|ext| path.ends_with(ext))
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
///
/// GAP 97：规则提取结果原样返回——书源正文含 HTML 标签（@html 提取或 JSON 正文源
/// 直接携带 <p>/<br> 等）时不做剥离/转义，前端已有纯文本渲染负责展示。
/// GAP 109：contentReplace（legacy 命名）即 ruleContent.replaceRegex（`模式##替换`），
/// 与 sourceRegex（删除型）均在解析期应用——正文净化在 getBookContent 返回前完成。
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
        let info = analyze_book_info(
            html,
            "http://127.0.0.1:9999/book/1",
            &test_source(),
            "http://127.0.0.1:9999/book/1",
        );
        assert_eq!(info.name, "测试书");
        assert_eq!(info.author, "作者X");
        assert_eq!(info.intro.as_deref(), Some("简介内容"));
        assert_eq!(info.cover_url.as_deref(), Some("/cover.jpg"));
        assert_eq!(info.toc_url.as_deref(), Some("http://127.0.0.1:9999/toc"));
    }

    #[test]
    fn test_analyze_content_from() {
        let html =
            r#"<html><div class="content">第一章正文内容测试。</div><script>干扰</script></html>"#;
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
        let body =
            r#"{"data":[{"title":"第一章","href":"/c/1"},{"title":"第二章","href":"/c/2"}]}"#;
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
        assert_eq!(
            related[0].cover_url.as_deref(),
            Some("http://127.0.0.1:9999/c1.jpg")
        );
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
        let info = analyze_book_info(
            html,
            "http://127.0.0.1:9999/book/1",
            &src,
            "http://127.0.0.1:9999/book/1",
        );
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

    /// GAP 97：书源正文含 HTML 标签（<p>/<br> 等）时保留标签原样返回。
    /// 行为确认：ruleContent.content 用 @html 提取（或 JSON 正文源直接含标签）时，
    /// 后端不做任何剥离/转义——原样透传；前端已有纯文本渲染（HTML → 文本），
    /// 段落分隔依赖 <br>/<p> 标签，因此此处必须保留。
    #[test]
    fn test_analyze_content_preserves_html_tags() {
        let mut src = test_source();
        // @html 提取：保留 <p>/<br> 标签原样（含匹配元素外层标签）
        src.rule_content = Some(serde_json::json!({ "content": "div.content@html" }));
        let html = r#"<div class="content"><p>第一段</p><br><p>第二段</p></div>"#;
        let content = analyze_content_from(html, &src);
        assert!(
            content.contains("<p>第一段</p><br><p>第二段</p>"),
            "HTML 标签应原样保留: {content}"
        );
        assert!(
            content.contains("<p>") && content.contains("<br>"),
            "<p>/<br> 不应被剥离: {content}"
        );
        assert!(
            content.contains("<div"),
            "@html 含匹配元素外层标签（前端纯文本渲染无影响）: {content}"
        );

        // 无 @ 的裸选择器（legacy 兼容）→ 取纯文本（仅此处剥离，规则显式 @html 时保留）
        src.rule_content = Some(serde_json::json!({ "content": "div.content" }));
        let content = analyze_content_from(html, &src);
        assert!(!content.contains("<p>"), "裸选择器取文本: {content}");
        assert!(content.contains("第一段") && content.contains("第二段"));

        // 清洗（sourceRegex/replaceRegex）作用于原样内容——HTML 标签不受影响
        src.rule_content = Some(serde_json::json!({
            "content": "div.content@html",
            "replaceRegex": "第一段##甲段"
        }));
        let content = analyze_content_from(html, &src);
        assert!(
            content.contains("<p>甲段</p><br><p>第二段</p>"),
            "{content}"
        );
    }

    /// GAP 109：contentReplace/replaceRegex 在 ruleContent 解析已应用——
    /// 构造含广告行的正文，断言净化（广告行移除、正文保留）。
    /// （legacy 的 contentReplace 对应 ruleContent.replaceRegex：`模式##替换`，
    /// 逐条 replace_all；sourceRegex 为删除型清洗，同样在解析期应用）
    #[test]
    fn test_analyze_content_replace_regex_cleans_ad_lines() {
        let mut src = test_source();
        src.rule_content = Some(serde_json::json!({
            "content": "div.content@html",
            // 广告行 + 推广行 → 替换为空（净化）
            "replaceRegex": "【广告】.*?。|（推广）.*?。##"
        }));
        let html = concat!(
            r#"<div class="content"><p>正文第一段。</p>"#,
            r#"<p>【广告】本书由某某网首发，请支持正版。</p>"#,
            r#"<p>（推广）加群领福利。</p>"#,
            r#"<p>正文第二段。</p></div>"#
        );
        let content = analyze_content_from(html, &src);
        assert!(
            !content.contains("【广告】"),
            "广告行应被 replaceRegex 清除: {content}"
        );
        assert!(!content.contains("（推广）"), "推广行应被清除: {content}");
        assert!(
            content.contains("正文第一段。") && content.contains("正文第二段。"),
            "正文应保留: {content}"
        );

        // sourceRegex（删除型）同样应用于正文
        src.rule_content = Some(serde_json::json!({
            "content": "div.content@text",
            "sourceRegex": "【广告】[^】]*"
        }));
        let html = r#"<div class="content">正文甲。【广告】这是一条广告】正文乙。</div>"#;
        let content = analyze_content_from(html, &src);
        assert!(
            !content.contains("广告"),
            "sourceRegex 应删除广告片段: {content}"
        );
        assert!(content.contains("正文甲。") && content.contains("正文乙。"));
    }

    // ---------------- 非文本内容分派（音频/漫画/视频/文件） ----------------

    /// 微型 HTTP 服务器：固定响应体（同 crawler 测试模式）
    async fn serve(body: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            for _ in 0..10 {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let mut resp = head.into_bytes();
                resp.extend_from_slice(body.as_bytes());
                let _ = sock.write_all(&resp).await;
            }
        });
        format!("http://{addr}")
    }

    /// 音频书：ruleContent.content 提取音频 URL → analyze_media_url 返回音频流直链
    #[tokio::test]
    async fn test_analyze_media_url_audio() {
        let _ssrf = crate::service::crawler::ssrf_allow_private_guard(true); // mock 绑定 127.0.0.1（P1 SSRF 校验放行，仅测试）
        let base =
            serve(r#"<html><div class="player"><audio src="/stream/1.mp3"></audio></div></html>"#)
                .await;
        let mut src = test_source();
        src.rule_content = Some(serde_json::json!({
            "content": "div.player audio@src"
        }));
        let url = analyze_media_url("default", &format!("{base}/chapter/1"), &src)
            .await
            .unwrap();
        assert_eq!(
            url,
            format!("{base}/stream/1.mp3"),
            "音频 URL 应提取并绝对化"
        );
        // contentType 映射
        assert_eq!(audio_content_type(&url), "audio/mpeg");
        assert_eq!(
            audio_content_type("https://x/a.m3u8?t=1"),
            "application/vnd.apple.mpegurl"
        );
        assert_eq!(audio_content_type("https://x/a.m4a"), "audio/mp4");
    }

    /// 音频书：无 ruleContent（章节 URL 即音频流直链）→ 原样返回章节 URL
    #[tokio::test]
    async fn test_analyze_media_url_direct_chapter() {
        let mut src = test_source();
        src.rule_content = None;
        let url = analyze_media_url("default", "https://cdn.example.com/audio/42.m4a", &src)
            .await
            .unwrap();
        assert_eq!(url, "https://cdn.example.com/audio/42.m4a");
    }

    /// 漫画书：CSS 规则命中多个图片节点 → images 列表（绝对化 + 去重）
    #[tokio::test]
    async fn test_analyze_comic_images_css() {
        let _ssrf = crate::service::crawler::ssrf_allow_private_guard(true); // mock 绑定 127.0.0.1（P1 SSRF 校验放行，仅测试）
        let base = serve(
            r#"<html><div class="imgs"><img src="/p/1.jpg"><img src="/p/2.jpg"><img src="/p/1.jpg"></div></html>"#,
        )
        .await;
        let mut src = test_source();
        src.rule_content = Some(serde_json::json!({ "content": "div.imgs img@src" }));
        let images = analyze_comic_images("default", &format!("{base}/comic/1"), &src)
            .await
            .unwrap();
        assert_eq!(
            images,
            vec![format!("{base}/p/1.jpg"), format!("{base}/p/2.jpg")],
            "应提取全部图片且去重保序、相对转绝对"
        );
    }

    /// 漫画书：@js: 规则返回 JSON 字符串数组 → images 列表
    #[tokio::test]
    async fn test_analyze_comic_images_js_array() {
        let _ssrf = crate::service::crawler::ssrf_allow_private_guard(true); // mock 绑定 127.0.0.1（P1 SSRF 校验放行，仅测试）
        let base = serve(r#"{"data":["/a/1.webp","/a/2.webp"]}"#).await;
        let mut src = test_source();
        src.rule_content = Some(serde_json::json!({
            "content": "@js:JSON.parse(result).data"
        }));
        let images = analyze_comic_images("default", &format!("{base}/comic/2"), &src)
            .await
            .unwrap();
        assert_eq!(
            images,
            vec![format!("{base}/a/1.webp"), format!("{base}/a/2.webp")],
            "JS 数组应逐个提取并绝对化"
        );
    }

    /// 漫画书：无规则且章节 URL 即图片直链 → 单图列表；有规则但提取不到 → 空列表
    #[tokio::test]
    async fn test_analyze_comic_images_direct() {
        let _ssrf = crate::service::crawler::ssrf_allow_private_guard(true); // mock 绑定 127.0.0.1（P1 SSRF 校验放行，仅测试）
                                                                             // 有规则但页面无匹配 → 空列表
        let base = serve(r#"<html><div class="imgs"></div></html>"#).await;
        let mut src = test_source();
        src.rule_content = Some(serde_json::json!({ "content": "div.imgs img@src" }));
        let images = analyze_comic_images("default", &format!("{base}/comic/3"), &src)
            .await
            .unwrap();
        assert!(images.is_empty(), "有规则但提取不到 → 空列表");

        // 无规则且章节 URL 即图片直链 → 单图列表（不抓取）
        let mut src2 = test_source();
        src2.rule_content = None;
        let images2 = analyze_comic_images("default", "https://img.example.com/comic/5.jpg", &src2)
            .await
            .unwrap();
        assert_eq!(
            images2,
            vec!["https://img.example.com/comic/5.jpg".to_string()]
        );
    }

    /// collect_urls 纯函数：URL 文本 / JSON 字符串数组 / 对象数组 / 空
    #[test]
    fn test_collect_urls() {
        let mut out = Vec::new();
        collect_urls("https://a.mp3", &mut out);
        collect_urls(r#"["https://b.jpg","/c.png"]"#, &mut out);
        collect_urls(
            r#"[{"url":"https://d.webp"},{"src":"https://e.avif"}]"#,
            &mut out,
        );
        collect_urls("  ", &mut out);
        assert_eq!(
            out,
            vec![
                "https://a.mp3",
                "https://b.jpg",
                "/c.png",
                "https://d.webp",
                "https://e.avif"
            ]
        );
        // 对象数组取 url/src/href 首命中
        let mut out2 = Vec::new();
        collect_urls(r#"[{"name":"x","href":"/h/1"}]"#, &mut out2);
        assert_eq!(out2, vec!["/h/1"]);
    }
}
