//! 规则解析引擎（legado 多规则：CSS / JSONPath / XPath / Regex / JS，逐项移植中）

pub mod js;
pub mod rule;
pub mod xpath;

pub use rule::{apply, parse_rule, Rule, RuleKind};
