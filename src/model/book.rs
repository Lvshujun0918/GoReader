//! 书籍实体（兼容 legacy Book / bookshelf.json，JSON 字段 camelCase，全字段无丢失）

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// 书架书籍（books 表 ↔ bookshelf.json ↔ /reader3/getBookshelf 输出）
///
/// - serde：camelCase 与 legacy bookshelf.json / API 输出一致（legacy 全字段）
/// - sqlx：snake_case 与 books 表列名一致（`group` 为 SQLite 关键字 → 列名 `group_name`；
///   `order` 为 SQLite 关键字 → 列名 `order_num`）
#[derive(Debug, Clone, Default, Serialize, Deserialize, FromRow)]
#[serde(default)]
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
    /// 分类信息（用户修改）
    #[serde(rename = "customTag")]
    #[sqlx(rename = "custom_tag")]
    pub custom_tag: Option<String>,
    #[serde(rename = "coverUrl")]
    #[sqlx(rename = "cover_url")]
    pub cover_url: Option<String>,
    #[serde(rename = "customCoverUrl")]
    #[sqlx(rename = "custom_cover_url")]
    pub custom_cover_url: Option<String>,
    pub intro: Option<String>,
    /// 简介（用户修改）
    #[serde(rename = "customIntro")]
    #[sqlx(rename = "custom_intro")]
    pub custom_intro: Option<String>,
    /// 自定义字符集（仅本地书籍）
    pub charset: Option<String>,
    /// 书籍类型 @BookType（`type` 为 Rust 关键字 → 字段名 book_type）
    #[serde(rename = "type")]
    #[sqlx(rename = "type")]
    pub book_type: i64,
    /// 自定义分组索引号（books 表列名 group_name）
    #[sqlx(rename = "group_name")]
    pub group: i64,
    /// 最新章节标题
    #[serde(rename = "latestChapterTitle")]
    #[sqlx(rename = "latest_chapter_title")]
    pub latest_chapter_title: Option<String>,
    /// 最新章节标题更新时间
    #[serde(rename = "latestChapterTime")]
    #[sqlx(rename = "latest_chapter_time")]
    pub latest_chapter_time: i64,
    /// 最近一次更新书籍信息的时间
    #[serde(rename = "lastCheckTime")]
    #[sqlx(rename = "last_check_time")]
    pub last_check_time: i64,
    /// 最近一次发现新章节的数量
    #[serde(rename = "lastCheckCount")]
    #[sqlx(rename = "last_check_count")]
    pub last_check_count: i64,
    /// 书籍目录总数
    #[serde(rename = "totalChapterNum")]
    #[sqlx(rename = "total_chapter_num")]
    pub total_chapter_num: i64,
    #[serde(rename = "durChapterTitle")]
    #[sqlx(rename = "dur_chapter_title")]
    pub dur_chapter_title: Option<String>,
    #[serde(rename = "durChapterIndex")]
    #[sqlx(rename = "dur_chapter_index")]
    pub dur_chapter_index: i64,
    #[serde(rename = "durChapterPos")]
    #[sqlx(rename = "dur_chapter_pos")]
    pub dur_chapter_pos: i64,
    #[serde(rename = "durChapterTime")]
    #[sqlx(rename = "dur_chapter_time")]
    pub dur_chapter_time: i64,
    /// 字数
    #[serde(rename = "wordCount")]
    #[sqlx(rename = "word_count")]
    pub word_count: Option<String>,
    #[serde(rename = "canUpdate")]
    #[sqlx(rename = "can_update")]
    pub can_update: bool,
    /// 手动排序（order 为 SQLite 关键字 → 列名 order_num）
    #[sqlx(rename = "order_num")]
    pub order: i64,
    /// 书源排序
    #[serde(rename = "originOrder")]
    #[sqlx(rename = "origin_order")]
    pub origin_order: i64,
    /// 正文使用净化替换规则
    #[serde(rename = "useReplaceRule")]
    #[sqlx(rename = "use_replace_rule")]
    pub use_replace_rule: bool,
    /// 自定义书籍变量（书源规则检索用）
    pub variable: Option<String>,
    /// 阅读配置（legacy ReadConfig 对象——存 JSON 文本）
    #[serde(rename = "readConfig")]
    #[sqlx(rename = "read_config")]
    pub read_config: Option<serde_json::Value>,
    /// 是否在书架
    #[serde(rename = "isInShelf")]
    #[sqlx(rename = "is_in_shelf")]
    pub is_in_shelf: bool,
    #[serde(rename = "lastCheckError")]
    #[sqlx(rename = "last_check_error")]
    pub last_check_error: Option<String>,
    /// 详情页 HTML 缓存
    #[serde(rename = "infoHtml")]
    #[sqlx(rename = "info_html")]
    pub info_html: Option<String>,
    /// 目录页 HTML 缓存
    #[serde(rename = "tocHtml")]
    #[sqlx(rename = "toc_html")]
    pub toc_html: Option<String>,
    /// 用户命名空间（secure 模式用户名 / default）
    #[serde(skip)]
    #[sqlx(rename = "user_namespace")]
    pub user_namespace: String,
    /// 创建时间
    #[serde(rename = "createdAt")]
    #[sqlx(rename = "created_at")]
    pub created_at: i64,
    /// 迁移保底：原始 JSON 全量（未知字段也不丢）
    #[serde(skip)]
    #[sqlx(rename = "raw_json")]
    pub raw_json: Option<String>,
}
