//! legado 链式 CSS（对齐 AnalyzeByJSoup getElementsSingle / getResultLast / RuleAnalyzer）
//!
//! 支持：
//! - `<js>...</js>` / `@js:` 链（规则结果进 JS——result 变量；委托 rule::apply）
//! - 组合分隔：`&&` 多规则合并 / `||` 首个命中即止 / `%%` 按位交错（对齐 RuleAnalyzer splitRule）
//! - `@` 链式步进：选择器 / children / parent / 索引；末段属性/文本提取
//! - 索引：`tag.p.2` / `tag.p!0`（! = 排除）/ 负数 / `tag.p[1:3]` 区间 / `[!0,2]` 排除集 /
//!   `[-1:0]` 反向（对齐 ElementsSingle.findIndexSet）
//! - 简写：`class.x` / `id.x` / `tag.x` / `text.x`（ownText 包含）/ `children`
//! - 属性选择器 [a=b]/[a$=b]/[a^=b]/[a*=b]（scraper 原生）；[a~=regex]（jsoup 语义=正则匹配）
//! - 末段提取：text/textNodes/ownText/html（去 script/style）/all/href/src/content/任意属性
//! - 单段规则 CSS 解析失败 → 正则回退（legacy 兼容；GAP 153：lookbehind 经 fancy-regex）

use scraper::{ElementRef, Html, Selector};

use crate::parser::rule;

/// 链式 CSS 入口。含 `<js>`/`@js:` 标记时委托规则引擎（类型逐段识别）；
/// 否则走纯 CSS 链。
pub fn css_chain(rule: &str, html: &str) -> Vec<String> {
    if rule::contains_js_marker(rule) {
        return rule::apply(rule, html);
    }
    css_chain_plain(rule, html)
}

/// 纯 CSS 链（无 JS 段）：`##` 尾段剥离 + `&&`/`||`/`%%` 组合
fn css_chain_plain(rule: &str, html: &str) -> Vec<String> {
    let main = rule.split("##").next().unwrap_or(rule).trim();
    if main.is_empty() {
        return vec![];
    }
    let (sep, subs) = rule::split_combined(main);
    let mut groups: Vec<Vec<String>> = Vec::new();
    for sub in subs {
        let r = css_chain_single(sub.trim(), html);
        if !r.is_empty() {
            groups.push(r);
            if sep == Some("||") {
                break;
            }
        }
    }
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

/// 单条 CSS 链：`@` 切分；末段为提取器 → 提取；否则全为选择器步进 → 返回元素 HTML
fn css_chain_single(rule: &str, doc_html: &str) -> Vec<String> {
    let doc = Html::parse_document(doc_html);
    let parts: Vec<&str> = rule
        .split('@')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        return vec![];
    }
    // current 为空 = 整个文档（对齐 legado：elements 初始为 Document 本身）
    let mut current: Vec<ElementRef> = Vec::new();
    let last = parts.len() - 1;
    let mut selector_parse_failed = false;
    for (i, part) in parts.iter().enumerate() {
        if i == last && is_attr_extractor(part) {
            return extract_attr(&doc, &current, part);
        }
        let next = step_elements(&doc, &current, part, &mut selector_parse_failed);
        if next.is_empty() {
            // 单段规则：CSS 选择器无法解析（如 legacy 正则规则）→ 正则回退
            if parts.len() == 1 && selector_parse_failed {
                if let Ok(re) = crate::util::regex::Regex::new(&selector_part(part)) {
                    let r: Vec<String> = re
                        .captures_iter(doc_html)
                        .map(|c| {
                            c.get(1)
                                .or_else(|| c.get(0))
                                .map(|m| m.as_str().trim().to_string())
                                .unwrap_or_default()
                        })
                        .filter(|s| !s.is_empty())
                        .collect();
                    if !r.is_empty() {
                        return r;
                    }
                }
            }
            return vec![];
        }
        current = next;
    }
    // 全为选择器（无末段提取）：返回元素 HTML（bookList 场景）
    current.iter().map(|e| e.html()).collect()
}

/// 单段步进（对齐 legado getElementsSingle + findIndexSet）
/// 索引作用于每个当前元素的独立结果集（legado 逐元素语义）
fn step_elements<'a>(
    doc: &'a Html,
    current: &[ElementRef<'a>],
    part: &str,
    selector_parse_failed: &mut bool,
) -> Vec<ElementRef<'a>> {
    let (before_rule, exclude, indices) = parse_index_spec(part);
    let mut out: Vec<ElementRef<'a>> = Vec::new();
    if current.is_empty() {
        out.extend(step_one(
            doc,
            None,
            &before_rule,
            exclude,
            &indices,
            selector_parse_failed,
        ));
    } else {
        for e in current {
            out.extend(step_one(
                doc,
                Some(*e),
                &before_rule,
                exclude,
                &indices,
                selector_parse_failed,
            ));
        }
    }
    out
}

/// 单元素步进：选择 + 索引过滤（beforeRule 关键词分发，对齐 legado）
fn step_one<'a>(
    doc: &'a Html,
    root: Option<ElementRef<'a>>,
    before_rule: &str,
    exclude: bool,
    indices: &[IndexSpec],
    selector_parse_failed: &mut bool,
) -> Vec<ElementRef<'a>> {
    // beforeRule 关键词分发（对齐 legado：children/class/tag/id/text；parent 为扩展）
    let mut els: Vec<ElementRef<'a>> = if before_rule.is_empty() {
        // 索引直接作用于子元素（允许索引作为根）
        children_one(doc, root)
    } else {
        let rules: Vec<&str> = before_rule.split('.').collect();
        match rules[0] {
            "children" => children_one(doc, root),
            "class" if rules.len() > 1 => {
                let name = rules[1];
                if let Some(sel) = parse_selector(
                    &format!(".{}", css_escape_ident(name)),
                    selector_parse_failed,
                ) {
                    select_one(doc, root, &sel)
                } else {
                    // CSS 类名转义失败（罕见）→ 按 class 属性 token 过滤
                    select_all_one(doc, root)
                        .into_iter()
                        .filter(|e| e.value().classes().any(|c| c == name))
                        .collect()
                }
            }
            "id" if rules.len() > 1 => {
                let name = rules[1];
                if let Some(sel) = parse_selector(
                    &format!("#{}", css_escape_ident(name)),
                    selector_parse_failed,
                ) {
                    select_one(doc, root, &sel)
                } else {
                    select_all_one(doc, root)
                        .into_iter()
                        .filter(|e| e.value().attr("id") == Some(name))
                        .collect()
                }
            }
            "tag" if rules.len() > 1 => {
                let name = rules[1];
                if let Some(sel) = parse_selector(name, selector_parse_failed) {
                    select_one(doc, root, &sel)
                } else {
                    select_all_one(doc, root)
                        .into_iter()
                        .filter(|e| e.value().name() == name)
                        .collect()
                }
            }
            // text.x → ownText 包含 x（jsoup getElementsContainingOwnText）
            "text" if rules.len() > 1 => select_all_one(doc, root)
                .into_iter()
                .filter(|e| own_text(e).contains(rules[1]))
                .collect(),
            // parent（扩展：取当前元素的父元素）
            "parent" if rules.len() == 1 => match root {
                Some(e) => e.parent().and_then(ElementRef::wrap).into_iter().collect(),
                None => vec![],
            },
            _ => {
                // jsoup ~= 是正则匹配语义（CSS 是单词匹配）——预提取 [attr~=regex] 后置过滤
                let (base, regex_filters) = extract_tilde_attr(before_rule);
                let base = normalize_selector(&base);
                if regex_filters.is_empty() {
                    match parse_selector(&base, selector_parse_failed) {
                        Some(sel) => select_one(doc, root, &sel),
                        None => vec![],
                    }
                } else {
                    let sel = match parse_selector(&base, selector_parse_failed) {
                        Some(s) => s,
                        None => return vec![],
                    };
                    let els = select_one(doc, root, &sel);
                    els.into_iter()
                        .filter(|e| {
                            regex_filters.iter().all(|(attr, re)| {
                                e.attr(attr).map(|v| re.is_match(v)).unwrap_or(false)
                            })
                        })
                        .collect()
                }
            }
        }
    };
    // 索引过滤（对齐 legado：'!' 排除 / '.' 选择 / 无索引原样返回）
    if !indices.is_empty() {
        let set = resolve_index_set(els.len(), indices);
        if exclude {
            let mut kept: Vec<ElementRef<'a>> = Vec::with_capacity(els.len());
            for (i, e) in els.drain(..).enumerate() {
                if !set.contains(&i) {
                    kept.push(e);
                }
            }
            kept
        } else {
            set.iter().filter_map(|&i| els.get(i).copied()).collect()
        }
    } else {
        els
    }
}

/// 索引规格（对齐 legado ElementsSingle.findIndexSet）
#[derive(Debug, Clone)]
enum IndexSpec {
    One(i64),
    Range(Option<i64>, Option<i64>, i64),
}

/// 解析 `part` 为 (beforeRule, 排除?, 索引列表)
/// - legacy：`tag.p.2` / `tag.p!0:1` / `tag.p.1:3`（: 分隔多索引）
/// - 括号：`tag.p[1:3]` / `tag.p[!0,2]` / `tag.p[-1:0]` / `tag.p[0]`
fn parse_index_spec(part: &str) -> (String, bool, Vec<IndexSpec>) {
    let rus = part.trim();
    if rus.is_empty() {
        return (String::new(), false, vec![]);
    }
    let bytes = rus.as_bytes();
    let mut indices: Vec<IndexSpec> = Vec::new();
    let mut cur_list: Vec<i64> = Vec::new(); // 当前区间右端/间隔
    let mut l = String::new();
    let mut cur_minus = false;

    let push_number = |l: &mut String, cur_minus: &mut bool, _cur_list: &mut Vec<i64>| {
        let n: i64 = if l.is_empty() {
            0
        } else {
            l.parse().unwrap_or(0)
        };
        let n = if *cur_minus { -n } else { n };
        *l = String::new();
        *cur_minus = false;
        n
    };

    if rus.ends_with(']') {
        // 常规索引写法 [index...]，逆向遍历（可无前置规则）
        let mut pos = rus.len().saturating_sub(2); // 指向 ']' 前一位
        let mut split_exclude = false;
        let mut before_rule: Option<String> = None;
        let mut bracket_indices: Vec<IndexSpec> = Vec::new();
        loop {
            let rl = bytes[pos];
            if rl == b' ' {
                // 跳过空格
            } else if rl.is_ascii_digit() {
                l.insert(0, rl as char);
            } else if rl == b'-' {
                cur_minus = true;
            } else {
                let cur_int: Option<i64> = if l.is_empty() {
                    None
                } else {
                    Some(if cur_minus {
                        -l.parse::<i64>().unwrap_or(0)
                    } else {
                        l.parse().unwrap_or(0)
                    })
                };
                l.clear();
                cur_minus = false;
                match rl {
                    b':' => {
                        cur_list.push(cur_int.unwrap_or(0));
                    }
                    _ => {
                        // 区间或单个索引加入集合
                        if cur_list.is_empty() {
                            match cur_int {
                                None => break, // 是 jsoup 选择器而非索引列表，跳出
                                Some(n) => bracket_indices.push(IndexSpec::One(n)),
                            }
                        } else {
                            let end = cur_list.pop().unwrap_or(0);
                            let step = if cur_list.is_empty() {
                                1
                            } else {
                                cur_list.pop().unwrap_or(1)
                            };
                            cur_list.clear();
                            bracket_indices.push(IndexSpec::Range(cur_int, Some(end), step));
                        }
                        if rl == b'!' {
                            split_exclude = true;
                            // 对齐 legado：读取 '!' 前的字符（跳过空格）判断结构
                            let mut prev = pos.saturating_sub(1);
                            while prev > 0 && bytes[prev] == b' ' {
                                prev -= 1;
                            }
                            if bytes[prev] == b'[' {
                                before_rule = Some(rus[..prev].to_string());
                                break;
                            }
                            if bytes[prev] != b',' {
                                break; // 非索引结构，跳出
                            }
                        }
                        if rl == b'[' {
                            before_rule = Some(rus[..pos].to_string());
                            break;
                        }
                        if rl != b',' {
                            break; // 非索引结构，跳出
                        }
                    }
                }
            }
            if pos == 0 {
                break;
            }
            pos -= 1;
        }
        match before_rule {
            Some(br) => {
                // 逆向扫描压入，需反转恢复书写顺序（对齐 legado 双重反转）
                indices.extend(bracket_indices.into_iter().rev());
                (br, split_exclude, indices)
            }
            None => {
                // 未闭合索引结构（如 a[href] 属性选择器）→ 整体作为选择器
                (rus.to_string(), false, vec![])
            }
        }
    } else {
        // 阅读原本写法，逆向遍历（可无前置规则）
        let mut len = rus.len();
        let mut split_exclude = false;
        let mut before_rule: Option<String> = None;
        let mut legacy_indices: Vec<i64> = Vec::new();
        loop {
            if len == 0 || len > bytes.len() {
                break;
            }
            len -= 1;
            let rl = bytes[len];
            if rl == b' ' {
                continue;
            }
            if rl.is_ascii_digit() {
                l.insert(0, rl as char);
            } else if rl == b'-' {
                cur_minus = true;
            } else if rl == b'!' || rl == b'.' || rl == b':' {
                let n = push_number(&mut l, &mut cur_minus, &mut cur_list);
                legacy_indices.push(n);
                if rl != b':' {
                    split_exclude = rl == b'!';
                    before_rule = Some(rus[..len].to_string());
                    break;
                }
            } else {
                break; // 非索引结构，跳出
            }
        }
        match before_rule {
            Some(br) => {
                // 逆向扫描压入，需反转恢复升序（对齐 legado indexDefault 双重反转）
                indices.extend(legacy_indices.into_iter().rev().map(IndexSpec::One));
                (br, split_exclude, indices)
            }
            None => (rus.to_string(), false, vec![]),
        }
    }
}

/// 解析索引集合并按 legado 语义展开（负数回绕 / 区间 / 反向 / 去重保持书写顺序）
fn resolve_index_set(len: usize, indices: &[IndexSpec]) -> Vec<usize> {
    let mut set: Vec<usize> = Vec::new();
    let push = |set: &mut Vec<usize>, i: usize| {
        if !set.contains(&i) {
            set.push(i);
        }
    };
    let len_i = len as i64;
    for spec in indices {
        match spec {
            IndexSpec::One(it) => {
                if *it >= 0 {
                    if *it < len_i {
                        push(&mut set, *it as usize);
                    }
                } else if len_i >= -*it {
                    push(&mut set, (*it + len_i) as usize);
                }
            }
            IndexSpec::Range(start_x, end_x, step_x) => {
                let mut start = start_x.unwrap_or(0);
                if start < 0 {
                    start += len_i;
                }
                let mut end = end_x.unwrap_or(len_i - 1);
                if end < 0 {
                    end += len_i;
                }
                if (start < 0 && end < 0) || (start >= len_i && end >= len_i) {
                    continue; // start 和 end 同侧越界，无效区间
                }
                if start >= len_i {
                    start = len_i - 1;
                } else if start < 0 {
                    start = 0;
                }
                if end >= len_i {
                    end = len_i - 1;
                } else if end < 0 {
                    end = 0;
                }
                if start == end || *step_x >= len_i {
                    push(&mut set, start as usize);
                    continue;
                }
                let step = if *step_x > 0 {
                    *step_x
                } else if -*step_x < len_i {
                    *step_x + len_i
                } else {
                    1
                };
                if end > start {
                    let mut i = start;
                    while i <= end {
                        push(&mut set, i as usize);
                        i += step;
                    }
                } else {
                    let mut i = start;
                    while i >= end {
                        push(&mut set, i as usize);
                        i -= step;
                    }
                }
            }
        }
    }
    set
}

/// 单元素/文档选择
fn select_one<'a>(
    doc: &'a Html,
    root: Option<ElementRef<'a>>,
    sel: &Selector,
) -> Vec<ElementRef<'a>> {
    match root {
        Some(e) => e.select(sel).collect(),
        None => doc.select(sel).collect(),
    }
}

/// 单元素/文档全部后代（用于关键字过滤）
fn select_all_one<'a>(doc: &'a Html, root: Option<ElementRef<'a>>) -> Vec<ElementRef<'a>> {
    match root {
        Some(e) => e.descendent_elements().collect(),
        None => doc.root_element().descendent_elements().collect(),
    }
}

/// children 步进：无上下文 → 文档根元素（对齐 legado Document.children() = [html]）
fn children_one<'a>(doc: &'a Html, root: Option<ElementRef<'a>>) -> Vec<ElementRef<'a>> {
    match root {
        Some(e) => e.child_elements().collect(),
        None => vec![doc.root_element()],
    }
}

/// 解析 CSS 选择器；失败置位（单段规则触发正则回退）
fn parse_selector(sel: &str, selector_parse_failed: &mut bool) -> Option<Selector> {
    match Selector::parse(sel) {
        Ok(s) => Some(s),
        Err(_) => {
            *selector_parse_failed = true;
            None
        }
    }
}

/// 末段是否为属性/文本提取器（legado getResultLast 关键字 + 常见属性名）
fn is_attr_extractor(part: &str) -> bool {
    let p = part.to_ascii_lowercase();
    matches!(
        p.as_str(),
        "text"
            | "textnodes"
            | "owntext"
            | "html"
            | "all"
            | "outerhtml"
            | "href"
            | "src"
            | "content"
            | "value"
            | "title"
            | "alt"
            | "name"
            | "id"
            | "class"
            | "data-src"
            | "data-original"
            | "data-url"
            | "data-id"
            | "data-bookid"
            | "data-book"
            | "data-name"
            | "data-href"
            | "data-type"
            | "data-value"
            | "data-title"
            | "data-index"
            | "data-num"
    ) || p.starts_with("data-")
        || p.starts_with("aria-")
}

/// legado 简写转换：class.active → .active；tag.div → div；id.xxx → #xxx
/// （含 CSS 标识符转义——类名可能含特殊字符）
fn normalize_selector(sel: &str) -> String {
    if let Some(c) = sel.strip_prefix("class.") {
        format!(".{}", css_escape_ident(c))
    } else if let Some(c) = sel.strip_prefix("id.") {
        format!("#{}", css_escape_ident(c))
    } else if let Some(c) = sel.strip_prefix("tag.") {
        c.to_string()
    } else {
        sel.to_string()
    }
}

/// 选择器部分的原始文本（正则回退用——去掉索引后缀）
fn selector_part(part: &str) -> String {
    let (before, _, _) = parse_index_spec(part);
    normalize_selector(&before)
}

/// CSS 标识符转义：非 [a-zA-Z0-9_-] 字符 → \XX（码点十六进制）转义
fn css_escape_ident(s: &str) -> String {
    if s.is_empty() {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    for (i, c) in s.chars().enumerate() {
        let first = i == 0;
        let ok =
            c.is_ascii_alphanumeric() || c == '-' || c == '_' || (!first && c.is_ascii_digit());
        if ok {
            out.push(c);
        } else {
            out.push('\\');
            out.push_str(&format!("{:X} ", c as u32));
        }
    }
    out
}

/// 提取 `[attr~=regex]`（jsoup 语义：正则匹配，大小写不敏感）→ (基础选择器, 过滤器)
fn extract_tilde_attr(sel: &str) -> (String, Vec<(String, crate::util::regex::Regex)>) {
    let bytes = sel.as_bytes();
    let mut base = String::new();
    let mut filters: Vec<(String, crate::util::regex::Regex)> = Vec::new();
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\'' && !in_double {
            in_single = !in_single;
            base.push(c as char);
            i += 1;
            continue;
        }
        if c == b'"' && !in_single {
            in_double = !in_double;
            base.push(c as char);
            i += 1;
            continue;
        }
        if c == b'[' && !in_single && !in_double {
            // 找到匹配的 ]
            let mut j = i + 1;
            let mut qs = false;
            let mut qd = false;
            while j < bytes.len() {
                let d = bytes[j];
                if d == b'\'' && !qd {
                    qs = !qs;
                } else if d == b'"' && !qs {
                    qd = !qd;
                } else if d == b']' && !qs && !qd {
                    break;
                }
                j += 1;
            }
            if j >= bytes.len() {
                // 未闭合——原样保留
                base.push_str(&sel[i..]);
                break;
            }
            let inner = &sel[i + 1..j];
            if let Some(tilde_pos) = find_attr_op(inner, "~=") {
                let attr = inner[..tilde_pos].trim();
                let val = inner[tilde_pos + 2..].trim();
                let val = val.trim_matches(|c| c == '\'' || c == '"');
                if !attr.is_empty() && !val.is_empty() {
                    if let Ok(re) = crate::util::regex::RegexBuilder::new(val)
                        .case_insensitive(true)
                        .build()
                    {
                        filters.push((attr.to_string(), re));
                        i = j + 1;
                        continue;
                    }
                }
            }
            base.push_str(&sel[i..=j]);
            i = j + 1;
            continue;
        }
        base.push(c as char);
        i += 1;
    }
    (base, filters)
}

/// 在属性选择器内容中查找操作符位置（跳过引号内）
fn find_attr_op(inner: &str, op: &str) -> Option<usize> {
    let bytes = inner.as_bytes();
    let mut i = 0;
    let mut qs = false;
    let mut qd = false;
    while i + op.len() <= bytes.len() {
        let c = bytes[i];
        if c == b'\'' && !qd {
            qs = !qs;
        } else if c == b'"' && !qs {
            qd = !qd;
        } else if !qs && !qd && inner[i..].starts_with(op) {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// 单元素直接文本（ownText 语义：直接文本节点，空白折叠 + 首尾修剪）
fn own_text(el: &ElementRef) -> String {
    let mut s = String::new();
    for child in el.children() {
        if let scraper::node::Node::Text(txt) = child.value() {
            s.push_str(&txt.text);
        }
    }
    collapse_ws(&s)
}

/// 空白折叠（jsoup normaliseWhitespace：连续空白 → 单空格 + 首尾修剪）
fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_ws = true;
    for c in s.chars() {
        if c.is_whitespace() {
            if !last_ws {
                out.push(' ');
            }
            last_ws = true;
        } else {
            out.push(c);
            last_ws = false;
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

/// 属性/文本提取（对齐 legado getResultLast）
fn extract_attr<'a>(doc: &'a Html, current: &[ElementRef<'a>], attr: &str) -> Vec<String> {
    let attr_l = attr.to_ascii_lowercase();
    let mut out: Vec<String> = Vec::new();
    // 无上下文 → 根元素（legado：Document 本身）
    let items: Vec<ElementRef<'a>> = if current.is_empty() {
        vec![doc.root_element()]
    } else {
        current.to_vec()
    };
    for el in items {
        match attr_l.as_str() {
            "text" => {
                let t = collapse_ws(&el.text().collect::<String>());
                if !t.is_empty() {
                    out.push(t);
                }
            }
            "textnodes" => {
                let tn: Vec<String> = el
                    .children()
                    .filter_map(|n| match n.value() {
                        scraper::node::Node::Text(txt) => {
                            let t = txt.text.trim().to_string();
                            if t.is_empty() {
                                None
                            } else {
                                Some(t)
                            }
                        }
                        _ => None,
                    })
                    .collect();
                if !tn.is_empty() {
                    out.push(tn.join("\n"));
                }
            }
            "owntext" => {
                let t = own_text(&el);
                if !t.is_empty() {
                    out.push(t);
                }
            }
            "html" => {
                // legado：先移除 script/style 再取 outerHTML
                let h = html_without_scripts(&el);
                if !h.is_empty() {
                    out.push(h);
                }
            }
            "all" | "outerhtml" => {
                let h = el.html();
                if !h.is_empty() {
                    out.push(h);
                }
            }
            _ => {
                // 属性（href/src/content/data-*…）：去空白 + 去重（legado attr 分支）
                if let Some(v) = el.value().attr(&attr_l) {
                    let v = v.trim().to_string();
                    if !v.is_empty() && !out.contains(&v) {
                        out.push(v);
                    }
                }
            }
        }
    }
    out
}

/// 元素 outerHTML，但移除内部 script/style（legado @html 语义）
fn html_without_scripts(el: &ElementRef) -> String {
    let mut frag = Html::parse_fragment(&el.html());
    let sel = match Selector::parse("script, style") {
        Ok(s) => s,
        Err(_) => return el.html(),
    };
    let ids: Vec<_> = frag.select(&sel).map(|e| e.id()).collect();
    for id in ids {
        if let Some(mut n) = frag.tree.get_mut(id) {
            n.detach();
        }
    }
    let h = frag.html();
    // parse_fragment 包裹的 html/body 外壳剥除
    let h = h.strip_prefix("<html><body>").unwrap_or(&h);
    h.strip_suffix("</body></html>").unwrap_or(h).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chain_booklist() {
        let html = r#"<ul class="ItemListbody"><li><a href="/b/1">书1</a></li><li><a href="/b/2">书2</a></li></ul>"#;
        let r = css_chain("ul.ItemListbody@li", html);
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn test_chain_field_text() {
        let html = r#"<li><span>书名</span><dd>作者</dd></li>"#;
        let r = css_chain("li@span@text", html);
        assert_eq!(r, vec!["书名".to_string()]);
    }

    #[test]
    fn test_chain_index() {
        let html = r#"<li><dd>甲</dd><dd>乙</dd></li>"#;
        let r = css_chain("li@dd.1@text", html);
        assert_eq!(r, vec!["乙".to_string()]);
    }

    #[test]
    fn test_chain_href() {
        let html = r#"<li><a href="/book/9">x</a></li>"#;
        let r = css_chain("li@a@href", html);
        assert_eq!(r, vec!["/book/9".to_string()]);
    }

    #[test]
    fn test_chain_and_rules() {
        let html = r#"<p>甲</p><span>乙</span>"#;
        let r = css_chain("p@text&&span@text", html);
        assert_eq!(r, vec!["甲".to_string(), "乙".to_string()]);
    }

    #[test]
    fn test_legado_shortcuts() {
        let html = r#"<div class="active"><span>书名</span></div><div class="active"><span>书2</span></div>"#;
        let r = css_chain("class.active@span@text", html);
        assert_eq!(r, vec!["书名".to_string(), "书2".to_string()]);
        let html2 = r#"<ul id="list"><li>x</li></ul>"#;
        let r2 = css_chain("id.list@li@text", html2);
        assert_eq!(r2, vec!["x".to_string()]);
    }

    #[test]
    fn test_chain_own_text() {
        let html = r#"<div>直接<span>子</span></div>"#;
        let r = css_chain("div@ownText", html);
        assert_eq!(r, vec!["直接".to_string()]);
        // jsoup ownText：直接文本节点按原文空白折叠拼接（元素子节点跳过）
        let html2 = r#"<div>a <b>x</b> c</div>"#;
        let r2 = css_chain("div@ownText", html2);
        assert_eq!(r2, vec!["a c".to_string()]);
        let html3 = r#"<div>a<b>x</b>c</div>"#;
        assert_eq!(css_chain("div@ownText", html3), vec!["ac".to_string()]);
    }

    #[test]
    fn test_chain_text_normalization() {
        // jsoup text()：空白折叠为单空格
        let html = "<div>书名：\n    测试书\t 作者</div>";
        let r = css_chain("div@text", html);
        assert_eq!(r, vec!["书名： 测试书 作者".to_string()]);
    }

    #[test]
    fn test_chain_text_nodes() {
        let html = r#"<div>甲<span>子</span>乙</div>"#;
        let r = css_chain("div@textNodes", html);
        assert_eq!(r, vec!["甲\n乙".to_string()]);
    }

    #[test]
    fn test_chain_html_extractor_removes_scripts() {
        let html =
            r#"<div class="c">正文<script>var x=1;</script><style>.a{}</style><p>内容</p></div>"#;
        let r = css_chain("class.c@html", html);
        assert_eq!(r.len(), 1);
        assert!(
            !r[0].contains("<script>"),
            "html 提取应移除 script: {}",
            r[0]
        );
        assert!(!r[0].contains("<style>"), "html 提取应移除 style: {}", r[0]);
        assert!(r[0].contains("<p>内容</p>"));
        // all 保留 script/style
        let r2 = css_chain("class.c@all", html);
        assert!(r2[0].contains("<script>"));
    }

    #[test]
    fn test_chain_attr_extractors() {
        let html =
            r#"<meta name="description" content="简介内容"><img data-src="/a.jpg" src="/b.jpg">"#;
        assert_eq!(
            css_chain("meta@content", html),
            vec!["简介内容".to_string()]
        );
        assert_eq!(css_chain("img@data-src", html), vec!["/a.jpg".to_string()]);
        assert_eq!(css_chain("img@src", html), vec!["/b.jpg".to_string()]);
        // 属性去重
        let html2 = r#"<a href="/x">1</a><a href="/x">2</a>"#;
        assert_eq!(css_chain("a@href", html2), vec!["/x".to_string()]);
    }

    #[test]
    fn test_chain_children() {
        let html = r#"<div class="w"><span>一</span><b>二</b></div>"#;
        // children 步进 = 直接子元素
        let r = css_chain("class.w@children@text", html);
        assert_eq!(r, vec!["一".to_string(), "二".to_string()]);
        // 真实书源：@children@children@class.x 深链（children 是容器，class.x 在其中）
        let html2 = r#"<div class="a"><div><div><span class="m">元1</span><span class="m">元2</span></div></div></div>"#;
        let r2 = css_chain("class.a@children@children@class.m@text", html2);
        assert_eq!(r2, vec!["元1".to_string(), "元2".to_string()]);
    }

    #[test]
    fn test_chain_parent() {
        let html = r#"<ul><li>一</li><li>二</li></ul>"#;
        // 每个 li 的父元素（ul）HTML——两个 li 共享同一父 → 2 份（元素列表语义）
        let r = css_chain("li@parent", html);
        assert_eq!(r.len(), 2);
        assert!(r[0].starts_with("<ul>"));
        assert_eq!(r[0], r[1]);
        // 链式：父元素文本（无空白源 → jsoup text() 同样无分隔）
        let r2 = css_chain("li@parent@text", html);
        assert_eq!(r2, vec!["一二".to_string(), "一二".to_string()]);
    }

    #[test]
    fn test_index_exclusion_legacy() {
        // tag.p!0 = 排除第一个（真实书源：@@class.smallreadbody!0@span@text）
        let html = r#"<div class="x"><p>一</p><p>二</p><p>三</p></div>"#;
        let r = css_chain("class.x@tag.p!0@text", html);
        assert_eq!(r, vec!["二".to_string(), "三".to_string()]);
        // 多索引排除 !0:1
        let r2 = css_chain("class.x@tag.p!0:1@text", html);
        assert_eq!(r2, vec!["三".to_string()]);
        // 负索引排除（div!-1 = 排除最后一个）
        let r3 = css_chain("class.x@tag.p!-1@text", html);
        assert_eq!(r3, vec!["一".to_string(), "二".to_string()]);
    }

    #[test]
    fn test_index_negative_and_bracket() {
        let html = r#"<ul><li>一</li><li>二</li><li>三</li><li>四</li></ul>"#;
        // 负索引（真实书源：tag.em.-1@text / tag.tr.-2）
        assert_eq!(css_chain("ul@li.-1@text", html), vec!["四".to_string()]);
        // [n] 索引
        assert_eq!(css_chain("ul@li[1]@text", html), vec!["二".to_string()]);
        // 区间 [1:3] → 1,2,3（legado Kotlin 区间 end 包含）
        assert_eq!(
            css_chain("ul@li[1:3]@text", html),
            vec!["二".to_string(), "三".to_string(), "四".to_string()]
        );
        // 排除集 [!0,2]
        assert_eq!(
            css_chain("ul@li[!0,2]@text", html),
            vec!["二".to_string(), "四".to_string()]
        );
        // 反向 [-1:0]（legado 特殊用法：任意位置让列表反向）
        assert_eq!(
            css_chain("ul@li[-1:0]@text", html),
            vec![
                "四".to_string(),
                "三".to_string(),
                "二".to_string(),
                "一".to_string()
            ]
        );
        // 步进 [0:4:2] → 0,2
        assert_eq!(
            css_chain("ul@li[0:4:2]@text", html),
            vec!["一".to_string(), "三".to_string()]
        );
    }

    #[test]
    fn test_attr_selectors() {
        let html = r#"<a href="http://a.com/1">1</a><a href="https://b.com/2">2</a><a href="/rel">3</a><a class="x y">4</a><a class="xy">5</a>"#;
        // [a=b]
        assert_eq!(
            css_chain("a[href='http://a.com/1']@text", html),
            vec!["1".to_string()]
        );
        // [a^=b] 前缀
        assert_eq!(
            css_chain("a[href^='https']@text", html),
            vec!["2".to_string()]
        );
        // [a$=b] 后缀
        assert_eq!(
            css_chain("a[href$='.com/2']@text", html),
            vec!["2".to_string()]
        );
        // [a*=b] 包含
        assert_eq!(
            css_chain("a[href*='b.com']@text", html),
            vec!["2".to_string()]
        );
        // [a~=b]：jsoup 语义 = 正则匹配（大小写不敏感）
        assert_eq!(
            css_chain("a[class~='x']@text", html),
            vec!["4".to_string(), "5".to_string()]
        );
        assert_eq!(
            css_chain("a[class~='^xy$']@text", html),
            vec!["5".to_string()]
        );
    }

    #[test]
    fn test_tag_chain() {
        let html = r#"<ul><li><a href="/1">链接1</a></li><li><a href="/2">链接2</a></li></ul>"#;
        // tag.li@tag.a 链（真实书源：tag.span.1@a@text / tag.strong@a@text）
        let r = css_chain("tag.ul@tag.li@tag.a@href", html);
        assert_eq!(r, vec!["/1".to_string(), "/2".to_string()]);
    }

    #[test]
    fn test_or_rules() {
        let html = r#"<div class="a">甲</div>"#;
        // || 首个命中即止（真实书源：@@class.h5-4con@html||class.error_msg@text）
        assert_eq!(
            css_chain("class.a@text||class.missing@text", html),
            vec!["甲".to_string()]
        );
        assert_eq!(
            css_chain("class.missing@text||class.a@text", html),
            vec!["甲".to_string()]
        );
        // || 与 ## 替换共存：替换由 rule.rs apply_post 在结果上应用
    }

    #[test]
    fn test_percent_interleave() {
        // 真实书源：class.bookinfo@a.1@text%%class.bookinfo@span.0@text
        // 索引逐元素作用：每个 bookinfo 内 a[1]、span[0]
        let html = r#"<div class="bookinfo"><a>甲书</a><a>甲书2</a><span>甲作者</span></div><div class="bookinfo"><a>乙书</a><a>乙书2</a><span>乙作者</span></div>"#;
        let r = css_chain("class.bookinfo@a.1@text%%class.bookinfo@span.0@text", html);
        assert_eq!(
            r,
            vec![
                "甲书2".to_string(),
                "甲作者".to_string(),
                "乙书2".to_string(),
                "乙作者".to_string()
            ]
        );
    }

    #[test]
    fn test_comma_group_selector() {
        // 真实书源：@@h3,li —— CSS 选择器组
        let html = r#"<h3>标题</h3><li>项</li>"#;
        let r = css_chain("h3,li@text", html);
        assert_eq!(r, vec!["标题".to_string(), "项".to_string()]);
    }

    #[test]
    fn test_text_keyword_contains_own_text() {
        // text.x → ownText 包含 x（jsoup getElementsContainingOwnText）
        let html = r#"<div class="l"><span>首页</span><span>书城</span></div>"#;
        let r = css_chain("class.l@text.首页@text", html);
        assert_eq!(r, vec!["首页".to_string()]);
    }

    #[test]
    fn test_single_selector_returns_elements() {
        let html = r#"<div class="book"><a>书名A</a></div><div class="book"><a>书名B</a></div>"#;
        let r = css_chain("div.book", html);
        assert_eq!(r.len(), 2);
        assert!(r[0].starts_with("<div class=\"book\">"));
    }

    #[test]
    fn test_css_parse_failure_regex_fallback_only() {
        // 合法 CSS 但无匹配 → 空（不再触发正则回退）
        let html = r#"<div>书名：测试书</div>"#;
        assert!(css_chain("span.nope", html).is_empty());
        // 非法 CSS（legacy 正则规则）→ 正则回退
        let html2 = "书名：测试书 作者：张三";
        let r = css_chain("书名：(.+?)\\s", html2);
        assert_eq!(r, vec!["测试书".to_string()]);
    }

    /// GAP 153：CSS 解析失败回退正则时支持 lookbehind（fancy-regex 升级）
    #[test]
    fn test_chain_regex_fallback_lookbehind() {
        let html = "书名：测试书 作者：张三";
        let r = css_chain("(?<=书名：)\\S+", html);
        assert_eq!(r, vec!["测试书".to_string()]);
    }

    #[test]
    fn test_chain_js_segment() {
        // <js> 链：规则结果进 JS（result 变量），再进后续规则
        let html = r#"<div class="b">abc123</div>"#;
        // 提取后进 JS（@js: 链）
        let r2 = css_chain("class.b@text@js:result.replace('abc','xyz')", html);
        assert_eq!(r2, vec!["xyz123".to_string()]);
        // <js> 在前：JS 处理（result=原文档）后接 CSS 链
        let r = css_chain("<js>result.replace('abc','xyz')</js>@class.b@text", html);
        assert_eq!(r, vec!["xyz123".to_string()]);
        // @js: 贪婪匹配到末尾（legado JS_PATTERN 同语义）——第二个 @js: 并入首个 JS 代码 → JS 语法错误 → 空
        let r3 = css_chain(
            "class.b@text@js:result.replace('abc','xyz')@js:result.toUpperCase()",
            html,
        );
        assert!(r3.is_empty());
    }
}
