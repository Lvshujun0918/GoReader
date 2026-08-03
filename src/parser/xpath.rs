//! XPath 规则执行（sxd-xpath，对齐 legado AnalyzeByXPath）

use sxd_document::parser;
use sxd_xpath::nodeset::Node;
use sxd_xpath::{Context, Factory, Value};

/// 执行 XPath，返回字符串列表（对齐 legado getStringList）
pub fn xpath_select(rule: &str, xml: &str) -> Vec<String> {
    let package = match parser::parse(xml) {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!("XPath 文档解析失败: {e}");
            return vec![];
        }
    };
    let document = package.as_document();
    let factory = Factory::new();
    let xpath = match factory.build(rule) {
        Ok(Some(x)) => x,
        Ok(None) => {
            tracing::debug!("XPath 规则无效（空表达式） [{rule}]");
            return vec![];
        }
        Err(e) => {
            tracing::debug!("XPath 规则编译失败 [{rule}]: {e}");
            return vec![];
        }
    };
    let context = Context::new();
    let value = match xpath.evaluate(&context, document.root()) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!("XPath 求值失败 [{rule}]: {e}");
            return vec![];
        }
    };
    value_to_strings(&value)
}

fn value_to_strings(value: &Value) -> Vec<String> {
    match value {
        Value::Nodeset(nodes) => nodes
            .document_order()
            .iter()
            .filter_map(node_to_string)
            .filter(|s| !s.is_empty())
            .collect(),
        Value::String(s) => {
            if s.is_empty() {
                vec![]
            } else {
                vec![s.clone()]
            }
        }
        Value::Number(n) => vec![n.to_string()],
        Value::Boolean(b) => vec![b.to_string()],
    }
}

/// 提取单个节点的字符串值：
/// - 元素：XPath string-value（全部后代文本节点拼接，参考 sxd_xpath::nodeset::Node::string_value）
/// - 属性：属性值
/// - 文本节点：文本内容
/// Root / 注释 / 处理指令 / 命名空间节点不产出结果
fn node_to_string(node: &Node) -> Option<String> {
    match node {
        Node::Element(_) => Some(node.string_value()),
        Node::Attribute(attr) => Some(attr.value().to_string()),
        Node::Text(text) => Some(text.text().trim().to_string()),
        Node::Root(_) | Node::Comment(_) | Node::ProcessingInstruction(_) | Node::Namespace(_) => {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<library>
  <book id="1">
    <title>三体</title>
    <author>刘慈欣</author>
    <link href="https://example.com/1">链接一</link>
  </book>
  <book id="2">
    <title>流浪地球</title>
    <author>刘慈欣</author>
    <link href="https://example.com/2">链接二</link>
  </book>
</library>"#;

    #[test]
    fn xpath_select_returns_element_text() {
        let result = xpath_select("//book/title", XML);
        assert_eq!(result, vec!["三体", "流浪地球"]);
    }

    #[test]
    fn xpath_select_returns_attribute_values() {
        let result = xpath_select("//book/link/@href", XML);
        assert_eq!(
            result,
            vec!["https://example.com/1", "https://example.com/2"]
        );
    }

    #[test]
    fn xpath_select_returns_text_nodes_and_strings() {
        let texts = xpath_select("//book/title/text()", XML);
        assert_eq!(texts, vec!["三体", "流浪地球"]);

        // string() 返回 Value::String 分支
        let single = xpath_select("string(//book[1]/title)", XML);
        assert_eq!(single, vec!["三体"]);

        // 无匹配时返回空列表
        assert!(xpath_select("//book/nonexistent", XML).is_empty());
    }
}
