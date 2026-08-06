//! legado 规则引擎 v2：规则字符串解析 + CSS/JSONPath/Regex/XPath/JS 执行
//!
//! 规则语法（对齐 legado analyzeRule / AnalyzeByJSoup / AnalyzeByJSonPath / RuleAnalyzer）：
//! - 三段式：`规则体##替换正则##替换串`（## 分隔；第三段 `##` 后跟 `#` → 仅替换首个匹配）
//! - 纯替换规则：`##pat##rep###`（规则体为空 → 直接对输入应用替换）
//! - 类型检测：`{...}` JSONPath / `//` XPath / `@js:`|`js:` JS / `:` 正则（列表规则）/ 其余 CSS 或 Regex
//! - JS 链：`<js>...</js>` / `@js:`（贪婪到末尾）——规则结果进 JS（result 变量），再进后续规则
//! - `{{...}}` 内嵌表达式：`{{$.x}}`/`{{$[n]}}` JSONPath 提取；`{{@rule}}`/`{{//xpath}}`
//!   规则引用（legado isRule）；其余按 JS 执行（注入 result/key/page），结果替换回规则
//! - 组合分隔：`&&` 合并 / `||` 首个命中 / `%%` 按位交错（CSS/JSONPath/XPath 均支持）
//! - JSONPath v2：`$..` 递归下降 / `[?()]` 过滤（@ 属性、比较、&&/||）/ `[-1]` / 切片 / 通配
//! - 结果：字符串列表（legado 返回字符串列表语义）

// GAP 153：正则经 util::regex 兼容层执行（lookbehind 自动升级 fancy-regex）

/// 解析后的规则
#[derive(Debug, Clone)]
pub struct Rule {
    /// 规则类型
    pub kind: RuleKind,
    /// 规则主体（类型检测前的原始文本，## 尾段已剥离）
    pub body: String,
    /// `##@前缀`（legacy 旧格式：结果前缀拼接，可选）
    pub prefix: Option<String>,
    /// `##` 第二段（替换正则，可选）
    pub replace_regex: Option<String>,
    /// `##` 第三段（替换串，可选；无第三段 = 替换为空串）
    pub replacement: Option<String>,
    /// `###` 标志（仅替换首个匹配；无匹配 → 空串，legado replaceFirst）
    pub replace_first: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RuleKind {
    Css,
    JsonPath,
    Regex,
    XPath, // v2 支持（sxd-xpath）
    Js,    // v2 支持（boa）
    Url,   // 直接拼接/替换（保留兼容；现无检测路径）
}

/// 解析规则字符串（对齐 legado SourceRule + makeUpRule）
/// - `@@` 前缀去掉（默认规则）
/// - `@CSS:`/`@XPath:`/`@Json:`/`@js:`/`js:` 前缀（大小写不敏感，对齐 legado startsWith(ignoreCase)）
/// - `:` 前缀 → 正则规则（legado allInOne：书籍列表/目录列表专用）
/// - 孤立 `@` 前缀剥除（legado RuleAnalyzer.trim——链式规则中 @ 为冗余符号）
/// - `##` 多段：第二段为替换正则（@ 开头 → legacy 前缀）；第三段为替换串；
///   存在第四段（`###`）→ 仅替换首个匹配
pub fn parse_rule(rule: &str) -> Rule {
    // ## 切分需避开 {{...}} 内嵌规则（legado：evalMatcher 先于 makeUpRule 的 ## 切分）
    let parts = split_hashes(rule);
    let raw_main = parts[0].trim();
    let (main, kind) = if raw_main.starts_with("@@") {
        (raw_main[2..].trim().to_string(), RuleKind::Css)
    } else if let Some(rest) = strip_prefix_ci(raw_main, "@CSS:") {
        (rest.trim().to_string(), RuleKind::Css)
    } else if let Some(rest) = strip_prefix_ci(raw_main, "@XPath:") {
        (rest.trim().to_string(), RuleKind::XPath)
    } else if let Some(rest) = strip_prefix_ci(raw_main, "@Json:") {
        (rest.trim().to_string(), RuleKind::JsonPath)
    } else if let Some(rest) = strip_prefix_ci(raw_main, "@js:") {
        (rest.trim().to_string(), RuleKind::Js)
    } else if let Some(rest) = strip_prefix_ci(raw_main, "js:") {
        (rest.trim().to_string(), RuleKind::Js)
    } else if raw_main.starts_with(':') {
        // legado allInOne：: 开头整条规则为正则
        (raw_main[1..].trim().to_string(), RuleKind::Regex)
    } else {
        // 孤立 @ 前缀剥除（legado RuleAnalyzer.trim：@ 或空白符）
        let cleaned = raw_main.trim_start_matches('@').trim();
        (cleaned.to_string(), detect_kind(cleaned))
    };

    let mut prefix = None;
    let mut replace_regex = None;
    let mut replacement = None;
    let mut replace_first = false;
    if parts.len() > 1 {
        let tail = parts[1].trim();
        if tail.starts_with('@') {
            // legacy 前缀格式：规则##@前缀（拼接在结果前）
            prefix = Some(parts[1..].join("##"));
        } else {
            replace_regex = Some(tail.to_string());
            if parts.len() > 2 {
                replacement = Some(parts[2].to_string());
            }
            if parts.len() > 3 {
                replace_first = true;
            }
        }
    }

    Rule {
        kind,
        body: main,
        prefix,
        replace_regex,
        replacement,
        replace_first,
    }
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    if head.eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

/// 按 `##` 切分（避开 `{{...}}` 内嵌规则——其内容可含 `##` 替换链）
fn split_hashes(rule: &str) -> Vec<&str> {
    let b = rule.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0i32;
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == b'{' && b[i + 1] == b'{' {
            depth += 1;
            i += 2;
            continue;
        }
        if b[i] == b'}' && b[i + 1] == b'}' && depth > 0 {
            depth -= 1;
            i += 2;
            continue;
        }
        if depth == 0 && b[i] == b'#' && b[i + 1] == b'#' {
            parts.push(&rule[start..i]);
            i += 2;
            start = i;
            continue;
        }
        i += 1;
    }
    parts.push(&rule[start..]);
    parts
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
    } else if b.starts_with(':') {
        RuleKind::Regex // legado allInOne：: 前缀正则规则
    } else if b.starts_with('/') {
        RuleKind::XPath // XPath 特征明显，无需标识头
    } else if b.starts_with("@js:") || b.starts_with("js:") {
        RuleKind::Js
    } else if b.contains("$1") || b.contains("$2") {
        RuleKind::Regex // $N 引用 → 正则
    } else {
        RuleKind::Css
    }
}

/// 对文档执行规则，返回结果列表（含 <js>/@js: 链）
pub fn apply(rule: &str, html: &str) -> Vec<String> {
    apply_depth(rule, html, 0)
}

/// 链式执行（legado splitSourceRule：先按 JS 标记切段，逐段顺序执行、结果管道传递）
fn apply_depth(rule: &str, html: &str, depth: usize) -> Vec<String> {
    let segs = split_js_chain(rule);
    if segs.len() == 1 && !segs[0].is_js {
        return apply_single(segs[0].text, html, depth);
    }
    let mut result: Option<Vec<String>> = None;
    for seg in segs {
        let input = result
            .as_ref()
            .map(|r| r.join("\n"))
            .unwrap_or_else(|| html.to_string());
        if seg.is_js {
            // JS 段：{{...}} 先展开（legado makeUpRule 对 JS 规则同样处理），再以 result 执行
            let code = expand_inline_depth(seg.text, &input, depth);
            let mut vars = std::collections::HashMap::new();
            vars.insert("result".to_string(), input);
            vars.insert("key".to_string(), String::new());
            vars.insert("page".to_string(), "1".to_string());
            vars.insert("baseUrl".to_string(), String::new());
            vars.insert("urlSearchSeries".to_string(), String::new());
            vars.insert("urlSearch".to_string(), String::new());
            vars.insert("url".to_string(), String::new());
            match crate::parser::js::eval_js(&code, &vars) {
                Ok(s) => {
                    // 空串结果 → 空列表（repo 语义）；后续段输入为空串（legado result="" 非 null）
                    result = Some(if s.is_empty() { vec![] } else { vec![s] });
                }
                Err(_) => return vec![], // legado：JS 失败 result=null → 整链终止为空
            }
        } else {
            result = Some(apply_single(seg.text, &input, depth));
        }
    }
    result.unwrap_or_default()
}

/// 单条规则执行（parse + {{}} 展开 + 类型分发 + 前缀/替换）
fn apply_single(rule_str: &str, html: &str, depth: usize) -> Vec<String> {
    let rule = parse_rule(rule_str);
    apply_rule_inner(&rule, html, depth)
}

fn apply_rule_inner(rule: &Rule, html: &str, depth: usize) -> Vec<String> {
    // {{...}} 内嵌表达式：先展开，再重新解析执行（类型可能变化，如 {{$.x}} 拼接出 CSS）
    let rule = if depth < 4 && rule.body.contains("{{") {
        let expanded = expand_inline_depth(&rule.body, html, depth);
        if expanded != rule.body {
            // 重建完整规则串（保留 ##前缀/##替换段）后重新解析
            let mut full = expanded.clone();
            if let Some(p) = &rule.prefix {
                full.push_str("##");
                full.push_str(p);
            } else if let Some(re) = &rule.replace_regex {
                full.push_str("##");
                full.push_str(re);
                if let Some(rep) = &rule.replacement {
                    full.push_str("##");
                    full.push_str(rep);
                    if rule.replace_first {
                        full.push_str("##");
                    }
                }
            }
            let r = apply_single(&full, html, depth + 1);
            if r.is_empty() && !expanded.trim().is_empty() {
                // legado：含 {{}} 的规则展开后即结果文本（{{}} 使规则进入 Regex 模式 →
                // 规则串本身即结果）。执行无果时返回展开文本（前缀/替换仍应用）
                return apply_post(vec![expanded], rule);
            }
            return r;
        }
        rule
    } else {
        rule
    };
    // 空规则：纯替换规则（##pat##rep###）→ 直接对输入应用替换；否则空结果
    if rule.body.trim().is_empty() {
        if rule.replace_regex.is_some() || rule.prefix.is_some() {
            return apply_post(vec![html.to_string()], rule);
        }
        return vec![];
    }
    let results = match rule.kind {
        RuleKind::Css => css_select(rule, html),
        RuleKind::JsonPath => json_path(rule, html),
        RuleKind::Regex => regex_match(&rule.body, html),
        RuleKind::XPath => xpath_select_rules(rule, html),
        RuleKind::Js => {
            // JS 规则：注入 result/key/page/baseUrl 环境
            let mut vars = std::collections::HashMap::new();
            vars.insert("result".to_string(), html.to_string());
            vars.insert("key".to_string(), String::new());
            vars.insert("page".to_string(), "1".to_string());
            vars.insert("baseUrl".to_string(), String::new());
            vars.insert("urlSearchSeries".to_string(), String::new());
            vars.insert("urlSearch".to_string(), String::new());
            vars.insert("url".to_string(), String::new());
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
/// - `{{@rule}}` / `{{//xpath}}`：规则引用（legado isRule：@ 开头或 // 开头）
/// - 其他内容：作为 JS 执行（注入 result=上下文文本 / key / page），结果替换回规则
/// - 提取失败 / JS 报错 / 结果为空 → 替换为空串；未闭合的 `{{` → 原样返回
///
/// 注意：JS 字符串内若含 `}}` 会提前截断（v1 限制，规则 JS 避免字面 `}}`）
fn expand_inline_depth(body: &str, text: &str, depth: usize) -> String {
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
        } else if expr.starts_with('@') || expr.starts_with("//") {
            // legado isRule：@ 开头（@@/@CSS:/@XPath:/@Json:/@js:）或 // → 作为规则递归求值
            apply_depth(expr, text, depth + 1).join("\n")
        } else {
            inline_js(expr, text)
        };
        out.push_str(&replaced);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// 展开 `{{...}}`（测试/外部便捷入口，无递归深度）
fn expand_inline(body: &str, text: &str) -> String {
    expand_inline_depth(body, text, 0)
}

/// 内嵌 JSONPath：`{{$.a.b}}` / `{{$[0].c}}` → 从上下文文本提取
/// （多结果以换行拼接；无结果 → 空串）
fn inline_json_path(expr: &str, text: &str) -> String {
    let path = if let Some(p) = expr.strip_prefix('$') {
        p // "$.a" → ".a"；"$..a" → "..a"（保留递归标记）；"$[0]" → "[0]"
    } else if let Some(p) = expr.strip_prefix('.') {
        p
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

/// CSS 选择器执行（legado 链式：<js> 链 + &&/||/%% 组合 + @ 链 + 末段属性）
fn css_select(rule: &Rule, html: &str) -> Vec<String> {
    crate::parser::css_chain::css_chain(&rule.body, html)
}

/// XPath 执行（legado AnalyzeByXPath：&&/||/%% 组合）
fn xpath_select_rules(rule: &Rule, html: &str) -> Vec<String> {
    let (sep, subs) = split_combined(&rule.body);
    let mut groups: Vec<Vec<String>> = Vec::new();
    for sub in subs {
        let r = crate::parser::xpath::xpath_select(sub.trim(), html);
        if !r.is_empty() {
            groups.push(r);
            if sep == Some("||") {
                break;
            }
        }
    }
    merge_groups(sep, groups)
}

/// Regex 执行（legado：规则整体当正则，提取 group 1 或全匹配）
/// GAP 153：经 fancy-regex 兼容层编译（支持 lookbehind）；编译失败记日志并返回空
fn regex_match(pattern: &str, text: &str) -> Vec<String> {
    let re = match crate::util::regex::Regex::new(pattern) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("正则规则编译失败（规则引擎返回空）: {e}");
            return vec![];
        }
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

/// JSONPath 执行（对齐 legado AnalyzeByJSonPath：&&/||/%% 组合 + 完整路径语法）
fn json_path(rule: &Rule, text: &str) -> Vec<String> {
    let (sep, subs) = split_combined(&rule.body);
    let mut groups: Vec<Vec<String>> = Vec::new();
    for sub in subs {
        let r = json_path_single(sub, text);
        if !r.is_empty() {
            groups.push(r);
            if sep == Some("||") {
                break;
            }
        }
    }
    merge_groups(sep, groups)
}

fn merge_groups(sep: Option<&'static str>, groups: Vec<Vec<String>>) -> Vec<String> {
    match sep {
        // legado %%：按 results[0] 的行号交错取各组同位置元素
        Some("%%") => {
            let mut out = Vec::new();
            if let Some(first) = groups.first() {
                for i in 0..first.len() {
                    for g in &groups {
                        if i < g.len() {
                            out.push(g[i].clone());
                        }
                    }
                }
            }
            out
        }
        _ => groups.into_iter().flatten().collect(),
    }
}

/// 单条 JSONPath（输入可能是 JSON 文本或 HTML 中的 JSON 片段）
fn json_path_single(body: &str, text: &str) -> Vec<String> {
    // 提取 body 内路径：{$.list.xxx} 或 {.list.xxx}
    let inner = body.trim().trim_start_matches('{').trim_end_matches('}');
    let json: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => {
            // HTML 中可能内嵌 JSON（如 <script>），尝试按行提取
            return json_from_html(inner, text);
        }
    };
    let path = if let Some(p) = inner.strip_prefix('$') {
        p // "$.a" → ".a"；"$..a" → "..a"（保留递归标记）；"$[0]" → "[0]"
    } else if let Some(p) = inner.strip_prefix('.') {
        p
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

// ---------- JSONPath 路径引擎 ----------

#[derive(Debug, Clone, PartialEq)]
enum JSeg {
    Key(String),
    RecKey(String),
    Wildcard,
    Index(i64),
    Slice(Option<i64>, Option<i64>, i64),
    Multi(Vec<JItem>),
    Filter(String),
    QuotedKey(String),
}

#[derive(Debug, Clone, PartialEq)]
enum JItem {
    I(i64),
    S(Option<i64>, Option<i64>, i64),
}

/// 简化 JSONPath 遍历（支持 .a.b / [0] / [-1] / [*] / $..递归 / [?()] 过滤 / 切片）
fn walk_json(value: &serde_json::Value, path: &str, out: &mut Vec<String>) {
    let segs = tokenize_json_path(path);
    let mut found: Vec<&serde_json::Value> = Vec::new();
    eval_segments(value, &segs, &mut found);
    for v in found {
        push_json_value(v, out);
    }
}

fn push_json_value(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::Array(arr) => {
            for item in arr {
                push_json_value(item, out);
            }
        }
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Null => {}
        other => out.push(other.to_string()), // 数字/布尔/对象（JSON 序列化）
    }
}

fn tokenize_json_path(path: &str) -> Vec<JSeg> {
    let b = path.as_bytes();
    let mut segs = Vec::new();
    let mut i = 0;
    if b.first() == Some(&b'$') {
        i = 1;
    }
    while i < b.len() {
        match b[i] {
            b'.' => {
                if i + 1 < b.len() && b[i + 1] == b'.' {
                    // 递归下降 ..name
                    i += 2;
                    let (name, ni) = read_name(path, i);
                    segs.push(JSeg::RecKey(name));
                    i = ni;
                } else if i + 1 < b.len() && b[i + 1] == b'*' {
                    segs.push(JSeg::Wildcard);
                    i += 2;
                } else {
                    i += 1;
                    let (name, ni) = read_name(path, i);
                    if !name.is_empty() {
                        segs.push(JSeg::Key(name));
                    }
                    i = ni;
                }
            }
            b'[' => {
                let (content, ni) = read_bracket(path, i);
                segs.push(parse_bracket(&content));
                i = ni;
            }
            b'*' => {
                segs.push(JSeg::Wildcard);
                i += 1;
            }
            _ => {
                let (name, ni) = read_name(path, i);
                segs.push(JSeg::Key(name));
                i = ni;
            }
        }
    }
    segs
}

fn read_name(path: &str, mut i: usize) -> (String, usize) {
    let b = path.as_bytes();
    let start = i;
    while i < b.len() && b[i] != b'.' && b[i] != b'[' {
        i += 1;
    }
    (path[start..i].to_string(), i)
}

/// 读取 `[...]` 内容（引号与括号平衡）
fn read_bracket(path: &str, start: usize) -> (String, usize) {
    let b = path.as_bytes();
    let mut i = start + 1;
    let mut depth = 0i32;
    let mut in_s = false;
    let mut in_d = false;
    while i < b.len() {
        let c = b[i];
        if c == b'\'' && !in_d {
            in_s = !in_s;
        } else if c == b'"' && !in_s {
            in_d = !in_d;
        } else if !in_s && !in_d {
            match c {
                b'[' | b'(' => depth += 1,
                b']' => {
                    if depth == 0 {
                        return (path[start + 1..i].to_string(), i + 1);
                    }
                    depth -= 1;
                }
                b')' => depth -= 1,
                _ => {}
            }
        }
        i += 1;
    }
    (path[start + 1..].to_string(), b.len())
}

fn parse_bracket(content: &str) -> JSeg {
    let c = content.trim();
    if let Some(rest) = c.strip_prefix("?(") {
        if let Some(expr) = rest.strip_suffix(')') {
            return JSeg::Filter(expr.to_string());
        }
    }
    if c.starts_with('\'') || c.starts_with('"') {
        let q = c.chars().next().unwrap();
        if c.len() >= 2 && c.ends_with(q) {
            return JSeg::QuotedKey(c[1..c.len() - 1].to_string());
        }
    }
    if c == "*" {
        return JSeg::Wildcard;
    }
    let items: Vec<&str> = split_top_level(c, ",");
    if items.is_empty() {
        return JSeg::Key(String::new());
    }
    let parsed: Vec<JItem> = items
        .iter()
        .map(|it| {
            let it = it.trim();
            if it.contains(':') {
                let parts: Vec<&str> = it.split(':').collect();
                JItem::S(
                    parts
                        .first()
                        .copied()
                        .filter(|s| !s.is_empty())
                        .and_then(|x| x.trim().parse::<i64>().ok()),
                    parts
                        .get(1)
                        .copied()
                        .filter(|s| !s.is_empty())
                        .and_then(|x| x.trim().parse::<i64>().ok()),
                    parts
                        .get(2)
                        .copied()
                        .filter(|s| !s.is_empty())
                        .and_then(|x| x.trim().parse::<i64>().ok())
                        .unwrap_or(1),
                )
            } else {
                JItem::I(it.parse::<i64>().unwrap_or(0))
            }
        })
        .collect();
    if parsed.len() == 1 {
        match parsed.into_iter().next().unwrap() {
            JItem::I(n) => JSeg::Index(n),
            JItem::S(s, e, st) => JSeg::Slice(s, e, st),
        }
    } else {
        JSeg::Multi(parsed)
    }
}

fn eval_segments<'v>(
    value: &'v serde_json::Value,
    segs: &[JSeg],
    out: &mut Vec<&'v serde_json::Value>,
) {
    if segs.is_empty() {
        out.push(value);
        return;
    }
    match &segs[0] {
        JSeg::Key(name) | JSeg::QuotedKey(name) => match value {
            serde_json::Value::Object(map) => {
                if let Some(v) = map.get(name) {
                    eval_segments(v, &segs[1..], out);
                }
            }
            serde_json::Value::Array(arr) => {
                // 数组自动展开（对齐 v1 行为）
                for item in arr {
                    eval_segments(item, segs, out);
                }
            }
            _ => {}
        },
        JSeg::RecKey(name) => {
            let mut found: Vec<&'v serde_json::Value> = Vec::new();
            collect_rec(value, name, &mut found);
            for v in found {
                eval_segments(v, &segs[1..], out);
            }
        }
        JSeg::Wildcard => match value {
            serde_json::Value::Array(arr) => {
                for item in arr {
                    eval_segments(item, &segs[1..], out);
                }
            }
            serde_json::Value::Object(map) => {
                for v in map.values() {
                    eval_segments(v, &segs[1..], out);
                }
            }
            _ => {}
        },
        JSeg::Index(n) => match value {
            serde_json::Value::Array(arr) => {
                if let Some(v) = norm_index(*n, arr.len()).and_then(|idx| arr.get(idx)) {
                    eval_segments(v, &segs[1..], out);
                }
            }
            serde_json::Value::Object(map) => {
                if let Some(v) = map.get(&n.to_string()) {
                    eval_segments(v, &segs[1..], out);
                }
            }
            _ => {}
        },
        JSeg::Slice(s, e, st) => {
            if let serde_json::Value::Array(arr) = value {
                for v in slice_items(arr, *s, *e, *st) {
                    eval_segments(v, &segs[1..], out);
                }
            }
        }
        JSeg::Multi(items) => {
            if let serde_json::Value::Array(arr) = value {
                for it in items {
                    match it {
                        JItem::I(n) => {
                            if let Some(v) = norm_index(*n, arr.len()).and_then(|idx| arr.get(idx))
                            {
                                eval_segments(v, &segs[1..], out);
                            }
                        }
                        JItem::S(s, e, st) => {
                            for v in slice_items(arr, *s, *e, *st) {
                                eval_segments(v, &segs[1..], out);
                            }
                        }
                    }
                }
            }
        }
        JSeg::Filter(expr) => match value {
            serde_json::Value::Array(arr) => {
                for item in arr {
                    if eval_filter(expr, item) {
                        eval_segments(item, &segs[1..], out);
                    }
                }
            }
            other => {
                if eval_filter(expr, other) {
                    eval_segments(other, &segs[1..], out);
                }
            }
        },
    }
}

fn norm_index(n: i64, len: usize) -> Option<usize> {
    let len_i = len as i64;
    let idx = if n < 0 { n + len_i } else { n };
    if idx >= 0 && idx < len_i {
        Some(idx as usize)
    } else {
        None
    }
}

/// $..name：任意深度收集键 name 的值
fn collect_rec<'v>(value: &'v serde_json::Value, name: &str, out: &mut Vec<&'v serde_json::Value>) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(v) = map.get(name) {
                out.push(v);
            }
            for v in map.values() {
                collect_rec(v, name, out);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                collect_rec(v, name, out);
            }
        }
        _ => {}
    }
}

/// Python 风格切片（end 排除；负数回绕；step 可为负反向）
fn slice_items(
    arr: &[serde_json::Value],
    s: Option<i64>,
    e: Option<i64>,
    st: i64,
) -> Vec<&serde_json::Value> {
    let len = arr.len() as i64;
    let step = if st == 0 { 1 } else { st };
    let mut start = s.unwrap_or(if step > 0 { 0 } else { len - 1 });
    if start < 0 {
        start += len;
    }
    let mut end = e.unwrap_or(if step > 0 { len } else { -(len + 1) });
    if end < 0 {
        end += len;
    }
    let mut out = Vec::new();
    if step > 0 {
        let mut i = start.max(0);
        while i < end.min(len) {
            out.push(&arr[i as usize]);
            i += step;
        }
    } else {
        let mut i = start.min(len - 1);
        while i > end && i >= 0 {
            out.push(&arr[i as usize]);
            i += step;
        }
    }
    out
}

// ---------- [?()] 过滤表达式 ----------

fn eval_filter(expr: &str, item: &serde_json::Value) -> bool {
    for part in split_top_level(expr, "||") {
        if eval_and(part, item) {
            return true;
        }
    }
    false
}

fn eval_and(expr: &str, item: &serde_json::Value) -> bool {
    for part in split_top_level(expr, "&&") {
        if !eval_primary(part, item) {
            return false;
        }
    }
    true
}

fn eval_primary(expr: &str, item: &serde_json::Value) -> bool {
    let s = expr.trim();
    if s.is_empty() {
        return false;
    }
    if let Some(rest) = s.strip_prefix('(') {
        if let Some(inner) = rest.strip_suffix(')') {
            return eval_filter(inner, item);
        }
    }
    if let Some(rest) = s.strip_prefix('!') {
        return !eval_primary(rest, item);
    }
    for op in ["==", "!=", "<=", ">=", "<", ">"] {
        if let Some(pos) = find_op_top(s, op) {
            let lhs = s[..pos].trim();
            let rhs = s[pos + op.len()..].trim();
            let lv = eval_filter_path(lhs, item);
            let rv = parse_filter_literal(rhs);
            return compare_filter(lv, rv.as_ref(), op);
        }
    }
    // 无比较：路径真值（存在且非 null/false/空串）
    eval_filter_path(s, item)
        .map(|v| {
            !v.is_null()
                && v != &serde_json::Value::Bool(false)
                && v.as_str().map(|s| !s.is_empty()).unwrap_or(true)
        })
        .unwrap_or(false)
}

/// 过滤内路径求值：@ / @.a.b / @['a']（取首个结果）
fn eval_filter_path<'v>(path: &str, item: &'v serde_json::Value) -> Option<&'v serde_json::Value> {
    let p = path.trim();
    if p == "@" {
        return Some(item);
    }
    let p = p.strip_prefix('@').unwrap_or(p);
    let segs = tokenize_json_path(p);
    let mut found: Vec<&'v serde_json::Value> = Vec::new();
    eval_segments(item, &segs, &mut found);
    found.into_iter().next()
}

fn parse_filter_literal(s: &str) -> Option<serde_json::Value> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2)
        || (s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
    {
        return Some(serde_json::Value::String(s[1..s.len() - 1].to_string()));
    }
    match s {
        "true" => return Some(serde_json::Value::Bool(true)),
        "false" => return Some(serde_json::Value::Bool(false)),
        "null" => return Some(serde_json::Value::Null),
        _ => {}
    }
    if let Ok(n) = s.parse::<i64>() {
        return Some(serde_json::json!(n));
    }
    if let Ok(f) = s.parse::<f64>() {
        return Some(serde_json::json!(f));
    }
    Some(serde_json::Value::String(s.to_string()))
}

fn compare_filter(
    lv: Option<&serde_json::Value>,
    rv: Option<&serde_json::Value>,
    op: &str,
) -> bool {
    let (Some(l), Some(r)) = (lv, rv) else {
        return false; // 缺失值比较恒 false（含 !=）
    };
    match op {
        "==" => json_eq(l, r),
        "!=" => !json_eq(l, r),
        "<" | "<=" | ">" | ">=" => match (l.as_f64(), r.as_f64()) {
            (Some(a), Some(b)) => cmp_num(a, b, op),
            _ => match (l.as_str(), r.as_str()) {
                (Some(a), Some(b)) => cmp_str(a, b, op),
                _ => false,
            },
        },
        _ => false,
    }
}

fn json_eq(l: &serde_json::Value, r: &serde_json::Value) -> bool {
    match (l, r) {
        (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
            a.as_f64().unwrap_or(0.0) == b.as_f64().unwrap_or(0.0)
        }
        (serde_json::Value::String(a), serde_json::Value::String(b)) => a == b,
        (serde_json::Value::Bool(a), serde_json::Value::Bool(b)) => a == b,
        (serde_json::Value::Null, serde_json::Value::Null) => true,
        _ => false,
    }
}

fn cmp_num(a: f64, b: f64, op: &str) -> bool {
    match op {
        "<" => a < b,
        "<=" => a <= b,
        ">" => a > b,
        ">=" => a >= b,
        _ => false,
    }
}

fn cmp_str(a: &str, b: &str, op: &str) -> bool {
    match op {
        "<" => a < b,
        "<=" => a <= b,
        ">" => a > b,
        ">=" => a >= b,
        _ => false,
    }
}

/// 顶层切分（引号/括号内不切）
fn split_top_level<'a>(s: &'a str, sep: &str) -> Vec<&'a str> {
    let b = s.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0i32;
    let mut in_s = false;
    let mut in_d = false;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'\'' && !in_d {
            in_s = !in_s;
        } else if c == b'"' && !in_s {
            in_d = !in_d;
        } else if !in_s && !in_d {
            match c {
                b'(' | b'[' => depth += 1,
                b')' | b']' => depth -= 1,
                _ if depth == 0 && s[i..].starts_with(sep) => {
                    parts.push(&s[start..i]);
                    i += sep.len();
                    start = i;
                    continue;
                }
                _ => {}
            }
        }
        i += 1;
    }
    parts.push(&s[start..]);
    parts
}

fn find_op_top(s: &str, op: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut in_s = false;
    let mut in_d = false;
    while i + op.len() <= b.len() {
        let c = b[i];
        if c == b'\'' && !in_d {
            in_s = !in_s;
        } else if c == b'"' && !in_s {
            in_d = !in_d;
        } else if !in_s && !in_d && s[i..].starts_with(op) {
            return Some(i);
        }
        i += 1;
    }
    None
}

// ---------- 组合分隔与 JS 链切分 ----------

/// JS 链段
pub struct JsSeg<'a> {
    pub is_js: bool,
    pub text: &'a str,
}

/// 是否含 JS 标记（<js> 或 @js:，大小写不敏感——对齐 legado JS_PATTERN）
pub fn contains_js_marker(rule: &str) -> bool {
    find_ci(rule, "<js>").is_some() || find_ci(rule, "@js:").is_some()
}

fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let nb = needle.as_bytes();
    haystack
        .as_bytes()
        .windows(nb.len())
        .position(|w| w.eq_ignore_ascii_case(nb))
}

/// 按 JS 标记切段（legado JS_PATTERN：`<js>...</js>` 或 `@js:` 贪婪到末尾，均大小写不敏感）
pub fn split_js_chain(rule: &str) -> Vec<JsSeg<'_>> {
    let mut segs: Vec<JsSeg<'_>> = Vec::new();
    let mut start = 0;
    while start < rule.len() {
        let js_tag = find_ci(&rule[start..], "<js>").map(|p| p + start);
        let js_at = find_ci(&rule[start..], "@js:").map(|p| p + start);
        // <js> 需有闭合 </js> 才成为 JS 段（legado 非贪婪匹配失败则跳过）
        let tag_ok = js_tag.and_then(|p| find_ci(&rule[p + 4..], "</js>").map(|q| (p, p + 4 + q)));
        enum Kind {
            Tag,
            At,
        }
        let cand = match (tag_ok, js_at) {
            (Some((p, _)), Some(q)) if q < p => Some((Kind::At, q, rule.len())),
            (Some((p, end)), _) => Some((Kind::Tag, p, end)),
            (None, Some(q)) => Some((Kind::At, q, rule.len())),
            (None, None) => None,
        };
        match cand {
            None => {
                let t = rule[start..].trim();
                if !t.is_empty() {
                    segs.push(JsSeg {
                        is_js: false,
                        text: t,
                    });
                }
                break;
            }
            Some((kind, seg_start, seg_end)) => {
                let t = rule[start..seg_start].trim();
                if !t.is_empty() {
                    segs.push(JsSeg {
                        is_js: false,
                        text: t,
                    });
                }
                let code = match kind {
                    Kind::Tag => &rule[seg_start + 4..seg_end],
                    Kind::At => &rule[seg_start + 4..],
                };
                let code = code.trim();
                if !code.is_empty() {
                    segs.push(JsSeg {
                        is_js: true,
                        text: code,
                    });
                }
                start = match kind {
                    Kind::Tag => seg_end + 5, // 跳过 </js>（5 字符）
                    Kind::At => rule.len(),
                };
            }
        }
    }
    segs
}

/// 组合分隔切分（对齐 legado RuleAnalyzer.splitRule）：
/// 取最早出现在平衡组（[] / () / {} / 引号）外的 `&&` / `||` / `%%`，
/// 确定分隔符后剩余部分按该分隔符朴素切分（legado 二段切分语义）。
/// 返回 (分隔符, 子规则列表)；无分隔符 → (None, [整条])
pub fn split_combined(rule: &str) -> (Option<&'static str>, Vec<&str>) {
    let b = rule.as_bytes();
    let mut depth_sq = 0i32;
    let mut depth_par = 0i32;
    let mut depth_cur = 0i32;
    let mut in_s = false;
    let mut in_d = false;
    let mut esc = false;
    let mut sep_pos: Option<usize> = None;
    let mut sep_kind: &'static str = "&&";
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if esc {
            esc = false;
            i += 1;
            continue;
        }
        if c == b'\\' {
            esc = true;
            i += 1;
            continue;
        }
        if c == b'\'' && !in_d {
            in_s = !in_s;
            i += 1;
            continue;
        }
        if c == b'"' && !in_s {
            in_d = !in_d;
            i += 1;
            continue;
        }
        if in_s || in_d {
            i += 1;
            continue;
        }
        match c {
            b'[' => depth_sq += 1,
            b']' => depth_sq -= 1,
            b'(' => depth_par += 1,
            b')' => depth_par -= 1,
            b'{' => depth_cur += 1,
            b'}' => depth_cur -= 1,
            _ if depth_sq == 0 && depth_par == 0 && depth_cur == 0 => {
                let rest = &b[i..];
                if rest.starts_with(b"&&") {
                    sep_pos = Some(i);
                    sep_kind = "&&";
                    break;
                }
                if rest.starts_with(b"||") {
                    sep_pos = Some(i);
                    sep_kind = "||";
                    break;
                }
                if rest.starts_with(b"%%") {
                    sep_pos = Some(i);
                    sep_kind = "%%";
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    match sep_pos {
        None => (None, vec![rule]),
        Some(pos) => {
            let mut subs = vec![&rule[..pos]];
            let rest = &rule[pos + sep_kind.len()..];
            let mut start = 0;
            while let Some(p) = rest[start..].find(sep_kind) {
                subs.push(&rest[start..start + p]);
                start += p + sep_kind.len();
            }
            subs.push(&rest[start..]);
            (Some(sep_kind), subs)
        }
    }
}

/// URL 规则：@{url} 或直接替换（v1：原样返回拼接；现无检测路径，保留兼容）
fn url_replace(rule: &Rule, input: &str) -> String {
    let body = rule.body.trim_start_matches('@');
    if body.is_empty() {
        input.to_string()
    } else {
        format!("{body}{input}")
    }
}

/// 应用前缀与替换（legado 语义：前缀拼接 + 正则替换/替换首个）
fn apply_post(results: Vec<String>, rule: &Rule) -> Vec<String> {
    results
        .into_iter()
        .map(|mut s| {
            if let Some(prefix) = &rule.prefix {
                if !s.starts_with(prefix.as_str()) {
                    s = format!("{prefix}{s}");
                }
            }
            if let Some(re) = &rule.replace_regex {
                if !re.is_empty() {
                    let rep = rule.replacement.as_deref().unwrap_or("");
                    s = replace_regex_str(&s, re, rep, rule.replace_first);
                }
            }
            s
        })
        .collect()
}

/// 替换执行（对齐 legado replaceRegex）：
/// - replaceFirst（###）：仅替换首个匹配；无匹配 → 空串；正则编译失败 → 替换串本身
/// - 普通：全部替换；正则编译失败 → 字面串替换
fn replace_regex_str(result: &str, re_str: &str, replacement: &str, first: bool) -> String {
    if first {
        match crate::util::regex::Regex::new(re_str) {
            Ok(re) => {
                if re.is_match(result) {
                    re.replace_first(result, replacement).into_owned()
                } else {
                    String::new()
                }
            }
            Err(_) => replacement.to_string(),
        }
    } else {
        match crate::util::regex::Regex::new(re_str) {
            Ok(re) => re.replace_all(result, replacement).into_owned(),
            Err(_) => result.replace(re_str, replacement),
        }
    }
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
        assert!(r.replace_regex.is_none());
    }

    #[test]
    fn test_parse_legado_flags() {
        assert_eq!(parse_rule("@Json:$.list.name").kind, RuleKind::JsonPath);
        assert_eq!(parse_rule("$.list.name").kind, RuleKind::JsonPath);
        assert_eq!(parse_rule("@XPath://div/a").kind, RuleKind::XPath);
        assert_eq!(parse_rule("//div/a").kind, RuleKind::XPath);
        assert_eq!(parse_rule("@@div.book").kind, RuleKind::Css);
        assert_eq!(parse_rule("a@href").kind, RuleKind::Css);
        // 大小写不敏感标志（legado startsWith(ignoreCase)）
        assert_eq!(parse_rule("@json:$.a").kind, RuleKind::JsonPath);
        assert_eq!(parse_rule("@xpath://a").kind, RuleKind::XPath);
        assert_eq!(parse_rule("@JS:result").kind, RuleKind::Js);
        // 孤立 @ 前缀剥除（链式冗余符号）
        assert_eq!(parse_rule("@class.b@text").kind, RuleKind::Css);
        assert_eq!(parse_rule("@class.b@text").body, "class.b@text");
        // : 前缀 → 正则规则（legado allInOne）
        assert_eq!(parse_rule(":第(.+?)章").kind, RuleKind::Regex);
    }

    #[test]
    fn test_parse_replace_segments() {
        // 三段：替换正则 + 替换串
        let r = parse_rule("a@href##(\\d+)##[$1]");
        assert_eq!(r.replace_regex.as_deref(), Some("(\\d+)"));
        assert_eq!(r.replacement.as_deref(), Some("[$1]"));
        assert!(!r.replace_first);
        // 两段：替换正则，替换串为空（删除匹配）
        let r2 = parse_rule("a@href##\\s+");
        assert_eq!(r2.replace_regex.as_deref(), Some("\\s+"));
        assert!(r2.replacement.is_none());
        // 四段（###）：replaceFirst
        let r3 = parse_rule("##(第.章)##[$1]###");
        assert!(r3.replace_first);
        assert_eq!(r3.body, "");
        assert_eq!(r3.replacement.as_deref(), Some("[$1]"));
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

    /// GAP 153：规则正则支持 lookbehind（fancy-regex 升级路径）
    #[test]
    fn test_regex_lookbehind() {
        let html = "书名：测试书 作者：张三";
        // 规则主体 lookbehind
        let r = apply("(?<=书名：)\\S+", html);
        assert_eq!(r, vec!["测试书".to_string()]);
        // 替换规则（## 第三段）lookbehind：旧值 (?<=：)(.+?)$ 命中 "测试书"
        let r = apply("(.+?)$##(?<=：)(.+?)$##[$1]", "书名：测试书");
        assert_eq!(r, vec!["书名：[测试书]".to_string()]);
    }

    #[test]
    fn test_regex_case_flag() {
        // 大小写标志：内联 (?i)
        let r = apply("(?i)ABC", "xx abc yy");
        assert_eq!(r, vec!["abc".to_string()]);
        // : 前缀正则规则
        let r2 = apply(":第(.+?)章", "第一章 第二章");
        assert_eq!(r2, vec!["一".to_string(), "二".to_string()]);
    }

    #[test]
    fn test_regex_chain_replace() {
        // ##pat##rep## 多段替换
        let r = apply("##(第.章)##[$1]", "第一章 第二章");
        assert_eq!(r, vec!["[第一章] [第二章]".to_string()]);
        // 两段：删除匹配
        let r2 = apply("##\\s+", "a b  c");
        assert_eq!(r2, vec!["abc".to_string()]);
        // ### replaceFirst：仅替换首个；无匹配 → 空串
        let r3 = apply("##(第.章)##[$1]###", "第一章 第二章");
        assert_eq!(r3, vec!["[第一章] 第二章".to_string()]);
        let r4 = apply("##(第.章)##[$1]###", "无匹配文本");
        assert_eq!(r4, vec![String::new()]);
        // 主规则 + 替换链（书名去书名号）
        let html = r#"<h2 class="t">《测试书》</h2>"#;
        let r5 = apply("class.t@text##《(.*)》##$1", html);
        assert_eq!(r5, vec!["测试书".to_string()]);
    }

    #[test]
    fn test_json_path() {
        let json = r#"{"data":{"list":[{"name":"书1"},{"name":"书2"}]}}"#;
        let r = apply("{$.data.list.name}", json);
        assert_eq!(r, vec!["书1".to_string(), "书2".to_string()]);
    }

    #[test]
    fn test_json_path_recursive() {
        let json = r#"{"a":{"content":"深1","b":{"content":"深2"}},"content":"浅"}"#;
        // $..content 任意深度（DFS 前序：父键先出）
        let r = apply("$..content", json);
        assert_eq!(
            r,
            vec!["浅".to_string(), "深1".to_string(), "深2".to_string()]
        );
    }

    #[test]
    fn test_json_path_indexes() {
        let json = r#"{"data":["a","b","c","d"]}"#;
        assert_eq!(apply("$.data[0]", json), vec!["a".to_string()]);
        assert_eq!(apply("$.data[-1]", json), vec!["d".to_string()]);
        assert_eq!(
            apply("$.data[1:3]", json),
            vec!["b".to_string(), "c".to_string()]
        );
        assert_eq!(
            apply("$.data[0,2]", json),
            vec!["a".to_string(), "c".to_string()]
        );
        assert_eq!(
            apply("$.data[*]", json),
            vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string()
            ]
        );
        // 对象通配
        let obj = r#"{"x":1,"y":2}"#;
        assert_eq!(apply("$.*", obj), vec!["1".to_string(), "2".to_string()]);
        // 引号键
        let obj2 = r#"{"a b":{"c":7}}"#;
        assert_eq!(apply("$['a b'].c", obj2), vec!["7".to_string()]);
    }

    #[test]
    fn test_json_path_filters() {
        let json = r#"{"list":[{"name":"书1","grade":5,"volume":false},{"name":"书2","grade":1},{"name":"书3","grade":8,"volume":true}]}"#;
        // 存在性过滤（真实书源：[?(@.bookName)]）
        let r = apply("$.list[?(@.name)]", json);
        assert_eq!(r.len(), 3);
        // 数值比较（真实书源：[?(@.grade > 1)]）
        let r2 = apply("$.list[?(@.grade > 1)].name", json);
        assert_eq!(r2, vec!["书1".to_string(), "书3".to_string()]);
        // 布尔比较（真实书源：[?(@.volume == false)]）
        let r3 = apply("$.list[?(@.volume == false)].name", json);
        assert_eq!(r3, vec!["书1".to_string()]);
        // 字符串比较 + && 组合
        let r4 = apply("$.list[?(@.grade > 1 && @.name == '书3')].name", json);
        assert_eq!(r4, vec!["书3".to_string()]);
        // 过滤后对象 JSON 化（bookList 场景）
        let r5 = apply("$.list[?(@.grade > 5)]", json);
        assert_eq!(r5.len(), 1);
        assert!(r5[0].contains("\"name\":\"书3\""));
        // || 组合规则：首个命中
        let r6 = apply("$.list[?(@.grade > 5)].name||$.list[0].name", json);
        assert_eq!(r6, vec!["书3".to_string()]);
        let r7 = apply("$.missing||$.list[0].name", json);
        assert_eq!(r7, vec!["书1".to_string()]);
    }

    #[test]
    fn test_json_path_html_embedded() {
        // HTML 内嵌 JSON 行（json_from_html 回退）
        let html = "前文\n{\"data\":{\"name\":\"内嵌\"}}\n后文";
        let r = apply("$.data.name", html);
        assert_eq!(r, vec!["内嵌".to_string()]);
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
    fn test_js_chain() {
        let html = r#"<div class="b">abc123</div>"#;
        // <js> 链：规则结果进 JS（result 变量），再进后续规则
        let r = apply("<js>result.toUpperCase()</js>", html);
        assert_eq!(r, vec![html.to_uppercase()]);
        // CSS → JS → 结果
        let r2 = apply("class.b@text@js:result.replace('abc','xyz')", html);
        assert_eq!(r2, vec!["xyz123".to_string()]);
        // <js> 在前 → JS → CSS 链（真实书源：<js>...</js>$.data[*] 形态）
        let r3 = apply("<js>result.replace('abc','xyz')</js>@class.b@text", html);
        assert_eq!(r3, vec!["xyz123".to_string()]);
        // JS 失败 → 整链空
        assert!(apply("class.b@text@js:throw new Error('x')", html).is_empty());
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
    fn test_inline_rule_ref_substitution() {
        // {{@@rule}} 规则引用（真实书源：{{@@[name$=update_time]@content##T##🔸}}）
        let html =
            r#"<meta name="update_time" content="2024-01-01"><div class="card"><p>正文</p></div>"#;
        let r = apply("更新时间：{{@@[name$=update_time]@content##-##/}}", html);
        assert_eq!(r, vec!["更新时间：2024/01/01".to_string()]);
        let r2 = apply("{{@@.card@p@text}}", html);
        assert_eq!(r2, vec!["正文".to_string()]);
        // {{//xpath}} 规则引用（真实书源：{{//data[@name='Title']/text()}}）
        let xml = r#"<data><item name="Title">测试书</item></data>"#;
        let r3 = apply("书名：{{//data/item[@name='Title']/text()}}", xml);
        assert_eq!(r3, vec!["书名：测试书".to_string()]);
        // {{$.x}} 多结果换行拼接
        assert_eq!(
            expand_inline(
                "{{$.list.name}}",
                r#"{"list":[{"name":"书1"},{"name":"书2"}]}"#
            ),
            "书1\n书2"
        );
    }

    #[test]
    fn test_expand_inline() {
        // 数组下标形式 {{$.a[0]}}
        assert_eq!(
            expand_inline("{{$.list[0]}}", r#"{"list":["书1","书2"]}"#),
            "书1"
        );
        // 上下文非完整 JSON → 逐行提取 JSON 片段（json_from_html 回退）
        assert_eq!(
            expand_inline(
                "{{$.data.name}}",
                "前文\n{\"data\":{\"name\":\"内嵌\"}}\n后文"
            ),
            "内嵌"
        );
        // 未闭合 {{ 原样返回
        assert_eq!(expand_inline("{{div.book", "<html></html>"), "{{div.book");
    }

    #[test]
    fn test_split_combined_basic() {
        let (sep, subs) = split_combined("a&&b&&c");
        assert_eq!(sep, Some("&&"));
        assert_eq!(subs, vec!["a", "b", "c"]);
        let (sep, subs) = split_combined("a||b");
        assert_eq!(sep, Some("||"));
        assert_eq!(subs, vec!["a", "b"]);
        let (sep, _subs) = split_combined("a%%b");
        assert_eq!(sep, Some("%%"));
        // 无分隔符
        let (sep, subs) = split_combined("a.b@c");
        assert_eq!(sep, None);
        assert_eq!(subs, vec!["a.b@c"]);
        // 平衡组内分隔符不参与（属性选择器 / 引号 / {{}} / 过滤表达式）
        let (sep, subs) = split_combined("a[href='x&&y']@b||c");
        assert_eq!(sep, Some("||"));
        assert_eq!(subs, vec!["a[href='x&&y']@b", "c"]);
        let (sep, _subs) = split_combined("[?(@.a == 'x' && @.b)]");
        assert_eq!(sep, None);
        let (sep, subs) = split_combined("{{a&&b}}&&c");
        assert_eq!(sep, Some("&&"));
        assert_eq!(subs, vec!["{{a&&b}}", "c"]);
    }

    #[test]
    fn test_split_js_chain() {
        let segs = split_js_chain("a@href<js>code1</js>b@text@js:code2");
        let kinds: Vec<bool> = segs.iter().map(|s| s.is_js).collect();
        let texts: Vec<&str> = segs.iter().map(|s| s.text).collect();
        assert_eq!(kinds, vec![false, true, false, true]);
        assert_eq!(texts, vec!["a@href", "code1", "b@text", "code2"]);
        // 大小写不敏感
        let segs2 = split_js_chain("<JS>x</JS>");
        assert_eq!(segs2.len(), 1);
        assert!(segs2[0].is_js);
        assert_eq!(segs2[0].text, "x");
        // 未闭合 <js> → 普通段
        let segs3 = split_js_chain("<js>unclosed");
        assert_eq!(segs3.len(), 1);
        assert!(!segs3[0].is_js);
        // @js: 贪婪到末尾（后续 @js: 并入首个代码）
        let segs4 = split_js_chain("x@js:a@js:b");
        assert_eq!(segs4.len(), 2);
        assert_eq!(segs4[1].text, "a@js:b");
        // 无标记 → 单普通段
        let segs5 = split_js_chain("class.a@text");
        assert_eq!(segs5.len(), 1);
        assert!(!segs5[0].is_js);
    }

    #[test]
    fn test_xpath_rules() {
        let xml = r#"<?xml version="1.0"?>
<library>
  <book id="1"><title>三体</title></book>
  <book id="2"><title>流浪地球</title></book>
</library>"#;
        assert_eq!(
            apply("//book/title", xml),
            vec!["三体".to_string(), "流浪地球".to_string()]
        );
        // && 组合
        let r = apply("//book[1]/title&&//book[2]/title", xml);
        assert_eq!(r, vec!["三体".to_string(), "流浪地球".to_string()]);
        // || 首个命中（legado：同一条规则只按最早出现的分隔符切分——|| 单独使用）
        let r2 = apply("//nonexistent||//book[1]/title", xml);
        assert_eq!(r2, vec!["三体".to_string()]);
    }

    #[test]
    fn test_url_kind_kept() {
        // Url 变体保留（无检测路径——孤立 @ 现按 CSS/正则处理）
        let r = parse_rule("@class.a");
        assert_eq!(r.kind, RuleKind::Css);
        assert_eq!(r.body, "class.a");
    }
}
