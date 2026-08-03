//! 书架分组实体（兼容 legacy BookGroup / bookGroup.json）
//!
//! - serde：camelCase 与 legacy API 输出一致（id/name/order）
//! - sqlx：`order` 为 SQLite 关键字 → 列名 `order_num`
//! - books.group_name 存分组 id（int 引用）

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Default, Serialize, Deserialize, FromRow)]
#[serde(default)]
pub struct BookGroup {
    /// 分组 id（AUTOINCREMENT；>0 时 save 按 id 覆盖）
    pub id: i64,
    /// 分组名（必填）
    pub name: String,
    /// 排序（order 为 SQLite 关键字 → 列名 order_num）
    #[sqlx(rename = "order_num")]
    pub order: i64,
    /// 命名空间（secure 模式用户名 / default）
    #[serde(skip)]
    #[sqlx(rename = "user_namespace")]
    pub user_namespace: String,
}
