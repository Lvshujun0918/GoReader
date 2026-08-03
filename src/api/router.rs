//! 路由：/health + /reader3/*（兼容 legacy API）

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::body::{Body, Bytes};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use futures::StreamExt;
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

/// F-10：目录缓存 TTL（5 分钟）
const TOC_CACHE_TTL_MS: i64 = 5 * 60 * 1000;

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
        // F-4 远程书源订阅导入
        .route("/reader3/saveFromRemoteSource", post(save_from_remote_source))
        // F-13 书架单书
        .route("/reader3/getShelfBook", get(get_shelf_book).post(get_shelf_book))
        // F-25 退出登录
        .route("/reader3/logout", post(logout))
        // F-34 不活跃用户清理（secure + secureKey）
        .route("/reader3/clearInactiveUsers", post(clear_inactive_users))
        // F-39 手动备份到 WebDAV（书架数据 zip）
        .route("/reader3/backupToWebdav", post(backup_to_webdav))
        // F-38 文件管理（home 语义对齐 legacy FileController）
        .route("/reader3/file/list", get(crate::api::files::list))
        .route("/reader3/file/get", get(crate::api::files::get))
        .route("/reader3/file/save", post(crate::api::files::save))
        .route("/reader3/file/mkdir", post(crate::api::files::mkdir))
        .route("/reader3/file/download", get(crate::api::files::download))
        .route(
            "/reader3/file/upload",
            post(crate::api::files::upload).layer(axum::extract::DefaultBodyLimit::max(100 * 1024 * 1024)),
        )
        .route("/reader3/file/delete", post(crate::api::files::delete))
        .route("/reader3/deleteBook", post(delete_book))
        .route("/reader3/saveBook", post(save_book))
        .route("/reader3/saveBookProgress", post(save_book_progress))
        .route("/reader3/getExploreUrls", get(get_explore_urls).post(get_explore_urls))
        .route("/reader3/exploreBook", get(explore_book).post(explore_book))
        .route("/reader3/searchBookMultiSSE", get(search_book_multi_sse).post(search_book_multi_sse))
        .route("/reader3/saveBookmark", post(save_bookmark))
        .route("/reader3/getBookmarks", get(get_bookmarks).post(get_bookmarks))
        .route("/reader3/deleteBookmark", post(delete_bookmark))
        .route("/reader3/getBookGroups", get(get_book_groups).post(get_book_groups))
        .route("/reader3/saveBookGroup", post(save_book_group))
        .route("/reader3/updateBookGroupId", post(update_book_group_id))
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
        // RSS 模块（兼容 legacy rss 路由）
        .route("/reader3/getRssSources", get(get_rss_sources).post(get_rss_sources))
        .route("/reader3/saveRssSource", post(save_rss_source))
        .route("/reader3/deleteRssSource", post(delete_rss_source))
        .route("/reader3/getRssArticles", get(get_rss_articles).post(get_rss_articles))
        .route("/reader3/getRssArticle", get(get_rss_article).post(get_rss_article))
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
    // F-7 书源数上限（users.book_source_limit；limit<=0 不限制；已存在覆盖不计名额）
    if let Some(limit) = state.storage.book_source_limit_for(&namespace).await.ok().flatten() {
        if limit > 0 {
            let exists = state
                .storage
                .find_book_source(&namespace, &source.book_source_url)
                .await
                .ok()
                .flatten()
                .is_some();
            if !exists {
                let count = match state.storage.count_book_sources(&namespace).await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!("统计书源数失败: {e}");
                        return Json(ReturnData::err("系统错误"));
                    }
                };
                if count >= limit {
                    return Json(ReturnData::err("超过书源数上限"));
                }
            }
        }
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
    if sources.iter().any(|s| s.book_source_url.is_empty()) {
        return Json(ReturnData::err("参数错误"));
    }
    // F-7 书源数上限：逐条统计新增数（已存在覆盖不计名额），超限整批拒绝
    if let Some(limit) = state.storage.book_source_limit_for(&namespace).await.ok().flatten() {
        if limit > 0 {
            let mut new_count = 0i64;
            for s in &sources {
                let exists = state
                    .storage
                    .find_book_source(&namespace, &s.book_source_url)
                    .await
                    .ok()
                    .flatten()
                    .is_some();
                if !exists {
                    new_count += 1;
                }
            }
            let count = match state.storage.count_book_sources(&namespace).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("统计书源数失败: {e}");
                    return Json(ReturnData::err("系统错误"));
                }
            };
            if count + new_count > limit {
                return Json(ReturnData::err("超过书源数上限"));
            }
        }
    }
    match state.storage.save_book_sources(&namespace, &sources).await {
        Ok(_) => Json(ReturnData::ok(serde_json::json!({ "count": sources.len() }))),
        Err(e) => {
            tracing::error!("saveBookSources 失败: {e}");
            Json(ReturnData::err("保存失败"))
        }
    }
}

/// F-4：POST /reader3/saveFromRemoteSource：远程书源订阅导入
/// body/query {url} → 抓取 JSON → 校验书源数组 → save_book_sources 批量入库（已存在覆盖）
async fn save_from_remote_source(
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
        return Json(ReturnData::err("请输入远程书源链接"));
    }
    let headers_map: HashMap<String, String> = HashMap::new();
    let resp = match crate::service::crawler::fetch(&url, &headers_map, 15, "GET", None, None).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("saveFromRemoteSource 抓取失败 [{url}]: {e}");
            return Json(ReturnData::err("远程书源链接错误"));
        }
    };
    // 校验：必须是书源数组（每项含 bookSourceUrl）
    let json: serde_json::Value = match serde_json::from_str(&resp.body) {
        Ok(v) => v,
        Err(_) => return Json(ReturnData::err("书源数据格式错误")),
    };
    let sources: Vec<crate::model::BookSource> = match serde_json::from_value(json) {
        Ok(s) => s,
        Err(_) => return Json(ReturnData::err("书源数据格式错误")),
    };
    if sources.is_empty() || sources.iter().any(|s| s.book_source_url.trim().is_empty()) {
        return Json(ReturnData::err("书源数据格式错误"));
    }
    // F-7 书源数上限（同 saveBookSources）
    if let Some(limit) = state.storage.book_source_limit_for(&namespace).await.ok().flatten() {
        if limit > 0 {
            let mut new_count = 0i64;
            for s in &sources {
                let exists = state
                    .storage
                    .find_book_source(&namespace, &s.book_source_url)
                    .await
                    .ok()
                    .flatten()
                    .is_some();
                if !exists {
                    new_count += 1;
                }
            }
            match state.storage.count_book_sources(&namespace).await {
                Ok(count) if count + new_count > limit => {
                    return Json(ReturnData::err("超过书源数上限"));
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::error!("统计书源数失败: {e}");
                    return Json(ReturnData::err("系统错误"));
                }
            }
        }
    }
    match state.storage.save_book_sources(&namespace, &sources).await {
        Ok(_) => Json(ReturnData::ok(serde_json::json!({ "count": sources.len() }))),
        Err(e) => {
            tracing::error!("saveFromRemoteSource 入库失败: {e}");
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

// ---------------- RSS ----------------

/// GET/POST /reader3/getRssSources：RSS 源列表（用户命名空间，无则回退 default）
async fn get_rss_sources(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let _ = body;
    match state.storage.get_rss_sources(&namespace).await {
        Ok(list) => {
            let arr: Vec<Value> = list.iter().map(rss_source_json).collect();
            Json(ReturnData::ok(Value::Array(arr)))
        }
        Err(e) => {
            tracing::error!("getRssSources [{namespace}] 失败: {e}");
            Json(ReturnData::err("系统错误"))
        }
    }
}

/// POST /reader3/saveRssSource：保存 RSS 源（body = 完整 RSS 源 JSON，sourceUrl/sourceName 必填）
async fn save_rss_source(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let Some(body) = body else {
        return Json(ReturnData::err("参数错误"));
    };
    let body_str = String::from_utf8_lossy(&body).to_string();
    let mut source: crate::model::RssSource = match serde_json::from_slice(&body) {
        Ok(s) => s,
        Err(_) => return Json(ReturnData::err("参数错误")),
    };
    if source.source_url.trim().is_empty() {
        return Json(ReturnData::err("RSS链接不能为空"));
    }
    if source.source_name.trim().is_empty() {
        return Json(ReturnData::err("RSS名称不能为空"));
    }
    // raw_json：完整 JSON 原文保底（未知字段不丢，列表接口原样回吐）
    source.raw_json = Some(body_str);
    match state.storage.save_rss_source(&namespace, &source).await {
        Ok(()) => Json(ReturnData::ok(Value::String(String::new()))),
        Err(e) => {
            tracing::error!("saveRssSource 失败: {e}");
            Json(ReturnData::err("保存失败"))
        }
    }
}

/// POST /reader3/deleteRssSource：删除 RSS 源（rssSourceUrl 参数）
async fn delete_rss_source(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let body_json: Option<Value> = body
        .as_ref()
        .and_then(|b| serde_json::from_slice(b).ok());
    let url = param_of(&params, body_json.as_ref(), "rssSourceUrl");
    if url.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    match state.storage.delete_rss_source(&namespace, &url).await {
        Ok(_) => Json(ReturnData::ok(Value::String(String::new()))),
        Err(e) => {
            tracing::error!("deleteRssSource 失败: {e}");
            Json(ReturnData::err("删除失败"))
        }
    }
}

/// GET/POST /reader3/getRssArticles：抓取 feed → 解析文章列表 → 入库 → 返回
async fn get_rss_articles(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let body_json: Option<Value> = body
        .as_ref()
        .and_then(|b| serde_json::from_slice(b).ok());
    // rssSourceUrl 为主参数（兼容 legacy sourceUrl）
    let mut source_url = param_of(&params, body_json.as_ref(), "rssSourceUrl");
    if source_url.is_empty() {
        source_url = param_of(&params, body_json.as_ref(), "sourceUrl");
    }
    let page = body_json
        .as_ref()
        .and_then(|b| b.get("page").and_then(|v| v.as_i64()))
        .or_else(|| params.get("page").and_then(|v| v.parse().ok()))
        .unwrap_or(1);
    if source_url.is_empty() {
        return Json(ReturnData::err("RSS源链接不能为空"));
    }
    let Some(source) = state.storage.find_rss_source(&namespace, &source_url).await.ok().flatten() else {
        return Json(ReturnData::err("RSS源不存在"));
    };
    match crate::service::rss::fetch_articles(&source, page).await {
        Ok(articles) => {
            if let Err(e) = state.storage.save_rss_articles(&namespace, &articles).await {
                tracing::warn!("getRssArticles 入库失败: {e}");
            }
            Json(ReturnData::ok(serde_json::to_value(&articles).unwrap_or(Value::Null)))
        }
        Err(e) => {
            tracing::error!("getRssArticles 抓取失败 [{}]: {e}", source.source_url);
            Json(ReturnData::err("抓取失败"))
        }
    }
}

/// GET/POST /reader3/getRssArticle：文章正文（url 参数；feed 已带 content 直接返回，
/// 否则抓取文章网页用 CSS 选择器提取正文）
async fn get_rss_article(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let _ = namespace;
    let body_json: Option<Value> = body
        .as_ref()
        .and_then(|b| serde_json::from_slice(b).ok());
    let url = param_of(&params, body_json.as_ref(), "url");
    if url.is_empty() {
        return Json(ReturnData::err("RSS文章链接不能为空"));
    }
    // 已入库且带 content → 直接返回（content 字段随序列化输出）
    if let Ok(Some(article)) = state.storage.get_rss_article(&url).await {
        if article.content.as_deref().is_some_and(|c| !c.trim().is_empty()) {
            return Json(ReturnData::ok(serde_json::to_value(&article).unwrap_or(Value::Null)));
        }
    }
    // 未带正文 → 抓取网页提取正文
    match crate::service::rss::fetch_web_content(&url).await {
        Ok(content) => {
            let article = crate::model::RssArticle {
                url: url.clone(),
                title: String::new(),
                content: Some(content),
                ..Default::default()
            };
            Json(ReturnData::ok(serde_json::to_value(&article).unwrap_or(Value::Null)))
        }
        Err(e) => {
            tracing::error!("getRssArticle 正文提取失败 [{url}]: {e}");
            Json(ReturnData::err("正文提取失败"))
        }
    }
}

/// RSS 源 JSON 输出：raw_json（完整 legacy 字段）为基底，表列字段覆盖（名称/分组/启用状态）
fn rss_source_json(source: &crate::model::RssSource) -> Value {
    let mut v = source
        .raw_json
        .as_deref()
        .and_then(|r| serde_json::from_str::<Value>(r).ok())
        .unwrap_or_else(|| serde_json::to_value(source).unwrap_or(Value::Null));
    if let Some(obj) = v.as_object_mut() {
        obj.insert("sourceUrl".into(), Value::String(source.source_url.clone()));
        obj.insert("sourceName".into(), Value::String(source.source_name.clone()));
        obj.insert(
            "sourceGroup".into(),
            source
                .source_group
                .as_ref()
                .map(|g| Value::String(g.clone()))
                .unwrap_or(Value::Null),
        );
        obj.insert("enabled".into(), Value::Bool(source.enabled));
    }
    v
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
    // 本地书（local:// 或文件路径型）——查书架返回信息，不走书源
    let books = match state.storage.list_books(&namespace).await {
        Ok(b) => b,
        Err(_) => return Json(ReturnData::err("系统错误")),
    };
    let shelf_match = books.iter().find(|b| b.book_url == url);
    if shelf_match.is_some() && crate::service::local_book::is_local_book(&url, shelf_match.unwrap().origin.as_str()) {
        let book = shelf_match.unwrap();
        let info = crate::model::book_chapter::BookInfo {
            name: book.name.clone(),
            author: book.author.clone(),
            kind: book.kind.clone(),
            intro: book.intro.clone(),
            cover_url: book.custom_cover_url.clone().or_else(|| book.cover_url.clone()),
            toc_url: Some(if book.toc_url.is_empty() { book.book_url.clone() } else { book.toc_url.clone() }),
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
    if url.starts_with("local://") || url.ends_with(".txt") {
        // 书架无此本地书
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
    // 文件型本地书（legacy：bookUrl = storage/data/.../xx.txt）——读 TXT 分章
    if toc_url.ends_with(".txt") {
        if let Some(ret) = get_book_toc_file(&state, &toc_url).await {
            return ret;
        }
        return Json(ReturnData::err("本地书文件不存在"));
    }
    // F-10：目录缓存命中（TTL 5 分钟，同 tocUrl 直读）直接返回，不依赖书源
    if let Ok(Some(cached)) = state.storage.get_toc_cache(&toc_url, TOC_CACHE_TTL_MS).await {
        if let Ok(chapters) =
            serde_json::from_str::<Vec<crate::model::book_chapter::BookChapter>>(&cached)
        {
            tracing::debug!("getBookToc 命中目录缓存 [{toc_url}]");
            return Json(ReturnData::ok(
                serde_json::to_value(chapters).unwrap_or(serde_json::Value::Null),
            ));
        }
    }
    let bs_param = param_of(&params, body_json.as_ref(), "bookSource");
    let Some(source) = resolve_book_source(&state, &namespace, &bs_param).await else {
        return Json(ReturnData::err("书源不存在"));
    };
    match crate::service::book::analyze_toc(&toc_url, &source, 20).await {
        Ok(chapters) => {
            // F-10：抓取成功后缓存目录（book_url 未知时以 toc_url 为键）
            if let Ok(json) = serde_json::to_string(&chapters) {
                let _ = state.storage.cache_toc(&toc_url, &toc_url, &json).await;
            }
            Json(ReturnData::ok(
                serde_json::to_value(chapters).unwrap_or(serde_json::Value::Null),
            ))
        }
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
    // 文件型本地书：bookUrl#index
    if chapter_url.contains(".txt#") {
        if let Some(ret) = get_book_content_file(&state, &chapter_url).await {
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

/// F-13：GET/POST /reader3/getShelfBook：书架单书（url 参数；不存在报“书籍不存在”）
async fn get_shelf_book(
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
        return Json(ReturnData::err("书源链接不能为空"));
    }
    match state.storage.find_book(&namespace, &url).await {
        Ok(Some(book)) => Json(ReturnData::ok(
            serde_json::to_value(book).unwrap_or(serde_json::Value::Null),
        )),
        Ok(None) => Json(ReturnData::err("书籍不存在")),
        Err(e) => {
            tracing::error!("getShelfBook 失败 [{url}]: {e}");
            Json(ReturnData::err("系统错误"))
        }
    }
}

/// F-25：POST /reader3/logout：退出登录（清 token，token 立即失效）
async fn logout(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let _ = body;
    // 非 secure 模式无会话概念（legacy：不支持的操作）
    if !state.storage.config.secure {
        return Json(ReturnData::err("不支持的操作"));
    }
    let username = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    match state.storage.logout_user(&username).await {
        Ok(_) => {
            tracing::info!("用户退出登录: {username}");
            Json(ReturnData::ok(serde_json::Value::Null))
        }
        Err(e) => {
            tracing::error!("logout 失败: {e}");
            Json(ReturnData::err("系统错误"))
        }
    }
}

/// F-34：POST /reader3/clearInactiveUsers：清理不活跃用户（secure + secureKey 校验）
/// body/query：inactiveDay（默认 0）；简化：仅删 users 行，返回被删用户名列表
async fn clear_inactive_users(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let config = &state.storage.config;
    if !config.secure || config.secure_key.is_empty() {
        return Json(ReturnData::err("不支持的操作"));
    }
    // 需登录（legacy checkAuth）
    let username = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    // secureKey 管理校验（legacy checkManagerAuth）
    let body_json = body.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let secure_key = param_of(&params, body_json.as_ref(), "secureKey");
    if secure_key != config.secure_key {
        return Json(ReturnData {
            is_success: false,
            error_msg: "请输入管理密码".to_string(),
            data: json!("NEED_SECURE_KEY"),
        });
    }
    let inactive_day = params
        .get("inactiveDay")
        .and_then(|v| v.parse::<i64>().ok())
        .or_else(|| body_json.as_ref().and_then(|b| b.get("inactiveDay").and_then(|v| v.as_i64())))
        .unwrap_or(0);
    let before = now_millis() - inactive_day * 86400 * 1000;
    match state.storage.clear_inactive_users(before, Some(&username)).await {
        Ok(deleted) => {
            tracing::info!("clearInactiveUsers：删除 {} 个不活跃用户: {deleted:?}", deleted.len());
            Json(ReturnData::ok(json!({
                "deleted": deleted,
                "count": deleted.len(),
            })))
        }
        Err(e) => {
            tracing::error!("clearInactiveUsers 失败: {e}");
            Json(ReturnData::err("系统错误"))
        }
    }
}

/// F-39：POST /reader3/backupToWebdav：书架数据 zip 打包写入
/// storage/data/{ns}/webdav/legado/backup-{ts}.zip（secure 模式需开启 webdav 权限）
async fn backup_to_webdav(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    if state.storage.config.secure {
        let user = match state.storage.find_user(&namespace).await {
            Ok(Some(u)) => u,
            _ => return Json(ReturnData::err("请登录后使用")),
        };
        if !user.enable_webdav {
            return Json(ReturnData::err("未开启webdav功能"));
        }
    }
    match state.storage.create_backup_zip(&namespace).await {
        Ok(path) => Json(ReturnData::ok(json!({ "path": path }))),
        Err(e) => {
            tracing::error!("backupToWebdav 失败 [{namespace}]: {e}");
            Json(ReturnData::err("备份失败"))
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
pub(crate) async fn resolve_namespace(
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
    let max_chapters = params.get("maxChapters").and_then(|v| v.parse::<usize>().ok());
    match opds_ns(&state, &headers).await {
        Ok(ns) => match crate::api::opds::download(&state.storage, &ns, &id, max_chapters).await {
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

/// POST /reader3/saveBook：入架/编辑（完整 Book JSON）
///
/// 语义（对齐 legacy saveBook）：
/// - body = 完整 Book JSON（camelCase，如搜索结果/书架书），bookUrl 必填
/// - 书不在书架 → 全量 INSERT 入架（book_source 校验按任务规格简化为跳过）
/// - 书已在书架 → 按 body 出现的字段增量 UPDATE（未提供字段保持原值，兼容旧版四字段编辑）
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
    let Some(body) = body else {
        return Json(ReturnData::err("参数错误"));
    };
    let body_json: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return Json(ReturnData::err("参数错误")),
    };
    let book: crate::model::Book = match serde_json::from_value(body_json.clone()) {
        Ok(b) => b,
        Err(_) => return Json(ReturnData::err("参数错误")),
    };
    let book_url = if book.book_url.is_empty() {
        param_of(&params, Some(&body_json), "bookUrl")
    } else {
        book.book_url.clone()
    };
    if book_url.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }

    let exists = match state.storage.find_book(&namespace, &book_url).await {
        Ok(b) => b.is_some(),
        Err(e) => {
            tracing::error!("saveBook 查询失败: {e}");
            return Json(ReturnData::err("系统错误"));
        }
    };
    let result = if exists {
        // 编辑：按 body 出现的字段增量更新
        let patch = body_json.as_object().cloned().unwrap_or_default();
        state.storage.patch_book(&namespace, &book_url, &patch).await
    } else {
        // 新增入架：全量写入
        let mut b = book;
        b.book_url = book_url.clone();
        b.user_namespace = namespace.clone();
        if b.created_at == 0 {
            b.created_at = now_millis();
        }
        state.storage.upsert_book(&namespace, &b).await.map(|_| 1u64)
    };
    match result {
        Ok(_) => Json(ReturnData::ok(serde_json::Value::Null)),
        Err(e) => {
            tracing::error!("saveBook 失败: {e}");
            Json(ReturnData::err("保存失败"))
        }
    }
}

/// POST /reader3/saveBookProgress：保存阅读进度（body/query：bookUrl + durChapterIndex/durChapterPos/durChapterTime/durChapterTitle；兼容 legacy url/index 命名）
async fn save_book_progress(
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
        return Json(ReturnData::err("请输入书籍链接"));
    }
    let int_of = |keys: &[&str]| -> Option<i64> {
        for k in keys {
            if let Some(v) = params.get(*k).and_then(|v| v.parse::<i64>().ok()) {
                return Some(v);
            }
            if let Some(b) = body_json.as_ref() {
                if let Some(v) = b.get(*k).and_then(|v| v.as_i64()) {
                    return Some(v);
                }
            }
        }
        None
    };
    let index = int_of(&["durChapterIndex", "index"]).unwrap_or(0);
    let pos = int_of(&["durChapterPos"]).unwrap_or(0);
    let time = int_of(&["durChapterTime"]).unwrap_or_else(now_millis);
    let title = if params.contains_key("durChapterTitle") {
        params.get("durChapterTitle").cloned()
    } else {
        body_json
            .as_ref()
            .and_then(|b| b.get("durChapterTitle").and_then(|v| v.as_str()))
            .map(str::to_string)
    };
    match state
        .storage
        .update_book_progress(&namespace, &book_url, title.as_deref(), index, pos, time)
        .await
    {
        Ok(0) => Json(ReturnData::err("书籍未加入书架")),
        Ok(_) => Json(ReturnData::ok(serde_json::Value::Null)),
        Err(e) => {
            tracing::error!("saveBookProgress 失败: {e}");
            Json(ReturnData::err("保存失败"))
        }
    }
}

/// GET/POST /reader3/getExploreUrls：返回书源的 exploreUrl 集合（bookSource 参数：书源 URL 或完整 JSON）
async fn get_explore_urls(
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
    let bs_param = param_of(&params, body_json.as_ref(), "bookSource");
    let Some(source) = resolve_book_source(&state, &namespace, &bs_param).await else {
        return Json(ReturnData::err("书源不存在"));
    };
    let urls = crate::service::explore::parse_explore_urls(source.explore_url.as_deref().unwrap_or(""));
    Json(ReturnData::ok(serde_json::to_value(urls).unwrap_or(serde_json::Value::Null)))
}

/// GET/POST /reader3/exploreBook：探索/书海（url=ruleFindUrl + bookSource + page）
async fn explore_book(
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
    let url = if url.is_empty() {
        param_of(&params, body_json.as_ref(), "ruleFindUrl")
    } else {
        url
    };
    if url.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    let page: i64 = params
        .get("page")
        .and_then(|v| v.parse().ok())
        .or_else(|| body_json.as_ref().and_then(|b| b.get("page").and_then(|v| v.as_i64())))
        .unwrap_or(1);
    let bs_param = param_of(&params, body_json.as_ref(), "bookSource");
    let Some(source) = resolve_book_source(&state, &namespace, &bs_param).await else {
        return Json(ReturnData::err("书源不存在"));
    };
    // 分页占位（{{page}}/{page}）
    let target = url
        .replace("{{page}}", &page.to_string())
        .replace("{page}", &page.to_string());
    match crate::service::explore::explore_url(&target, &source).await {
        Ok(books) => Json(ReturnData::ok(
            serde_json::to_value(books).unwrap_or(serde_json::Value::Null),
        )),
        Err(e) => {
            tracing::error!("exploreBook 失败 [{target}]: {e}");
            Json(ReturnData::err("探索失败"))
        }
    }
}

/// GET/POST /reader3/searchBookMultiSSE：多书源流式搜索（SSE）
///
/// 参数：key/bookSourceGroup/lastIndex/searchSize/concurrentCount（POST body 或 GET query）
/// 输出：逐源 `event: book` + data {"lastIndex", "data":[SearchBook]}，结束 `event: end`；
/// 校验失败输出 `event: error`（兼容 legacy searchBookMultiSSE）
async fn search_book_multi_sse(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Response {
    // 参数解析（POST body JSON 优先，GET query 兜底）
    let mut key = params.get("key").cloned().unwrap_or_default();
    let mut group = params.get("bookSourceGroup").cloned().unwrap_or_default();
    let mut last_index = params
        .get("lastIndex")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(-1);
    let mut search_size = params
        .get("searchSize")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(50);
    let mut concurrent_count = params
        .get("concurrentCount")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(24);
    if let Some(body) = body {
        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&body) {
            if let Some(v) = json.get("key").and_then(|v| v.as_str()) {
                key = v.to_string();
            }
            if let Some(v) = json.get("bookSourceGroup").and_then(|v| v.as_str()) {
                group = v.to_string();
            }
            if let Some(v) = json.get("lastIndex").and_then(|v| v.as_i64()) {
                last_index = v;
            }
            if let Some(v) = json.get("searchSize").and_then(|v| v.as_u64()) {
                search_size = v as usize;
            }
            if let Some(v) = json.get("concurrentCount").and_then(|v| v.as_u64()) {
                concurrent_count = v as usize;
            }
        }
    }

    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return sse_error(ret),
    };
    if key.is_empty() {
        return sse_error(ReturnData::err("请输入搜索关键字"));
    }
    let sources = match state.storage.get_book_sources(&namespace).await {
        Ok(s) => s,
        Err(_) => return sse_error(ReturnData::err("系统错误")),
    };
    let sources: Vec<crate::model::BookSource> = sources
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
    if sources.is_empty() {
        return sse_error(ReturnData::err("未配置书源"));
    }
    if last_index >= sources.len() as i64 - 1 {
        return sse_error(ReturnData::err("没有更多了"));
    }
    search_size = search_size.max(1);
    concurrent_count = concurrent_count.max(1);

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::convert::Infallible>>(
        concurrent_count.min(64).max(4),
    );
    let total = sources.len() as i64;
    let start = (last_index + 1).max(0) as usize;
    let end = (start + search_size).min(sources.len());
    tokio::spawn(async move {
        // 并发受控（semaphore），结果到达即推送（FuturesUnordered 完成顺序）
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrent_count));
        let mut tasks = futures::stream::FuturesUnordered::new();
        for i in start..end {
            let sem = sem.clone();
            let key = key.clone();
            let source = sources[i].clone();
            tasks.push(Box::pin(async move {
                let _permit = sem.acquire().await;
                let books =
                    crate::service::search::search_one_source(&source, &key, 1).await.unwrap_or_default();
                let payload = serde_json::json!({ "lastIndex": i as i64, "data": books });
                (i as i64, format!("event: book\ndata: {payload}\n\n"))
            }));
        }
        let mut last = last_index;
        while let Some((i, text)) = tasks.next().await {
            last = i;
            if tx.send(Ok(Bytes::from(text))).await.is_err() {
                break; // 客户端断开
            }
        }
        let end_payload = serde_json::json!({ "lastIndex": last, "isEnd": last >= total - 1 });
        let _ = tx
            .send(Ok(Bytes::from(format!("event: end\ndata: {end_payload}\n\n"))))
            .await;
    });

    // mpsc receiver → TryStream → SSE body
    let stream = futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|item| (item, rx)) });
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream; charset=utf-8")
        .header("Cache-Control", "no-cache")
        .body(Body::from_stream(stream))
        .unwrap()
}

/// SSE 错误事件（兼容 legacy：event: error + data: ReturnData）
fn sse_error(ret: ReturnData) -> Response {
    let payload = serde_json::to_string(&ret).unwrap_or_default();
    let body = format!("event: error\ndata: {payload}\n\n");
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream; charset=utf-8")
        .header("Cache-Control", "no-cache")
        .body(Body::from(body))
        .unwrap()
}

/// POST /reader3/saveBookmark：保存书签（body：bookUrl/title/paragraphIndex/chapterIndex/createdAt）
async fn save_bookmark(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let Some(body) = body else {
        return Json(ReturnData::err("参数错误"));
    };
    let mut bookmark: crate::model::Bookmark = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(_) => return Json(ReturnData::err("参数错误")),
    };
    if bookmark.book_url.is_empty() || bookmark.title.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    bookmark.user_namespace = namespace.clone();
    if bookmark.created_at == 0 {
        bookmark.created_at = now_millis();
    }
    match state.storage.save_bookmark(&namespace, &bookmark).await {
        Ok(_) => Json(ReturnData::ok(serde_json::Value::Null)),
        Err(e) => {
            tracing::error!("saveBookmark 失败: {e}");
            Json(ReturnData::err("保存失败"))
        }
    }
}

/// GET/POST /reader3/getBookmarks：书签列表（bookUrl 参数）
async fn get_bookmarks(
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
    match state.storage.list_bookmarks(&namespace, &book_url).await {
        Ok(bookmarks) => Json(ReturnData::ok(
            serde_json::to_value(bookmarks).unwrap_or(serde_json::Value::Null),
        )),
        Err(e) => {
            tracing::error!("getBookmarks 失败: {e}");
            Json(ReturnData::err("系统错误"))
        }
    }
}

/// POST /reader3/deleteBookmark：删除书签（body：bookUrl + title）
async fn delete_bookmark(
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
    let title = param_of(&params, body_json.as_ref(), "title");
    if book_url.is_empty() || title.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    match state.storage.delete_bookmark(&namespace, &book_url, &title).await {
        Ok(_) => Json(ReturnData::ok(serde_json::Value::Null)),
        Err(e) => {
            tracing::error!("deleteBookmark 失败: {e}");
            Json(ReturnData::err("删除失败"))
        }
    }
}

/// GET/POST /reader3/getBookGroups：书架分组列表
async fn get_book_groups(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    match state.storage.list_book_groups(&namespace).await {
        Ok(groups) => Json(ReturnData::ok(
            serde_json::to_value(groups).unwrap_or(serde_json::Value::Null),
        )),
        Err(e) => {
            tracing::error!("getBookGroups 失败: {e}");
            Json(ReturnData::err("系统错误"))
        }
    }
}

/// POST /reader3/saveBookGroup：保存分组（body：id?/name/order?；id>0 覆盖，否则新建）
async fn save_book_group(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let Some(body) = body else {
        return Json(ReturnData::err("参数错误"));
    };
    let group: crate::model::BookGroup = match serde_json::from_slice(&body) {
        Ok(g) => g,
        Err(_) => return Json(ReturnData::err("参数错误")),
    };
    if group.name.is_empty() {
        return Json(ReturnData::err("分组名称不能为空"));
    }
    match state.storage.save_book_group(&namespace, &group).await {
        Ok(saved) => Json(ReturnData::ok(
            serde_json::to_value(saved).unwrap_or(serde_json::Value::Null),
        )),
        Err(e) => {
            tracing::error!("saveBookGroup 失败: {e}");
            Json(ReturnData::err("保存失败"))
        }
    }
}

/// POST /reader3/updateBookGroupId：书设分组（body：bookUrl + group）
async fn update_book_group_id(
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
    let group = params
        .get("group")
        .and_then(|v| v.parse::<i64>().ok())
        .or_else(|| body_json.as_ref().and_then(|b| b.get("group").and_then(|v| v.as_i64())))
        .unwrap_or(-1);
    if book_url.is_empty() || group < 0 {
        return Json(ReturnData::err("参数错误"));
    }
    match state.storage.update_book_group_id(&namespace, &book_url, group).await {
        Ok(_) => Json(ReturnData::ok(serde_json::Value::Null)),
        Err(e) => {
            tracing::error!("updateBookGroupId 失败: {e}");
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

/// storage 内安全路径解析（防穿越）
fn resolve_storage_path(storage_dir: &std::path::Path, book_url: &str) -> Option<std::path::PathBuf> {
    let rel = book_url.trim_start_matches("storage/");
    let candidate = storage_dir.join(rel);
    let abs = candidate.canonicalize().unwrap_or_else(|_| candidate.clone());
    let root = storage_dir.canonicalize().unwrap_or_else(|_| storage_dir.to_path_buf());
    if abs.starts_with(&root) && abs.is_file() {
        Some(abs)
    } else {
        None
    }
}

/// 文件型本地书目录：读 TXT 分章 → 章节列表（chapterUrl = bookUrl#index）
async fn get_book_toc_file(state: &AppState, book_url: &str) -> Option<Json<ReturnData>> {
    let path = resolve_storage_path(&state.storage.config.storage_dir(), book_url)?;
    let imported = crate::service::local_book::parse_txt_file(&path).ok()?;
    let list: Vec<serde_json::Value> = imported
        .chapters
        .iter()
        .enumerate()
        .map(|(i, c)| {
            serde_json::json!({
                "title": c.title,
                "url": format!("{book_url}#{i}"),
                "isVolume": false,
                "index": i,
            })
        })
        .collect();
    Some(Json(ReturnData::ok(serde_json::Value::Array(list))))
}

/// 文件型本地书正文：bookUrl#index → 读 TXT → 提取章节
async fn get_book_content_file(state: &AppState, chapter_url: &str) -> Option<Json<ReturnData>> {
    let (book_part, idx_part) = chapter_url.rsplit_once('#')?;
    let index: usize = idx_part.parse().ok()?;
    let path = resolve_storage_path(&state.storage.config.storage_dir(), book_part)?;
    let imported = crate::service::local_book::parse_txt_file(&path).ok()?;
    let content = imported.chapters.get(index)?.content.clone();
    Some(Json(ReturnData::ok(serde_json::json!({ "content": content }))))
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
    // 前端静态资源（/static/** 等构建产物——按扩展名 MIME，防路径穿越）
    let web_root = std::path::PathBuf::from(&state.storage.config.web_root);
    let rel = path.trim_start_matches('/');
    let file = web_root.join(rel);
    let file_abs = file.canonicalize().unwrap_or_else(|_| file.clone());
    let root_abs = web_root.canonicalize().unwrap_or_else(|_| web_root.clone());
    if file_abs.starts_with(&root_abs) && file.is_file() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Bytes;
    use axum::extract::State as AxumState;

    /// 独立临时目录存储（避免污染真实 storage/reader.db）
    async fn test_state(tag: &str) -> (AppState, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "reader-router-test-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut config = crate::AppConfig::from_env();
        config.work_dir = dir.to_string_lossy().into_owned();
        let storage = crate::storage::init(&config).await.unwrap();
        (AppState { storage }, dir)
    }

    async fn cleanup(state: AppState, dir: std::path::PathBuf) {
        state.storage.pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn source_json(i: i64) -> serde_json::Value {
        serde_json::json!({
            "bookSourceUrl": format!("https://s{i}.com"),
            "bookSourceName": format!("源{i}"),
        })
    }

    /// F-7：saveBookSource / saveBookSources 书源数上限（users.book_source_limit）
    #[tokio::test]
    async fn test_save_book_source_limit() {
        let (state, dir) = test_state("bslimit").await;
        state
            .storage
            .insert_user(&User {
                username: "default".into(),
                book_source_limit: 2,
                ..Default::default()
            })
            .await
            .unwrap();

        // 单个保存：前两个成功
        for i in 1..=2 {
            let body = Bytes::from(source_json(i).to_string());
            let ret = save_book_source(
                AxumState(state.clone()),
                Query(HashMap::new()),
                HeaderMap::new(),
                Some(body),
            )
            .await;
            assert!(ret.0.is_success, "第 {i} 个书源应保存成功: {}", ret.0.error_msg);
        }
        // 第三个超限
        let body = Bytes::from(source_json(3).to_string());
        let ret = save_book_source(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "超过书源数上限");
        // 覆盖已存在的不计名额
        let body = Bytes::from(source_json(1).to_string());
        let ret = save_book_source(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "覆盖已存在书源应成功");

        // 批量：3 个新源超限整批拒绝
        let batch = serde_json::json!([source_json(10), source_json(11), source_json(12)]);
        let ret = save_book_sources(
            AxumState(state.clone()),
            Query(HashMap::new()),
            HeaderMap::new(),
            Some(Bytes::from(batch.to_string())),
        )
        .await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "超过书源数上限");
        // 上限提到 3：批量 1 个新源 + 已存在源 → 成功
        state
            .storage
            .insert_user(&User {
                username: "default".into(),
                book_source_limit: 3,
                ..Default::default()
            })
            .await
            .unwrap();
        let batch = serde_json::json!([source_json(1), source_json(10)]);
        let ret = save_book_sources(
            AxumState(state.clone()),
            Query(HashMap::new()),
            HeaderMap::new(),
            Some(Bytes::from(batch.to_string())),
        )
        .await;
        assert!(ret.0.is_success, "新增 1 个不超限应成功: {}", ret.0.error_msg);

        // limit=0（无用户行/不限制）→ 放行
        state
            .storage
            .insert_user(&User {
                username: "default".into(),
                book_source_limit: 0,
                ..Default::default()
            })
            .await
            .unwrap();
        let body = Bytes::from(source_json(20).to_string());
        let ret = save_book_source(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success);

        cleanup(state, dir).await;
    }

    /// F-13：getShelfBook 返回书架单书 / 不存在报“书籍不存在”
    #[tokio::test]
    async fn test_get_shelf_book() {
        let (state, dir) = test_state("shelf").await;
        state
            .storage
            .upsert_book(
                "default",
                &crate::model::Book {
                    book_url: "https://book.com/a".into(),
                    name: "测试书".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let params: HashMap<String, String> =
            [("url".into(), "https://book.com/a".into())].into_iter().collect();
        let ret = get_shelf_book(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(ret.0.is_success);
        assert_eq!(ret.0.data["bookUrl"], "https://book.com/a");
        assert_eq!(ret.0.data["name"], "测试书");

        let params: HashMap<String, String> =
            [("url".into(), "https://nope.com".into())].into_iter().collect();
        let ret = get_shelf_book(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "书籍不存在");

        cleanup(state, dir).await;
    }

    /// F-25：logout——非 secure 拒绝；secure 清 token 且 token 立即失效
    #[tokio::test]
    async fn test_logout_clears_token() {
        let (state, dir) = test_state("logout").await;
        // 非 secure → 不支持的操作
        let ret = logout(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "不支持的操作");

        // secure：登录用户 logout 后 token 清空、旧 token 失效
        let mut state = state;
        state.storage.config.secure = true;
        state
            .storage
            .insert_user(&User {
                username: "alice".into(),
                token: "tok123".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        let params: HashMap<String, String> =
            [("accessToken".into(), "alice:tok123".into())].into_iter().collect();
        let ret = logout(AxumState(state.clone()), Query(params.clone()), HeaderMap::new(), None).await;
        assert!(ret.0.is_success, "logout 应成功: {}", ret.0.error_msg);
        assert!(state.storage.find_user("alice").await.unwrap().unwrap().token.is_empty());

        // 旧 token 再次访问 → NEED_LOGIN
        let ret = logout(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.data, json!("NEED_LOGIN"));

        cleanup(state, dir).await;
    }

    /// F-34：clearInactiveUsers——secureKey 校验 + 仅删超期用户（调用者受保护）
    #[tokio::test]
    async fn test_clear_inactive_users() {
        let (state, dir) = test_state("inactive").await;
        let mut state = state;
        state.storage.config.secure = true;
        state.storage.config.secure_key = "sk".into();
        let mk = |name: &str, last: i64| User {
            username: name.into(),
            token: "t".into(),
            last_login_at: last,
            ..Default::default()
        };
        state.storage.insert_user(&mk("old", 1000)).await.unwrap();
        state.storage.insert_user(&mk("new", now_millis())).await.unwrap();

        // 缺 secureKey → NEED_SECURE_KEY（需先登录，legacy checkAuth 优先）
        let body = Bytes::from(r#"{"inactiveDay":1}"#);
        let auth_params: HashMap<String, String> =
            [("accessToken".into(), "new:t".into())].into_iter().collect();
        let ret = clear_inactive_users(
            AxumState(state.clone()),
            Query(auth_params),
            HeaderMap::new(),
            Some(body.clone()),
        )
        .await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.data, json!("NEED_SECURE_KEY"));

        // 带 secureKey（登录 accessToken）→ 删除 old，保留 new
        let params: HashMap<String, String> = [
            ("accessToken".into(), "new:t".into()),
            ("secureKey".into(), "sk".into()),
        ]
        .into_iter()
        .collect();
        let ret = clear_inactive_users(AxumState(state.clone()), Query(params), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "清理应成功: {}", ret.0.error_msg);
        assert_eq!(ret.0.data["deleted"], json!(["old"]));
        assert_eq!(ret.0.data["count"], 1);
        assert!(state.storage.find_user("old").await.unwrap().is_none());
        assert!(state.storage.find_user("new").await.unwrap().is_some());

        cleanup(state, dir).await;
    }

    /// F-39：backupToWebdav——secure 未开启 webdav 拒绝；成功返回 zip 路径
    #[tokio::test]
    async fn test_backup_to_webdav() {
        let (state, dir) = test_state("backup").await;
        let mut state = state;
        state.storage.config.secure = true;
        state
            .storage
            .insert_user(&User {
                username: "alice".into(),
                token: "t1".into(),
                enable_webdav: false,
                ..Default::default()
            })
            .await
            .unwrap();
        let params: HashMap<String, String> =
            [("accessToken".into(), "alice:t1".into())].into_iter().collect();
        // 未开启 webdav → 拒绝
        let ret = backup_to_webdav(AxumState(state.clone()), Query(params.clone()), HeaderMap::new(), None).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "未开启webdav功能");
        // 开启 webdav → 打包成功，zip 在 webdav/legado 下
        state
            .storage
            .insert_user(&User {
                username: "alice".into(),
                token: "t1".into(),
                enable_webdav: true,
                ..Default::default()
            })
            .await
            .unwrap();
        let ret = backup_to_webdav(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(ret.0.is_success, "备份应成功: {}", ret.0.error_msg);
        let path = ret.0.data["path"].as_str().expect("应返回 zip 路径");
        assert!(
            path.contains("legado") && path.contains("backup-") && path.ends_with(".zip"),
            "路径: {path}"
        );
        assert!(std::path::Path::new(path).exists());

        cleanup(state, dir).await;
    }
}
