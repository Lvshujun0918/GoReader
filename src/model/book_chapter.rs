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
