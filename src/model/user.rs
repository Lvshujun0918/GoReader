//! 用户实体（兼容 legacy users.json / User 实体字段 + users 表）

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, Default, FromRow)]
pub struct User {
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub salt: String,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub enable_webdav: bool,
    #[serde(default)]
    pub enable_local_store: bool,
    #[serde(default)]
    pub enable_book_source: bool,
    #[serde(default)]
    pub enable_rss_source: bool,
    #[serde(default)]
    pub book_source_limit: i64,
    #[serde(default)]
    pub book_limit: i64,
    #[serde(default)]
    pub last_login_at: i64,
    #[serde(default)]
    pub created_at: i64,
    /// 数据命名空间（users 表列，= username；兼容迁移）
    #[serde(default)]
    #[sqlx(rename = "user_namespace")]
    pub user_namespace: String,
}
