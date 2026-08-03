//! 路由：/health + /reader3/*（兼容 legacy API）

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Json;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::model::User;
use crate::storage::Storage;
use crate::util::md5::gen_encrypted_password;
use crate::util::md5_encode;

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
        .route("/reader3/getBookSources", get(get_book_sources))
        .route("/reader3/searchBook", get(search_book).post(search_book))
        .route("/reader3/searchBookMulti", get(search_book_multi).post(search_book_multi))
        .route("/reader3/login", post(login))
        .with_state(state)
        // TODO: 挂载 legacy 前端静态资源（rust-embed，兼容阶段复用 legacy dist）
}

async fn health() -> &'static str {
    "ok!"
}

/// POST /reader3/login 请求体（兼容 legacy：username/password/isLogin/code）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginBody {
    username: Option<String>,
    password: Option<String>,
    is_login: Option<bool>,
    code: Option<String>,
}

/// POST /reader3/login：注册或登录，返回 formatUser（camelCase）
async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginBody>,
) -> Json<ReturnData> {
    let username = body.username.clone().unwrap_or_default();
    let password = body.password.clone().unwrap_or_default();
    let is_login = body.is_login.unwrap_or(false);

    if username.is_empty() {
        return Json(ReturnData::err("请输入用户名"));
    }
    if password.is_empty() {
        return Json(ReturnData::err("请输入密码"));
    }

    let user = match state.storage.find_user(&username).await {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("查询用户 {username} 失败: {e}");
            return Json(ReturnData::err("系统错误"));
        }
    };

    let Some(mut user) = user else {
        // 用户不存在
        if is_login {
            return Json(ReturnData::err("用户不存在"));
        }
        return register(&state, &username, &password, body.code.clone()).await;
    };

    // 用户已存在
    if !is_login {
        return Json(ReturnData::err("用户名已被占用"));
    }
    let encrypted = gen_encrypted_password(&password, &user.salt);
    if encrypted != user.password {
        return Json(ReturnData::err("密码错误"));
    }

    // 生成新 token 并更新会话
    let now = now_millis();
    let token = md5_encode(&format!("{username}{now}"));
    if let Err(e) = state.storage.update_user_session(&username, &token, now).await {
        tracing::error!("更新用户 {username} 会话失败: {e}");
        return Json(ReturnData::err("系统错误"));
    }
    user.token = token;
    user.last_login_at = now;
    tracing::info!("用户登录: {username}");
    Json(ReturnData::ok(format_user(&user)))
}

/// 自动注册（校验顺序与错误消息兼容 legacy）
async fn register(
    state: &AppState,
    username: &str,
    password: &str,
    code: Option<String>,
) -> Json<ReturnData> {
    let config = &state.storage.config;

    if username.len() < 5 {
        return Json(ReturnData::err("用户名不能低于5位"));
    }
    if (password.len() as i64) < config.min_user_password_length {
        return Json(ReturnData::err(format!(
            "密码不能低于{}位",
            config.min_user_password_length
        )));
    }
    if username == "default" {
        return Json(ReturnData::err("用户名不能为非法字符"));
    }
    let username_re = Regex::new("^[a-zA-Z0-9]+$").expect("static regex");
    if !username_re.is_match(username) {
        return Json(ReturnData::err("用户名只能由字母和数字组成"));
    }

    // 邀请码校验（配置了才要求）
    if !config.invite_code.is_empty() {
        let code = code.unwrap_or_default();
        if code.is_empty() {
            return Json(ReturnData::err("请输入邀请码"));
        }
        if code != config.invite_code {
            return Json(ReturnData::err("邀请码错误"));
        }
    }

    // 用户数上限（兼容 legacy：max(userLimit, 1)）
    let count = match state.storage.count_users().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("统计用户数失败: {e}");
            return Json(ReturnData::err("系统错误"));
        }
    };
    let user_limit = config.user_limit.max(1);
    if count >= user_limit {
        return Json(ReturnData::err("超过用户数上限"));
    }

    // 创建用户：salt = 8 位随机，默认权限取自 env（READER_APP_DEFAULTUSER*）
    use rand::Rng;
    let salt: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(8)
        .map(char::from)
        .collect();
    let now = now_millis();
    let token = md5_encode(&format!("{username}{now}"));
    let user = User {
        username: username.to_string(),
        password: gen_encrypted_password(password, &salt),
        salt,
        token: token.clone(),
        token_map: None,
        enable_webdav: config.default_user_enable_webdav,
        enable_local_store: config.default_user_enable_local_store,
        enable_book_source: config.default_user_enable_book_source,
        enable_rss_source: config.default_user_enable_rss_source,
        book_source_limit: config.default_user_book_source_limit,
        book_limit: config.default_user_book_limit,
        last_login_at: now,
        created_at: now,
        user_namespace: username.to_string(),
        raw_json: None,
    };
    if let Err(e) = state.storage.insert_user(&user).await {
        tracing::error!("创建用户 {username} 失败: {e}");
        return Json(ReturnData::err("系统错误"));
    }
    tracing::info!("新用户注册: {username}");
    Json(ReturnData::ok(format_user(&user)))
}

/// GET /reader3/getBookSources：按命名空间返回书源（legacy 语义：用户无书源回退 default）
async fn get_book_sources(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    match state.storage.get_book_sources(&namespace).await {
        Ok(sources) => Json(ReturnData::ok(serde_json::to_value(sources).unwrap_or(serde_json::Value::Null))),
        Err(e) => {
            tracing::error!("getBookSources [{namespace}] 失败: {e}");
            Json(ReturnData::err("系统错误"))
        }
    }
}

/// POST/GET /reader3/searchBook：单书源搜索（bookSource 参数：书源 URL 或完整 JSON）
async fn search_book(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    // 参数解析（POST body JSON 优先，GET query 兜底）
    let mut key = params.get("key").cloned().unwrap_or_default();
    let mut page = params.get("page").and_then(|v| v.parse().ok()).unwrap_or(1i64);
    let mut book_source_param = params.get("bookSource").cloned().unwrap_or_default();
    if let Some(body) = body {
        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&body) {
            if let Some(v) = json.get("key").and_then(|v| v.as_str()) {
                key = v.to_string();
            }
            if let Some(v) = json.get("page").and_then(|v| v.as_i64()) {
                page = v;
            }
            if let Some(v) = json.get("bookSource").and_then(|v| v.as_str()) {
                book_source_param = v.to_string();
            }
        }
    }
    if key.is_empty() {
        return Json(ReturnData::err("请输入搜索关键字"));
    }
    if book_source_param.is_empty() {
        return Json(ReturnData::err("未配置书源"));
    }

    // 解析书源：完整 JSON 或 URL（从库查）
    let source: Option<crate::model::BookSource> = if book_source_param.trim_start().starts_with('{') {
        serde_json::from_str(&book_source_param).ok()
    } else {
        state.storage.find_book_source(&namespace, &book_source_param).await.ok().flatten()
    };
    let Some(source) = source else {
        return Json(ReturnData::err("书源不存在"));
    };

    match crate::service::search::search_one_source(&source, &key, page).await {
        Ok(books) => Json(ReturnData::ok(serde_json::to_value(books).unwrap_or(serde_json::Value::Null))),
        Err(e) => {
            tracing::error!("搜索失败 [{}]: {e}", source.book_source_name);
            Json(ReturnData::err("搜索失败"))
        }
    }
}

/// POST/GET /reader3/searchBookMulti：多书源并发搜索（可选 bookSourceGroup 过滤）
async fn search_book_multi(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let mut key = params.get("key").cloned().unwrap_or_default();
    let mut page = params.get("page").and_then(|v| v.parse().ok()).unwrap_or(1i64);
    let mut group = params.get("bookSourceGroup").cloned().unwrap_or_default();
    let mut max_sources = params
        .get("maxSources")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(usize::MAX);
    if let Some(body) = body {
        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&body) {
            if let Some(v) = json.get("key").and_then(|v| v.as_str()) {
                key = v.to_string();
            }
            if let Some(v) = json.get("page").and_then(|v| v.as_i64()) {
                page = v;
            }
            if let Some(v) = json.get("bookSourceGroup").and_then(|v| v.as_str()) {
                group = v.to_string();
            }
            if let Some(v) = json.get("maxSources").and_then(|v| v.as_u64()) {
                max_sources = v as usize;
            }
        }
    }
    if key.is_empty() {
        return Json(ReturnData::err("请输入搜索关键字"));
    }
    let sources = match state.storage.get_book_sources(&namespace).await {
        Ok(s) => s,
        Err(_) => return Json(ReturnData::err("系统错误")),
    };
    let mut sources: Vec<crate::model::BookSource> = sources
        .into_iter()
        .filter(|s| s.enabled && s.search_url.is_some())
        .filter(|s| {
            group.is_empty()
                || s.book_source_group
                    .as_deref()
                    .map(|g| g.split(' ').any(|part| part == group))
                    .unwrap_or(false)
        })
        .collect();
    // 防炸：限制搜索源数量（前端按组搜索时通常远小于此）
    if sources.len() > max_sources {
        sources.truncate(max_sources);
    }
    if sources.is_empty() {
        return Json(ReturnData::err("未配置书源"));
    }

    // 并发搜索（限制并发数 8）
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
    let mut handles = Vec::with_capacity(sources.len());
    for source in sources {
        let sem = semaphore.clone();
        let key = key.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await;
            crate::service::search::search_one_source(&source, &key, page).await.unwrap_or_default()
        }));
    }
    let mut all: Vec<crate::service::search::SearchBook> = Vec::new();
    for h in handles {
        if let Ok(books) = h.await {
            all.extend(books);
        }
    }
    Json(ReturnData::ok(serde_json::to_value(all).unwrap_or(serde_json::Value::Null)))
}

/// GET /reader3/getBookshelf：按命名空间返回书架（user_namespace 取自 accessToken；非 secure 用 default）
async fn get_bookshelf(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Json<ReturnData> {
    let _refresh = params.get("refresh").map(|v| v == "1").unwrap_or(false);
    // TODO(后续切片): refresh=1 时刷新书籍更新信息（legacy getBookShelfBooks）
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    match state.storage.list_books(&namespace).await {
        Ok(books) => {
            tracing::info!("getBookshelf [{}]: {} 本", namespace, books.len());
            Json(ReturnData::ok(json!(books)))
        }
        Err(e) => {
            tracing::error!("查询书架 [{namespace}] 失败: {e}");
            Json(ReturnData::err("系统错误"))
        }
    }
}

/// 未登录返回（兼容 legacy checkAuth 失败：errorMsg=请登录后使用，data=NEED_LOGIN）
fn login_required() -> ReturnData {
    ReturnData {
        is_success: false,
        error_msg: "请登录后使用".to_string(),
        data: json!("NEED_LOGIN"),
    }
}

/// 解析命名空间：
/// - 非 secure → "default"
/// - secure → 从 query/header 解析 accessToken（username:token）并校验 token，合法则返回用户名
async fn resolve_namespace(
    state: &AppState,
    params: &HashMap<String, String>,
    headers: &HeaderMap,
) -> Result<String, ReturnData> {
    if !state.storage.config.secure {
        return Ok("default".to_string());
    }
    let access_token = params
        .get("accessToken")
        .cloned()
        .or_else(|| {
            headers
                .get("accessToken")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        })
        .or_else(|| {
            headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.strip_prefix("Bearer ").unwrap_or(v).to_string())
        });
    let Some(access_token) = access_token else {
        return Err(login_required());
    };
    let Some((username, token)) = access_token.split_once(':') else {
        return Err(login_required());
    };
    if username.is_empty() || token.is_empty() {
        return Err(login_required());
    }
    match state.storage.find_user(username).await {
        Ok(Some(user)) if !user.token.is_empty() && user.token == token => Ok(user.username),
        _ => Err(login_required()),
    }
}

/// formatUser：登录/注册返回结构（camelCase，兼容 legacy BaseController.formatUser）
fn format_user(user: &User) -> Value {
    json!({
        "username": user.username,
        "lastLoginAt": user.last_login_at,
        "accessToken": format!("{}:{}", user.username, user.token),
        "enableWebdav": user.enable_webdav,
        "enableLocalStore": user.enable_local_store,
        "enableBookSource": user.enable_book_source,
        "enableRssSource": user.enable_rss_source,
        "bookSourceLimit": user.book_source_limit,
        "bookLimit": user.book_limit,
        "createdAt": user.created_at,
    })
}

fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 通用错误 JSON（axum 兜底）
pub fn internal_error(err: anyhow::Error) -> axum::response::Response {
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "isSuccess": false, "errorMsg": err.to_string(), "data": null })),
    )
        .into_response()
}
