//! 自定义 TXT 目录规则实体（对齐 legado TxtTocRule）
//!
//! legado TxtTocRule：id(Long)/name/rule(正则)/serialNumber(Int)/enable(Bool)
//! 表：txt_toc_rules（id TEXT PK / name / rule / enable / serial_number / user_namespace）
//!
//! 上传 TXT 分章时使用用户自定义规则（get_txt_toc_rules → parse_txt 传入），
//! 未配置用户规则时回退内置 DEFAULT_TOC_RULES。

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Default, Serialize, Deserialize, FromRow)]
#[serde(default)]
pub struct TxtTocRule {
    /// 规则 id（前端生成字符串 id；后端缺失时补 uuid）
    pub id: String,
    /// 规则名称（必填）
    pub name: String,
    /// 正则规则（必填；匹配行作为章节标题）
    pub rule: String,
    /// 是否启用（列名 enable，legacy 兼容）
    #[sqlx(rename = "enable")]
    pub enable: bool,
    /// 排序（legacy serialNumber）
    #[serde(rename = "serialNumber")]
    #[sqlx(rename = "serial_number")]
    pub serial_number: i64,
    /// 命名空间（secure 模式用户名 / default）
    #[serde(skip)]
    #[sqlx(rename = "user_namespace")]
    pub user_namespace: String,
}
