//! 替换规则实体（兼容 legado ReplaceRule / replaceRule.json）
//!
//! - serde：`enabled`/`order`（与前端 ReplaceRule 类型一致）
//! - sqlx：列名 `enable` / `order_num`（order 为 SQLite 关键字）

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Default, Serialize, Deserialize, FromRow)]
#[serde(default)]
pub struct ReplaceRule {
    /// 规则 id（前端生成字符串 id；后端缺失时补 uuid）
    pub id: String,
    /// 规则名称（必填）
    pub name: String,
    /// 查找内容（必填）
    pub find: String,
    /// 替换为（可空 = 删除匹配文字）
    pub replace: String,
    /// 是否启用（列名 enable，legacy 兼容）
    #[sqlx(rename = "enable")]
    pub enabled: bool,
    /// 排序（order 为 SQLite 关键字 → 列名 order_num）
    #[sqlx(rename = "order_num")]
    pub order: i64,
    /// 命名空间（secure 模式用户名 / default）
    #[serde(skip)]
    #[sqlx(rename = "user_namespace")]
    pub user_namespace: String,
}
