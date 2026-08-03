//! 搜索链路：searchUrl 构造 + 抓取 + ruleSearch 规则应用 → SearchBook
//!
//! 对齐 legacy WebBook.searchBook / BookList.analyzeBookList 语义（v1：无 JS）

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::model::BookSource;
use crate::parser::rule::{apply, parse_rule, RuleKind};
use crate::service::crawler;

/// 搜索结果（兼容 legacy SearchBook 字段）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchBook {
    pub book_url: String,
    pub origin: String,
    pub origin_name: String,
    #[serde(rename = "type")]
    pub book_type: i64,
    pub name: String,
    pub author: String,
    pub kind: Option<String>,
    pub cover_url: Option<String>,
    pub intro: Option<String>,
    pub word_count: Option<String>,
    pub latest_chapter_title: Option<String>,
    pub toc_url: String,
    pub time: i64,
    pub variable: Option<String>,
    pub origin_order: i64,
}

/// ruleSearch 结构（legacy BookListRule 字段）
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRule {
    pub book_list: Option<String>,
    pub name: Option<String>,
    pub author: Option<String>,
    pub kind: Option<String>,
    pub intro: Option<String>,
    pub book_url: Option<String>,
    pub cover_url: Option<String>,
    pub word_count: Option<String>,
    pub last_chapter: Option<String>,
    pub update_time: Option<String>,
    pub score: Option<String>,
    pub comment: Option<String>,
    pub tags: Option<String>,
    pub serial_number: Option<String>,
    pub variable: Option<serde_json::Value>,
}

/// 构造搜索 URL（legado 语义：{{key}}/{{page}} 双花括号 + {key} 单花括号 + 相对路径拼 baseUrl）
pub fn build_search_url(search_url: &str, key: &str, page: i64, base_url: &str) -> String {
    let mut url = search_url.to_string();
    // 双花括号优先
    url = url.replace("{{key}}", key).replace("{{page}}", &page.to_string());
    // 单花括号
    url = url.replace("{key}", key).replace("{page}", &page.to_string());
    // <2,3,4> 页数规则：取第 page 个（超出取最后）
    if url.contains('<') && url.contains('>') {
        if let Some(start) = url.find('<') {
            if let Some(end) = url.find('>') {
                let inner = &url[start + 1..end];
                let pages: Vec<&str> = inner.split(',').map(|s| s.trim()).collect();
                if !pages.is_empty() {
                    let idx = ((page as usize).saturating_sub(1)).min(pages.len() - 1);
                    let rep = format!("<{inner}>");
                    url = url.replace(&rep, pages[idx]);
                }
            }
        }
    }
    // 相对路径拼 baseUrl
    if url.starts_with('/') && !url.starts_with("//") {
        if let Ok(base) = Url::parse(base_url) {
            if let Some(host) = base.host_str() {
                let scheme = base.scheme();
                let port = base.port().map(|p| format!(":{p}")).unwrap_or_default();
                return format!("{scheme}://{host}{port}{url}");
            }
        }
    }
    url
}

/// 相对 URL → 绝对（基于 base）
pub fn to_absolute(url: &str, base: &str) -> String {
    if url.is_empty() {
        return url.to_string();
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        return url.to_string();
    }
    if url.starts_with("//") {
        if let Ok(b) = Url::parse(base) {
            return format!("{}:{url}", b.scheme());
        }
        return url.to_string();
    }
    if let Ok(joined) = Url::parse(base).and_then(|b| b.join(url)) {
        return joined.to_string();
    }
    url.to_string()
}

/// searchUrl 附加参数（`url,{...}` 后缀 JSON；v1 支持 js/bodyJs，其他键忽略）
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UrlSuffix {
    /// js：执行 JS 修改 URL（注入 key/page/result（空字符串）/baseUrl/headerMap），返回值作为 URL
    pub js: Option<String>,
    /// bodyJs：对响应体执行 JS 后作为新响应体（注入 result=原响应体）
    pub body_js: Option<String>,
    /// 请求方法（POST/GET，默认 GET）
    pub method: Option<String>,
    /// POST body（支持 {{key}}/{{page}} 模板替换）
    pub body: Option<String>,
    /// 附加请求头（与书源 header 合并）
    pub headers: Option<std::collections::HashMap<String, String>>,
    /// 响应字符集（GB2312/GBK/UTF-8 等）
    pub charset: Option<String>,
}

/// 切分 `url,{...}` 后缀：从最后一个「逗号后整段为合法 JSON」的位置切分
pub(crate) fn split_url_suffix(url: &str) -> (String, UrlSuffix) {
    let mut split: Option<(usize, UrlSuffix)> = None;
    for (i, ch) in url.char_indices() {
        if ch != ',' {
            continue;
        }
        let rest = url[i + 1..].trim_start();
        if !rest.starts_with('{') {
            continue;
        }
        if let Ok(suffix) = serde_json::from_str::<UrlSuffix>(rest) {
            split = Some((i, suffix));
        }
    }
    match split {
        Some((i, suffix)) => (url[..i].to_string(), suffix),
        None => (url.to_string(), UrlSuffix::default()),
    }
}

/// JS 注入变量（key/page/baseUrl/headerMap(JSON 字符串)/result）
fn js_vars(
    key: &str,
    page: i64,
    base_url: &str,
    headers: &HashMap<String, String>,
    result: &str,
) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    vars.insert("key".to_string(), key.to_string());
    vars.insert("page".to_string(), page.to_string());
    vars.insert("baseUrl".to_string(), base_url.to_string());
    vars.insert(
        "headerMap".to_string(),
        serde_json::to_string(headers).unwrap_or_else(|_| "{}".to_string()),
    );
    vars.insert("result".to_string(), result.to_string());
    vars
}

/// 构造搜索请求 URL：
/// 1) `@js:`/`js:` 前缀 → JS 返回值作为搜索 URL（注入 key/page/baseUrl/headerMap）；
/// 2) `,{...}` 后缀解析：js 键对 URL 执行 JS（注入 key/page/result 为空字符串/baseUrl/headerMap）；
/// 3) 模板替换（{{key}}/{key}/{{page}}/{page}）与相对路径拼接
fn build_request_url(
    search_url: &str,
    key: &str,
    page: i64,
    base_url: &str,
    headers: &HashMap<String, String>,
) -> Result<(String, UrlSuffix)> {
    // 1) @js:/js: 前缀
    let raw = search_url.trim_start();
    let url = match raw.strip_prefix("@js:").or_else(|| raw.strip_prefix("js:")) {
        Some(code) => {
            let vars = js_vars(key, page, base_url, headers, "");
            crate::parser::js::eval_js(code.trim(), &vars)?
        }
        None => search_url.to_string(),
    };
    // 2) `,{...}` 后缀
    let (url_part, mut suffix) = split_url_suffix(&url);
    let url = match suffix.js.take() {
        Some(js) => {
            let vars = js_vars(key, page, base_url, headers, "");
            crate::parser::js::eval_js(&js, &vars)?
        }
        None => url_part,
    };
    // 3) 模板替换 + 相对路径拼接
    Ok((build_search_url(&url, key, page, base_url), suffix))
}

/// bodyJs：对响应体执行 JS 后作为新响应体（注入 result=原响应体）
fn apply_body_js(
    body: &str,
    suffix: &UrlSuffix,
    key: &str,
    page: i64,
    base_url: &str,
    headers: &HashMap<String, String>,
) -> Result<String> {
    let Some(js) = &suffix.body_js else {
        return Ok(body.to_string());
    };
    let vars = js_vars(key, page, base_url, headers, body);
    crate::parser::js::eval_js(js, &vars)
}

/// 并发率（legado concurrentRate）：纯数字 = 每次请求前 sleep 该毫秒；
/// `n/window`（如 20/60000）→ 每次请求间隔 window/n 毫秒
fn concurrent_rate_sleep_ms(rate: Option<&str>) -> u64 {
    let Some(rate) = rate else { return 0 };
    let rate = rate.trim();
    if rate.is_empty() {
        return 0;
    }
    if let Ok(ms) = rate.parse::<u64>() {
        return ms;
    }
    if let Some((count, window)) = rate.split_once('/') {
        if let (Ok(c), Ok(w)) = (count.trim().parse::<u64>(), window.trim().parse::<u64>()) {
            if c > 0 {
                return w / c;
            }
        }
    }
    0
}

/// 执行单个书源搜索
pub async fn search_one_source(
    ns: &str,
    source: &BookSource,
    key: &str,
    page: i64,
) -> Result<Vec<SearchBook>> {
    let Some(search_url) = source.search_url.clone() else {
        return Ok(vec![]);
    };
    let rule: SearchRule = match &source.rule_search {
        Some(v) => serde_json::from_value(v.clone()).unwrap_or_default(),
        None => return Ok(vec![]),
    };
    let Some(book_list_rule) = rule.book_list.clone() else {
        return Ok(vec![]);
    };

    let headers = source
        .header
        .as_deref()
        .map(crawler::parse_header)
        .unwrap_or_default();

    // 1) @js:/js: 前缀 + 2) `,{...}` 后缀（js 修改 URL）→ 最终请求 URL
    let (url, suffix) =
        build_request_url(&search_url, key, page, &source.book_source_url, &headers)?;

    // 3) 并发率：数字 → 请求前 sleep 该毫秒
    let delay_ms = concurrent_rate_sleep_ms(source.concurrent_rate.as_deref());
    if delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }

    // 附加 headers（书源 header + 后缀 headers 合并）
    let mut req_headers = headers.clone();
    if let Some(extra) = &suffix.headers {
        for (k, v) in extra {
            req_headers.insert(k.clone(), v.clone());
        }
    }
    // POST body 模板替换（{{key}}/{{page}}）
    let post_body = suffix.body.as_ref().map(|b| {
        b.replace("{{key}}", key)
            .replace("{{page}}", &page.to_string())
            .replace("{key}", key)
            .replace("{page}", &page.to_string())
    });
    // 书源抓取（自动带书源 cookie——按用户命名空间）
    let method = suffix.method.as_deref().unwrap_or("GET");
    let resp = if method.eq_ignore_ascii_case("POST") {
        crawler::http_post(ns, &url, &req_headers, 15, post_body.as_deref(), suffix.charset.as_deref()).await?
    } else {
        crawler::http_get(ns, &url, &req_headers, 15).await?
    };
    let base = resp.url.clone();
    // bodyJs：对响应体执行 JS 后作为新响应体
    let body = apply_body_js(&resp.body, &suffix, key, page, &source.book_source_url, &req_headers)?;
    let books = analyze_book_list(&body, &base, source, &rule, &book_list_rule, key);

    tracing::info!(
        "搜索 [{}] key={} → {} 条",
        source.book_source_name,
        key,
        books.len()
    );
    Ok(books)
}

/// 发现页解析（无 key）
pub(crate) fn analyze_book_list_for_explore(
    body: &str,
    base_url: &str,
    source: &BookSource,
    rule: &SearchRule,
    book_list_rule: &str,
) -> Vec<SearchBook> {
    analyze_book_list_impl(body, base_url, source, rule, book_list_rule, "")
}

/// 解析书单（对齐 legacy BookList.analyzeBookList v1：无 JS/无变量）
fn analyze_book_list(
    body: &str,
    base_url: &str,
    source: &BookSource,
    rule: &SearchRule,
    book_list_rule: &str,
    _key: &str,
) -> Vec<SearchBook> {
    analyze_book_list_impl(body, base_url, source, rule, book_list_rule, _key)
}

fn analyze_book_list_impl(
    body: &str,
    base_url: &str,
    source: &BookSource,
    rule: &SearchRule,
    book_list_rule: &str,
    _key: &str,
) -> Vec<SearchBook> {
    // bookList 规则类型检测
    let parsed = parse_rule(book_list_rule);
    let mut items: Vec<String> = match parsed.kind {
        RuleKind::Css => css_items(book_list_rule, body),
        RuleKind::JsonPath => apply(book_list_rule, body),
        RuleKind::Regex => apply(book_list_rule, body),
        RuleKind::Js => js_book_list(book_list_rule, body, base_url),
        _ => vec![],
    };
    // JS 规则（<js> 或 @js: 开头——eval 返回 JSON 书单数组）
    if items.is_empty() && (book_list_rule.contains("<js>") || book_list_rule.trim_start().starts_with("@js:")) {
        items = js_book_list(book_list_rule, body, base_url);
    }

    items
        .into_iter()
        .enumerate()
        .filter_map(|(idx, item_html)| {
            let mut book = SearchBook {
                origin: source.book_source_url.clone(),
                origin_name: source.book_source_name.clone(),
                origin_order: source.custom_order,
                time: chrono::Utc::now().timestamp_millis(),
                ..Default::default()
            };
            // 字段规则（在每本书元素上下文中应用）
            book.name = field(&item_html, rule.name.as_deref(), &book.name);
            if book.name.is_empty() {
                return None;
            }
            book.author = field(&item_html, rule.author.as_deref(), "");
            book.kind = opt_field(&item_html, rule.kind.as_deref());
            book.intro = opt_field(&item_html, rule.intro.as_deref());
            book.cover_url = rule
                .cover_url
                .as_deref()
                .map(|r| field_url(&item_html, Some(r), "", base_url))
                .filter(|v| !v.is_empty());
            book.word_count = opt_field(&item_html, rule.word_count.as_deref());
            book.latest_chapter_title = opt_field(&item_html, rule.last_chapter.as_deref());
            let book_url = field_url(&item_html, rule.book_url.as_deref(), "", base_url);
            if book_url.is_empty() {
                return None;
            }
            book.book_url = book_url;
            // 详情页 URL 规则（bookUrlPattern 正则应匹配——v1 记录即可）
            if book.name.is_empty() {
                return None;
            }
            // 搜索阶段 tocUrl 留空（进入详情时获取）
            let _ = idx;
            book.toc_url = String::new();
            Some(book)
        })
        .collect()
}

/// CSS 书单：链式 CSS（legado）→ 元素 html 列表
/// JS 书单规则（legado `<js>代码</js>` 或 `@js:代码`——eval 返回 JSON 数组，每项为书对象）
fn js_book_list(rule: &str, body: &str, base_url: &str) -> Vec<String> {
    // 提取 JS 代码
    let code = if rule.trim_start().starts_with("@js:") {
        rule.trim_start()[4..].to_string()
    } else if let Some(start) = rule.find("<js>") {
        let rest = &rule[start + 4..];
        let end = rest.find("</js>").unwrap_or(rest.len());
        rest[..end].to_string()
    } else {
        return vec![];
    };
    // 执行（注入 result=响应体、key/page）
    let mut vars = std::collections::HashMap::new();
    vars.insert("result".to_string(), body.to_string());
    vars.insert("key".to_string(), String::new());
    vars.insert("page".to_string(), "1".to_string());
    vars.insert("baseUrl".to_string(), base_url.to_string());
    let Ok(result) = crate::parser::js::eval_js(&code, &vars) else {
        return vec![];
    };
    // 解析 JSON 数组（每项书对象 → 上下文 JSON 字符串）
    let trimmed = result.trim();
    let arr: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => {
            // 可能不是纯 JSON——尝试找数组片段
            return vec![];
        }
    };
    match arr {
        serde_json::Value::Array(list) => list
            .iter()
            .filter(|item| item.is_object())
            .map(|item| item.to_string())
            .collect(),
        _ => vec![],
    }
}

fn css_items(rule: &str, body: &str) -> Vec<String> {
    crate::parser::css_chain::css_chain(rule, body)
}

/// URL 型字段规则（legado isUrl 语义）：展开内嵌后若是路径/URL 直接拼接，否则走规则解析
fn field_url(context: &str, rule: Option<&str>, default: &str, base: &str) -> String {
    let Some(rule) = rule else { return default.to_string() };
    let expanded = expand_embedded(rule, context);
    // URL 型：路径或完整 URL → 直接返回（相对转绝对）；// 开头是 XPath 不在此列
    if expanded.starts_with('/') && !expanded.starts_with("//") {
        return to_absolute(&expanded, base);
    }
    if expanded.starts_with("http://") || expanded.starts_with("https://") {
        return expanded;
    }
    // 规则解析（CSS/JSONPath/Regex 等）；结果为相对路径时转绝对
    let v = field(context, Some(&expanded), default);
    if v.starts_with('/') && !v.starts_with("//") {
        to_absolute(&v, base)
    } else {
        v
    }
}

/// 展开 {{$.xxx}} 内嵌规则（legado：{{}} 内为 JSONPath/JS，v1 支持 JSONPath）
pub(crate) fn expand_embedded(rule: &str, context: &str) -> String {
    if !rule.contains("{{") {
        return rule.to_string();
    }
    let mut result = rule.to_string();
    loop {
        let Some(start) = result.find("{{") else { break };
        let Some(end_rel) = result[start + 2..].find("}}") else { break };
        let end = start + 2 + end_rel;
        let inner = &result[start + 2..end];
        let mut replacement = String::new();
        if inner.starts_with("$.") || inner.starts_with("$[") || inner.starts_with('{') {
            // JSONPath 内嵌：从上下文（可能是 JSON 对象文本）提取
            let values = apply(inner, context);
            if let Some(v) = values.first() {
                replacement = v.clone();
            }
        }
        result.replace_range(start..=end + 1, &replacement);
    }
    result
}

/// 字段规则应用（上下文为单本书元素 html）
pub(crate) fn field(context: &str, rule: Option<&str>, default: &str) -> String {
    let Some(rule) = rule else { return default.to_string() };
    // legado 内嵌规则：{{$.xxx}} 从上下文提取替换（v1 支持 JSONPath 内嵌）
    let rule = expand_embedded(rule, context);
    // @js: 后缀链（legado）：`提取规则@js:code` → 先提取，结果注入 result 执行 JS
    // （如猫眼章节 URL：$.path@js:java.aesBase64DecodeToString(...)）
    if let Some((main_part, js_code)) = rule.split_once("@js:") {
        let main_part = main_part.trim();
        if !main_part.is_empty() {
            let extracted = if main_part.starts_with("$.") || main_part.starts_with('{') {
                crate::parser::rule::apply(main_part, context)
            } else if main_part.starts_with("//") {
                crate::parser::xpath::xpath_select(main_part, context)
            } else {
                crate::parser::css_chain::css_chain(main_part, context)
            };
            let first = extracted.into_iter().next().unwrap_or_default();
            let mut vars = std::collections::HashMap::new();
            vars.insert("result".to_string(), first);
            if let Ok(s) = crate::parser::js::eval_js(js_code.trim(), &vars) {
                if !s.is_empty() {
                    return s;
                }
            }
            return default.to_string();
        }
    }
    let r = parse_rule(&rule);
    match r.kind {
        RuleKind::Css => {
            // 链式 CSS（legado：class./tag./@text/@href 等）
            let v = crate::parser::css_chain::css_chain(&r.body, context);
            if let Some(first) = v.first() {
                // 无 @ 的单选择器规则：元素 HTML → 取文本（兼容旧书源写法）
                if !r.body.contains('@') {
                    let doc = scraper::Html::parse_fragment(first);
                    let txt = doc.root_element().text().collect::<String>().trim().to_string();
                    if !txt.is_empty() {
                        return txt;
                    }
                }
                return first.clone();
            }
            default.to_string()
        }
        RuleKind::JsonPath => {
            let v = apply(&rule, context);
            v.into_iter().next().unwrap_or_else(|| default.to_string())
        }
        RuleKind::Regex => {
            let v = apply(&rule, context);
            v.into_iter().next().unwrap_or_else(|| default.to_string())
        }
        _ => default.to_string(),
    }
}

pub(crate) fn opt_field(context: &str, rule: Option<&str>) -> Option<String> {
    let v = field(context, rule, "");
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_url_double_brace() {
        let u = build_search_url("/novel/search?q={{key}}&p={{page}}", "诡秘", 2, "https://a.com");
        assert_eq!(u, "https://a.com/novel/search?q=诡秘&p=2");
    }

    #[test]
    fn test_build_url_single_brace() {
        let u = build_search_url("https://a.com/s?k={key}", "测试", 1, "https://a.com");
        assert_eq!(u, "https://a.com/s?k=测试");
    }

    #[test]
    fn test_build_url_page_picker() {
        let u = build_search_url("https://a.com/<1,2,3>", "x", 2, "https://a.com");
        assert_eq!(u, "https://a.com/2");
    }

    #[test]
    fn test_absolute() {
        assert_eq!(to_absolute("/b/1", "https://a.com"), "https://a.com/b/1");
        assert_eq!(to_absolute("https://x.com/b", "https://a.com"), "https://x.com/b");
    }

    #[test]
    fn test_analyze_real_json_list() {
        // 真实猫眼 JSON（15 条）+ 真实规则
        let body = match std::fs::read_to_string("target/cat-eye.json") {
            Ok(b) => b,
            Err(_) => return, // 无测试数据时跳过
        };
        let rule: SearchRule = serde_json::from_value(serde_json::json!({
            "bookList": "$.data[*]", "name": "$.novelName", "author": "$.authorName",
            "intro": "$.summary", "bookUrl": "/novel/{{$.novelId}}?isSearch=1",
            "coverUrl": "$.cover", "wordCount": "$.wordNum"
        })).unwrap();
        // 中间环节：bookList 提取
        let items = crate::parser::rule::apply("$.data[*]", &body);
        println!("bookList items: {}", items.len());
        if let Some(first) = items.first() {
            println!("首项前 100: {}", first.chars().take(100).collect::<String>());
        }
        // 直接测字段规则
        let name = field(&items[0], Some("$.novelName"), "");
        println!("field('$.novelName') = {:?}", name);
        let book_url = field(&items[0], Some("/novel/{{$.novelId}}?isSearch=1"), "");
        println!("field(bookUrl 内嵌) = {:?}", book_url);
        let src = BookSource { book_source_url: "http://api.jmlldsc.com".into(), ..Default::default() };
        let books = analyze_book_list(&body, "http://api.jmlldsc.com", &src, &rule, "$.data[*]", "诡秘之主");
        println!("真实 JSON 解析: {} 本", books.len());
        assert!(!books.is_empty(), "真实数据解析为空");
        assert_eq!(books[0].name, "诡秘之主");
        assert!(books[0].book_url.contains("bY7oM0"), "bookUrl 内嵌规则: {}", books[0].book_url);
    }

    #[test]
    fn test_analyze_html_list() {
        let html = r#"<div class="book"><h2>书名A</h2><p>作者甲</p><a href="/book/1">详情</a></div>
                       <div class="book"><h2>书名B</h2><p>作者乙</p><a href="/book/2">详情</a></div>"#;
        let rule = SearchRule {
            book_list: Some("div.book".into()),
            name: Some("h2".into()),
            author: Some("p".into()),
            book_url: Some("a@href".into()),
            ..Default::default()
        };
        let src = BookSource { book_source_url: "https://a.com".into(), ..Default::default() };
        let books = analyze_book_list(html, "https://a.com", &src, &rule, "div.book", "key");
        assert_eq!(books.len(), 2);
        assert_eq!(books[0].name, "书名A");
        assert_eq!(books[0].author, "作者甲");
        assert_eq!(books[0].book_url, "https://a.com/book/1");
    }

    #[test]
    fn test_js_search_url_prefix() {
        // @js: 前缀：JS 返回值作为搜索 URL（注入 key/page/baseUrl）
        let headers = HashMap::new();
        let (url, suffix) = build_request_url(
            r#"@js:baseUrl + "/s?q=" + key + "&p=" + page"#,
            "测试书",
            2,
            "https://a.com",
            &headers,
        )
        .unwrap();
        assert_eq!(url, "https://a.com/s?q=测试书&p=2");
        assert!(suffix.js.is_none() && suffix.body_js.is_none());
    }

    #[test]
    fn test_js_search_url_header_map_json() {
        // headerMap 以 JSON 字符串注入，JS 可读取
        let headers = HashMap::from([("User-Agent".to_string(), "UA1".to_string())]);
        let (url, _) = build_request_url(
            r#"@js:baseUrl + "/s?h=" + (JSON.parse(headerMap)["User-Agent"] ? "yes" : "no")"#,
            "k",
            1,
            "https://a.com",
            &headers,
        )
        .unwrap();
        assert_eq!(url, "https://a.com/s?h=yes");
    }

    #[test]
    fn test_url_suffix_js_modifies_url() {
        // `,{"js":...}` 后缀：JS 修改 URL（注入 key/page/result 为空字符串/baseUrl）
        let headers = HashMap::new();
        let (url, suffix) = build_request_url(
            r#"https://a.com/search?q={{key}},{"js":"baseUrl + '/mod?k=' + key + '&p=' + page"}"#,
            "测试",
            1,
            "https://a.com",
            &headers,
        )
        .unwrap();
        assert_eq!(url, "https://a.com/mod?k=测试&p=1");
        // js 键已消费：不再出现在后缀中，bodyJs 保留为空
        assert!(suffix.js.is_none());
    }

    #[test]
    fn test_url_suffix_body_js_rewrites_body() {
        // bodyJs：对响应体执行 JS 后作为新响应体（注入 result=原响应体）
        let suffix: UrlSuffix = serde_json::from_str(r#"{"bodyJs":"result.replace('A','B')"}"#).unwrap();
        let headers = HashMap::new();
        let body = apply_body_js("AAA", &suffix, "k", 1, "https://a.com", &headers).unwrap();
        assert_eq!(body, "BAA");
    }

    #[test]
    fn test_url_suffix_parse_ignores_unknown_keys() {
        // 其他键（method 等）忽略；js/bodyJs 同时解析
        let (url, suffix) =
            split_url_suffix(r#"https://a.com/s,{"js":"baseUrl","method":"POST","bodyJs":"result + '!'"}"#);
        assert_eq!(url, "https://a.com/s");
        assert_eq!(suffix.js.as_deref(), Some("baseUrl"));
        assert_eq!(suffix.body_js.as_deref(), Some("result + '!'"));
    }

    #[test]
    fn test_url_without_suffix_unchanged() {
        let headers = HashMap::new();
        let (url, suffix) =
            build_request_url("https://a.com/s?q={{key}}", "k", 1, "https://a.com", &headers)
                .unwrap();
        assert_eq!(url, "https://a.com/s?q=k");
        assert!(suffix.js.is_none() && suffix.body_js.is_none());
    }

    #[test]
    fn test_concurrent_rate_sleep_ms() {
        assert_eq!(concurrent_rate_sleep_ms(Some("1000")), 1000);
        assert_eq!(concurrent_rate_sleep_ms(Some("20/60000")), 3000);
        assert_eq!(concurrent_rate_sleep_ms(Some(" 500 ")), 500);
        assert_eq!(concurrent_rate_sleep_ms(Some("abc")), 0);
        assert_eq!(concurrent_rate_sleep_ms(None), 0);
    }
}
