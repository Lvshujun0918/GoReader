//! 路由：/health + /reader3/*（兼容 legacy API）

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::body::Body;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
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

    // /assets 静态资源（封面等：storage/assets/**，legacy 兼容）
    let assets_dir = config.storage_dir().join("assets");
    let assets_service = tower_http::services::ServeDir::new(assets_dir);

    // 前端静态资源（web-ui/dist，SPA fallback index.html——fallback 不强制 404 状态码）
    let web_dir = std::path::PathBuf::from(&config.web_root);
    let web_service = tower_http::services::ServeDir::new(&web_dir)
        .fallback(tower_http::services::ServeFile::new(web_dir.join("index.html")));

    axum::Router::new()
        .nest_service("/assets", assets_service)
        .route("/health", get(health))
        .route("/opds", get(opds_catalog))
        .route("/opds/", get(opds_catalog))
        .route("/opds/search", get(opds_search))
        .route("/opds/download/*id", get(opds_download))
        .route(
            "/reader3/uploadLocalBook",
            post(upload_local_book).layer(axum::extract::DefaultBodyLimit::max(100 * 1024 * 1024)),
        )
        .route("/reader3/deleteBook", post(delete_book))
        .route("/reader3/saveBook", post(save_book))
        // SPA fallback：未匹配路由 → webdav 分流 / API 404 / 前端
        .fallback(fallback_handler)
        .route("/reader3/getBookshelf", get(get_bookshelf))
        .route("/reader3/getBookSources", get(get_book_sources).post(get_book_sources))
        .route("/reader3/getBookSource", get(get_book_source).post(get_book_source))
        .route("/reader3/saveBookSource", post(save_book_source))
        .route("/reader3/saveBookSources", post(save_book_sources))
        .route("/reader3/deleteBookSource", post(delete_book_source))
        .route("/reader3/deleteBookSources", post(delete_book_sources))
        .route("/reader3/deleteAllBookSources", post(delete_all_book_sources))
        .route("/reader3/searchBook", get(search_book).post(search_book))
        .route("/reader3/searchBookMulti", get(search_book_multi).post(search_book_multi))
        .route("/reader3/getBookInfo", get(get_book_info).post(get_book_info))
        .route("/reader3/getBookToc", get(get_book_toc).post(get_book_toc))
        .route("/reader3/getBookContent", get(get_book_content).post(get_book_content))
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

/// POST/GET /reader3/getBookSource：单个书源（url 参数）
async fn get_book_source(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let body_json = body.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let url = param_of(&params, body_json.as_ref(), "bookSourceUrl");
    let url = if url.is_empty() {
        param_of(&params, body_json.as_ref(), "url")
    } else {
        url
    };
    if url.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    match state.storage.find_book_source(&namespace, &url).await {
        Ok(Some(s)) => Json(ReturnData::ok(serde_json::to_value(s).unwrap_or(serde_json::Value::Null))),
        Ok(None) => Json(ReturnData::err("书源不存在")),
        Err(_) => Json(ReturnData::err("系统错误")),
    }
}

/// POST /reader3/saveBookSource：保存单个书源（body = 完整书源 JSON）
async fn save_book_source(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let Some(body) = body else { return Json(ReturnData::err("参数错误")) };
    let source: crate::model::BookSource = match serde_json::from_slice(&body) {
        Ok(s) => s,
        Err(_) => return Json(ReturnData::err("参数错误")),
    };
    if source.book_source_url.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    match state.storage.save_book_source(&namespace, &source).await {
        Ok(_) => Json(ReturnData::ok(serde_json::Value::Null)),
        Err(e) => {
            tracing::error!("saveBookSource 失败: {e}");
            Json(ReturnData::err("保存失败"))
        }
    }
}

/// POST /reader3/saveBookSources：批量保存（body = 书源数组）
async fn save_book_sources(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let Some(body) = body else { return Json(ReturnData::err("参数错误")) };
    let sources: Vec<crate::model::BookSource> = match serde_json::from_slice(&body) {
        Ok(s) => s,
        Err(_) => return Json(ReturnData::err("参数错误")),
    };
    match state.storage.save_book_sources(&namespace, &sources).await {
        Ok(_) => Json(ReturnData::ok(serde_json::json!({ "count": sources.len() }))),
        Err(e) => {
            tracing::error!("saveBookSources 失败: {e}");
            Json(ReturnData::err("保存失败"))
        }
    }
}

/// POST /reader3/deleteBookSource：删除书源（body/query bookSourceUrl）
async fn delete_book_source(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let body_json = body.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let url = param_of(&params, body_json.as_ref(), "bookSourceUrl");
    let url = if url.is_empty() {
        param_of(&params, body_json.as_ref(), "url")
    } else {
        url
    };
    if url.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    match state.storage.delete_book_source(&namespace, &url).await {
        Ok(_) => Json(ReturnData::ok(serde_json::Value::Null)),
        Err(e) => {
            tracing::error!("deleteBookSource 失败: {e}");
            Json(ReturnData::err("删除失败"))
        }
    }
}

/// POST /reader3/deleteBookSources：批量删除（body = [bookSourceUrl]）
async fn delete_book_sources(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let Some(body) = body else { return Json(ReturnData::err("参数错误")) };
    let urls: Vec<String> = match serde_json::from_slice(&body) {
        Ok(u) => u,
        Err(_) => return Json(ReturnData::err("参数错误")),
    };
    let mut deleted = 0u64;
    for url in &urls {
        if let Ok(n) = state.storage.delete_book_source(&namespace, url).await {
            deleted += n;
        }
    }
    Json(ReturnData::ok(serde_json::json!({ "deleted": deleted })))
}

/// POST /reader3/deleteAllBookSources：清空用户书源
async fn delete_all_book_sources(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    match state.storage.delete_all_book_sources(&namespace).await {
        Ok(n) => Json(ReturnData::ok(serde_json::json!({ "deleted": n }))),
        Err(e) => {
            tracing::error!("deleteAllBookSources 失败: {e}");
            Json(ReturnData::err("删除失败"))
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

/// 解析书源参数（完整 JSON 或 URL 查库）
async fn resolve_book_source(
    state: &AppState,
    ns: &str,
    param: &str,
) -> Option<crate::model::BookSource> {
    if param.trim_start().starts_with('{') {
        serde_json::from_str(param).ok()
    } else {
        state.storage.find_book_source(ns, param).await.ok().flatten()
    }
}

/// 从 query/body 取参
fn param_of(params: &HashMap<String, String>, body: Option<&serde_json::Value>, key: &str) -> String {
    if let Some(b) = body {
        if let Some(v) = b.get(key).and_then(|v| v.as_str()) {
            return v.to_string();
        }
    }
    params.get(key).cloned().unwrap_or_default()
}

/// POST/GET /reader3/getBookInfo：书籍详情（ruleBookInfo）
async fn get_book_info(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let body_json = body.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let url = param_of(&params, body_json.as_ref(), "url");
    if url.is_empty() {
        return Json(ReturnData::err("请输入书籍链接"));
    }
    // 本地书（local://）——查书架返回信息，不走书源
    if url.starts_with("local://") {
        let books = match state.storage.list_books(&namespace).await {
            Ok(b) => b,
            Err(_) => return Json(ReturnData::err("系统错误")),
        };
        if let Some(book) = books.iter().find(|b| b.book_url == url) {
            let info = crate::model::book_chapter::BookInfo {
                name: book.name.clone(),
                author: book.author.clone(),
                kind: book.kind.clone(),
                intro: book.intro.clone(),
                cover_url: book
                    .custom_cover_url
                    .clone()
                    .or_else(|| book.cover_url.clone()),
                toc_url: Some(book.toc_url.clone()),
                book_url: book.book_url.clone(),
                origin: book.origin.clone(),
                origin_name: book.origin_name.clone(),
                language: book.language.clone(),
                publisher: book.publisher.clone(),
                published_at: book.published_at.clone(),
                ..Default::default()
            };
            return Json(ReturnData::ok(serde_json::to_value(info).unwrap_or(serde_json::Value::Null)));
        }
        return Json(ReturnData::err("未找到这本书（可能不在书架中）"));
    }
    let bs_param = param_of(&params, body_json.as_ref(), "bookSource");
    let Some(source) = resolve_book_source(&state, &namespace, &bs_param).await else {
        return Json(ReturnData::err("书源不存在"));
    };
    match crate::service::book::fetch_url(&url, &source).await {
        Ok(resp) => {
            let info = crate::service::book::analyze_book_info(&resp.body, &resp.url, &source, &url);
            Json(ReturnData::ok(serde_json::to_value(info).unwrap_or(serde_json::Value::Null)))
        }
        Err(e) => {
            tracing::error!("getBookInfo 失败 [{url}]: {e}");
            Json(ReturnData::err("获取详情失败"))
        }
    }
}

/// POST/GET /reader3/getBookToc：章节目录（ruleToc）
async fn get_book_toc(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let body_json = body.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let toc_url = param_of(&params, body_json.as_ref(), "tocUrl");
    let toc_url = if toc_url.is_empty() {
        param_of(&params, body_json.as_ref(), "url")
    } else {
        toc_url
    };
    if toc_url.is_empty() {
        return Json(ReturnData::err("请输入目录链接"));
    }
    // 本地书（local://）——不走书源解析
    if toc_url.starts_with("local://") {
        let book_id = toc_url
            .trim_start_matches("local://")
            .split('/')
            .next()
            .unwrap_or("");
        if let Some(ret) = get_book_toc_local(&state, &namespace, &format!("local://{book_id}")).await {
            return ret;
        }
        return Json(ReturnData::err("本地书目录不存在"));
    }
    let bs_param = param_of(&params, body_json.as_ref(), "bookSource");
    let Some(source) = resolve_book_source(&state, &namespace, &bs_param).await else {
        return Json(ReturnData::err("书源不存在"));
    };
    match crate::service::book::analyze_toc(&toc_url, &source, 20).await {
        Ok(chapters) => Json(ReturnData::ok(serde_json::to_value(chapters).unwrap_or(serde_json::Value::Null))),
        Err(e) => {
            tracing::error!("getBookToc 失败 [{toc_url}]: {e}");
            Json(ReturnData::err("获取目录失败"))
        }
    }
}

/// POST/GET /reader3/getBookContent：章节正文（ruleContent）
async fn get_book_content(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let body_json = body.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let chapter_url = param_of(&params, body_json.as_ref(), "chapterUrl");
    let chapter_url = if chapter_url.is_empty() {
        param_of(&params, body_json.as_ref(), "url")
    } else {
        chapter_url
    };
    if chapter_url.is_empty() {
        return Json(ReturnData::err("请输入章节链接"));
    }
    // 本地书（local://）——不走书源解析
    if chapter_url.starts_with("local://") {
        if let Some(ret) = get_book_content_local(&state, &chapter_url).await {
            return ret;
        }
        return Json(ReturnData::err("本地书章节不存在"));
    }
    let bs_param = param_of(&params, body_json.as_ref(), "bookSource");
    let Some(source) = resolve_book_source(&state, &namespace, &bs_param).await else {
        return Json(ReturnData::err("书源不存在"));
    };
    match crate::service::book::analyze_content(&chapter_url, &source, 5).await {
        Ok(content) => Json(ReturnData::ok(serde_json::json!({ "content": content }))),
        Err(e) => {
            tracing::error!("getBookContent 失败 [{chapter_url}]: {e}");
            Json(ReturnData::err("获取正文失败"))
        }
    }
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

/// 命名空间解析（OPDS：Basic 认证或非 secure default）
async fn opds_ns(state: &AppState, headers: &HeaderMap) -> Result<String, Response> {
    match crate::api::webdav::authenticate(&state.storage, headers).await {
        Some((_, ns, _)) => Ok(ns),
        None => Err(
            Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header("WWW-Authenticate", "Basic realm=\"reader\"")
                .body(Body::empty())
                .unwrap(),
        ),
    }
}

/// GET /opds：根目录
async fn opds_catalog(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    match opds_ns(&state, &headers).await {
        Ok(ns) => match crate::api::opds::catalog(&state.storage, &ns).await {
            Ok(xml) => Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/atom+xml;profile=opds-catalog;charset=utf-8")
                .body(Body::from(xml))
                .unwrap(),
            Err(e) => {
                tracing::error!("OPDS catalog 失败: {e}");
                Response::builder().status(StatusCode::INTERNAL_SERVER_ERROR).body(Body::empty()).unwrap()
            }
        },
        Err(resp) => resp,
    }
}

/// GET /opds/search?q=
async fn opds_search(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let q = params.get("q").cloned().unwrap_or_default();
    match opds_ns(&state, &headers).await {
        Ok(ns) => match crate::api::opds::search(&state.storage, &ns, &q).await {
            Ok(xml) => Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/atom+xml;profile=opds-catalog;charset=utf-8")
                .body(Body::from(xml))
                .unwrap(),
            Err(e) => {
                tracing::error!("OPDS search 失败: {e}");
                Response::builder().status(StatusCode::INTERNAL_SERVER_ERROR).body(Body::empty()).unwrap()
            }
        },
        Err(resp) => resp,
    }
}

/// GET /opds/books/{id}/download?format=txt
async fn opds_download(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let _format = params.get("format").cloned().unwrap_or_else(|| "txt".to_string());
    match opds_ns(&state, &headers).await {
        Ok(ns) => match crate::api::opds::download(&state.storage, &ns, &id, 100).await {
            Ok((name, bytes)) => Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "text/plain; charset=utf-8")
                .header("Content-Disposition", format!("attachment; filename=\"{}\"", name))
                .body(Body::from(bytes))
                .unwrap(),
            Err(e) => {
                tracing::warn!("OPDS 下载失败: {e}");
                Response::builder().status(StatusCode::NOT_FOUND).body(Body::empty()).unwrap()
            }
        },
        Err(resp) => resp,
    }
}

/// POST /reader3/deleteBook：移出书架（bookUrl）
async fn delete_book(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let body_json = body.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let book_url = param_of(&params, body_json.as_ref(), "bookUrl");
    let book_url = if book_url.is_empty() {
        param_of(&params, body_json.as_ref(), "url")
    } else {
        book_url
    };
    if book_url.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    match state.storage.delete_book(&namespace, &book_url).await {
        Ok(_) => Json(ReturnData::ok(serde_json::Value::Null)),
        Err(e) => {
            tracing::error!("deleteBook 失败: {e}");
            Json(ReturnData::err("删除失败"))
        }
    }
}

/// POST /reader3/saveBook：编辑书（bookUrl/name/author/coverUrl/group）
async fn save_book(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let body_json = body.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let book_url = param_of(&params, body_json.as_ref(), "bookUrl");
    if book_url.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    let name = body_json.as_ref().and_then(|b| b.get("name").and_then(|v| v.as_str()));
    let author = body_json.as_ref().and_then(|b| b.get("author").and_then(|v| v.as_str()));
    let cover_url = body_json.as_ref().and_then(|b| b.get("coverUrl").and_then(|v| v.as_str()));
    let group = body_json.as_ref().and_then(|b| b.get("group").and_then(|v| v.as_i64()));
    match state
        .storage
        .update_book(&namespace, &book_url, name, author, cover_url, group)
        .await
    {
        Ok(_) => Json(ReturnData::ok(serde_json::Value::Null)),
        Err(e) => {
            tracing::error!("saveBook 失败: {e}");
            Json(ReturnData::err("保存失败"))
        }
    }
}

/// POST /reader3/uploadLocalBook：导入本地书（multipart：file）
async fn upload_local_book(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut file_name = String::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            file_name = field.file_name().unwrap_or("book").to_string();
            if let Ok(bytes) = field.bytes().await {
                file_bytes = Some(bytes.to_vec());
            }
        }
    }
    let Some(bytes) = file_bytes else {
        return Json(ReturnData::err("未收到文件"));
    };
    if bytes.is_empty() {
        return Json(ReturnData::err("文件为空"));
    }

    let lower = file_name.to_lowercase();
    let imported = if lower.ends_with(".epub") {
        match crate::service::local_book::parse_epub(&bytes) {
            Ok(b) => b,
            Err(e) => return Json(ReturnData::err(format!("EPUB 解析失败：{e}"))),
        }
    } else if lower.ends_with(".txt") {
        crate::service::local_book::parse_txt(&bytes).unwrap_or_else(|e| {
            tracing::error!("TXT 解析失败: {e}");
            crate::service::local_book::ImportedBook {
                meta: Default::default(),
                chapters: vec![],
                cover: None,
                format: "txt".into(),
            }
        })
    } else {
        return Json(ReturnData::err("仅支持 EPUB/TXT"));
    };

    if imported.chapters.is_empty() {
        return Json(ReturnData::err("未解析到章节内容"));
    }

    let book_url = format!("local://{}", uuid::Uuid::new_v4());
    let book = crate::model::book_chapter::BookInfo {
        name: if imported.meta.title.is_empty() {
            file_name.trim_end_matches(".epub").trim_end_matches(".txt").to_string()
        } else {
            imported.meta.title.clone()
        },
        author: imported.meta.author.clone(),
        kind: imported.meta.subjects.first().cloned(),
        intro: imported.meta.description.clone(),
        language: imported.meta.language.clone(),
        publisher: imported.meta.publisher.clone(),
        published_at: imported.meta.published_at.clone(),
        toc_url: Some(format!("{book_url}/toc")),
        book_url: book_url.clone(),
        origin: "local".to_string(),
        origin_name: "本地书".to_string(),
        ..Default::default()
    };

    if let Err(e) = state.storage.save_local_book(&namespace, &book, &imported).await {
        tracing::error!("本地书入库失败: {e}");
        return Json(ReturnData::err("入库失败"));
    }

    if let Some(cover) = &imported.cover {
        let cover_dir = state.storage.config.storage_dir().join("assets").join(&namespace).join("covers");
        let _ = std::fs::create_dir_all(&cover_dir);
        let file_id = format!("{}.jpg", uuid::Uuid::new_v4());
        if std::fs::write(cover_dir.join(&file_id), cover).is_ok() {
            let _ = state
                .storage
                .update_book_cover(&namespace, &book_url, &format!("/assets/{namespace}/covers/{file_id}"))
                .await;
        }
    }

    tracing::info!("本地书导入 [{namespace}]：{}（{} 章）", book.name, imported.chapters.len());
    Json(ReturnData::ok(serde_json::to_value(book).unwrap_or(serde_json::Value::Null)))
}

/// 本地书目录（local://book_id/toc）
async fn get_book_toc_local(
    state: &AppState,
    _namespace: &str,
    book_url: &str,
) -> Option<Json<ReturnData>> {
    let chapters = state.storage.list_chapters(book_url).await.ok()?;
    let list: Vec<serde_json::Value> = chapters
        .iter()
        .map(|(idx, title)| {
            serde_json::json!({
                "title": title,
                "url": format!("{book_url}/{idx}"),
                "isVolume": false,
                "index": idx,
            })
        })
        .collect();
    Some(Json(ReturnData::ok(serde_json::Value::Array(list))))
}

/// 本地书正文（local://book_id/index）
async fn get_book_content_local(
    state: &AppState,
    chapter_url: &str,
) -> Option<Json<ReturnData>> {
    let rest = chapter_url.trim_start_matches("local://");
    let (book_id, idx_str) = rest.rsplit_once('/')?;
    let index: i64 = idx_str.parse().ok()?;
    let content = state
        .storage
        .get_chapter_content(&format!("local://{book_id}"), index)
        .await
        .ok()??;
    Some(Json(ReturnData::ok(serde_json::json!({ "content": content }))))
}

/// fallback：webdav 分流 / API 404 JSON / 前端 SPA（index.html）
async fn fallback_handler(
    State(state): State<AppState>,
    method: axum::http::Method,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let path = uri.path();
    tracing::debug!("fallback: {} {}", method, path);
    // WebDAV
    if path.starts_with("/reader3/webdav") {
        return crate::api::webdav::handle(&state.storage, method, path, &headers, body).await;
    }
    // 其他 /reader3 未匹配 → JSON 404
    if path.starts_with("/reader3") {
        return (
            axum::http::StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({"isSuccess": false, "errorMsg": "接口不存在", "data": null})),
        )
            .into_response();
    }
    // 前端静态资源（/static/** 等构建产物——按扩展名 MIME）
    let web_root = std::path::PathBuf::from(&state.storage.config.web_root);
    let rel = path.trim_start_matches('/');
    let file = web_root.join(rel);
    if file.is_file() {
        if let Ok(bytes) = tokio::fs::read(&file).await {
            return Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", mime_for(&file))
                .body(Body::from(bytes))
                .unwrap();
        }
    }
    // 前端 SPA：index.html
    let index = web_root.join("index.html");
    match tokio::fs::read(&index).await {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/html; charset=utf-8")
            .body(Body::from(bytes))
            .unwrap(),
        Err(_) => webdav_status_404(),
    }
}

/// 按扩展名推断 MIME（前端静态资源）
fn mime_for(file: &std::path::Path) -> &'static str {
    match file.extension().and_then(|e| e.to_str()) {
        Some("js") => "application/javascript; charset=utf-8",
        Some("mjs") => "application/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("map") => "application/json",
        _ => "application/octet-stream",
    }
}

fn webdav_status_404() -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::empty())
        .unwrap()
}
