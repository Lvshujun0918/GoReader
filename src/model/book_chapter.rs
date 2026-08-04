//! 书籍信息与章节（兼容 legacy Book/BookChapter 字段）

use serde::{Deserialize, Serialize};

/// 书籍详情（兼容 legacy Book 的详情字段）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BookInfo {
    pub name: String,
    pub author: String,
    pub kind: Option<String>,
    pub intro: Option<String>,
    pub cover_url: Option<String>,
    pub toc_url: Option<String>,
    pub word_count: Option<String>,
    pub latest_chapter_title: Option<String>,
    pub book_url: String,
    pub origin: String,
    pub origin_name: String,
    /// 语言（本地书/EPUB）
    pub language: Option<String>,
    /// 出版社
    pub publisher: Option<String>,
    /// 出版时间
    #[serde(rename = "publishedAt")]
    pub published_at: Option<String>,
    /// 相关推荐（GAP 17b：书源 ruleRelated 解析，[{name, author, bookUrl, coverUrl}]）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_books: Vec<RelatedBook>,
}

/// 相关推荐书（GAP 17b：legacy RelatedBook 字段）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RelatedBook {
    pub name: String,
    pub author: String,
    pub book_url: String,
    pub cover_url: Option<String>,
}

/// 章节（兼容 legacy BookChapter）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BookChapter {
    pub title: String,
    pub url: String,
    /// 1=卷标题（legacy isVolume）
    #[serde(rename = "isVolume")]
    pub is_volume: bool,
    pub index: i64,
}
