//! RSS 链路：feed 抓取（feed-rs 解析）→ 文章列表；网页正文提取（简单 CSS 选择器）
//!
//! 对齐 legacy Rss.getArticles / getContent 的简化 v1：
//! - 列表：GET sortUrl（未配置则 sourceUrl）→ feed-rs 解析 RSS/Atom → RssArticle 列表
//! - 正文：优先文章 content 字段（feed content/summary）；为空则抓取文章链接，
//!   用常见正文容器 CSS 选择器提取段落文本

use anyhow::{Context, Result};
use std::collections::HashMap;

use crate::model::rss::{RssArticle, RssSource};
use crate::service::crawler;

/// 抓取 feed 并解析为文章列表（含分页参数替换；{{page}} 存在时替换为页码）
pub async fn fetch_articles(source: &RssSource, page: i64) -> Result<Vec<RssArticle>> {
    let url = build_feed_url(source, page);
    let headers = crawler::parse_header(source.header().as_deref().unwrap_or(""));
    let resp = crawler::fetch(&url, &headers, 30, "GET", None, None)
        .await
        .with_context(|| format!("抓取 RSS 失败: {url}"))?;
    parse_feed(&resp.body, source)
}

/// 构造抓取 URL：sortUrl 多段（&&/换行分隔，每段 name::url）取第一段 URL；无有效段用 sourceUrl
pub fn build_feed_url(source: &RssSource, page: i64) -> String {
    let mut url = source.source_url.clone();
    if let Some(sort_url) = source.sort_url().filter(|s| !s.trim().is_empty()) {
        // 兼容 legacy sortUrls()：每段 "name::url"（JS 前缀 v1 不执行），取首个含 :: 的段
        for seg in sort_url.split(['\n', '&']).map(str::trim).filter(|s| !s.is_empty()) {
            if let Some((_, u)) = seg.split_once("::") {
                if !u.trim().is_empty() {
                    url = u.trim().to_string();
                    break;
                }
            }
        }
    }
    if url.contains("{{page}}") {
        url = url.replace("{{page}}", &page.to_string());
    }
    url
}

/// 解析 feed XML → 文章列表（纯函数，单测直接调用）
pub fn parse_feed(xml: &str, source: &RssSource) -> Result<Vec<RssArticle>> {
    let feed = feed_rs::parser::parse(xml.as_bytes())
        .with_context(|| format!("解析 RSS 失败: {}", source.source_url))?;
    let mut articles: Vec<RssArticle> = feed
        .entries
        .iter()
        .map(|e| article_from_entry(e, source))
        .collect();
    // 按发布时间倒序（无时间戳的排最后，保持 feed 顺序）
    articles.sort_by(|a, b| b.time.cmp(&a.time));
    Ok(articles)
}

/// feed-rs Entry → RssArticle
fn article_from_entry(entry: &feed_rs::model::Entry, source: &RssSource) -> RssArticle {
    let url = entry
        .links
        .iter()
        .find(|l| l.rel.as_deref().unwrap_or("alternate") == "alternate")
        .map(|l| l.href.clone())
        .or_else(|| entry.links.first().map(|l| l.href.clone()))
        .or_else(|| {
            // guid 为 URL 时兜底
            if entry.id.starts_with("http://") || entry.id.starts_with("https://") {
                Some(entry.id.clone())
            } else {
                None
            }
        })
        .unwrap_or_default();
    let title = entry.title.as_ref().map(|t| t.content.clone()).unwrap_or_default();
    // 作者：feed-rs 的 RSS2 <author> 解析为 name="author" + email=原文（如“作者乙”），
    // 因此 name 为占位符“author”或空时回退 email；Atom/dc:creator 直接用 name
    let author = entry
        .authors
        .first()
        .map(|a| {
            if a.name.is_empty() || a.name == "author" {
                a.email.clone().unwrap_or_else(|| a.name.clone())
            } else {
                a.name.clone()
            }
        })
        .unwrap_or_default();
    // 发布时间：published 优先，缺省用 updated；无时间戳置 0
    let time = entry
        .published
        .or(entry.updated)
        .map(|t| t.timestamp_millis())
        .unwrap_or(0);
    // 正文：content.body 优先，缺省用 summary
    let content = entry
        .content
        .as_ref()
        .and_then(|c| c.body.clone())
        .or_else(|| entry.summary.as_ref().map(|s| s.content.clone()));
    // 配图：media 组第一张图
    let cover = entry
        .media
        .iter()
        .flat_map(|m| m.content.iter())
        .find_map(|c| c.url.as_ref().map(|u| u.to_string()));
    RssArticle {
        url,
        source_url: source.source_url.clone(),
        title,
        author,
        time,
        content,
        cover,
        read: false,
        user_namespace: String::new(),
    }
}

/// 抓取网页正文（简单 CSS 选择器：常见正文容器 → 段落文本；兜底 body 全文）
pub async fn fetch_web_content(url: &str) -> Result<String> {
    let headers = HashMap::new();
    let resp = crawler::fetch(url, &headers, 30, "GET", None, None)
        .await
        .with_context(|| format!("抓取文章页面失败: {url}"))?;
    let text = extract_web_content(&resp.body);
    if text.is_empty() {
        anyhow::bail!("网页正文提取为空: {url}");
    }
    Ok(text)
}

/// 从 HTML 提取正文（纯函数，单测直接调用）
pub fn extract_web_content(html: &str) -> String {
    let doc = scraper::Html::parse_document(html);
    // 常见正文容器（按优先级）
    const CANDIDATES: &[&str] = &[
        "article",
        ".article-content",
        ".post-content",
        ".entry-content",
        ".article",
        "#content",
        ".content",
        "main",
    ];
    for sel in CANDIDATES {
        let Ok(selector) = scraper::Selector::parse(sel) else { continue };
        if let Some(node) = doc.select(&selector).next() {
            let text = visible_text(&node);
            if !text.is_empty() {
                return text;
            }
        }
    }
    // 兜底：body 全部可见文本
    if let Ok(selector) = scraper::Selector::parse("body") {
        if let Some(node) = doc.select(&selector).next() {
            return visible_text(&node);
        }
    }
    String::new()
}

/// 收集子树内可见文本（跳过 script/style），按行合并去空白
fn visible_text(root: &scraper::ElementRef<'_>) -> String {
    let mut out = Vec::new();
    collect_visible_text(root, &mut out);
    clean_text(&out.join("\n"))
}

/// 递归收集文本节点，跳过 script/style 子树
fn collect_visible_text(elem: &scraper::ElementRef<'_>, out: &mut Vec<String>) {
    if elem.value().name() == "script" || elem.value().name() == "style" {
        return;
    }
    for child in elem.children() {
        match child.value() {
            scraper::node::Node::Text(t) => out.push(t.text.to_string()),
            scraper::node::Node::Element(_) => {
                if let Some(e) = scraper::ElementRef::wrap(child) {
                    collect_visible_text(&e, out);
                }
            }
            _ => {}
        }
    }
}

/// 合并文本行：去空白、去空行、按行拼接
fn clean_text(raw: &str) -> String {
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> RssSource {
        RssSource {
            source_url: "https://example.com/feed.xml".into(),
            source_name: "测试源".into(),
            enabled: true,
            ..Default::default()
        }
    }

    const SAMPLE_RSS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>测试频道</title>
    <link>https://example.com</link>
    <description>测试</description>
    <item>
      <title>第一篇</title>
      <link>https://example.com/1</link>
      <guid>https://example.com/1</guid>
      <author>作者甲</author>
      <pubDate>Wed, 01 Jan 2025 00:00:00 GMT</pubDate>
      <description><![CDATA[<p>第一篇文章摘要</p>]]></description>
    </item>
    <item>
      <title>第二篇</title>
      <link>https://example.com/2</link>
      <guid>guid-2</guid>
      <author>作者乙</author>
      <pubDate>Thu, 02 Jan 2025 00:00:00 GMT</pubDate>
      <description>第二篇摘要</description>
    </item>
  </channel>
</rss>"#;

    /// feed 解析：标题 / 链接 / 作者 / 时间 / 摘要提取，且按时间倒序
    #[test]
    fn test_parse_feed_extracts_articles() {
        let articles = parse_feed(SAMPLE_RSS, &source()).expect("RSS 解析应成功");
        assert_eq!(articles.len(), 2);
        // 按发布时间倒序：第二篇（01-02）在前
        assert_eq!(articles[0].title, "第二篇");
        assert_eq!(articles[0].url, "https://example.com/2");
        assert_eq!(articles[0].author, "作者乙");
        assert_eq!(articles[0].time, 1735776000000);
        assert_eq!(articles[0].content.as_deref(), Some("第二篇摘要"));
        assert_eq!(articles[1].title, "第一篇");
        assert_eq!(articles[1].author, "作者甲");
        assert_eq!(articles[1].content.as_deref(), Some("<p>第一篇文章摘要</p>"));
        assert_eq!(articles[1].source_url, "https://example.com/feed.xml");
    }

    /// 无链接的条目用 http(s) guid 兜底；无时间戳置 0
    #[test]
    fn test_parse_feed_fallbacks() {
        let xml = r#"<?xml version="1.0"?>
<rss version="2.0"><channel>
  <item><title>无链接</title><guid isPermaLink="true">https://example.com/guid-only</guid></item>
  <item><title>纯文本guid</title><guid>abc</guid><description>摘要</description></item>
</channel></rss>"#;
        let articles = parse_feed(xml, &source()).expect("解析应成功");
        assert_eq!(articles.len(), 2);
        assert_eq!(articles[0].url, "https://example.com/guid-only");
        assert_eq!(articles[1].url, "", "非 URL guid 不应作为链接");
        assert_eq!(articles[1].time, 0);
        assert_eq!(articles[1].content.as_deref(), Some("摘要"));
    }

    /// Atom feed 解析
    #[test]
    fn test_parse_atom_feed() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Atom 频道</title>
  <entry>
    <title>Atom文章</title>
    <link href="https://example.com/atom/1"/>
    <id>tag:example.com,2025:1</id>
    <author><name>作者丙</name></author>
    <published>2025-03-01T08:00:00Z</published>
    <content type="html">&lt;p&gt;Atom正文&lt;/p&gt;</content>
  </entry>
</feed>"#;
        let articles = parse_feed(xml, &source()).expect("Atom 解析应成功");
        assert_eq!(articles.len(), 1);
        assert_eq!(articles[0].title, "Atom文章");
        assert_eq!(articles[0].url, "https://example.com/atom/1");
        assert_eq!(articles[0].author, "作者丙");
        assert_eq!(articles[0].time, 1740816000000);
        assert_eq!(articles[0].content.as_deref(), Some("<p>Atom正文</p>"));
    }

    /// 无效 feed：返回错误
    #[test]
    fn test_parse_feed_invalid() {
        let err = parse_feed("<html>不是feed</html>", &source());
        assert!(err.is_err(), "非 feed 内容应报错");
    }

    /// sortUrl 构造：无 sortUrl 用 sourceUrl；多段取第一段；无 :: 的段按 legacy 丢弃；{{page}} 替换
    /// （sortUrl 从 raw_json 读取）
    #[test]
    fn test_build_feed_url() {
        let mut s = source();
        assert_eq!(build_feed_url(&s, 1), "https://example.com/feed.xml");
        s.raw_json =
            Some(r#"{"sortUrl":"列表::https://example.com/list\n详情::https://example.com/detail"}"#.into());
        assert_eq!(build_feed_url(&s, 1), "https://example.com/list");
        // 无 :: 的段被丢弃 → 回退 sourceUrl（legacy sortUrls 语义）
        s.raw_json = Some(r#"{"sortUrl":"https://example.com/page/{{page}}.xml"}"#.into());
        assert_eq!(build_feed_url(&s, 3), "https://example.com/feed.xml");
        // 带 :: 且含 {{page}} → 替换页码
        s.raw_json = Some(r#"{"sortUrl":"分页::https://example.com/page/{{page}}.xml"}"#.into());
        assert_eq!(build_feed_url(&s, 3), "https://example.com/page/3.xml");
        // sourceUrl 本身含 {{page}} → 替换
        s.raw_json = None;
        s.source_url = "https://example.com/feed/{{page}}.xml".into();
        assert_eq!(build_feed_url(&s, 2), "https://example.com/feed/2.xml");
    }

    /// 正文提取：优先常见容器，忽略脚本/样式，按行合并
    #[test]
    fn test_extract_web_content_selectors() {
        let html = r#"<html><head><style>.x{}</style></head><body>
            <nav>导航</nav>
            <article class="article-content">
                <h1>标题</h1>
                <p>第一段</p>
                <p>第二段</p>
            </article>
            <footer>页脚</footer>
        </body></html>"#;
        let text = extract_web_content(html);
        assert!(text.contains("第一段"));
        assert!(text.contains("第二段"));
        assert!(!text.contains("导航"), "容器外文本不应混入");
        assert!(!text.contains("页脚"));
        assert!(!text.contains("style"), "style 文本不应混入");
        // 无正文容器 → body 兜底
        let bare = extract_web_content("<html><body><p>只有一段</p></body></html>");
        assert_eq!(bare, "只有一段");
        // 完全无文本
        assert_eq!(extract_web_content("<html><body><script>var a=1;</script></body></html>"), "");
    }
}
