//! legado 规则引擎 v1：规则字符串解析 + CSS/JSONPath/Regex 执行
//!
//! 规则语法（对齐 legado analyzeRule）：
//! - 三段式：`规则体##@前缀##替换规则`（## 分隔，后两段可选）
//! - 类型检测：`{...}` JSONPath / `//` XPath / `@js:`|`js:` JS / 其余 CSS 或 Regex
//! - `{{...}}` 内嵌表达式：`{{$.x}}`/`{{$[n]}}` JSONPath 提取；其余按 JS 执行（注入 result/key/page），结果替换回规则
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
    Regex,
    XPath, // v2 支持（sxd-xpath）
    Js,    // v2 支持（rquickjs）
    Url,   // 直接拼接/替换
}

/// 解析规则字符串（对齐 legado SourceRule）
/// - `@@` 前缀去掉（默认规则）
/// - `##` 两段式：第二段为替换规则（正则替换）；若第二段以 `@` 开头视为前缀拼接（兼容 legacy 书源旧格式）
pub fn parse_rule(rule: &str) -> Rule {
    let parts: Vec<&str> = rule.splitn(2, "##").collect();
    let raw_main = parts[0].trim();
    let tail = parts.get(1).map(|s| s.trim().to_string());

    // 去掉 @@ 前缀
    let (main, kind) = if raw_main.starts_with("@@") {
        (raw_main[2..].trim().to_string(), RuleKind::Css)
    } else {
        let k = detect_kind(raw_main);
        // @CSS: 去前缀
        if raw_main.starts_with("@CSS:") {
            (raw_main[5..].trim().to_string(), RuleKind::Css)
        } else if raw_main.starts_with("@XPath:") {
            (raw_main[6..].trim().to_string(), RuleKind::XPath)
        } else if raw_main.starts_with("@Json:") {
            (raw_main[6..].trim().to_string(), RuleKind::JsonPath)
        } else if raw_main.starts_with("@js:") {
            (raw_main[4..].trim().to_string(), RuleKind::Js)
        } else if raw_main.starts_with("js:") {
            (raw_main[3..].trim().to_string(), RuleKind::Js)
        } else {
            (raw_main.to_string(), k)
        }
    };

    // ## 第二段：@ 开头 → 前缀；否则 → 替换规则
    let mut prefix = None;
    let mut replace = None;
    if let Some(tail) = tail {
        if tail.starts_with('@') {
            prefix = Some(tail);
        } else {
            replace = Some(tail);
        }
    }

    Rule {
        kind,
        body: main,
        prefix,
        replace,
    }
}

fn detect_kind(body: &str) -> RuleKind {
    let b = body.trim();
    // 对齐 legado SourceRule 类型检测（AnalyzeRule.kt）
    if b.starts_with("@CSS:") {
        RuleKind::Css // @CSS: 显式 CSS
    } else if b.starts_with("@@") {
        RuleKind::Css // @@ 默认规则（去前缀由 parse 处理）
    } else if b.starts_with("@XPath:") {
        RuleKind::XPath
    } else if b.starts_with("@Json:") {
        RuleKind::JsonPath
    } else if b.starts_with("$.") || b.starts_with("$[") || b.starts_with('{') {
        RuleKind::JsonPath // $. / $[ 或 JSON 片段
    } else if b.starts_with('/') {
        RuleKind::XPath // XPath 特征明显，无需标识头
    } else if b.starts_with("@js:") || b.starts_with("js:") {
        RuleKind::Js
    } else if b.contains("$1") || b.contains("$2") {
        RuleKind::Regex // $N 引用 → 正则
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
    apply_rule_inner(rule, html, 0)
}

fn apply_rule_inner(rule: &Rule, html: &str, depth: usize) -> Vec<String> {
    // {{...}} 内嵌表达式：先展开，再重新解析执行（类型可能变化，如 {{$.x}} 拼接出 CSS）
    let rule = if depth < 4 && rule.body.contains("{{") {
        let expanded = expand_inline(&rule.body, html);
        if expanded != rule.body {
            // 重建完整规则串（保留 ##前缀/##替换段）后重新解析
            let mut full = expanded;
            if let Some(p) = &rule.prefix {
                full.push_str("##");
                full.push_str(p);
            } else if let Some(r) = &rule.replace {
                full.push_str("##");
                full.push_str(r);
            }
            return apply_rule_inner(&parse_rule(&full), html, depth + 1);
        }
        rule
    } else {
        rule
    };
    // 空规则（如 {{...}} 失败展开为空）→ 空结果
    if rule.body.trim().is_empty() {
        return vec![];
    }
    let results = match rule.kind {
        RuleKind::Css => css_select(rule, html),
        RuleKind::JsonPath => json_path(rule, html),
        RuleKind::Regex => regex_match(&rule.body, html),
        RuleKind::XPath => crate::parser::xpath::xpath_select(&rule.body, html),
        RuleKind::Js => {
            // JS 规则：注入 result/key/page/baseUrl 环境
            let mut vars = std::collections::HashMap::new();
            vars.insert("result".to_string(), html.to_string());
            vars.insert("key".to_string(), String::new());
            vars.insert("page".to_string(), "1".to_string());
            vars.insert("baseUrl".to_string(), String::new());
            match crate::parser::js::eval_js(&rule.body, &vars) {
                Ok(s) if !s.is_empty() => vec![s],
                _ => vec![],
            }
        }
        RuleKind::Url => vec![url_replace(rule, html)],
    };
    // 前缀/替换处理（legado：@@/替换在结果上应用）
    apply_post(results, rule)
}

/// 展开规则中的 `{{...}}` 内嵌表达式（legado 模板替换语义）：
/// - `{{$.xxx}}` / `{{$[n]}}`：JSONPath 从当前上下文文本提取（复用 json_path 逻辑）
/// - 其他内容：作为 JS 执行（注入 result=上下文文本 / key / page），结果替换回规则
/// - 提取失败 / JS 报错 / 结果为空 → 替换为空串；未闭合的 `{{` → 原样返回
///
/// 注意：JS 字符串内若含 `}}` 会提前截断（v1 限制，规则 JS 避免字面 `}}`）
fn expand_inline(body: &str, text: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = match after.find("}}") {
            Some(e) => e,
            None => return body.to_string(), // 未闭合：不处理
        };
        let expr = after[..end].trim();
        let replaced = if expr.starts_with("$.") || expr.starts_with("$[") {
            inline_json_path(expr, text)
        } else {
            inline_js(expr, text)
        };
        out.push_str(&replaced);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// 内嵌 JSONPath：`{{$.a.b}}` / `{{$[0].c}}` → 从上下文文本提取
/// （多结果以换行拼接；无结果 → 空串）
fn inline_json_path(expr: &str, text: &str) -> String {
    let path = if expr.starts_with("$.") {
        &expr[2..]
    } else if expr.starts_with("$[") {
        &expr[1..]
    } else {
        expr
    };
    let mut results = vec![];
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(v) => walk_json(&v, path, &mut results),
        Err(_) => results = json_from_html(path, text),
    }
    if results.is_empty() {
        String::new()
    } else {
        results.join("\n")
    }
}

/// 内嵌 JS：`{{expr}}` → 执行（注入 result=上下文文本 / key / page），失败 → 空串
fn inline_js(expr: &str, text: &str) -> String {
    let mut vars = std::collections::HashMap::new();
    vars.insert("result".to_string(), text.to_string());
    vars.insert("key".to_string(), String::new());
    vars.insert("page".to_string(), "1".to_string());
    crate::parser::js::eval_js(expr, &vars).unwrap_or_default()
}

/// CSS 选择器执行（legado 链式：&& 多规则 + @ 链 + 末段属性）
fn css_select(rule: &Rule, html: &str) -> Vec<String> {
    crate::parser::css_chain::css_chain(&rule.body, html)
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
                } else if seg == "*" {
                    // [*] 通配：对每个元素继续剩余路径；无剩余路径时输出对象（JSON 序列化）
                    let rest = &segments[i + 1..];
                    if rest.is_empty() {
                        for item in arr {
                            out.push(serde_json::to_string(item).unwrap_or_default());
                        }
                    } else {
                        for item in arr {
                            let mut r = vec![];
                            walk_json_segments(item, rest, &mut r);
                            out.extend(r);
                        }
                    }
                    return;
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
                } else if item.is_object() {
                    out.push(serde_json::to_string(item).unwrap_or_default());
                }
            }
        }
        v if v.is_string() => out.push(v.as_str().unwrap().to_string()),
        v if v.is_number() || v.is_boolean() => out.push(v.to_string()),
        v if v.is_object() => out.push(serde_json::to_string(v).unwrap_or_default()),
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
    fn test_parse_prefix() {
        // ## 第二段 @ 开头 → 前缀（兼容 legacy 旧格式）
        let r = parse_rule("div.book##@https://a.com");
        assert_eq!(r.kind, RuleKind::Css);
        assert_eq!(r.prefix.as_deref(), Some("@https://a.com"));
        assert!(r.replace.is_none());
    }

    #[test]
    fn test_parse_legado_flags() {
        assert_eq!(parse_rule("@Json:$.list.name").kind, RuleKind::JsonPath);
        assert_eq!(parse_rule("$.list.name").kind, RuleKind::JsonPath);
        assert_eq!(parse_rule("@XPath://div/a").kind, RuleKind::XPath);
        assert_eq!(parse_rule("//div/a").kind, RuleKind::XPath);
        assert_eq!(parse_rule("@@div.book").kind, RuleKind::Css);
        assert_eq!(parse_rule("a@href").kind, RuleKind::Css);
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

    #[test]
    fn test_js_rule() {
        let html = "abc123";
        // js: / @js: 前缀剥离 + result 变量注入
        assert_eq!(apply("js:result.length", html), vec!["6".to_string()]);
        assert_eq!(
            apply("@js:result.toUpperCase()", html),
            vec!["ABC123".to_string()]
        );
        // JS 失败 → 空结果
        assert!(apply("@js:throw new Error('x')", html).is_empty());
        // JS 返回空串 → 空结果
        assert!(apply("@js:''", html).is_empty());
    }

    #[test]
    fn test_inline_js_substitution() {
        let html = r#"<html><body><div class="book">书名A</div><div class="book">书名B</div></body></html>"#;
        // {{...}} JS 构建 CSS 选择器，替换回规则后执行
        let r = apply("{{'div.' + 'book'}}", html);
        assert_eq!(r.len(), 2);
        // JS 可读取注入的 result（当前上下文文本），条件返回正则规则
        let html2 = "书名：测试书 作者：张三";
        let rule = r#"{{result.startsWith('书名') ? '书名：(.+?)\\s' : 'div'}}"#;
        let r2 = apply(rule, html2);
        assert_eq!(r2.first().map(String::as_str), Some("测试书"));
        // JS 失败 → 展开为空 → 空结果
        assert!(apply("{{nonexistent.fn()}}", html).is_empty());
        // 未闭合 {{ 原样处理（按 JsonPath 分支解析失败 → 空结果），不 panic
        assert!(apply("{{div.book", html).is_empty());
    }

    #[test]
    fn test_inline_jsonpath_substitution() {
        let json = r#"{"data":{"n":42}}"#;
        // {{$.x}} → JSONPath 提取（非 JS 执行），替换回规则后执行
        let r = apply("@js:{{$.data.n}}", json);
        assert_eq!(r, vec!["42".to_string()]);
        // 提取失败 → 替换为空 → 空结果
        let r2 = apply("@js:{{$.missing}}", json);
        assert!(r2.is_empty());
    }

    #[test]
    fn test_expand_inline() {
        // 数组下标形式 {{$.a[0]}}
        assert_eq!(
            expand_inline("{{$.list[0]}}", r#"{"list":["书1","书2"]}"#),
            "书1"
        );
        // 多结果以换行拼接
        assert_eq!(
            expand_inline(
                "{{$.list.name}}",
                r#"{"list":[{"name":"书1"},{"name":"书2"}]}"#
            ),
            "书1\n书2"
        );
        // 上下文非完整 JSON → 逐行提取 JSON 片段（json_from_html 回退）
        assert_eq!(
            expand_inline("{{$.data.name}}", "前文\n{\"data\":{\"name\":\"内嵌\"}}\n后文"),
            "内嵌"
        );
        // 未闭合 {{ 原样返回
        assert_eq!(expand_inline("{{div.book", "<html></html>"), "{{div.book");
    }
}
