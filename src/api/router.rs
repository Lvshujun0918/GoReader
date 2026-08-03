//! 路由：/health + /reader3/*（兼容 legacy API）

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Json;
use serde_json::{json, Value};

use crate::storage::Storage;

/// 统一返回结构（兼容 legacy ReturnData：isSuccess/errorMsg/data——camelCase）
#[derive(Debug, serde::Serialize)]
pub struct ReturnData {
    #[serde(rename = "isSuccess")]
    pub is_success: bool,
    #[serde(rename = "errorMsg")]
    pub error_msg: String,
    pub data: Value,
}

impl ReturnData {
    pub fn ok(data: Value) -> Self {
        Self {
            is_success: true,
            error_msg: String::new(),
            data,
        }
    }
    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            is_success: false,
            error_msg: msg.into(),
            data: Value::Null,
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub storage: Storage,
}

/// 构建路由
pub fn router(config: crate::AppConfig, storage: Storage) -> axum::Router {
    let state = AppState { storage };

    axum::Router::new()
        .route("/health", get(health))
        .route("/reader3/getBookshelf", get(get_bookshelf))
        .route("/reader3/login", post(login))
        .with_state(state)
        // TODO: 挂载 legacy 前端静态资源（rust-embed，兼容阶段复用 legacy dist）
}

async fn health() -> &'static str {
    "ok!"
}

/// 书架列表（占位：SQLite 迁移完成后读取 books 表）
async fn get_bookshelf(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Json<ReturnData> {
    let _refresh = params.get("refresh").map(|v| v == "1").unwrap_or(false);
    let _ = state.storage.pool;
    Json(ReturnData::ok(json!([])))
}

/// 登录（占位：用户表接入后实现）
async fn login(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Json<ReturnData> {
    let username = params.get("username").cloned().unwrap_or_default();
    let password = params.get("password").cloned().unwrap_or_default();
    let _ = (state.storage.pool, username, password);
    Json(ReturnData::err("暂未实现（骨架阶段）"))
}

/// 通用错误 JSON（axum 兜底）
pub fn internal_error(err: anyhow::Error) -> axum::response::Response {
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "isSuccess": false, "errorMsg": err.to_string(), "data": null })),
    )
        .into_response()
}
