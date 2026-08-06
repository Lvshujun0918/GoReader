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

// ==================== GAP 59 多设备 token（users.token_map） ====================
//
// token_map 存储为 JSON 数组（字符串 token 列表，按登录时间升序，上限 5）；
// 兼容 legacy 旧数据形态（JSON 对象 Map<token, 过期时间戳>——取键作为 token 列表）。

/// 会话 token 上限（GAP 59：多设备登录最多 5 个有效 token）
pub const MAX_USER_TOKENS: usize = 5;

/// 解析 token_map → token 列表（None / 非法 JSON / 空 → 空列表）
pub fn token_map_list(token_map: &Option<serde_json::Value>) -> Vec<String> {
    let Some(v) = token_map else { return vec![] };
    match v {
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .filter(|s| !s.is_empty())
            .collect(),
        // legacy 形态：{token: 过期时间戳}——键即 token
        serde_json::Value::Object(map) => map.keys().cloned().collect(),
        _ => vec![],
    }
}

/// token_map 是否包含指定 token（GAP 59：resolve_namespace 任一 token 均可通过）
pub fn token_map_contains(token_map: &Option<serde_json::Value>, token: &str) -> bool {
    token_map_list(token_map).iter().any(|t| t == token)
}

/// 追加 token（去重；超出上限丢最旧），返回新 token_map JSON 字符串
pub fn token_map_push(token_map: &Option<serde_json::Value>, token: &str) -> String {
    let mut list = token_map_list(token_map);
    list.retain(|t| t != token);
    list.push(token.to_string());
    while list.len() > MAX_USER_TOKENS {
        list.remove(0);
    }
    serde_json::to_string(&list).unwrap_or_else(|_| serde_json::json!([]).to_string())
}

/// 移除 token，返回 (移除后 token_map JSON, 是否确实移除了该 token)
pub fn token_map_remove(token_map: &Option<serde_json::Value>, token: &str) -> (String, bool) {
    let mut list = token_map_list(token_map);
    let before = list.len();
    list.retain(|t| t != token);
    let removed = list.len() != before;
    let json = serde_json::to_string(&list).unwrap_or_else(|_| serde_json::json!([]).to_string());
    (json, removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_token_map_list_formats() {
        // 数组形态
        let m = Some(json!(["t1", "t2"]));
        assert_eq!(token_map_list(&m), vec!["t1", "t2"]);
        // legacy 对象形态（键为 token）
        let m = Some(json!({"t1": 1700000000000i64, "t2": 1700000001000i64}));
        let mut list = token_map_list(&m);
        list.sort();
        assert_eq!(list, vec!["t1", "t2"]);
        // None / 非法
        assert!(token_map_list(&None).is_empty());
        assert!(token_map_list(&Some(json!("x"))).is_empty());
    }

    #[test]
    fn test_token_map_push_dedup_and_cap() {
        let mut m: Option<serde_json::Value> = None;
        for i in 0..7 {
            m = Some(serde_json::from_str(&token_map_push(&m, &format!("t{i}"))).unwrap());
        }
        let list = token_map_list(&m);
        assert_eq!(list.len(), MAX_USER_TOKENS, "上限 5");
        assert_eq!(list, vec!["t2", "t3", "t4", "t5", "t6"], "最旧被丢");
        // 去重：重新推入已存在的 token 不重复
        let m = Some(json!(["t2", "t3"]));
        let m2 = serde_json::from_str::<serde_json::Value>(&token_map_push(&m, "t3")).unwrap();
        assert_eq!(token_map_list(&Some(m2)), vec!["t2", "t3"]);
    }

    #[test]
    fn test_token_map_contains_and_remove() {
        let m = Some(json!(["t1", "t2"]));
        assert!(token_map_contains(&m, "t1"));
        assert!(token_map_contains(&m, "t2"));
        assert!(!token_map_contains(&m, "t3"));
        assert!(!token_map_contains(&None, ""));
        let (json, removed) = token_map_remove(&m, "t1");
        assert!(removed);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(token_map_list(&Some(v)), vec!["t2"]);
        let (json, removed) = token_map_remove(&Some(json!(["t1"])), "nope");
        assert!(!removed);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(token_map_list(&Some(v)), vec!["t1"]);
    }
}
