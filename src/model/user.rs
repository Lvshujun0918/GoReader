//! 用户实体（兼容 legacy users.json / User 实体字段）

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct User {
    pub username: String,
    pub password: String,
    pub salt: String,
    pub token: String,
    pub enable_webdav: bool,
    pub enable_local_store: bool,
    pub enable_book_source: bool,
    pub enable_rss_source: bool,
    pub book_source_limit: i64,
    pub book_limit: i64,
    pub last_login_at: i64,
    pub created_at: i64,
}
