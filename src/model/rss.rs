//! RSS 实体（兼容 legacy RssSource / RssArticle 字段子集）
//!
//! - RssSource：表列仅保留任务规格字段（rss_source_url / rss_source_name / rss_source_group /
//!   enabled / user_namespace / raw_json），完整 legacy JSON 原文保存在 raw_json（保底不丢字段）；
//!   API 返回时以 raw_json 为基底、表列为覆盖（见 router::rss_source_json）
//! - RssArticle：url 为主键，content 为 feed 正文/摘要（HTML 或文本）

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// RSS 源（表：rss_sources）
#[derive(Debug, Clone, Default, Serialize, Deserialize, FromRow)]
#[serde(default)]
pub struct RssSource {
    #[serde(rename = "sourceUrl")]
    #[sqlx(rename = "rss_source_url")]
    pub source_url: String,
    #[serde(rename = "sourceName")]
    #[sqlx(rename = "rss_source_name")]
    pub source_name: String,
    #[serde(rename = "sourceGroup")]
    #[sqlx(rename = "rss_source_group")]
    pub source_group: Option<String>,
    pub enabled: bool,
    #[serde(skip)]
    #[sqlx(rename = "user_namespace")]
    pub user_namespace: String,
    #[serde(skip)]
    #[sqlx(rename = "raw_json")]
    pub raw_json: Option<String>,
}

impl RssSource {
    /// 从 raw_json（完整 legacy JSON 原文）读取表列之外的字段
    fn json_field(&self, key: &str) -> Option<String> {
        self.raw_json
            .as_deref()
            .and_then(|r| serde_json::from_str::<serde_json::Value>(r).ok())
            .and_then(|v| v.get(key).and_then(|x| x.as_str()).map(str::to_string))
    }

    /// 请求头（legacy header 字段）
    pub fn header(&self) -> Option<String> {
        self.json_field("header")
    }

    /// 排序/列表 URL（legacy sortUrl 字段）
    pub fn sort_url(&self) -> Option<String> {
        self.json_field("sortUrl")
    }
}

/// RSS 文章（表：rss_articles）
#[derive(Debug, Clone, Default, Serialize, Deserialize, FromRow)]
#[serde(default, rename_all = "camelCase")]
pub struct RssArticle {
    /// 文章链接（主键）
    pub url: String,
    /// 所属 RSS 源 URL
    #[sqlx(rename = "source_url")]
    pub source_url: String,
    pub title: String,
    pub author: String,
    /// 发布时间（毫秒时间戳）
    pub time: i64,
    /// 正文（feed content/summary，或抓取网页提取）
    pub content: Option<String>,
    /// 封面/配图
    pub cover: Option<String>,
    /// 已读标记（JSON 输出为 hasRead；SQLite 列 read）
    #[serde(rename = "hasRead")]
    pub read: bool,
    #[serde(skip)]
    #[sqlx(rename = "user_namespace")]
    pub user_namespace: String,
}
