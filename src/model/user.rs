//! 用户实体（兼容 legacy User / users.json 全字段，JSON 字段 snake_case 与 legacy 一致）

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Default, Serialize, Deserialize, FromRow)]
#[serde(default)]
pub struct User {
    pub username: String,
    pub password: String,
    pub salt: String,
    pub token: String,
    /// 多会话 token → 过期时间（legacy token_map）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_map: Option<serde_json::Value>,
    pub enable_webdav: bool,
    pub enable_local_store: bool,
    pub enable_book_source: bool,
    pub enable_rss_source: bool,
    pub book_source_limit: i64,
    pub book_limit: i64,
    pub last_login_at: i64,
    pub created_at: i64,
    #[serde(skip)]
    #[sqlx(rename = "user_namespace")]
    pub user_namespace: String,
    /// 迁移保底：原始 JSON 全量
    #[serde(skip)]
    #[sqlx(rename = "raw_json")]
    pub raw_json: Option<String>,
}
