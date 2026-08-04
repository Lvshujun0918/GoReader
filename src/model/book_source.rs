//! 书源实体（兼容 legacy BookSource / bookSource.json 全字段）
//!
//! - 规则字段（ruleSearch 等）为嵌套 JSON 对象 → `Option<serde_json::Value>`（存文本/原样输出）
//! - 序列化：字段名与 legacy bookSource.json 一致（camelCase）
//! - raw_json 保底：未知字段不丢

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Default, Serialize, Deserialize, FromRow)]
#[serde(default)]
pub struct BookSource {
    #[serde(rename = "bookSourceUrl")]
    #[sqlx(rename = "book_source_url")]
    pub book_source_url: String,
    #[serde(rename = "bookSourceName")]
    #[sqlx(rename = "book_source_name")]
    pub book_source_name: String,
    #[serde(rename = "bookSourceGroup")]
    #[sqlx(rename = "book_source_group")]
    pub book_source_group: Option<String>,
    #[serde(rename = "bookSourceType")]
    #[sqlx(rename = "book_source_type")]
    pub book_source_type: i64,
    #[serde(rename = "bookUrlPattern")]
    #[sqlx(rename = "book_url_pattern")]
    pub book_url_pattern: Option<String>,
    #[serde(rename = "customOrder")]
    #[sqlx(rename = "custom_order")]
    pub custom_order: i64,
    pub enabled: bool,
    #[serde(rename = "enabledExplore")]
    #[sqlx(rename = "enabled_explore")]
    pub enabled_explore: bool,
    #[serde(rename = "enabledCookieJar")]
    #[sqlx(rename = "enabled_cookie_jar")]
    pub enabled_cookie_jar: Option<bool>,
    #[serde(rename = "concurrentRate")]
    #[sqlx(rename = "concurrent_rate")]
    pub concurrent_rate: Option<String>,
    pub header: Option<String>,
    #[serde(rename = "loginUrl")]
    #[sqlx(rename = "login_url")]
    pub login_url: Option<String>,
    #[serde(rename = "loginUi")]
    #[sqlx(rename = "login_ui")]
    pub login_ui: Option<String>,
    #[serde(rename = "loginCheckJs")]
    #[sqlx(rename = "login_check_js")]
    pub login_check_js: Option<String>,
    #[serde(rename = "loginJs")]
    #[sqlx(rename = "login_js")]
    pub login_js: Option<String>,
    #[serde(rename = "bookSourceComment")]
    #[sqlx(rename = "book_source_comment")]
    pub book_source_comment: Option<String>,
    #[serde(rename = "variableComment")]
    #[sqlx(rename = "variable_comment")]
    pub variable_comment: Option<String>,
    #[serde(rename = "lastUpdateTime")]
    #[sqlx(rename = "last_update_time")]
    pub last_update_time: i64,
    #[serde(rename = "respondTime")]
    #[sqlx(rename = "respond_time")]
    pub respond_time: i64,
    pub weight: i64,
    #[serde(rename = "exploreUrl")]
    #[sqlx(rename = "explore_url")]
    pub explore_url: Option<String>,
    #[serde(rename = "searchUrl")]
    #[sqlx(rename = "search_url")]
    pub search_url: Option<String>,
    // ---- 规则（legacy + legado 两套命名，均为嵌套对象）----
    #[serde(rename = "ruleExplore")]
    #[sqlx(rename = "rule_explore")]
    pub rule_explore: Option<serde_json::Value>,
    #[serde(rename = "ruleSearch")]
    #[sqlx(rename = "rule_search")]
    pub rule_search: Option<serde_json::Value>,
    #[serde(rename = "ruleBookInfo")]
    #[sqlx(rename = "rule_book_info")]
    pub rule_book_info: Option<serde_json::Value>,
    #[serde(rename = "ruleToc")]
    #[sqlx(rename = "rule_toc")]
    pub rule_toc: Option<serde_json::Value>,
    #[serde(rename = "ruleContent")]
    #[sqlx(rename = "rule_content")]
    pub rule_content: Option<serde_json::Value>,
    #[serde(rename = "ruleRelated")]
    #[sqlx(rename = "rule_related")]
    pub rule_related: Option<serde_json::Value>,
    #[serde(rename = "searchRule")]
    #[sqlx(rename = "search_rule")]
    pub search_rule: Option<serde_json::Value>,
    #[serde(rename = "exploreRule")]
    #[sqlx(rename = "explore_rule")]
    pub explore_rule: Option<serde_json::Value>,
    #[serde(rename = "bookInfoRule")]
    #[sqlx(rename = "book_info_rule")]
    pub book_info_rule: Option<serde_json::Value>,
    #[serde(rename = "tocRule")]
    #[sqlx(rename = "toc_rule")]
    pub toc_rule: Option<serde_json::Value>,
    #[serde(rename = "contentRule")]
    #[sqlx(rename = "content_rule")]
    pub content_rule: Option<serde_json::Value>,
    // ---- legado 扩展 ----
    pub key: Option<String>,
    pub tag: Option<String>,
    pub logger: Option<serde_json::Value>,
    pub variable: Option<serde_json::Value>,
    #[serde(skip)]
    #[sqlx(rename = "user_namespace")]
    pub user_namespace: String,
    #[serde(skip)]
    #[sqlx(rename = "raw_json")]
    pub raw_json: Option<String>,
}
