//! 搜索链路：searchUrl 构造 + 抓取 + ruleSearch 规则应用 → SearchBook
//!
//! 对齐 legacy WebBook.searchBook / BookList.analyzeBookList 语义（v1：无 JS）

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::model::BookSource;
use crate::parser::js::JsBridge;
use crate::parser::rule::{apply, parse_rule, RuleKind};
use crate::service::crawler;
use crate::storage::Storage;

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
    url = url
        .replace("{{key}}", key)
        .replace("{{page}}", &page.to_string());
    // 单花括号
    url = url
        .replace("{key}", key)
        .replace("{page}", &page.to_string());
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
pub(crate) fn build_request_url(
    search_url: &str,
    key: &str,
    page: i64,
    base_url: &str,
    headers: &HashMap<String, String>,
    bridge: &JsBridge,
) -> Result<(String, UrlSuffix)> {
    // 1) @js:/js: 前缀
    let raw = search_url.trim_start();
    let url = match raw.strip_prefix("@js:").or_else(|| raw.strip_prefix("js:")) {
        Some(code) => {
            let vars = js_vars(key, page, base_url, headers, "");
            crate::parser::js::eval_js_with_bridge(code.trim(), &vars, bridge)?
        }
        None => search_url.to_string(),
    };
    // 2) `,{...}` 后缀
    let (url_part, mut suffix) = split_url_suffix(&url);
    let url = match suffix.js.take() {
        Some(js) => {
            let vars = js_vars(key, page, base_url, headers, "");
            crate::parser::js::eval_js_with_bridge(&js, &vars, bridge)?
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
    bridge: &JsBridge,
) -> Result<String> {
    let Some(js) = &suffix.body_js else {
        return Ok(body.to_string());
    };
    let vars = js_vars(key, page, base_url, headers, body);
    crate::parser::js::eval_js_with_bridge(js, &vars, bridge)
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

/// 执行单个书源搜索；搜索成功（命中 ≥1 条）时记录书源使用统计（use_count+1）
pub async fn search_one_source(
    storage: &Storage,
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

    // 书源 JS 桥接（带用户命名空间：java.ajax / java.startBrowserAwait 自动携带
    // 书源 cookie；同流程内多次 eval 共享 java.put/get / setContent 文档）
    let bridge =
        JsBridge::new(&source.book_source_url, &source.book_source_name).with_namespace(ns);

    // 1) @js:/js: 前缀 + 2) `,{...}` 后缀（js 修改 URL）→ 最终请求 URL
    let (url, suffix) = build_request_url(
        &search_url,
        key,
        page,
        &source.book_source_url,
        &headers,
        &bridge,
    )?;

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
    // 桥接同步请求头（JS 内 java.headerMap 可读/改写；eval 后 headers() 取回）
    bridge.set_headers(req_headers.clone());
    // POST body 模板替换（{{key}}/{{page}}）
    let post_body = suffix.body.as_ref().map(|b| {
        b.replace("{{key}}", key)
            .replace("{{page}}", &page.to_string())
            .replace("{key}", key)
            .replace("{page}", &page.to_string())
    });
    // 书源抓取（自动带书源 cookie——按用户命名空间）
    let method = suffix.method.as_deref().unwrap_or("GET");
    tracing::debug!(
        "搜索请求 [{}] {} {} body={}",
        source.book_source_name,
        method,
        url,
        post_body.as_deref().unwrap_or("")
    );
    let resp = if method.eq_ignore_ascii_case("POST") {
        crawler::http_post(
            ns,
            &url,
            &req_headers,
            15,
            post_body.as_deref(),
            suffix.charset.as_deref(),
        )
        .await?
    } else {
        crawler::http_get(ns, &url, &req_headers, 15).await?
    };
    let base = resp.url.clone();
    // bodyJs：对响应体执行 JS 后作为新响应体
    let body = apply_body_js(
        &resp.body,
        &suffix,
        key,
        page,
        &source.book_source_url,
        &req_headers,
        &bridge,
    )?;
    let books = analyze_book_list(&body, &base, source, &rule, &book_list_rule, key, &bridge);

    // 书源使用统计：搜索命中（结果非空）→ use_count+1 / use_ts 刷新；
    // 计数失败仅记 debug 日志，不影响搜索流程（搜索/换源共用此入口）
    if !books.is_empty() {
        if let Err(e) = storage
            .bump_book_source_use(ns, &source.book_source_url)
            .await
        {
            tracing::debug!("书源使用计数失败 [{}]: {e}", source.book_source_name);
        }
    }

    tracing::info!(
        "搜索 [{}] key={} → {} 条",
        source.book_source_name,
        key,
        books.len()
    );
    Ok(books)
}

/// 发现页解析（无 key；无书源桥接——发现流程暂不注入 cookie 上下文）
pub(crate) fn analyze_book_list_for_explore(
    body: &str,
    base_url: &str,
    source: &BookSource,
    rule: &SearchRule,
    book_list_rule: &str,
) -> Vec<SearchBook> {
    analyze_book_list_impl(body, base_url, source, rule, book_list_rule, "", None)
}

/// 解析书单（对齐 legacy BookList.analyzeBookList v1：无 JS/无变量）
fn analyze_book_list(
    body: &str,
    base_url: &str,
    source: &BookSource,
    rule: &SearchRule,
    book_list_rule: &str,
    _key: &str,
    bridge: &JsBridge,
) -> Vec<SearchBook> {
    analyze_book_list_impl(
        body,
        base_url,
        source,
        rule,
        book_list_rule,
        _key,
        Some(bridge),
    )
}

fn analyze_book_list_impl(
    body: &str,
    base_url: &str,
    source: &BookSource,
    rule: &SearchRule,
    book_list_rule: &str,
    _key: &str,
    bridge: Option<&JsBridge>,
) -> Vec<SearchBook> {
    // bookList 规则类型检测
    let parsed = parse_rule(book_list_rule);
    let mut items: Vec<String> = match parsed.kind {
        RuleKind::Css => css_items(book_list_rule, body),
        RuleKind::JsonPath => apply(book_list_rule, body),
        RuleKind::Regex => apply(book_list_rule, body),
        RuleKind::Js => js_book_list(book_list_rule, body, base_url, bridge),
        _ => vec![],
    };
    // JS 规则（<js> 或 @js: 开头——eval 返回 JSON 书单数组）
    if items.is_empty()
        && (book_list_rule.contains("<js>") || book_list_rule.trim_start().starts_with("@js:"))
    {
        items = js_book_list(book_list_rule, body, base_url, bridge);
    }

    items
        .into_iter()
        .enumerate()
        .filter_map(|(idx, item_html)| {
            let mut book = SearchBook {
                origin: source.book_source_url.clone(),
                origin_name: source.book_source_name.clone(),
                origin_order: source.custom_order,
                // 非文本书源（音频/漫画等）：bookSourceType 透传到搜索结果的 type——
                // 入架后 books.book_type 即带类型，阅读器按此分派渲染
                book_type: source.book_source_type.clamp(0, 4),
                time: chrono::Utc::now().timestamp_millis(),
                ..Default::default()
            };
            // 字段规则（在每本书元素上下文中应用）
            book.name = field_with_bridge(&item_html, rule.name.as_deref(), &book.name, bridge);
            if book.name.is_empty() {
                return None;
            }
            book.author = field_with_bridge(&item_html, rule.author.as_deref(), "", bridge);
            book.kind = opt_field_with_bridge(&item_html, rule.kind.as_deref(), bridge);
            book.intro = opt_field_with_bridge(&item_html, rule.intro.as_deref(), bridge);
            book.cover_url = rule
                .cover_url
                .as_deref()
                .map(|r| field_url(&item_html, Some(r), "", base_url))
                .filter(|v| !v.is_empty());
            book.word_count = opt_field_with_bridge(&item_html, rule.word_count.as_deref(), bridge);
            book.latest_chapter_title =
                opt_field_with_bridge(&item_html, rule.last_chapter.as_deref(), bridge);
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

/// 精确匹配文本归一化：去首尾空白 + 全角 ASCII → 半角（U+FF01..U+FF5E → U+21..U+7E，
/// 全角空格 U+3000 → 半角空格）+ 小写。用于「精确」搜索：大小写/全半角差异不敏感。
pub(crate) fn normalize_search_text(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| match c {
            '\u{3000}' => ' ',
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            _ => c,
        })
        .collect::<String>()
        .to_lowercase()
}

/// 单本是否精确命中：书名或作者与 key 严格等值（归一化后；作者为空不参与匹配）
pub(crate) fn exact_match(book: &SearchBook, key: &str) -> bool {
    let q = normalize_search_text(key);
    if q.is_empty() {
        return true;
    }
    let name = normalize_search_text(&book.name);
    let author = normalize_search_text(&book.author);
    name == q || (!author.is_empty() && author == q)
}

/// 精确搜索过滤：书源规则解析后，仅保留书名/作者等值命中的结果（模糊模式不做此过滤）
pub(crate) fn filter_exact(books: Vec<SearchBook>, key: &str) -> Vec<SearchBook> {
    books.into_iter().filter(|b| exact_match(b, key)).collect()
}

/// CSS 书单：链式 CSS（legado）→ 元素 html 列表
/// JS 书单规则（legado `<js>代码</js>` 或 `@js:代码`——eval 返回 JSON 数组，每项为书对象）
fn js_book_list(rule: &str, body: &str, base_url: &str, bridge: Option<&JsBridge>) -> Vec<String> {
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
    // 执行（注入 result=响应体、key/page）并直接取结构化结果：
    // 数组/对象经递归 JSON 转换（js_value_to_json）——避免 boa ToString 对数组
    // 元素对象输出 "[object Object]" 导致 JSON.parse 解析为空（bookList 修复核心）
    let mut vars = std::collections::HashMap::new();
    vars.insert("result".to_string(), body.to_string());
    vars.insert("key".to_string(), String::new());
    vars.insert("page".to_string(), "1".to_string());
    vars.insert("baseUrl".to_string(), base_url.to_string());
    // 带书源桥接执行（搜索流程：java.ajax/startBrowserAwait/setContent 等可用）
    let result = match bridge {
        Some(b) => crate::parser::js::eval_js_json_with_bridge(&code, &vars, b),
        None => crate::parser::js::eval_js_json(&code, &vars),
    };
    let Ok(result) = result else {
        return vec![];
    };
    // 数组：每项书对象/字符串 → 上下文（JSON 文本）；单对象兼容
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

fn css_items(rule: &str, body: &str) -> Vec<String> {
    crate::parser::css_chain::css_chain(rule, body)
}

/// URL 型字段规则（legado isUrl 语义）：展开内嵌后若是路径/URL 直接拼接，否则走规则解析
fn field_url(context: &str, rule: Option<&str>, default: &str, base: &str) -> String {
    let Some(rule) = rule else {
        return default.to_string();
    };
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
        let Some(start) = result.find("{{") else {
            break;
        };
        let Some(end_rel) = result[start + 2..].find("}}") else {
            break;
        };
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

/// 字段规则应用（上下文为单本书元素 html；无书源桥接——每次 eval 独立空 bridge）
pub(crate) fn field(context: &str, rule: Option<&str>, default: &str) -> String {
    field_with_bridge(context, rule, default, None)
}

/// 字段规则应用（带书源桥接：搜索流程共享 ns bridge，java.* 可用）
pub(crate) fn field_with_bridge(
    context: &str,
    rule: Option<&str>,
    default: &str,
    bridge: Option<&JsBridge>,
) -> String {
    let Some(rule) = rule else {
        return default.to_string();
    };
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
            let s = match bridge {
                Some(b) => crate::parser::js::eval_js_with_bridge(js_code.trim(), &vars, b),
                None => crate::parser::js::eval_js(js_code.trim(), &vars),
            };
            if let Ok(s) = s {
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
            // 链式 CSS（legado：class./tag./@text/@href 等）；经 apply 执行以保留
            // ##替换链 / <js> 链（apply_post：替换正则/替换串/### 首个匹配）
            let v = crate::parser::rule::apply(&rule, context);
            if let Some(first) = v.first() {
                // 无 @ 的单选择器规则：元素 HTML → 取文本（兼容旧书源写法）
                if !r.body.contains('@') {
                    let doc = scraper::Html::parse_fragment(first);
                    let txt = doc
                        .root_element()
                        .text()
                        .collect::<String>()
                        .trim()
                        .to_string();
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
        RuleKind::Js => {
            // 纯 JS 字段规则（@js:/js: 前缀——parse_rule 已剥前缀，body 即代码）：
            // 注入 result=上下文执行；数组/对象结果自动 JSON 化（js_result_to_string）
            let mut vars = std::collections::HashMap::new();
            vars.insert("result".to_string(), context.to_string());
            vars.insert("key".to_string(), String::new());
            vars.insert("page".to_string(), "1".to_string());
            let s = match bridge {
                Some(b) => crate::parser::js::eval_js_with_bridge(&r.body, &vars, b),
                None => crate::parser::js::eval_js(&r.body, &vars),
            }
            .unwrap_or_default();
            if s.is_empty() {
                default.to_string()
            } else {
                s
            }
        }
        _ => default.to_string(),
    }
}

pub(crate) fn opt_field(context: &str, rule: Option<&str>) -> Option<String> {
    opt_field_with_bridge(context, rule, None)
}

pub(crate) fn opt_field_with_bridge(
    context: &str,
    rule: Option<&str>,
    bridge: Option<&JsBridge>,
) -> Option<String> {
    let v = field_with_bridge(context, rule, "", bridge);
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
        let u = build_search_url(
            "/novel/search?q={{key}}&p={{page}}",
            "诡秘",
            2,
            "https://a.com",
        );
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
        assert_eq!(
            to_absolute("https://x.com/b", "https://a.com"),
            "https://x.com/b"
        );
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
        }))
        .unwrap();
        // 中间环节：bookList 提取
        let items = crate::parser::rule::apply("$.data[*]", &body);
        println!("bookList items: {}", items.len());
        if let Some(first) = items.first() {
            println!(
                "首项前 100: {}",
                first.chars().take(100).collect::<String>()
            );
        }
        // 直接测字段规则
        let name = field(&items[0], Some("$.novelName"), "");
        println!("field('$.novelName') = {:?}", name);
        let book_url = field(&items[0], Some("/novel/{{$.novelId}}?isSearch=1"), "");
        println!("field(bookUrl 内嵌) = {:?}", book_url);
        let src = BookSource {
            book_source_url: "http://api.jmlldsc.com".into(),
            ..Default::default()
        };
        let books = analyze_book_list(
            &body,
            "http://api.jmlldsc.com",
            &src,
            &rule,
            "$.data[*]",
            "诡秘之主",
            &JsBridge::default(),
        );
        println!("真实 JSON 解析: {} 本", books.len());
        assert!(!books.is_empty(), "真实数据解析为空");
        assert_eq!(books[0].name, "诡秘之主");
        assert!(
            books[0].book_url.contains("bY7oM0"),
            "bookUrl 内嵌规则: {}",
            books[0].book_url
        );
    }

    #[test]
    fn test_normalize_search_text() {
        // 全角 ASCII → 半角 + 小写 + 去首尾空白
        assert_eq!(normalize_search_text(" ＡＢＣ１２３ "), "abc123");
        assert_eq!(normalize_search_text("ＦｕｌｌＷｉｄｔｈ"), "fullwidth");
        assert_eq!(normalize_search_text("AbC"), "abc");
        // 全角空格 U+3000 → 半角空格；首尾空白（含全角）被 trim 去除
        assert_eq!(normalize_search_text("　全角　空格　"), "全角 空格");
        // 中文与混合内容原样保留（小写化对中文无影响）
        assert_eq!(normalize_search_text("诡秘之主"), "诡秘之主");
    }

    #[test]
    fn test_filter_exact() {
        let mk = |name: &str, author: &str| SearchBook {
            name: name.into(),
            author: author.into(),
            ..Default::default()
        };
        let books = vec![
            mk("诡秘之主", "爱潜水的乌贼"),
            mk("诡秘之主2", "爱潜水的乌贼"),
            mk("ＧＭＴ", "测试作者"),
            mk("凡人修仙传", "忘语"),
            mk("Book ABC", ""),
        ];
        // 书名等值命中（大小写不敏感）
        let hit = filter_exact(books.clone(), "诡秘之主");
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].name, "诡秘之主");
        // 包含但不等值（模糊命中）→ 精确下不命中
        let hit = filter_exact(books.clone(), "诡秘");
        assert!(hit.is_empty());
        // 全角书名 + 半角 key：全半角忽略等值命中
        let hit = filter_exact(books.clone(), "gmt");
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].name, "ＧＭＴ");
        // 半角书名 + 全角 key
        let hit = filter_exact(books.clone(), "ＢＯＯＫ ａｂｃ");
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].name, "Book ABC");
        // 作者等值命中
        let hit = filter_exact(books.clone(), "忘语");
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].name, "凡人修仙传");
        // 作者为空：不因空作者误命中；空 key 不过滤
        let hit = filter_exact(books.clone(), "");
        assert_eq!(hit.len(), books.len());
        // 无命中
        let hit = filter_exact(books, "不存在的书");
        assert!(hit.is_empty());
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
        let src = BookSource {
            book_source_url: "https://a.com".into(),
            ..Default::default()
        };
        let books = analyze_book_list(
            html,
            "https://a.com",
            &src,
            &rule,
            "div.book",
            "key",
            &JsBridge::default(),
        );
        assert_eq!(books.len(), 2);
        assert_eq!(books[0].name, "书名A");
        assert_eq!(books[0].author, "作者甲");
        assert_eq!(books[0].book_url, "https://a.com/book/1");
    }

    /// 字段规则 ## 替换链（真实书源：class.title@tag.h2@text##《(.*)》##$1### 形态）：
    /// 替换须在 field 层生效（此前 Css 分支绕过 apply_post 丢失替换）
    #[test]
    fn test_field_css_replace_chain() {
        let html = r#"<div class="book"><h2 class="t">《测试书》</h2><p>作者甲</p></div>"#;
        // 三段替换（去书名号）
        let name = field(html, Some("class.t@text##《(.*)》##$1"), "");
        assert_eq!(name, "测试书");
        // 字段上下文为单本书元素：链式提取 + 替换
        let el = crate::parser::css_chain::css_chain("div.book", html);
        let name2 = field(&el[0], Some("class.t@text##《(.*)》##$1"), "");
        assert_eq!(name2, "测试书");
        // @js: 链字段（提取结果进 JS result 变量）
        let name3 = field(
            &el[0],
            Some("class.t@text@js:result.replace('《','【').replace('》','】')"),
            "",
        );
        assert_eq!(name3, "【测试书】");
        // 纯替换规则（### replaceFirst）——上下文仅含书名
        let h2 = crate::parser::css_chain::css_chain("class.t", html);
        let name4 = field(&h2[0], Some("##《(.*)》##[$1]###"), "");
        assert_eq!(name4, "[测试书]");
    }

    /// bookList 修复：JS 返回 JSON.parse(result).data 数组 → 逐本书解析（此前 ToString
    /// 输出 "[object Object]" 导致解析为空）
    #[test]
    fn test_js_book_list_json_parse_array() {
        let body = r#"{"code":0,"data":[
            {"novelName":"书A","authorName":"作者甲","novelId":"id1"},
            {"novelName":"书B","authorName":"作者乙","novelId":"id2"}
        ]}"#;
        let rule = SearchRule {
            book_list: Some("@js:JSON.parse(result).data".into()),
            name: Some("$.novelName".into()),
            author: Some("$.authorName".into()),
            book_url: Some("/novel/{{$.novelId}}".into()),
            ..Default::default()
        };
        let src = BookSource {
            book_source_url: "https://api.test".into(),
            ..Default::default()
        };
        let books = analyze_book_list(
            body,
            "https://api.test",
            &src,
            &rule,
            "@js:JSON.parse(result).data",
            "key",
            &JsBridge::default(),
        );
        assert_eq!(books.len(), 2, "JS bookList 应解析出 2 本书");
        assert_eq!(books[0].name, "书A");
        assert_eq!(books[0].author, "作者甲");
        assert_eq!(books[0].book_url, "https://api.test/novel/id1");
        assert_eq!(books[1].name, "书B");
    }

    /// bookList 修复：JS 直接返回数组字面量（非字符串出口）
    #[test]
    fn test_js_book_list_array_literal() {
        let rule = SearchRule {
            book_list: Some("@js:[{name:'直A',url:'/a'},{name:'直B',url:'/b'}]".into()),
            name: Some("$.name".into()),
            book_url: Some("$.url".into()),
            ..Default::default()
        };
        let src = BookSource {
            book_source_url: "https://api.test".into(),
            ..Default::default()
        };
        let books = analyze_book_list(
            "{}",
            "https://api.test",
            &src,
            &rule,
            "@js:[{name:'直A',url:'/a'},{name:'直B',url:'/b'}]",
            "",
            &JsBridge::default(),
        );
        assert_eq!(books.len(), 2);
        assert_eq!(books[0].name, "直A");
        assert_eq!(books[0].book_url, "https://api.test/a");
    }

    /// bookList 兼容：JS 内 JSON.stringify 返回字符串数组——自动解析
    #[test]
    fn test_js_book_list_stringify_backward_compat() {
        let rule = SearchRule {
            book_list: Some("@js:JSON.stringify([{name:'串A',url:'/s/a'}])".into()),
            name: Some("$.name".into()),
            book_url: Some("$.url".into()),
            ..Default::default()
        };
        let src = BookSource {
            book_source_url: "https://api.test".into(),
            ..Default::default()
        };
        let books = analyze_book_list(
            "{}",
            "https://api.test",
            &src,
            &rule,
            "@js:JSON.stringify([{name:'串A',url:'/s/a'}])",
            "",
            &JsBridge::default(),
        );
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].name, "串A");
    }

    /// bookList：<js> 包裹形式
    #[test]
    fn test_js_book_list_html_wrapped() {
        let rule = SearchRule {
            book_list: Some("<js>JSON.parse(result).data</js>".into()),
            name: Some("$.name".into()),
            book_url: Some("$.url".into()),
            ..Default::default()
        };
        let body = r#"{"data":[{"name":"包A","url":"/w/1"}]}"#;
        let src = BookSource {
            book_source_url: "https://api.test".into(),
            ..Default::default()
        };
        let books = analyze_book_list(
            body,
            "https://api.test",
            &src,
            &rule,
            "<js>JSON.parse(result).data</js>",
            "",
            &JsBridge::default(),
        );
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].name, "包A");
    }

    /// 字段规则：@js: 前缀（analyze 系列字段出口——result 注入上下文执行）
    #[test]
    fn test_field_js_rule() {
        let ctx = r#"{"data":{"name":"字段书","author":"字段作者"}}"#;
        assert_eq!(
            field(ctx, Some("@js:JSON.parse(result).data.name"), ""),
            "字段书"
        );
        assert_eq!(
            field(ctx, Some("@js:JSON.parse(result).data.author"), ""),
            "字段作者"
        );
        // 失败回退默认值
        assert_eq!(
            field(ctx, Some("@js:JSON.parse(result).data.missing.x"), "默认"),
            "默认"
        );
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
            &JsBridge::default(),
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
            &JsBridge::default(),
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
            &JsBridge::default(),
        )
        .unwrap();
        assert_eq!(url, "https://a.com/mod?k=测试&p=1");
        // js 键已消费：不再出现在后缀中，bodyJs 保留为空
        assert!(suffix.js.is_none());
    }

    #[test]
    fn test_url_suffix_body_js_rewrites_body() {
        // bodyJs：对响应体执行 JS 后作为新响应体（注入 result=原响应体）
        let suffix: UrlSuffix =
            serde_json::from_str(r#"{"bodyJs":"result.replace('A','B')"}"#).unwrap();
        let headers = HashMap::new();
        let body = apply_body_js(
            "AAA",
            &suffix,
            "k",
            1,
            "https://a.com",
            &headers,
            &JsBridge::default(),
        )
        .unwrap();
        assert_eq!(body, "BAA");
    }

    #[test]
    fn test_url_suffix_parse_ignores_unknown_keys() {
        // 其他键（method 等）忽略；js/bodyJs 同时解析
        let (url, suffix) = split_url_suffix(
            r#"https://a.com/s,{"js":"baseUrl","method":"POST","bodyJs":"result + '!'"}"#,
        );
        assert_eq!(url, "https://a.com/s");
        assert_eq!(suffix.js.as_deref(), Some("baseUrl"));
        assert_eq!(suffix.body_js.as_deref(), Some("result + '!'"));
    }

    #[test]
    fn test_url_without_suffix_unchanged() {
        let headers = HashMap::new();
        let (url, suffix) = build_request_url(
            "https://a.com/s?q={{key}}",
            "k",
            1,
            "https://a.com",
            &headers,
            &JsBridge::default(),
        )
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
