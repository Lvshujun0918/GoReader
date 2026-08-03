//! 书籍实体（兼容 legacy Book / bookshelf.json，JSON 字段 camelCase）

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// 书架书籍（books 表 ↔ bookshelf.json ↔ /reader3/getBookshelf 输出）
///
/// - serde：camelCase 与 legacy bookshelf.json / API 输出一致
/// - sqlx：snake_case 与 books 表列名一致（`group` 为 SQLite 关键字 → 列名 `group_name`）
#[derive(Debug, Clone, Default, Serialize, Deserialize, FromRow)]
pub struct Book {
    #[serde(rename = "bookUrl")]
    #[sqlx(rename = "book_url")]
    pub book_url: String,
    #[serde(rename = "tocUrl")]
    #[sqlx(rename = "toc_url")]
    pub toc_url: String,
    pub origin: String,
    #[serde(rename = "originName")]
    #[sqlx(rename = "origin_name")]
    pub origin_name: String,
    pub name: String,
    pub author: String,
    pub kind: Option<String>,
    #[serde(rename = "coverUrl")]
    #[sqlx(rename = "cover_url")]
    pub cover_url: Option<String>,
    pub intro: Option<String>,
    /// 自定义分组索引号（books 表列名 group_name，group 为 SQLite 关键字）
    #[sqlx(rename = "group_name")]
    pub group: i64,
    /// 书籍类型 @BookType（`type` 为 Rust 关键字 → 字段名 book_type）
    #[serde(rename = "type")]
    #[sqlx(rename = "type")]
    pub book_type: i64,
    #[serde(rename = "canUpdate")]
    #[sqlx(rename = "can_update")]
    pub can_update: bool,
    #[serde(rename = "durChapterIndex")]
    #[sqlx(rename = "dur_chapter_index")]
    pub dur_chapter_index: i64,
    #[serde(rename = "durChapterPos")]
    #[sqlx(rename = "dur_chapter_pos")]
    pub dur_chapter_pos: i64,
    #[serde(rename = "durChapterTime")]
    #[sqlx(rename = "dur_chapter_time")]
    pub dur_chapter_time: i64,
    #[serde(rename = "durChapterTitle")]
    #[sqlx(rename = "dur_chapter_title")]
    pub dur_chapter_title: Option<String>,
    #[serde(rename = "customCoverUrl")]
    #[sqlx(rename = "custom_cover_url")]
    pub custom_cover_url: Option<String>,
    pub charset: Option<String>,
    #[serde(rename = "lastCheckError")]
    #[sqlx(rename = "last_check_error")]
    pub last_check_error: Option<String>,
}
