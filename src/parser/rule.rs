//! legado 规则引擎 v1：规则字符串解析 + CSS/JSONPath/Regex 执行
//!
//! 规则语法（对齐 legado analyzeRule）：
//! - 三段式：`规则体##@前缀##替换规则`（## 分隔，后两段可选）
//! - 类型检测：`{...}` JSONPath / `//` XPath / `@js:` JS（v1 暂不支持）/ 其余 CSS 或 Regex
//! - 多规则：`&&` 分隔依次执行
//! - 结果：字符串列表（legado 返回字符串列表语义）

use regex::Regex;
use scraper::{Html, Selector};

/// 解析后的规则
#[derive(Debug, Clone)]
pub struct Rule {
    /// 规则类型
    pub kind: RuleKind,
    /// 规则主体（类型检测前的原始文本）
    pub body: String,
    /// `##` 第二段（@ 前缀，可选）
    pub prefix: Option<String>,
    /// `##` 第三段（替换规则，可选）
    pub replace: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RuleKind {
    Css,
    JsonPath,
    XPath, // v2 支持（sxd-xpath）
    Js,    // v2 支持（rquickjs）
    Url,   // 直接拼接/替换
}

/// 解析规则字符串
pub fn parse_rule(rule: &str) -> Rule {
    let parts: Vec<&str> = rule.splitn(3, "##").collect();
    let main = parts[0].trim();
    let prefix = parts.get(1).map(|s| s.trim().to_string());
    let replace = parts.get(2).map(|s| s.trim().to_string());

    let kind = detect_kind(main);
    Rule {
        kind,
        body: main.to_string(),
        prefix,
        replace,
    }
}

fn detect_kind(body: &str) -> RuleKind {
    let b = body.trim();
    if b.starts_with('{') {
        RuleKind::JsonPath
    } else if b.starts_with("//") {
        RuleKind::XPath
    } else if b.starts_with("@js:") || b.starts_with("js:") {
        RuleKind::Js
    } else if b.starts_with('@') {
        RuleKind::Url
    } else {
        RuleKind::Css
    }
}

/// 对文档执行规则，返回结果列表
pub fn apply(rule: &str, html: &str) -> Vec<String> {
    let rule = parse_rule(rule);
    apply_rule(&rule, html)
}

fn apply_rule(rule: &Rule, html: &str) -> Vec<String> {
    let results = match rule.kind {
        RuleKind::Css => css_select(rule, html),
        RuleKind::JsonPath => json_path(rule, html),
        RuleKind::Url => vec![url_replace(rule, html)],
        RuleKind::XPath | RuleKind::Js => {
            tracing::warn!("规则类型 {:?} 暂未实现（v2）：{}", rule.kind, rule.body);
            vec![]
        }
    };
    // 前缀/替换处理（legado：@@/替换在结果上应用）
    apply_post(results, rule)
}

/// CSS 选择器执行
fn css_select(rule: &Rule, html: &str) -> Vec<String> {
    let document = Html::parse_document(html);
    let selector = match Selector::parse(&rule.body) {
        Ok(s) => s,
        Err(_) => {
            // CSS 失败回退 Regex（legado 语义：非 CSS 语法当正则）
            return regex_match(&rule.body, html);
        }
    };
    document
        .select(&selector)
        .map(|el| el.html().trim().to_string())
        .collect()
}

/// Regex 执行（legado：规则整体当正则，提取 group 1 或全匹配）
fn regex_match(pattern: &str, text: &str) -> Vec<String> {
    let re = match Regex::new(pattern) {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    re.captures_iter(text)
        .map(|c| {
            c.get(1)
                .or_else(|| c.get(0))
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default()
        })
        .collect()
}

/// JSONPath 执行（输入可能是 JSON 文本或 HTML 中的 JSON 片段）
fn json_path(rule: &Rule, text: &str) -> Vec<String> {
    // 提取 body 内路径：{$.list.xxx} 或 {.list.xxx}
    let inner = rule.body.trim().trim_start_matches('{').trim_end_matches('}');
    let json: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => {
            // HTML 中可能内嵌 JSON（如 <script>），尝试按行提取
            return json_from_html(inner, text);
        }
    };
    let path = if inner.starts_with("$.") {
        &inner[2..]
    } else if inner.starts_with('.') {
        &inner[1..]
    } else {
        inner
    };
    let mut results = vec![];
    walk_json(&json, path, &mut results);
    results
}

/// 在 HTML 中查找形如 `{"...` 的 JSON 片段尝试解析
fn json_from_html(path: &str, html: &str) -> Vec<String> {
    let mut results = vec![];
    // 简单策略：按行找包含 { 的片段
    for line in html.lines() {
        let line = line.trim();
        if line.starts_with('{') && line.ends_with('}') {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                let mut r = vec![];
                walk_json(&v, path, &mut r);
                results.extend(r);
            }
        }
    }
    results
}

/// 简化 JSONPath 遍历（支持 .a.b[0].c 与 $.a 形式，数组自动展开）
fn walk_json(value: &serde_json::Value, path: &str, out: &mut Vec<String>) {
    // 拆分路径段：a.b[0].c → ["a","b","0","c"]（[n] 转数组索引段）
    let mut segments: Vec<String> = vec![];
    let mut cur = String::new();
    let mut in_bracket = false;
    for ch in path.chars() {
        match ch {
            '[' => {
                if !cur.is_empty() {
                    segments.push(cur.clone());
                    cur.clear();
                }
                in_bracket = true;
            }
            ']' => {
                if !cur.is_empty() {
                    segments.push(cur.clone());
                    cur.clear();
                }
                in_bracket = false;
            }
            '.' if !in_bracket => {
                if !cur.is_empty() {
                    segments.push(cur.clone());
                    cur.clear();
                }
            }
            _ => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        segments.push(cur);
    }

    let mut current = value;
    for (i, seg) in segments.iter().enumerate() {
        match current {
            serde_json::Value::Object(map) => {
                if let Some(v) = map.get(seg) {
                    current = v;
                } else if let Ok(idx) = seg.parse::<usize>() {
                    // 对象内数字键
                    if let Some(v) = map.get(&idx.to_string()) {
                        current = v;
                    } else {
                        return;
                    }
                } else {
                    return;
                }
            }
            serde_json::Value::Array(arr) => {
                if let Ok(idx) = seg.parse::<usize>() {
                    if let Some(v) = arr.get(idx) {
                        current = v;
                    } else {
                        return;
                    }
                } else {
                    // 数组展开：对每个元素继续剩余路径
                    let rest = &segments[i..];
                    for item in arr {
                        let mut r = vec![];
                        walk_json_segments(item, rest, &mut r);
                        out.extend(r);
                    }
                    return;
                }
            }
            _ => return,
        }
    }
    match current {
        serde_json::Value::Array(arr) => {
            for item in arr {
                if let Some(s) = item.as_str() {
                    out.push(s.to_string());
                } else if item.is_number() || item.is_boolean() {
                    out.push(item.to_string());
                }
            }
        }
        v if v.is_string() => out.push(v.as_str().unwrap().to_string()),
        v if v.is_number() || v.is_boolean() => out.push(v.to_string()),
        _ => {}
    }
}

fn walk_json_segments(value: &serde_json::Value, segments: &[String], out: &mut Vec<String>) {
    if segments.is_empty() {
        match value {
            serde_json::Value::Array(arr) => {
                for item in arr {
                    if let Some(s) = item.as_str() {
                        out.push(s.to_string());
                    }
                }
            }
            v if v.is_string() => out.push(v.as_str().unwrap().to_string()),
            _ => {}
        }
        return;
    }
    let mut current = value;
    for (i, seg) in segments.iter().enumerate() {
        match current {
            serde_json::Value::Object(map) => {
                if let Some(v) = map.get(seg) {
                    current = v;
                } else {
                    return;
                }
            }
            serde_json::Value::Array(arr) => {
                if let Ok(idx) = seg.parse::<usize>() {
                    if let Some(v) = arr.get(idx) {
                        current = v;
                    } else {
                        return;
                    }
                } else {
                    for item in arr {
                        walk_json_segments(item, &segments[i..], out);
                    }
                    return;
                }
            }
            _ => return,
        }
    }
    if segments.len() > 0 {
        match current {
            serde_json::Value::Array(arr) => {
                for item in arr {
                    if let Some(s) = item.as_str() {
                        out.push(s.to_string());
                    }
                }
            }
            v if v.is_string() => out.push(v.as_str().unwrap().to_string()),
            _ => {}
        }
    }
}

/// URL 规则：@{url} 或直接替换（v1：原样返回拼接）
fn url_replace(rule: &Rule, input: &str) -> String {
    let body = rule.body.trim_start_matches('@');
    if body.is_empty() {
        input.to_string()
    } else {
        format!("{body}{input}")
    }
}

/// 应用前缀与替换（legado 语义：前缀拼接 + 正则替换）
fn apply_post(results: Vec<String>, rule: &Rule) -> Vec<String> {
    results
        .into_iter()
        .map(|mut s| {
            if let Some(prefix) = &rule.prefix {
                if !s.starts_with(prefix.as_str()) {
                    s = format!("{prefix}{s}");
                }
            }
            if let Some(replace) = &rule.replace {
                // 替换规则：旧值##新值（legado：正则替换）
                if let Some((old, new)) = replace.split_once("##") {
                    if let Ok(re) = Regex::new(old.trim()) {
                        s = re.replace_all(&s, new.trim()).to_string();
                    }
                }
            }
            s
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_three_part() {
        let r = parse_rule("div.book##@https://a.com##\\s+## ");
        assert_eq!(r.kind, RuleKind::Css);
        assert_eq!(r.prefix.as_deref(), Some("@https://a.com"));
        assert!(r.replace.is_some());
    }

    #[test]
    fn test_css_select() {
        let html = r#"<html><body><div class="book"><a href="/1">书名A</a></div><div class="book"><a href="/2">书名B</a></div></body></html>"#;
        let r = apply("div.book a", html);
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn test_regex_fallback() {
        let html = "书名：测试书 作者：张三";
        let r = apply("书名：(.+?)\\s", html);
        assert_eq!(r.first().map(String::as_str), Some("测试书"));
    }

    #[test]
    fn test_json_path() {
        let json = r#"{"data":{"list":[{"name":"书1"},{"name":"书2"}]}}"#;
        let r = apply("{$.data.list.name}", json);
        assert_eq!(r, vec!["书1".to_string(), "书2".to_string()]);
    }
}
