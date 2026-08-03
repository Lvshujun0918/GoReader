//! HttpTTS 听书源实体（兼容 legado HttpTTS / httpTTS.json）
//!
//! - 表主键：url（任务规格：http_tts_list 表 = url/name/type/user_namespace）
//! - 输出 JSON：`id` 与 `url` 同值（前端 HttpTts 类型 id 兼容；legacy HttpTTS 为 Long id）
//! - type：0=在线合成（http 请求音频），1=本地引擎（预留）

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Default, Serialize, Deserialize, FromRow)]
#[serde(default)]
pub struct HttpTts {
    /// 听书源 URL（主键；JSON 输出时同时提供 id 字段，见 handler 的 http_tts_json）
    pub url: String,
    /// 名称（必填）
    pub name: String,
    /// 类型（0=在线合成 / 1=本地引擎；type 为 Rust 关键字 → r#type）
    #[serde(rename = "type")]
    #[sqlx(rename = "type")]
    pub tts_type: i64,
    /// 命名空间（secure 模式用户名 / default）
    #[serde(skip)]
    #[sqlx(rename = "user_namespace")]
    pub user_namespace: String,
}
