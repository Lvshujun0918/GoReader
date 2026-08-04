//! 书源调试（bookSourceDebugSSE）：逐规则执行测试引擎
//!
//! 复用现有规则引擎（search/book/explore），按步骤输出：
//! 规则解析 → URL 构造 → 请求 → 规则应用，每步含规则名/请求 URL/耗时/结果长度/错误/解析明细。

use std::time::Instant;

use anyhow::Result;
use serde_json::{json, Value};

use crate::model::BookSource;
use crate::parser::rule::parse_rule;
use crate::service::book::{analyze_content_from, ContentRule, TocRule};
use crate::service::crawler;
use crate::service::search::{
    analyze_book_list_for_explore, field, split_url_suffix, to_absolute, SearchRule, UrlSuffix,
};

/// 调试步骤（SSE step 事件 message 载荷）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugStep {
    /// 规则名（如 ruleSearch.bookList / 请求 URL / 抓取 / 规则应用）
    pub rule_name: String,
    /// 请求 URL（非请求步骤为空）
    pub url: String,
    /// 耗时（毫秒）
    pub elapsed_ms: i64,
    /// 结果长度（字符数）
    pub result_len: usize,
    /// 错误信息（无则空）
    pub error: Option<String>,
    /// 解析明细（规则类型/字段等）
    pub detail: Value,
}

impl DebugStep {
    fn new(rule_name: impl Into<String>) -> Self {
        Self {
            rule_name: rule_name.into(),
            url: String::new(),
            elapsed_ms: 0,
            result_len: 0,
            error: None,
            detail: Value::Null,
        }
    }
}

/// 执行调试：逐步骤回调 on_step，返回最终结果 JSON
pub async fn run_debug(
    ns: &str,
    source: &BookSource,
    action: &str,
    key: &str,
    target_url: &str,
    mut on_step: impl FnMut(&DebugStep),
) -> Result<Value> {
    match action {
        "search" => debug_search(ns, source, key, &mut on_step).await,
        "explore" => debug_explore(ns, source, target_url, &mut on_step).await,
        "toc" => debug_toc(ns, source, target_url, &mut on_step).await,
        "content" => debug_content(ns, source, target_url, &mut on_step).await,
        _ => Err(anyhow::anyhow!("不支持的调试动作（search|explore|toc|content）")),
    }
}

/// 请求执行（带步骤输出）
async fn debug_fetch(
    ns: &str,
    url: &str,
    suffix: &UrlSuffix,
    source: &BookSource,
    key: &str,
    on_step: &mut impl FnMut(&DebugStep),
) -> Result<crawler::FetchResponse> {
    let mut headers = source
        .header
        .as_deref()
        .map(crawler::parse_header)
        .unwrap_or_default();
    if let Some(extra) = &suffix.headers {
        for (k, v) in extra {
            headers.insert(k.clone(), v.clone());
        }
    }
    let post_body = suffix.body.as_ref().map(|b| {
        b.replace("{{key}}", key)
            .replace("{{page}}", "1")
            .replace("{key}", key)
            .replace("{page}", "1")
    });
    let method = suffix.method.as_deref().unwrap_or("GET").to_string();
    let started = Instant::now();
    let result = if method.eq_ignore_ascii_case("POST") {
        crawler::http_post(ns, url, &headers, 15, post_body.as_deref(), suffix.charset.as_deref()).await
    } else {
        crawler::http_get(ns, url, &headers, 15).await
    };
    match result {
        Ok(resp) => {
            on_step(&DebugStep {
                rule_name: "请求 URL".into(),
                url: url.to_string(),
                elapsed_ms: started.elapsed().as_millis() as i64,
                result_len: resp.body.len(),
                error: None,
                detail: json!({ "method": method, "status": resp.status }),
            });
            Ok(resp)
        }
        Err(e) => {
            on_step(&DebugStep {
                rule_name: "请求 URL".into(),
                url: url.to_string(),
                elapsed_ms: started.elapsed().as_millis() as i64,
                result_len: 0,
                error: Some(e.to_string()),
                detail: json!({ "method": method }),
            });
            Err(e)
        }
    }
}

/// search：规则解析 → URL 构造 → 抓取 → 规则应用
async fn debug_search(
    ns: &str,
    source: &BookSource,
    key: &str,
    on_step: &mut impl FnMut(&DebugStep),
) -> Result<Value> {
    // ① 规则解析
    let mut step = DebugStep::new("规则解析（ruleSearch）");
    let rule: SearchRule = match &source.rule_search {
        Some(v) => serde_json::from_value(v.clone()).unwrap_or_default(),
        None => SearchRule::default(),
    };
    let book_list_rule = rule.book_list.clone().unwrap_or_default();
    let parsed = parse_rule(&book_list_rule);
    step.detail = json!({
        "bookList": book_list_rule,
        "bookListKind": format!("{:?}", parsed.kind),
        "name": rule.name, "author": rule.author, "bookUrl": rule.book_url,
        "coverUrl": rule.cover_url, "wordCount": rule.word_count,
    });
    step.result_len = book_list_rule.len();
    on_step(&step);

    let Some(search_url) = source.search_url.clone() else {
        on_step(&DebugStep {
            rule_name: "URL 构造".into(),
            url: String::new(),
            elapsed_ms: 0,
            result_len: 0,
            error: Some("书源未配置 searchUrl".into()),
            detail: Value::Null,
        });
        return Err(anyhow::anyhow!("书源未配置 searchUrl"));
    };

    // ② URL 构造
    let headers = source
        .header
        .as_deref()
        .map(crawler::parse_header)
        .unwrap_or_default();
    let started = Instant::now();
    // 书源桥接（带用户命名空间：URL 构造 JS 内 java.* 可用）
    let bridge = crate::parser::js::JsBridge::new(&source.book_source_url, &source.book_source_name)
        .with_namespace(ns);
    let (url, suffix) = match crate::service::search::build_request_url(
        &search_url,
        key,
        1,
        &source.book_source_url,
        &headers,
        &bridge,
    ) {
        Ok(v) => v,
        Err(e) => {
            on_step(&DebugStep {
                rule_name: "URL 构造".into(),
                url: search_url.clone(),
                elapsed_ms: started.elapsed().as_millis() as i64,
                result_len: 0,
                error: Some(e.to_string()),
                detail: Value::Null,
            });
            return Err(e);
        }
    };
    on_step(&DebugStep {
        rule_name: "URL 构造".into(),
        url: url.clone(),
        elapsed_ms: started.elapsed().as_millis() as i64,
        result_len: url.len(),
        error: None,
        detail: json!({
            "method": suffix.method, "js": suffix.js, "bodyJs": suffix.body_js,
            "charset": suffix.charset, "body": suffix.body,
        }),
    });

    // ③ 抓取
    let resp = debug_fetch(ns, &url, &suffix, source, key, on_step).await?;
    let base = resp.url.clone();

    // ④ 规则应用
    let mut step = DebugStep::new("规则应用（bookList 字段）");
    let started = Instant::now();
    let books = analyze_book_list_for_explore(&resp.body, &base, source, &rule, &book_list_rule);
    step.elapsed_ms = started.elapsed().as_millis() as i64;
    step.result_len = books.len();
    step.detail = json!({
        "count": books.len(),
        "first": books.first().map(|b| json!({
            "name": b.name, "author": b.author, "bookUrl": b.book_url,
        })),
    });
    on_step(&step);
    Ok(json!(books))
}

/// explore：规则解析（exploreUrl 条目）→ URL 构造 → 抓取 → 规则应用
async fn debug_explore(
    ns: &str,
    source: &BookSource,
    target_url: &str,
    on_step: &mut impl FnMut(&DebugStep),
) -> Result<Value> {
    let mut step = DebugStep::new("规则解析（ruleExplore）");
    let rule: SearchRule = match &source.rule_explore {
        Some(v) => serde_json::from_value(v.clone()).unwrap_or_default(),
        None => SearchRule::default(),
    };
    let book_list_rule = rule.book_list.clone().unwrap_or_default();
    let parsed = parse_rule(&book_list_rule);
    step.detail = json!({
        "bookList": book_list_rule,
        "bookListKind": format!("{:?}", parsed.kind),
        "exploreEntries": crate::service::explore::parse_explore_entries(source.explore_url.as_deref().unwrap_or("")).len(),
    });
    step.result_len = book_list_rule.len();
    on_step(&step);

    // 目标 URL：显式传入优先，否则取 exploreUrl 首个条目
    let raw = if !target_url.is_empty() {
        target_url.to_string()
    } else {
        crate::service::explore::parse_explore_entries(source.explore_url.as_deref().unwrap_or(""))
            .into_iter()
            .find(|e| e.r#type == "book")
            .map(|e| e.url)
            .unwrap_or_default()
    };
    if raw.is_empty() {
        return Err(anyhow::anyhow!("未配置 exploreUrl 且未传入 url"));
    }

    // URL 构造（{{page}} 占位 + 相对路径拼 base + ,{...} 后缀）
    let mut step = DebugStep::new("URL 构造");
    let started = Instant::now();
    let url = raw.replace("{{page}}", "1").replace("{page}", "1");
    let url = if url.starts_with('/') && !url.starts_with("//") {
        let base = source.book_source_url.split("##").next().unwrap_or("").trim_end_matches('/');
        format!("{base}{url}")
    } else {
        url
    };
    let (final_url, suffix) = split_url_suffix(&url);
    step.url = final_url.clone();
    step.elapsed_ms = started.elapsed().as_millis() as i64;
    step.result_len = final_url.len();
    step.detail = json!({ "method": suffix.method, "charset": suffix.charset });
    on_step(&step);

    // 抓取
    let resp = debug_fetch(ns, &final_url, &suffix, source, "", on_step).await?;
    let base = resp.url.clone();

    // 规则应用
    let mut step = DebugStep::new("规则应用（bookList 字段）");
    let started = Instant::now();
    let books = analyze_book_list_for_explore(&resp.body, &base, source, &rule, &book_list_rule);
    step.elapsed_ms = started.elapsed().as_millis() as i64;
    step.result_len = books.len();
    step.detail = json!({ "count": books.len() });
    on_step(&step);
    Ok(json!(books))
}

/// toc：规则解析 → 抓取目录页 → chapterList 提取 → 字段规则 → nextTocUrl 循环（≤5 页）
async fn debug_toc(
    ns: &str,
    source: &BookSource,
    toc_url: &str,
    on_step: &mut impl FnMut(&DebugStep),
) -> Result<Value> {
    if toc_url.is_empty() {
        return Err(anyhow::anyhow!("请输入目录链接（url 参数）"));
    }
    let mut step = DebugStep::new("规则解析（ruleToc）");
    let rule: TocRule = match &source.rule_toc {
        Some(v) => serde_json::from_value(v.clone()).unwrap_or_default(),
        None => TocRule::default(),
    };
    let list_rule = rule.chapter_list.clone().unwrap_or_default();
    let parsed = parse_rule(&list_rule);
    step.detail = json!({
        "chapterList": list_rule,
        "chapterListKind": format!("{:?}", parsed.kind),
        "chapterName": rule.chapter_name, "chapterUrl": rule.chapter_url,
        "nextTocUrl": rule.next_toc_url,
    });
    step.result_len = list_rule.len();
    on_step(&step);

    let mut all: Vec<Value> = Vec::new();
    let mut current_url = toc_url.to_string();
    for page in 0..5usize {
        // 抓取目录页
        let resp = match debug_fetch(ns, &current_url, &UrlSuffix::default(), source, "", on_step).await {
            Ok(r) => r,
            Err(e) => return Err(e),
        };
        let base = resp.url.clone();

        // chapterList 提取
        let mut step = DebugStep::new(format!("chapterList 提取（第 {} 页）", page + 1));
        let started = Instant::now();
        let items: Vec<String> = match parsed.kind {
            crate::parser::rule::RuleKind::Css => crate::parser::css_chain::css_chain(&list_rule, &resp.body),
            crate::parser::rule::RuleKind::JsonPath | crate::parser::rule::RuleKind::Regex => {
                crate::parser::rule::apply(&list_rule, &resp.body)
            }
            // JS chapterList（JSON 数组递归转换——与 analyze_toc 同源；含 <js> 包裹兜底）
            _ if list_rule.contains("<js>") || list_rule.trim_start().starts_with("@js:") => {
                crate::service::book::toc_items(&list_rule, &resp.body)
            }
            _ => vec![],
        };
        step.elapsed_ms = started.elapsed().as_millis() as i64;
        step.result_len = items.len();
        step.detail = json!({ "count": items.len() });
        on_step(&step);

        // 字段规则（前 20 条示例）
        let mut step = DebugStep::new("字段规则（chapterName/chapterUrl）");
        let started = Instant::now();
        let start_index = all.len() as i64;
        let mut page_chapters = Vec::new();
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
            page_chapters.push(json!({
                "title": title,
                "url": to_absolute(&url, &base),
                "index": start_index + i as i64,
            }));
            if page_chapters.len() >= 20 {
                break;
            }
        }
        step.elapsed_ms = started.elapsed().as_millis() as i64;
        step.result_len = page_chapters.len();
        step.detail = json!({ "sample": page_chapters.first() });
        on_step(&step);
        all.extend(page_chapters);

        // nextTocUrl
        let next = rule
            .next_toc_url
            .as_deref()
            .map(|r| field(&resp.body, Some(r), ""))
            .unwrap_or_default();
        if next.is_empty() {
            break;
        }
        current_url = to_absolute(&next, &base);
    }
    Ok(json!(all))
}

/// content：规则解析 → 抓取章节页 → content 规则应用 + sourceRegex/replaceRegex 清洗 → 多页循环
async fn debug_content(
    ns: &str,
    source: &BookSource,
    chapter_url: &str,
    on_step: &mut impl FnMut(&DebugStep),
) -> Result<Value> {
    if chapter_url.is_empty() {
        return Err(anyhow::anyhow!("请输入章节链接（chapterUrl 参数）"));
    }
    let mut step = DebugStep::new("规则解析（ruleContent）");
    let rule: ContentRule = match &source.rule_content {
        Some(v) => serde_json::from_value(v.clone()).unwrap_or_default(),
        None => ContentRule::default(),
    };
    step.detail = json!({
        "content": rule.content,
        "sourceRegex": rule.source_regex,
        "replaceRegex": rule.replace_regex,
        "nextContentUrl": rule.next_content_url,
    });
    step.result_len = rule.content.as_deref().map(|s| s.len()).unwrap_or(0);
    on_step(&step);

    let mut parts: Vec<String> = Vec::new();
    let mut current_url = chapter_url.to_string();
    for page in 0..5usize {
        let resp = match debug_fetch(ns, &current_url, &UrlSuffix::default(), source, "", on_step).await {
            Ok(r) => r,
            Err(e) => return Err(e),
        };
        let base = resp.url.clone();

        let mut step = DebugStep::new(format!("content 规则应用（第 {} 页）", page + 1));
        let started = Instant::now();
        let content = analyze_content_from(&resp.body, source);
        step.elapsed_ms = started.elapsed().as_millis() as i64;
        step.result_len = content.len();
        step.error = if content.is_empty() { Some("未提取到正文".into()) } else { None };
        step.detail = json!({ "chars": content.chars().count() });
        on_step(&step);
        if !content.is_empty() {
            parts.push(content);
        }

        let next = rule
            .next_content_url
            .as_deref()
            .map(|r| field(&resp.body, Some(r), ""))
            .unwrap_or_default();
        if next.is_empty() {
            break;
        }
        current_url = to_absolute(&next, &base);
    }
    Ok(json!({ "content": parts.join("\n") }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rule_parse_kind_detection() {
        // 规则解析结果（CSS/JSONPath）——调试步骤依赖的底层能力
        let css = parse_rule("div.book");
        assert_eq!(format!("{:?}", css.kind), "Css");
        let jp = parse_rule("$.data[*]");
        assert_eq!(format!("{:?}", jp.kind), "JsonPath");
    }

    #[test]
    fn test_debug_step_serialize() {
        let step = DebugStep {
            rule_name: "规则解析（ruleSearch）".into(),
            url: "https://a.com/s".into(),
            elapsed_ms: 12,
            result_len: 3,
            error: None,
            detail: json!({ "kind": "Css" }),
        };
        let v = serde_json::to_value(&step).unwrap();
        assert_eq!(v["ruleName"], "规则解析（ruleSearch）");
        assert_eq!(v["elapsedMs"], 12);
        assert_eq!(v["detail"]["kind"], "Css");
    }
}
