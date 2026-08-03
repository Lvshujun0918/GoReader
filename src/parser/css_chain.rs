//! legado 链式 CSS（对齐 AnalyzeByJSoup：&& 多规则 + @ 链 + 末段属性提取）

use scraper::{Html, Selector};

/// 链式 CSS 入口：`&&` 拆分多条规则，结果合并
pub fn css_chain(rule: &str, html: &str) -> Vec<String> {
    let main = rule.split("##").next().unwrap_or(rule).trim();
    let mut out: Vec<String> = Vec::new();
    for sub in main.split("&&") {
        out.extend(css_chain_single(sub.trim(), html));
    }
    out
}

fn css_chain_single(rule: &str, doc_html: &str) -> Vec<String> {
    // 首段上下文 = 完整文档；后续段 = 上一步元素
    let mut current: Vec<String> = vec![doc_html.to_string()];
    let parts: Vec<&str> = rule
        .split('@')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        return vec![];
    }

    for (i, part) in parts.iter().enumerate() {
        let is_last = i == parts.len() - 1;
        if is_last && is_attr_extractor(part) {
            // 末段属性/文本提取（legado getResultLast）
            return extract_attr(&current, part);
        }
        let (selector, index) = split_selector_index(part);
        let mut next: Vec<String> = Vec::new();
        for item in &current {
            let doc = if i == 0 {
                Html::parse_document(item)
            } else {
                Html::parse_fragment(item)
            };
            if let Ok(sel) = Selector::parse(selector) {
                let els: Vec<_> = doc.select(&sel).collect();
                if let Some(idx) = index {
                    if let Some(el) = els.get(idx) {
                        next.push(el.html().to_string());
                    }
                } else {
                    for el in els {
                        next.push(el.html().to_string());
                    }
                }
            }
        }
        // 单段规则且 CSS 解析无结果 → 回退正则（legacy 兼容）
        if next.is_empty() && parts.len() == 1 && i == 0 {
            if let Ok(re) = regex::Regex::new(selector) {
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
        if next.is_empty() {
            return vec![];
        }
        current = next;
    }
    // 全为选择器（无末段提取）：返回元素 HTML（bookList 场景）
    current
}

/// 末段是否为属性/文本提取器（legado getResultLast 支持集合）
fn is_attr_extractor(part: &str) -> bool {
    matches!(
        part,
        "text" | "textNodes" | "ownText" | "html" | "all" | "href" | "src" | "value" | "data-src"
            | "data-original" | "data-url"
    )
}

/// 拆分选择器与索引：`tag.dd.1` → ("tag.dd", Some(1))；`div.book` → ("div.book", None)
fn split_selector_index(part: &str) -> (&str, Option<usize>) {
    let bytes = part.as_bytes();
    let mut digit_start = None;
    let mut i = part.len();
    while i > 0 {
        i -= 1;
        if bytes[i].is_ascii_digit() {
            digit_start = Some(i);
        } else {
            break;
        }
    }
    if let Some(ds) = digit_start {
        if ds > 0 && bytes[ds - 1] == b'.' {
            let idx: usize = part[ds..].parse().unwrap_or(0);
            return (&part[..ds - 1], Some(idx));
        }
    }
    (part, None)
}

/// 属性/文本提取（legado getResultLast 语义）
/// 注意：parse_fragment 会包裹 html/body——统一取真实元素（select("*") 第一个）
fn extract_attr(items: &[String], attr: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let star = Selector::parse("*").ok();
    for item in items {
        let doc = Html::parse_fragment(item);
        // 排除 parse_fragment 的包裹元素（html/head/body）
        let el = star.as_ref().and_then(|s| {
            doc.select(s)
                .find(|e| !matches!(e.value().name(), "html" | "head" | "body"))
        });
        let Some(el) = el else { continue };
        match attr {
            "text" => {
                let t = el.text().collect::<String>();
                let t = t.trim().to_string();
                if !t.is_empty() {
                    out.push(t);
                }
            }
            "ownText" => {
                let t = el
                    .children()
                    .filter_map(|n| match n.value() {
                        scraper::node::Node::Text(txt) => Some(txt.text.trim().to_string()),
                        _ => None,
                    })
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("");
                if !t.is_empty() {
                    out.push(t);
                }
            }
            "textNodes" => {
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
            "html" | "all" => {
                let html = el.html();
                if !html.is_empty() {
                    out.push(html);
                }
            }
            _ => {
                // 属性（href/src/value/data-src...）
                if let Some(v) = el.value().attr(attr) {
                    out.push(v.trim().to_string());
                }
            }
        }
    }
    out
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
    fn test_chain_own_text() {
        let html = r#"<div>直接<span>子</span></div>"#;
        let r = css_chain("div@ownText", html);
        assert_eq!(r, vec!["直接".to_string()]);
    }
}
