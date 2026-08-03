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
        // 弱网优化：响应压缩（gzip/brotli）
        .layer(tower_http::compression::CompressionLayer::new())
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
        // F-32 用户管理（secure + secureKey）
        .route("/reader3/getUsers", get(get_users).post(get_users))
        .route("/reader3/updateUser", post(update_user))
        .route("/reader3/deleteUser", post(delete_user))
        .route("/reader3/resetUserPassword", post(reset_user_password))
        // F-25 TTS：Edge 语音 + HttpTTS + 语音列表
        .route("/reader3/getTTSVoices", get(get_tts_voices).post(get_tts_voices))
        .route("/reader3/tts", get(tts_synthesize).post(tts_synthesize))
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
        .route("/reader3/getExploreSources", get(get_explore_sources).post(get_explore_sources))
        .route("/reader3/getExploreUrls", get(get_explore_urls).post(get_explore_urls))
        .route("/reader3/exploreBook", get(explore_book).post(explore_book))
        .route("/reader3/searchBookMultiSSE", get(search_book_multi_sse).post(search_book_multi_sse))
        .route("/reader3/saveBookmark", post(save_bookmark))
        .route("/reader3/getBookmarks", get(get_bookmarks).post(get_bookmarks))
        .route("/reader3/deleteBookmark", post(delete_bookmark))
        .route("/reader3/getBookGroups", get(get_book_groups).post(get_book_groups))
        .route("/reader3/saveBookGroup", post(save_book_group))
        .route("/reader3/updateBookGroupId", post(update_book_group_id))
        .route("/reader3/deleteBookGroup", post(delete_book_group))
        // 命名兼容批（legacy 别名路由——外部客户端兼容）
        .route("/reader3/getChapterList", get(get_book_toc).post(get_book_toc))
        .route("/reader3/getRssContent", get(get_rss_article).post(get_rss_article))
        .route("/reader3/getUserList", get(get_users).post(get_users))
        .route("/reader3/getBookGroupList", get(get_book_groups).post(get_book_groups))
        .route("/reader3/saveBookGroupName", post(save_book_group))
        .route("/reader3/updateBookGroup", post(save_book_group))
        // F-28 替换规则
        .route("/reader3/getReplaceRules", get(get_replace_rules).post(get_replace_rules))
        .route("/reader3/saveReplaceRule", post(save_replace_rule))
        .route("/reader3/saveReplaceRules", post(save_replace_rules))
        .route("/reader3/deleteReplaceRule", post(delete_replace_rule))
        // F-26 HttpTTS 听书源管理
        .route("/reader3/getHttpTTSList", get(get_http_tts_list).post(get_http_tts_list))
        .route("/reader3/saveHttpTTS", post(save_http_tts))
        .route("/reader3/deleteHttpTTS", post(delete_http_tts))
        // 自定义 TXT 目录规则（对齐 legado TxtTocRule）
        .route("/reader3/getTxtTocRules", get(get_txt_toc_rules).post(get_txt_toc_rules))
        .route("/reader3/saveTxtTocRule", post(save_txt_toc_rule))
        .route("/reader3/deleteTxtTocRule", post(delete_txt_toc_rule))
        .route("/reader3/importDefaultTxtTocRules", post(import_default_txt_toc_rules))
        // 系统信息 + 书源导出
        .route("/reader3/getSystemInfo", get(get_system_info))
        .route("/reader3/exportBookSources", get(export_book_sources))
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
        // 缓存管理 + 全书搜索 + 书源订阅
        .route("/reader3/getCacheInfo", get(get_cache_info).post(get_cache_info))
        .route("/reader3/clearCache", post(clear_cache))
        .route("/reader3/searchBookContent", get(search_book_content).post(search_book_content))
        .route("/reader3/getSourceSubs", get(get_source_subs).post(get_source_subs))
        .route("/reader3/saveSourceSub", post(save_source_sub))
        .route("/reader3/deleteSourceSub", post(delete_source_sub))
        .route("/reader3/refreshSourceSub", post(refresh_source_sub))
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

// ---------------- 缓存管理 ----------------

/// GET/POST /reader3/getCacheInfo：缓存统计（toc_cache 行数 / book_chapters 行数 /
/// 章节近似大小 sum length(content) / 目录缓存大小 / 总大小）
async fn get_cache_info(State(state): State<AppState>) -> Json<ReturnData> {
    match state.storage.get_cache_info().await {
        Ok(info) => Json(ReturnData::ok(
            serde_json::to_value(info).unwrap_or(serde_json::Value::Null),
        )),
        Err(e) => {
            tracing::error!("getCacheInfo 失败: {e}");
            Json(ReturnData::err("系统错误"))
        }
    }
}

/// POST /reader3/clearCache：清空缓存（body/query {type: "toc"|"chapters"|"all"}）
async fn clear_cache(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let mut cache_type = params.get("type").cloned().unwrap_or_default();
    if let Some(body) = body {
        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&body) {
            if let Some(v) = json.get("type").and_then(|v| v.as_str()) {
                cache_type = v.to_string();
            }
        }
    }
    if cache_type != "toc" && cache_type != "chapters" && cache_type != "all" {
        return Json(ReturnData::err("参数错误"));
    }
    match state.storage.clear_cache(&cache_type).await {
        Ok((toc_deleted, chapters_deleted)) => Json(ReturnData::ok(serde_json::json!({
            "deletedToc": toc_deleted,
            "deletedChapters": chapters_deleted,
        }))),
        Err(e) => {
            tracing::error!("clearCache 失败: {e}");
            Json(ReturnData::err("系统错误"))
        }
    }
}

// ---------------- 全书搜索（仅本地书） ----------------

/// GET/POST /reader3/searchBookContent：全书搜索（params key + bookUrl）
/// 本地书：book_chapters 表 LIKE 匹配正文 → data: [{chapterIndex, title, snippet}]
/// 书源书：返回提示“仅支持本地书内容搜索”
async fn search_book_content(
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
    let key = param_of(&params, body_json.as_ref(), "key");
    let book_url = param_of(&params, body_json.as_ref(), "bookUrl");
    if key.is_empty() {
        return Json(ReturnData::err("请输入搜索关键字"));
    }
    if book_url.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    // 本地书判定：书架书（origin/url 形态）或 book_chapters 已有章节
    let shelf = state
        .storage
        .find_book(&namespace, &book_url)
        .await
        .ok()
        .flatten();
    let has_chapters = state.storage.count_chapters(&book_url).await.unwrap_or(0) > 0;
    match &shelf {
        Some(book) => {
            if !crate::service::local_book::is_local_book(&book.book_url, &book.origin) {
                return Json(ReturnData::err("仅支持本地书内容搜索"));
            }
        }
        None if !has_chapters => return Json(ReturnData::err("书籍不存在")),
        None => {}
    }
    match state.storage.search_book_content(&book_url, &key, 100).await {
        Ok(hits) => Json(ReturnData::ok(
            serde_json::to_value(hits).unwrap_or(serde_json::Value::Null),
        )),
        Err(e) => {
            tracing::error!("searchBookContent 失败 [{book_url}]: {e}");
            Json(ReturnData::err("搜索失败"))
        }
    }
}

// ---------------- 书源订阅 ----------------

/// GET/POST /reader3/getSourceSubs：订阅列表（url/name/enabled）
async fn get_source_subs(
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
    match state.storage.get_source_subs(&namespace).await {
        Ok(list) => Json(ReturnData::ok(
            serde_json::to_value(list).unwrap_or(serde_json::Value::Null),
        )),
        Err(e) => {
            tracing::error!("getSourceSubs [{namespace}] 失败: {e}");
            Json(ReturnData::err("系统错误"))
        }
    }
}

/// 抓取订阅 URL → 校验书源数组 → 订阅入库（raw_json 存原文）+ 批量导入书源（已存在覆盖）
/// （saveSourceSub / refreshSourceSub 共用）；返回导入书源数
async fn fetch_and_store_source_sub(
    state: &AppState,
    ns: &str,
    url: &str,
    name: &str,
) -> Result<usize, ReturnData> {
    let headers_map: HashMap<String, String> = HashMap::new();
    let resp = match crate::service::crawler::fetch(url, &headers_map, 15, "GET", None, None).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("订阅抓取失败 [{url}]: {e}");
            return Err(ReturnData::err("远程书源链接错误"));
        }
    };
    // 校验：必须是书源数组（每项含非空 bookSourceUrl）
    let json: serde_json::Value = match serde_json::from_str(&resp.body) {
        Ok(v) => v,
        Err(_) => return Err(ReturnData::err("书源数据格式错误")),
    };
    let sources: Vec<crate::model::BookSource> = match serde_json::from_value(json) {
        Ok(s) => s,
        Err(_) => return Err(ReturnData::err("书源数据格式错误")),
    };
    if sources.is_empty() || sources.iter().any(|s| s.book_source_url.trim().is_empty()) {
        return Err(ReturnData::err("书源数据格式错误"));
    }
    // F-7 书源数上限（同 saveFromRemoteSource：已存在覆盖不计名额，超限整批拒绝）
    if let Some(limit) = state.storage.book_source_limit_for(ns).await.ok().flatten() {
        if limit > 0 {
            let mut new_count = 0i64;
            for s in &sources {
                let exists = state
                    .storage
                    .find_book_source(ns, &s.book_source_url)
                    .await
                    .ok()
                    .flatten()
                    .is_some();
                if !exists {
                    new_count += 1;
                }
            }
            match state.storage.count_book_sources(ns).await {
                Ok(count) if count + new_count > limit => {
                    return Err(ReturnData::err("超过书源数上限"));
                }
                Ok(_) => {}
                Err(_) => return Err(ReturnData::err("系统错误")),
            }
        }
    }
    // 订阅入库 + 批量导入书源
    if let Err(e) = state
        .storage
        .save_source_sub(ns, url, name, &resp.body)
        .await
    {
        tracing::error!("保存订阅失败 [{url}]: {e}");
        return Err(ReturnData::err("保存失败"));
    }
    if let Err(e) = state.storage.save_book_sources(ns, &sources).await {
        tracing::error!("订阅书源入库失败 [{url}]: {e}");
        return Err(ReturnData::err("保存失败"));
    }
    Ok(sources.len())
}

/// POST /reader3/saveSourceSub：订阅书源集合（body {url, name}）——抓取校验后入库 + 批量导入书源
async fn save_source_sub(
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
        return Json(ReturnData::err("请输入订阅链接"));
    }
    let mut name = param_of(&params, body_json.as_ref(), "name");
    if name.is_empty() {
        name = url.clone();
    }
    match fetch_and_store_source_sub(&state, &namespace, &url, &name).await {
        Ok(count) => Json(ReturnData::ok(serde_json::json!({ "count": count }))),
        Err(ret) => Json(ret),
    }
}

/// POST /reader3/refreshSourceSub：重新拉取订阅并覆盖书源（url 参数；订阅需已存在）
async fn refresh_source_sub(
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
        return Json(ReturnData::err("请输入订阅链接"));
    }
    let sub = match state.storage.find_source_sub(&namespace, &url).await {
        Ok(Some(s)) => s,
        Ok(None) => return Json(ReturnData::err("订阅不存在")),
        Err(_) => return Json(ReturnData::err("系统错误")),
    };
    match fetch_and_store_source_sub(&state, &namespace, &url, &sub.name).await {
        Ok(count) => Json(ReturnData::ok(serde_json::json!({ "count": count }))),
        Err(ret) => Json(ret),
    }
}

/// POST /reader3/deleteSourceSub：删除订阅（url 参数；仅删订阅行，不影响已导入书源）
async fn delete_source_sub(
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
        return Json(ReturnData::err("参数错误"));
    }
    match state.storage.delete_source_sub(&namespace, &url).await {
        Ok(_) => Json(ReturnData::ok(serde_json::Value::Null)),
        Err(e) => {
            tracing::error!("deleteSourceSub 失败 [{url}]: {e}");
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
        if let Some(ret) = get_book_toc_file(&state, &namespace, &toc_url).await {
            return ret;
        }
        return Json(ReturnData::err("本地书文件不存在"));
    }
    // legacy 本地书（origin=loc_book——toc_url 可能是分章正则或 storage/ 文件路径）——查书架定位文件
    if toc_url.starts_with("storage/") || toc_url.starts_with("spin") || toc_url.contains("(?") || toc_url.contains("序章") || toc_url.contains("楔子") {
        let mut req_url = param_of(&params, body_json.as_ref(), "url");
        if req_url.is_empty() {
            req_url = toc_url.clone();
        }
        if let Some(ret) = get_book_toc_loc_book(&state, &namespace, &req_url, &toc_url).await {
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
    // legacy 本地书：bookUrl#index（bookUrl 可能是 storage/ 路径——不限于 .txt）
    if chapter_url.contains("#") && (chapter_url.starts_with("storage/") || chapter_url.contains(".txt#")) {
        if let Some(ret) = get_book_content_file(&state, &namespace, &chapter_url).await {
            return ret;
        }
        return Json(ReturnData::err("本地书章节不存在"));
    }
    // F-10：书源书正文缓存——book_url 为键 + chapter_index = chapterUrl md5 哈希，
    // 同 chapterUrl 直读（永久，清理接口 clearCache 可清）；local:// 键域不参与
    let book_url = param_of(&params, body_json.as_ref(), "bookUrl");
    if !book_url.is_empty() && !book_url.starts_with("local://") {
        let idx = crate::util::md5::chapter_url_hash(&chapter_url);
        if let Ok(Some(content)) = state.storage.get_chapter_content(&book_url, idx).await {
            if !content.trim().is_empty() {
                tracing::debug!("getBookContent 命中正文缓存 [{book_url} #{idx}]");
                return Json(ReturnData::ok(serde_json::json!({ "content": content })));
            }
        }
    }
    let bs_param = param_of(&params, body_json.as_ref(), "bookSource");
    let Some(source) = resolve_book_source(&state, &namespace, &bs_param).await else {
        return Json(ReturnData::err("书源不存在"));
    };
    match crate::service::book::analyze_content(&chapter_url, &source, 5).await {
        Ok(content) => {
            // 抓取成功 → 写回正文缓存（仅书源书且带 bookUrl）
            if !book_url.is_empty() && !book_url.starts_with("local://") {
                let idx = crate::util::md5::chapter_url_hash(&chapter_url);
                let title = param_of(&params, body_json.as_ref(), "title");
                let _ = state
                    .storage
                    .cache_chapter_content(&book_url, idx, &title, &content)
                    .await;
            }
            Json(ReturnData::ok(serde_json::json!({ "content": content })))
        }
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

/// F-32 用户管理：GET/POST /reader3/getUsers：用户列表（含启用状态；secure + secureKey 管理校验）
async fn get_users(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    // 需登录（legacy checkAuth）
    let _namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let body_json = body.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    if let Err(ret) = check_manager_auth(&state, &params, body_json.as_ref()) {
        return Json(ret);
    }
    match state.storage.list_users().await {
        Ok(users) => {
            let arr: Vec<Value> = users.iter().map(user_admin_json).collect();
            Json(ReturnData::ok(Value::Array(arr)))
        }
        Err(e) => {
            tracing::error!("getUsers 失败: {e}");
            Json(ReturnData::err("系统错误"))
        }
    }
}

/// 用户管理输出 JSON（不含密码/salt/token；camelCase 兼容 legacy）
fn user_admin_json(user: &User) -> Value {
    json!({
        "username": user.username,
        "enableWebdav": user.enable_webdav,
        "enableLocalStore": user.enable_local_store,
        "enableBookSource": user.enable_book_source,
        "enableRssSource": user.enable_rss_source,
        "bookSourceLimit": user.book_source_limit,
        "bookLimit": user.book_limit,
        "lastLoginAt": user.last_login_at,
        "createdAt": user.created_at,
    })
}

/// POST /reader3/updateUser：更新用户权限/限额（body/query：username + 可选字段；secureKey）
async fn update_user(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let _namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let body_json = body.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    if let Err(ret) = check_manager_auth(&state, &params, body_json.as_ref()) {
        return Json(ret);
    }
    let username = param_of(&params, body_json.as_ref(), "username");
    if username.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    // 布尔参数：body 布尔值或 query "true"/"1"
    let bool_param = |key: &str| -> Option<bool> {
        if let Some(b) = body_json.as_ref().and_then(|b| b.get(key)) {
            return b.as_bool();
        }
        params.get(key).map(|v| v == "true" || v == "1")
    };
    let int_param = |key: &str| -> Option<i64> {
        if let Some(b) = body_json.as_ref().and_then(|b| b.get(key)) {
            return b.as_i64();
        }
        params.get(key).and_then(|v| v.parse::<i64>().ok())
    };
    match state
        .storage
        .update_user_permissions(
            &username,
            bool_param("enableWebdav"),
            bool_param("enableLocalStore"),
            bool_param("enableBookSource"),
            bool_param("enableRssSource"),
            int_param("bookSourceLimit"),
            int_param("bookLimit"),
        )
        .await
    {
        Ok(0) => Json(ReturnData::err("用户不存在")),
        Ok(_) => Json(ReturnData::ok(Value::Null)),
        Err(e) => {
            tracing::error!("updateUser [{username}] 失败: {e}");
            Json(ReturnData::err("系统错误"))
        }
    }
}

/// POST /reader3/deleteUser：删除用户（secureKey；不能删除自己）
async fn delete_user(
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
    if let Err(ret) = check_manager_auth(&state, &params, body_json.as_ref()) {
        return Json(ret);
    }
    let username = param_of(&params, body_json.as_ref(), "username");
    if username.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    if username == namespace {
        return Json(ReturnData::err("不能删除自己"));
    }
    match state.storage.delete_user(&username).await {
        Ok(0) => Json(ReturnData::err("用户不存在")),
        Ok(_) => {
            tracing::info!("deleteUser：删除用户 {username}");
            Json(ReturnData::ok(Value::Null))
        }
        Err(e) => {
            tracing::error!("deleteUser [{username}] 失败: {e}");
            Json(ReturnData::err("系统错误"))
        }
    }
}

/// POST /reader3/resetUserPassword：重置用户密码（body/query：username + password/newPassword；secureKey）
async fn reset_user_password(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let _namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let body_json = body.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    if let Err(ret) = check_manager_auth(&state, &params, body_json.as_ref()) {
        return Json(ret);
    }
    let username = param_of(&params, body_json.as_ref(), "username");
    let mut password = param_of(&params, body_json.as_ref(), "password");
    if password.is_empty() {
        password = param_of(&params, body_json.as_ref(), "newPassword");
    }
    if username.is_empty() || password.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    // 新 salt（与注册一致：8 位随机字母数字）
    use rand::Rng;
    let salt: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(8)
        .map(char::from)
        .collect();
    let encrypted = gen_encrypted_password(&password, &salt);
    match state
        .storage
        .reset_user_password(&username, &salt, &encrypted)
        .await
    {
        Ok(0) => Json(ReturnData::err("用户不存在")),
        Ok(_) => {
            tracing::info!("resetUserPassword：重置用户 {username} 密码");
            Json(ReturnData::ok(Value::Null))
        }
        Err(e) => {
            tracing::error!("resetUserPassword [{username}] 失败: {e}");
            Json(ReturnData::err("系统错误"))
        }
    }
}

/// 管理校验（legacy checkManagerAuth）：secure 模式 + secureKey 匹配
/// 失败返回 NEED_SECURE_KEY（errorMsg=请输入管理密码）
fn check_manager_auth(
    state: &AppState,
    params: &HashMap<String, String>,
    body: Option<&serde_json::Value>,
) -> Result<(), ReturnData> {
    let config = &state.storage.config;
    if !config.secure || config.secure_key.is_empty() {
        return Err(ReturnData::err("不支持的操作"));
    }
    let secure_key = param_of(params, body, "secureKey");
    if secure_key != config.secure_key {
        return Err(ReturnData {
            is_success: false,
            error_msg: "请输入管理密码".to_string(),
            data: json!("NEED_SECURE_KEY"),
        });
    }
    Ok(())
}

// ---------------- F-25 TTS ----------------

/// GET/POST /reader3/getTTSVoices：Edge TTS 可用语音列表（预置 zh-CN/en-US）
async fn get_tts_voices(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let _ = (&state, &params, &headers, &body);
    let arr: Vec<Value> = crate::service::tts::edge_voices()
        .iter()
        .map(|v| {
            json!({
                "name": v.name,
                "value": v.value,
                "locale": v.locale,
                "gender": v.gender,
            })
        })
        .collect();
    Json(ReturnData::ok(Value::Array(arr)))
}

/// GET/POST /reader3/tts：语音合成
/// 参数：text（必填）、voice（默认 zh-CN-XiaoxiaoNeural）、rate（默认 +0%）、pitch（默认 +0Hz）、
/// engine（edge=Edge 语音 / http=HttpTTS，默认 edge）、url（engine=http 时的 HttpTTS 地址）
/// 成功：audio/mpeg 字节流；失败：ReturnData JSON
async fn tts_synthesize(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Response {
    let _namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret).into_response(),
    };
    let body_json = body.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let text = param_of(&params, body_json.as_ref(), "text");
    if text.trim().is_empty() {
        return Json(ReturnData::err("参数错误")).into_response();
    }
    let engine = param_of(&params, body_json.as_ref(), "engine");
    let engine = if engine.is_empty() { "edge" } else { engine.as_str() };
    let voice = param_of(&params, body_json.as_ref(), "voice");
    let voice = if voice.is_empty() {
        crate::service::tts::DEFAULT_VOICE.to_string()
    } else {
        voice
    };
    let rate = param_of(&params, body_json.as_ref(), "rate");
    let rate = if rate.is_empty() { "+0%".to_string() } else { rate };
    let pitch = param_of(&params, body_json.as_ref(), "pitch");
    let pitch = if pitch.is_empty() { "+0Hz".to_string() } else { pitch };

    let result = match engine {
        "edge" => crate::service::tts::edge_synthesize(&text, &voice, &rate, &pitch).await,
        "http" | "httptts" | "api" => {
            let url = param_of(&params, body_json.as_ref(), "url");
            if url.trim().is_empty() {
                return Json(ReturnData::err("参数错误")).into_response();
            }
            crate::service::tts::http_tts_synthesize(&url, &text, Some(&voice), Some(&rate), Some(&pitch))
                .await
        }
        _ => {
            return Json(ReturnData::err("不支持的TTS引擎")).into_response();
        }
    };

    match result {
        Ok(audio) => Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "audio/mpeg")
            .header("Cache-Control", "no-store")
            .body(Body::from(audio))
            .unwrap(),
        Err(e) => {
            tracing::warn!("tts 合成失败 [{engine}]: {e}");
            Json(ReturnData::err("合成失败")).into_response()
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
        // 进度保存静默：无 bookUrl 时不弹错（前端组件卸载竞态等场景）
        return Json(ReturnData::ok(serde_json::Value::Null));
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

/// GET/POST /reader3/getExploreSources：探索书源列表（精确分类数——parse_explore_entries 执行后计数）
async fn get_explore_sources(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let sources = match state.storage.get_book_sources(&namespace).await {
        Ok(s) => s,
        Err(_) => return Json(ReturnData::err("系统错误")),
    };
    let list: Vec<serde_json::Value> = sources
        .iter()
        .filter(|s| s.enabled_explore && s.explore_url.is_some())
        .map(|s| {
            let count = crate::service::explore::parse_explore_entries(s.explore_url.as_deref().unwrap_or("")).len();
            serde_json::json!({
                "bookSourceUrl": s.book_source_url,
                "bookSourceName": s.book_source_name,
                "categoryCount": count,
            })
        })
        .filter(|v| v.get("categoryCount").and_then(|c| c.as_u64()).unwrap_or(0) > 0)
        .collect();
    Json(ReturnData::ok(serde_json::Value::Array(list)))
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
    // legado 语义：exploreUrl 可能是 @js: 代码（执行后返回 [{title,url}]）或普通 URL 集合
    let raw = source.explore_url.as_deref().unwrap_or("");
    let entries = crate::service::explore::parse_explore_entries(raw);
    Json(ReturnData::ok(serde_json::to_value(entries).unwrap_or(serde_json::Value::Null)))
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
            Json(ReturnData::err(format!("探索失败：{e}")))
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

/// GET/POST /reader3/getBookGroups：书架分组列表（含组内书数 bookCount；order/orderNum 双字段）
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
    let _ = body;
    match state.storage.list_book_groups_with_count(&namespace).await {
        Ok(groups) => Json(ReturnData::ok(
            serde_json::to_value(groups).unwrap_or(serde_json::Value::Null),
        )),
        Err(e) => {
            tracing::error!("getBookGroups 失败: {e}");
            Json(ReturnData::err("系统错误"))
        }
    }
}

/// POST /reader3/saveBookGroup：保存分组（body：id?/name/order?；id>0 覆盖，否则新建）。
/// 分组重命名契约：body 仅 {id,name}（无 order）→ 只改名称、保留排序
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
    let v: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return Json(ReturnData::err("参数错误")),
    };
    let group: crate::model::BookGroup = match serde_json::from_value(v.clone()) {
        Ok(g) => g,
        Err(_) => return Json(ReturnData::err("参数错误")),
    };
    if group.name.is_empty() {
        return Json(ReturnData::err("分组名称不能为空"));
    }
    // 仅 {id,name} → 重命名（保留 order；saveBookGroupName/updateBookGroup 兼容契约）
    if group.id > 0 && v.get("order").is_none() && v.get("orderNum").is_none() {
        return match state.storage.rename_book_group(&namespace, group.id, &group.name).await {
            Ok(0) => Json(ReturnData::err("分组不存在")),
            Ok(_) => Json(ReturnData::ok(
                serde_json::to_value(group).unwrap_or(serde_json::Value::Null),
            )),
            Err(e) => {
                tracing::error!("saveBookGroup 重命名失败: {e}");
                Json(ReturnData::err("保存失败"))
            }
        };
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

/// POST /reader3/deleteBookGroup：删除分组（body/query：id；组内书 group 置 0）
async fn delete_book_group(
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
    let id = params
        .get("id")
        .and_then(|v| v.parse::<i64>().ok())
        .or_else(|| {
            body_json
                .as_ref()
                .and_then(|b| b.get("id").and_then(|v| v.as_i64()))
        })
        .unwrap_or(-1);
    if id <= 0 {
        return Json(ReturnData::err("参数错误"));
    }
    match state.storage.delete_book_group(&namespace, id).await {
        Ok(0) => Json(ReturnData::err("分组不存在")),
        Ok(_) => Json(ReturnData::ok(serde_json::Value::Null)),
        Err(e) => {
            tracing::error!("deleteBookGroup [{id}] 失败: {e}");
            Json(ReturnData::err("删除失败"))
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

// ---------------- F-28 替换规则 ----------------

/// GET/POST /reader3/getReplaceRules：替换规则列表（用户命名空间，无则回退 default）
async fn get_replace_rules(
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
    match state.storage.get_replace_rules(&namespace).await {
        Ok(rules) => Json(ReturnData::ok(
            serde_json::to_value(rules).unwrap_or(serde_json::Value::Null),
        )),
        Err(e) => {
            tracing::error!("getReplaceRules [{namespace}] 失败: {e}");
            Json(ReturnData::err("系统错误"))
        }
    }
}

/// POST /reader3/saveReplaceRule：保存单条替换规则（body = 完整规则 JSON；id 缺失自动补 uuid）
async fn save_replace_rule(
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
    let mut rule: crate::model::ReplaceRule = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => return Json(ReturnData::err("参数错误")),
    };
    if rule.name.trim().is_empty() {
        return Json(ReturnData::err("名称不能为空"));
    }
    if rule.find.trim().is_empty() {
        return Json(ReturnData::err("规则不能为空"));
    }
    if rule.id.trim().is_empty() {
        rule.id = format!("rule-{}", uuid::Uuid::new_v4());
    }
    rule.user_namespace = namespace.clone();
    match state.storage.save_replace_rule(&namespace, &rule).await {
        Ok(_) => Json(ReturnData::ok(serde_json::Value::Null)),
        Err(e) => {
            tracing::error!("saveReplaceRule 失败: {e}");
            Json(ReturnData::err("保存失败"))
        }
    }
}

/// POST /reader3/saveReplaceRules：批量保存（body = 规则数组；逐条校验，id 缺失自动补）
async fn save_replace_rules(
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
    let mut rules: Vec<crate::model::ReplaceRule> = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => return Json(ReturnData::err("参数错误")),
    };
    if rules.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    for rule in &mut rules {
        if rule.name.trim().is_empty() || rule.find.trim().is_empty() {
            return Json(ReturnData::err("参数错误"));
        }
        if rule.id.trim().is_empty() {
            rule.id = format!("rule-{}", uuid::Uuid::new_v4());
        }
        rule.user_namespace = namespace.clone();
    }
    match state.storage.save_replace_rules(&namespace, &rules).await {
        Ok(_) => Json(ReturnData::ok(serde_json::json!({ "count": rules.len() }))),
        Err(e) => {
            tracing::error!("saveReplaceRules 失败: {e}");
            Json(ReturnData::err("保存失败"))
        }
    }
}

/// POST /reader3/deleteReplaceRule：删除替换规则（body/query：id）
async fn delete_replace_rule(
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
    let id = param_of(&params, body_json.as_ref(), "id");
    if id.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    match state.storage.delete_replace_rule(&namespace, &id).await {
        Ok(_) => Json(ReturnData::ok(serde_json::Value::Null)),
        Err(e) => {
            tracing::error!("deleteReplaceRule 失败: {e}");
            Json(ReturnData::err("删除失败"))
        }
    }
}

// ---------------- F-26 HttpTTS ----------------

/// HttpTTS 输出 JSON：id 与 url 同值（前端 HttpTts 类型兼容）
fn http_tts_json(tts: &crate::model::HttpTts) -> serde_json::Value {
    serde_json::json!({
        "id": tts.url,
        "url": tts.url,
        "name": tts.name,
        "type": tts.tts_type,
    })
}

/// GET/POST /reader3/getHttpTTSList：HttpTTS 听书源列表（用户命名空间，无则回退 default）
async fn get_http_tts_list(
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
    match state.storage.get_http_tts_list(&namespace).await {
        Ok(list) => {
            let arr: Vec<serde_json::Value> = list.iter().map(http_tts_json).collect();
            Json(ReturnData::ok(serde_json::Value::Array(arr)))
        }
        Err(e) => {
            tracing::error!("getHttpTTSList [{namespace}] 失败: {e}");
            Json(ReturnData::err("系统错误"))
        }
    }
}

/// POST /reader3/saveHttpTTS：保存听书源（body：url/name/type；url 缺失时用 id 兜底）
async fn save_http_tts(
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
    let json: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return Json(ReturnData::err("参数错误")),
    };
    let mut tts: crate::model::HttpTts = match serde_json::from_value(json.clone()) {
        Ok(t) => t,
        Err(_) => return Json(ReturnData::err("参数错误")),
    };
    // url 主键；前端可能只传 id（旧契约），用 id 兜底
    if tts.url.trim().is_empty() {
        if let Some(id) = json.get("id").and_then(|v| v.as_str()) {
            tts.url = id.to_string();
        }
    }
    if tts.url.trim().is_empty() {
        return Json(ReturnData::err("链接不能为空"));
    }
    if tts.name.trim().is_empty() {
        return Json(ReturnData::err("名称不能为空"));
    }
    tts.user_namespace = namespace.clone();
    match state.storage.save_http_tts(&namespace, &tts).await {
        Ok(_) => Json(ReturnData::ok(serde_json::Value::Null)),
        Err(e) => {
            tracing::error!("saveHttpTTS 失败: {e}");
            Json(ReturnData::err("保存失败"))
        }
    }
}

/// POST /reader3/deleteHttpTTS：删除听书源（body/query：id 或 url）
async fn delete_http_tts(
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
    let mut url = param_of(&params, body_json.as_ref(), "url");
    if url.is_empty() {
        url = param_of(&params, body_json.as_ref(), "id");
    }
    if url.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    match state.storage.delete_http_tts(&namespace, &url).await {
        Ok(_) => Json(ReturnData::ok(serde_json::Value::Null)),
        Err(e) => {
            tracing::error!("deleteHttpTTS 失败: {e}");
            Json(ReturnData::err("删除失败"))
        }
    }
}

// ---------------- 自定义 TXT 目录规则 ----------------

/// GET/POST /reader3/getTxtTocRules：TXT 目录规则列表（legacy 语义：内置默认规则 + 用户自定义规则）
async fn get_txt_toc_rules(
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
    let mut rules: Vec<serde_json::Value> = Vec::new();
    // 内置默认规则（id 固定 default-{i}，可被 importDefaultTxtTocRules 导入为用户规则）
    for (i, rule) in crate::service::local_book::DEFAULT_TOC_RULES.iter().enumerate() {
        rules.push(serde_json::json!({
            "id": format!("default-{}", i + 1),
            "name": format!("默认规则{}", i + 1),
            "rule": rule,
            "enable": true,
            "serialNumber": i as i64,
        }));
    }
    // 用户自定义规则（含导入的默认规则副本）
    match state.storage.get_txt_toc_rules(&namespace).await {
        Ok(custom) => {
            for rule in custom {
                rules.push(serde_json::to_value(rule).unwrap_or(serde_json::Value::Null));
            }
            Json(ReturnData::ok(serde_json::Value::Array(rules)))
        }
        Err(e) => {
            tracing::error!("getTxtTocRules [{namespace}] 失败: {e}");
            Json(ReturnData::err("系统错误"))
        }
    }
}

/// POST /reader3/saveTxtTocRule：保存自定义 TXT 目录规则（body：id?/name/rule/enable/serialNumber）
async fn save_txt_toc_rule(
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
    let mut rule: crate::model::TxtTocRule = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => return Json(ReturnData::err("参数错误")),
    };
    if rule.name.trim().is_empty() {
        return Json(ReturnData::err("名称不能为空"));
    }
    if rule.rule.trim().is_empty() {
        return Json(ReturnData::err("规则不能为空"));
    }
    if rule.id.trim().is_empty() {
        rule.id = format!("toc-{}", uuid::Uuid::new_v4());
    }
    rule.user_namespace = namespace.clone();
    match state.storage.save_txt_toc_rule(&namespace, &rule).await {
        Ok(_) => Json(ReturnData::ok(serde_json::Value::Null)),
        Err(e) => {
            tracing::error!("saveTxtTocRule 失败: {e}");
            Json(ReturnData::err("保存失败"))
        }
    }
}

/// POST /reader3/deleteTxtTocRule：删除自定义 TXT 目录规则（body/query：id）
async fn delete_txt_toc_rule(
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
    let id = param_of(&params, body_json.as_ref(), "id");
    if id.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    match state.storage.delete_txt_toc_rule(&namespace, &id).await {
        Ok(_) => Json(ReturnData::ok(serde_json::Value::Null)),
        Err(e) => {
            tracing::error!("deleteTxtTocRule 失败: {e}");
            Json(ReturnData::err("删除失败"))
        }
    }
}

/// POST /reader3/importDefaultTxtTocRules：内置默认规则导入为用户规则（幂等，返回导入条数）
async fn import_default_txt_toc_rules(
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
    match state.storage.import_default_txt_toc_rules(&namespace).await {
        Ok(count) => Json(ReturnData::ok(serde_json::json!({ "count": count }))),
        Err(e) => {
            tracing::error!("importDefaultTxtTocRules [{namespace}] 失败: {e}");
            Json(ReturnData::err("导入失败"))
        }
    }
}

// ---------------- 系统信息 + 书源导出 ----------------

/// GET /reader3/getSystemInfo：系统信息（版本/端口/用户数/书数/书源数 + legacy 兼容内存字段）
async fn get_system_info(
    State(state): State<AppState>,
    Query(_params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Json<ReturnData> {
    let _ = headers;
    let user_count = state.storage.count_users().await.unwrap_or(0);
    let book_count = state.storage.count_books().await.unwrap_or(0);
    let source_count = state.storage.count_all_book_sources().await.unwrap_or(0);
    let version = env!("CARGO_PKG_VERSION");
    Json(ReturnData::ok(serde_json::json!({
        "version": version,
        "port": state.storage.config.port,
        "userCount": user_count,
        "bookCount": book_count,
        "bookSourceCount": source_count,
        // legacy 兼容字段（内存统计：暂不引入系统探针依赖，置 0）
        "freeMemory": "0M",
        "totalMemory": "0M",
        "maxMemory": "0M",
    })))
}

/// GET /reader3/exportBookSources：当前命名空间书源 JSON 下载（attachment）
async fn export_book_sources(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret).into_response(),
    };
    let sources = match state.storage.get_book_sources(&namespace).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("exportBookSources [{namespace}] 失败: {e}");
            return Json(ReturnData::err("系统错误")).into_response();
        }
    };
    let bytes = serde_json::to_vec_pretty(&sources).unwrap_or_default();
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json; charset=utf-8")
        .header("Content-Disposition", "attachment; filename=bookSource.json")
        .body(Body::from(bytes))
        .unwrap()
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
        // 用户自定义 TXT 目录规则（启用 + 按 serialNumber 排序）；无则用内置默认规则
        let user_rules = txt_toc_rule_regexes(&state, &namespace).await;
        crate::service::local_book::parse_txt_with_rules(&bytes, &user_rules).unwrap_or_else(|e| {
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

/// 用户 TXT 目录规则正则列表（启用 + 按 serialNumber 排序；失败/无规则返回空 → 调用方回退默认）
async fn txt_toc_rule_regexes(state: &AppState, ns: &str) -> Vec<String> {
    match state.storage.get_txt_toc_rules(ns).await {
        Ok(rules) => rules
            .into_iter()
            .filter(|r| r.enable && !r.rule.trim().is_empty())
            .map(|r| r.rule)
            .collect(),
        Err(e) => {
            tracing::warn!("getTxtTocRules 失败（回退默认规则）: {e}");
            Vec::new()
        }
    }
}

/// legacy 本地书文件定位：book_url 指向的文件可能缺失（legacy 导入时改名 index.epub）
/// 兜底：父目录 index.epub → 任意 epub/txt
fn resolve_loc_book_file(storage_dir: &std::path::Path, book_url: &str) -> Option<std::path::PathBuf> {
    let path = storage_dir.join(book_url.trim_start_matches("storage/"));
    if path.is_file() {
        return Some(path);
    }
    // legacy 的 epub 是目录（{书名}.epub/ 内含 index.epub）
    if path.is_dir() {
        let idx = path.join("index.epub");
        if idx.is_file() {
            return Some(idx);
        }
        let rd = std::fs::read_dir(&path).ok()?;
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() && p.to_string_lossy().to_lowercase().ends_with(".epub") {
                return Some(p);
            }
        }
    }
    let parent = path.parent()?;
    let idx = parent.join("index.epub");
    if idx.is_file() {
        return Some(idx);
    }
    let rd = std::fs::read_dir(parent).ok()?;
    for e in rd.flatten() {
        let p = e.path();
        if p.is_file() {
            let lower = p.to_string_lossy().to_lowercase();
            if lower.ends_with(".epub") || lower.ends_with(".txt") {
                return Some(p);
            }
        }
    }
    None
}

/// legacy 本地书目录：toc_url 是分章正则（或空）——查书架定位 TXT 文件 → 按规则分章
async fn get_book_toc_loc_book(
    state: &AppState,
    namespace: &str,
    req_url: &str,
    toc_rule: &str,
) -> Option<Json<ReturnData>> {
    // 书架找本地书：优先按传入 url 精确匹配，兜底第一本 loc_book
    let books = state.storage.list_books(namespace).await.ok()?;
    let book = books
        .iter()
        .find(|b| b.origin == "loc_book" && !b.book_url.is_empty() && b.book_url == req_url)
        .or_else(|| books.iter().find(|b| b.origin == "loc_book" && !b.book_url.is_empty()))
        .or_else(|| books.iter().find(|b| b.origin == "loc_book"))?;
    let book_url = &book.book_url;
    tracing::debug!("loc_book toc: req={req_url} matched={book_url}");
    if !book_url.starts_with("storage/") {
        tracing::debug!("loc_book toc: book_url 非 storage 路径");
        return None;
    }
    let Some(path) = resolve_loc_book_file(&state.storage.config.storage_dir(), book_url) else {
        tracing::debug!("loc_book toc: 文件定位失败 [{book_url}]");
        return None;
    };
    let path_lower = path.to_string_lossy().to_lowercase();
    let imported = if path_lower.ends_with(".epub") {
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                tracing::debug!("loc_book toc: 读取失败 [{path:?}] {e}");
                return None;
            }
        };
        match crate::service::local_book::parse_epub(&bytes) {
            Ok(b) => b,
            Err(e) => {
                tracing::debug!("loc_book toc: epub 解析失败 {e}");
                return None;
            }
        }
    } else {
        match crate::service::local_book::parse_txt_file(&path) {
            Ok(b) => b,
            Err(e) => {
                tracing::debug!("loc_book toc: txt 解析失败 {e}");
                return None;
            }
        }
    };
    let chapters = imported.chapters;
    let list: Vec<serde_json::Value> = chapters
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

/// 文件型本地书目录：读 TXT 分章 → 章节列表（chapterUrl = bookUrl#index）
async fn get_book_toc_file(state: &AppState, ns: &str, book_url: &str) -> Option<Json<ReturnData>> {
    let path = resolve_storage_path(&state.storage.config.storage_dir(), book_url)?;
    let user_rules = txt_toc_rule_regexes(state, ns).await;
    let imported = crate::service::local_book::parse_txt_file_with_rules(&path, &user_rules).ok()?;
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

/// 文件型本地书正文：bookUrl#index → 读文件（TXT/EPUB）→ 提取章节
async fn get_book_content_file(state: &AppState, ns: &str, chapter_url: &str) -> Option<Json<ReturnData>> {
    let (book_part, idx_part) = chapter_url.rsplit_once('#')?;
    let index: usize = idx_part.parse().ok()?;
    let path = resolve_loc_book_file(&state.storage.config.storage_dir(), book_part)?;
    let path_lower = path.to_string_lossy().to_lowercase();
    let imported = if path_lower.ends_with(".epub") {
        let bytes = std::fs::read(&path).ok()?;
        crate::service::local_book::parse_epub(&bytes).ok()?
    } else {
        let user_rules = txt_toc_rule_regexes(state, ns).await;
        crate::service::local_book::parse_txt_file_with_rules(&path, &user_rules).ok()?
    };
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

    /// F-28：替换规则 API——保存（缺 id 自动补）/列表/批量/删除/校验
    #[tokio::test]
    async fn test_replace_rules_api() {
        let (state, dir) = test_state("replapi").await;

        // 空名称/空 find → 校验失败
        let body = Bytes::from(r#"{"name":"","find":"a"}"#);
        let ret = save_replace_rule(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "名称不能为空");
        let body = Bytes::from(r#"{"name":"规则","find":""}"#);
        let ret = save_replace_rule(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "规则不能为空");

        // 保存（无 id → 后端补 uuid）
        let body = Bytes::from(r#"{"name":"净化","find":"口口","replace":"","enabled":true,"order":1}"#);
        let ret = save_replace_rule(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "保存应成功: {}", ret.0.error_msg);

        // 列表
        let ret = get_replace_rules(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert!(ret.0.is_success);
        let arr = ret.0.data.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "净化");
        assert_eq!(arr[0]["find"], "口口");
        assert!(arr[0]["id"].as_str().unwrap().starts_with("rule-"), "缺 id 应自动补: {arr:?}");

        // 批量
        let batch = serde_json::json!([
            { "id": "b1", "name": "批量1", "find": "x", "replace": "y", "enabled": true, "order": 0 },
            { "name": "批量2", "find": "z", "enabled": false, "order": 1 },
        ]);
        let ret = save_replace_rules(
            AxumState(state.clone()),
            Query(HashMap::new()),
            HeaderMap::new(),
            Some(Bytes::from(batch.to_string())),
        )
        .await;
        assert!(ret.0.is_success, "批量保存应成功: {}", ret.0.error_msg);
        assert_eq!(ret.0.data["count"], 2);
        let ret = get_replace_rules(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert_eq!(ret.0.data.as_array().unwrap().len(), 3);

        // 批量含空 find → 整批拒绝
        let batch = serde_json::json!([{ "name": "a", "find": "" }]);
        let ret = save_replace_rules(
            AxumState(state.clone()),
            Query(HashMap::new()),
            HeaderMap::new(),
            Some(Bytes::from(batch.to_string())),
        )
        .await;
        assert!(!ret.0.is_success);

        // 删除
        let body = Bytes::from(r#"{"id":"b1"}"#);
        let ret = delete_replace_rule(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success);
        let ret = get_replace_rules(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert_eq!(ret.0.data.as_array().unwrap().len(), 2);
        // query 参数删除
        let params: HashMap<String, String> = [("id".into(), "b1".into())].into_iter().collect();
        let ret = delete_replace_rule(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(ret.0.is_success, "重复删除不报错");
        // 缺 id
        let ret = delete_replace_rule(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert!(!ret.0.is_success);

        cleanup(state, dir).await;
    }

    /// F-26：HttpTTS API——保存（id 兜底 url）/列表（id+url 双字段）/删除
    #[tokio::test]
    async fn test_http_tts_api() {
        let (state, dir) = test_state("ttsapi").await;

        // 校验：缺 url/name
        let body = Bytes::from(r#"{"name":"甲"}"#);
        let ret = save_http_tts(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "链接不能为空");
        let body = Bytes::from(r#"{"url":"https://t.com/a"}"#);
        let ret = save_http_tts(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "名称不能为空");

        // 保存
        let body = Bytes::from(r#"{"name":"引擎甲","url":"https://t.com/a","type":0}"#);
        let ret = save_http_tts(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "保存应成功: {}", ret.0.error_msg);
        // 只传 id（旧契约）→ url 兜底
        let body = Bytes::from(r#"{"id":"https://t.com/b","name":"引擎乙","type":1}"#);
        let ret = save_http_tts(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success);

        // 列表：id 与 url 同值
        let ret = get_http_tts_list(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert!(ret.0.is_success);
        let arr = ret.0.data.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        let a = arr.iter().find(|v| v["name"] == "引擎甲").expect("应含引擎甲");
        assert_eq!(a["id"], a["url"]);
        assert_eq!(a["type"], 0);

        // 同 url 覆盖不新增
        let body = Bytes::from(r#"{"name":"引擎甲v2","url":"https://t.com/a","type":0}"#);
        let ret = save_http_tts(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success);
        let ret = get_http_tts_list(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert_eq!(ret.0.data.as_array().unwrap().len(), 2);

        // 删除（按 id）
        let body = Bytes::from(r#"{"id":"https://t.com/a"}"#);
        let ret = delete_http_tts(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success);
        let ret = get_http_tts_list(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert_eq!(ret.0.data.as_array().unwrap().len(), 1);

        cleanup(state, dir).await;
    }

    /// 自定义 TXT 目录规则 API：默认规则 + 用户规则合并列表/保存/删除/导入默认
    #[tokio::test]
    async fn test_txt_toc_rules_api() {
        let (state, dir) = test_state("tocapi").await;
        let default_len = crate::service::local_book::DEFAULT_TOC_RULES.len();

        // 初始：仅内置默认规则
        let ret = get_txt_toc_rules(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert!(ret.0.is_success);
        let arr = ret.0.data.as_array().unwrap();
        assert_eq!(arr.len(), default_len);
        assert_eq!(arr[0]["id"], "default-1");
        assert!(arr[0]["enable"].as_bool().unwrap());

        // 保存自定义规则
        let body = Bytes::from(r#"{"name":"我的规则","rule":"^第.+章$","enable":true,"serialNumber":0}"#);
        let ret = save_txt_toc_rule(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "保存应成功: {}", ret.0.error_msg);
        // 校验：空 name/rule
        let body = Bytes::from(r#"{"name":"","rule":"x"}"#);
        let ret = save_txt_toc_rule(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "名称不能为空");
        let body = Bytes::from(r#"{"name":"x","rule":""}"#);
        let ret = save_txt_toc_rule(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(!ret.0.is_success);

        // 列表：默认 + 自定义（自定义在尾部）
        let ret = get_txt_toc_rules(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        let arr = ret.0.data.as_array().unwrap();
        assert_eq!(arr.len(), default_len + 1);
        let last = arr.last().unwrap();
        assert_eq!(last["name"], "我的规则");
        assert_eq!(last["serialNumber"], 0);
        assert!(last["id"].as_str().unwrap().starts_with("toc-"), "缺 id 应自动补");

        // 删除
        let id = last["id"].as_str().unwrap();
        let body = Bytes::from(format!(r#"{{"id":"{id}"}}"#));
        let ret = delete_txt_toc_rule(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success);
        let ret = get_txt_toc_rules(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert_eq!(ret.0.data.as_array().unwrap().len(), default_len);

        // 导入默认规则（用户规则中出现 default-* 副本）
        let ret = import_default_txt_toc_rules(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert!(ret.0.is_success);
        assert_eq!(ret.0.data["count"], default_len as i64);
        let ret = get_txt_toc_rules(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        let arr = ret.0.data.as_array().unwrap();
        assert_eq!(arr.len(), default_len * 2, "默认规则 + 用户导入副本");
        // 重复导入幂等
        let ret = import_default_txt_toc_rules(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert_eq!(ret.0.data["count"], default_len as i64);
        let ret = get_txt_toc_rules(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert_eq!(ret.0.data.as_array().unwrap().len(), default_len * 2);

        cleanup(state, dir).await;
    }

    /// getSystemInfo：版本/端口/用户数/书数/书源数
    #[tokio::test]
    async fn test_get_system_info() {
        let (state, dir) = test_state("sysapi").await;
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
        state
            .storage
            .save_book_source(
                "default",
                &crate::model::BookSource {
                    book_source_url: "https://s.com".into(),
                    book_source_name: "源A".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let ret = get_system_info(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new()).await;
        assert!(ret.0.is_success);
        assert_eq!(ret.0.data["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(ret.0.data["port"], 8080, "默认端口");
        assert_eq!(ret.0.data["userCount"], 0);
        assert_eq!(ret.0.data["bookCount"], 1);
        assert_eq!(ret.0.data["bookSourceCount"], 1);
        assert!(ret.0.data["freeMemory"].is_string());

        cleanup(state, dir).await;
    }

    /// 书源导出：attachment + 内容为当前命名空间书源 JSON
    #[tokio::test]
    async fn test_export_book_sources() {
        let (state, dir) = test_state("expapi").await;
        state
            .storage
            .save_book_source(
                "default",
                &crate::model::BookSource {
                    book_source_url: "https://s1.com".into(),
                    book_source_name: "源一".into(),
                    search_url: Some("https://s1.com/search".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        state
            .storage
            .save_book_source(
                "default",
                &crate::model::BookSource {
                    book_source_url: "https://s2.com".into(),
                    book_source_name: "源二".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let resp = export_book_sources(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("Content-Disposition").and_then(|v| v.to_str().ok()),
            Some("attachment; filename=bookSource.json")
        );
        assert_eq!(resp.headers().get("Content-Type").and_then(|v| v.to_str().ok()), Some("application/json; charset=utf-8"));
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert!(arr.iter().any(|v| v["bookSourceUrl"] == "https://s1.com" && v["bookSourceName"] == "源一"));
        // 空命名空间 → 合法空数组（含 default 回退，此处 default 有数据）
        let params: HashMap<String, String> =
            [("accessToken".into(), "ghost:tok".into())].into_iter().collect();
        let resp = export_book_sources(AxumState(state.clone()), Query(params), HeaderMap::new()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 2, "非 secure 模式 accessToken 无效，仍走 default");

        cleanup(state, dir).await;
    }

    /// 文件型本地书 TXT 目录：用户自定义规则生效（无规则回退默认规则）
    #[tokio::test]
    async fn test_txt_toc_rules_in_local_book_toc() {
        let (state, dir) = test_state("localtoc").await;
        // 写一个文件型本地书（storage/data/default/books/示例.txt）
        let file_dir = state.storage.config.storage_dir().join("data/default/books");
        std::fs::create_dir_all(&file_dir).unwrap();
        let txt = "第一章 起点\n内容一。\n第二章 成长\n内容二。";
        std::fs::write(file_dir.join("示例.txt"), txt).unwrap();
        let book_url = "storage/data/default/books/示例.txt";

        // 无用户规则 → 默认规则分章（两章）
        let ret = get_book_toc_file(&state, "default", book_url).await.expect("默认规则应可解析");
        let titles: Vec<&str> = ret
            .0
            .data
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["title"].as_str().unwrap())
            .collect();
        assert_eq!(titles, vec!["第一章 起点", "第二章 成长"]);

        // 用户规则只匹配「第二章」→ 第一章内容并入前置「正文」章
        state
            .storage
            .save_txt_toc_rule(
                "default",
                &crate::model::TxtTocRule {
                    id: "t1".into(),
                    name: "仅第二章".into(),
                    rule: r"^第二章.*$".into(),
                    enable: true,
                    serial_number: 0,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let ret = get_book_toc_file(&state, "default", book_url).await.expect("自定义规则应可解析");
        let arr = ret.0.data.as_array().unwrap();
        let titles: Vec<&str> = arr.iter().map(|v| v["title"].as_str().unwrap()).collect();
        assert_eq!(titles, vec!["正文", "第二章 成长"], "用户规则应替代默认规则");
        // 正文按同一规则读取（章索引一致）
        let url = arr[1]["url"].as_str().unwrap();
        let ret = get_book_content_file(&state, "default", url).await.expect("正文应可解析");
        assert_eq!(ret.0.data["content"], "内容二。");

        // 禁用规则 → 回退默认
        state
            .storage
            .save_txt_toc_rule(
                "default",
                &crate::model::TxtTocRule {
                    id: "t1".into(),
                    name: "仅第二章".into(),
                    rule: r"^第二章.*$".into(),
                    enable: false,
                    serial_number: 0,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let ret = get_book_toc_file(&state, "default", book_url).await.unwrap();
        let titles: Vec<&str> = ret
            .0
            .data
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["title"].as_str().unwrap())
            .collect();
        assert_eq!(titles, vec!["第一章 起点", "第二章 成长"], "禁用规则后回退默认");

        cleanup(state, dir).await;
    }

    /// F-25：getTTSVoices——预置语音列表（zh-CN 晓晓 + en-US Aria）
    #[tokio::test]
    async fn test_get_tts_voices_api() {
        let (state, dir) = test_state("ttsvoices").await;
        let ret = get_tts_voices(
            AxumState(state.clone()),
            Query(HashMap::new()),
            HeaderMap::new(),
            None,
        )
        .await;
        assert!(ret.0.is_success);
        let arr = ret.0.data.as_array().unwrap();
        assert!(!arr.is_empty());
        assert!(arr.iter().any(|v| v["value"] == "zh-CN-XiaoxiaoNeural" && v["name"] == "晓晓"));
        assert!(arr.iter().any(|v| v["value"] == "en-US-AriaNeural"));
        for v in arr {
            assert!(v["locale"].is_string() && v["gender"].is_string());
        }
        cleanup(state, dir).await;
    }

    /// F-25：tts 合成——参数校验（无 text / 未知引擎 / http 缺 url），不发起网络请求
    #[tokio::test]
    async fn test_tts_synthesize_param_validation() {
        let (state, dir) = test_state("ttsval").await;

        // 缺 text → 参数错误
        let ret = tts_synthesize(
            AxumState(state.clone()),
            Query(HashMap::new()),
            HeaderMap::new(),
            None,
        )
        .await;
        let body = axum::body::to_bytes(ret.into_body(), 1024 * 1024).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(!json["isSuccess"].as_bool().unwrap());
        assert_eq!(json["errorMsg"], "参数错误");

        // 未知引擎 → 不支持的TTS引擎
        let params: HashMap<String, String> = [
            ("text".into(), "你好".into()),
            ("engine".into(), "nope".into()),
        ]
        .into_iter()
        .collect();
        let ret = tts_synthesize(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        let body = axum::body::to_bytes(ret.into_body(), 1024 * 1024).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["errorMsg"], "不支持的TTS引擎");

        // http 引擎缺 url → 参数错误（不发起网络请求）
        let params: HashMap<String, String> = [
            ("text".into(), "你好".into()),
            ("engine".into(), "http".into()),
        ]
        .into_iter()
        .collect();
        let ret = tts_synthesize(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        let body = axum::body::to_bytes(ret.into_body(), 1024 * 1024).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(!json["isSuccess"].as_bool().unwrap());
        assert_eq!(json["errorMsg"], "参数错误");

        cleanup(state, dir).await;
    }

    /// F-32：getUsers——secureKey 校验（未登录/缺 key/错 key）+ 用户列表含启用状态
    #[tokio::test]
    async fn test_get_users_api() {
        let (state, dir) = test_state("getusers").await;
        let mut state = state;
        state.storage.config.secure = true;
        state.storage.config.secure_key = "sk".into();
        state
            .storage
            .insert_user(&User {
                username: "admin".into(),
                token: "t1".into(),
                enable_webdav: true,
                enable_book_source: false,
                book_source_limit: 5,
                ..Default::default()
            })
            .await
            .unwrap();
        state
            .storage
            .insert_user(&User {
                username: "alice".into(),
                token: "t2".into(),
                ..Default::default()
            })
            .await
            .unwrap();

        // 未登录 → 请登录后使用
        let ret = get_users(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.data, json!("NEED_LOGIN"));

        // 已登录但缺 secureKey → NEED_SECURE_KEY
        let params: HashMap<String, String> = [("accessToken".into(), "admin:t1".into())]
            .into_iter()
            .collect();
        let ret = get_users(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.data, json!("NEED_SECURE_KEY"));

        // 错 secureKey → NEED_SECURE_KEY
        let params: HashMap<String, String> = [
            ("accessToken".into(), "admin:t1".into()),
            ("secureKey".into(), "wrong".into()),
        ]
        .into_iter()
        .collect();
        let ret = get_users(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert_eq!(ret.0.data, json!("NEED_SECURE_KEY"));

        // 正确 secureKey → 列表（含启用状态；不含密码字段）
        let params: HashMap<String, String> = [
            ("accessToken".into(), "admin:t1".into()),
            ("secureKey".into(), "sk".into()),
        ]
        .into_iter()
        .collect();
        let ret = get_users(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(ret.0.is_success, "getUsers 应成功: {}", ret.0.error_msg);
        let arr = ret.0.data.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        let admin = arr.iter().find(|v| v["username"] == "admin").unwrap();
        assert_eq!(admin["enableWebdav"], true);
        assert_eq!(admin["enableBookSource"], false);
        assert_eq!(admin["bookSourceLimit"], 5);
        assert!(admin.get("password").is_none(), "列表不应泄露密码");
        assert!(admin.get("salt").is_none());
        assert!(admin.get("token").is_none());

        cleanup(state, dir).await;
    }

    /// F-32：updateUser——权限/限额更新（body 布尔 + query int），不存在用户报错
    #[tokio::test]
    async fn test_update_user_api() {
        let (state, dir) = test_state("upduser").await;
        let mut state = state;
        state.storage.config.secure = true;
        state.storage.config.secure_key = "sk".into();
        state
            .storage
            .insert_user(&User {
                username: "alice".into(),
                token: "t1".into(),
                enable_webdav: false,
                enable_local_store: false,
                enable_book_source: true,
                enable_rss_source: true,
                book_source_limit: 10,
                book_limit: 20,
                ..Default::default()
            })
            .await
            .unwrap();
        let auth = |extra: Vec<(&str, &str)>| -> HashMap<String, String> {
            let mut m: HashMap<String, String> = [
                ("accessToken".into(), "alice:t1".into()),
                ("secureKey".into(), "sk".into()),
            ]
            .into_iter()
            .collect();
            for (k, v) in extra {
                m.insert(k.into(), v.into());
            }
            m
        };

        // body：部分字段更新（camelCase）
        let body = Bytes::from(
            r#"{"username":"alice","enableWebdav":true,"enableBookSource":false,"bookLimit":99}"#,
        );
        let ret = update_user(AxumState(state.clone()), Query(auth(vec![])), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "updateUser 应成功: {}", ret.0.error_msg);
        let alice = state.storage.find_user("alice").await.unwrap().unwrap();
        assert!(alice.enable_webdav);
        assert!(!alice.enable_book_source);
        assert_eq!(alice.book_limit, 99);
        assert_eq!(alice.book_source_limit, 10, "未提供的字段保持原值");
        assert!(alice.enable_rss_source, "未提供的字段保持原值");

        // query 参数：int + bool
        let ret = update_user(
            AxumState(state.clone()),
            Query(auth(vec![
                ("username", "alice"),
                ("enableRssSource", "false"),
                ("bookSourceLimit", "7"),
            ])),
            HeaderMap::new(),
            None,
        )
        .await;
        assert!(ret.0.is_success);
        let alice = state.storage.find_user("alice").await.unwrap().unwrap();
        assert!(!alice.enable_rss_source);
        assert_eq!(alice.book_source_limit, 7);

        // 不存在的用户 → 用户不存在
        let body = Bytes::from(r#"{"username":"ghost","enableWebdav":true}"#);
        let ret = update_user(AxumState(state.clone()), Query(auth(vec![])), HeaderMap::new(), Some(body)).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "用户不存在");

        // 缺 username → 参数错误；缺 secureKey → NEED_SECURE_KEY
        let body = Bytes::from(r#"{"enableWebdav":true}"#);
        let ret = update_user(AxumState(state.clone()), Query(auth(vec![])), HeaderMap::new(), Some(body.clone())).await;
        assert_eq!(ret.0.error_msg, "参数错误");
        let no_key: HashMap<String, String> = [("accessToken".into(), "alice:t1".into())]
            .into_iter()
            .collect();
        let ret = update_user(AxumState(state.clone()), Query(no_key), HeaderMap::new(), Some(body)).await;
        assert_eq!(ret.0.data, json!("NEED_SECURE_KEY"));

        cleanup(state, dir).await;
    }

    /// F-32：deleteUser——不能删自己；删他人成功；secureKey 校验
    #[tokio::test]
    async fn test_delete_user_api() {
        let (state, dir) = test_state("deluser").await;
        let mut state = state;
        state.storage.config.secure = true;
        state.storage.config.secure_key = "sk".into();
        state
            .storage
            .insert_user(&User {
                username: "admin".into(),
                token: "t1".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        state
            .storage
            .insert_user(&User {
                username: "bob".into(),
                token: "t2".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        let params: HashMap<String, String> = [
            ("accessToken".into(), "admin:t1".into()),
            ("secureKey".into(), "sk".into()),
        ]
        .into_iter()
        .collect();

        // 删自己 → 拒绝
        let body = Bytes::from(r#"{"username":"admin"}"#);
        let ret = delete_user(AxumState(state.clone()), Query(params.clone()), HeaderMap::new(), Some(body)).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "不能删除自己");
        assert!(state.storage.find_user("admin").await.unwrap().is_some());

        // 删他人 → 成功
        let body = Bytes::from(r#"{"username":"bob"}"#);
        let ret = delete_user(AxumState(state.clone()), Query(params.clone()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "deleteUser 应成功: {}", ret.0.error_msg);
        assert!(state.storage.find_user("bob").await.unwrap().is_none());

        // 不存在 → 用户不存在；缺 secureKey → NEED_SECURE_KEY
        let body = Bytes::from(r#"{"username":"ghost"}"#);
        let ret = delete_user(AxumState(state.clone()), Query(params.clone()), HeaderMap::new(), Some(body.clone())).await;
        assert_eq!(ret.0.error_msg, "用户不存在");
        let no_key: HashMap<String, String> = [("accessToken".into(), "admin:t1".into())]
            .into_iter()
            .collect();
        let ret = delete_user(AxumState(state.clone()), Query(no_key), HeaderMap::new(), Some(body)).await;
        assert_eq!(ret.0.data, json!("NEED_SECURE_KEY"));

        cleanup(state, dir).await;
    }

    /// F-32：resetUserPassword——新密码生效（genEncryptedPassword 可校验）+ token 失效；secureKey 校验
    #[tokio::test]
    async fn test_reset_user_password_api() {
        let (state, dir) = test_state("resetpw").await;
        let mut state = state;
        state.storage.config.secure = true;
        state.storage.config.secure_key = "sk".into();
        state
            .storage
            .insert_user(&User {
                username: "alice".into(),
                password: "old".into(),
                salt: "oldsalt".into(),
                token: "t1".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        let params: HashMap<String, String> = [
            ("accessToken".into(), "alice:t1".into()),
            ("secureKey".into(), "sk".into()),
        ]
        .into_iter()
        .collect();

        // body：username + newPassword
        let body = Bytes::from(r#"{"username":"alice","newPassword":"新密码abc"}"#);
        let ret = reset_user_password(AxumState(state.clone()), Query(params.clone()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "resetUserPassword 应成功: {}", ret.0.error_msg);
        let alice = state.storage.find_user("alice").await.unwrap().unwrap();
        assert_ne!(alice.password, "old");
        assert_ne!(alice.salt, "oldsalt", "salt 应重新生成");
        assert!(alice.token.is_empty(), "旧 token 应失效");
        assert_eq!(
            crate::util::md5::gen_encrypted_password("新密码abc", &alice.salt),
            alice.password,
            "新密码应可通过登录校验"
        );

        // 重置后旧 token 已失效——重新登录（新 token）以便继续测试管理接口
        state
            .storage
            .update_user_session("alice", "t2", now_millis())
            .await
            .unwrap();
        let params: HashMap<String, String> = [
            ("accessToken".into(), "alice:t2".into()),
            ("secureKey".into(), "sk".into()),
        ]
        .into_iter()
        .collect();

        // query：password 参数；不存在 → 用户不存在；缺 secureKey → NEED_SECURE_KEY
        let mut q = params.clone();
        q.insert("username".into(), "ghost".into());
        q.insert("password".into(), "whatever1".into());
        let ret = reset_user_password(AxumState(state.clone()), Query(q), HeaderMap::new(), None).await;
        assert_eq!(ret.0.error_msg, "用户不存在");
        let no_key: HashMap<String, String> = [("accessToken".into(), "alice:t2".into())]
            .into_iter()
            .collect();
        let ret = reset_user_password(AxumState(state.clone()), Query(no_key), HeaderMap::new(), None).await;
        assert_eq!(ret.0.data, json!("NEED_SECURE_KEY"));
        // 缺密码 → 参数错误
        let body = Bytes::from(r#"{"username":"alice"}"#);
        let ret = reset_user_password(AxumState(state.clone()), Query(params.clone()), HeaderMap::new(), Some(body)).await;
        assert_eq!(ret.0.error_msg, "参数错误");

        cleanup(state, dir).await;
    }

    /// 微型 HTTP 服务器：按 bodies 顺序应答每次请求（耗尽后重复最后一个）；返回 URL
    async fn serve_bodies(bodies: Vec<String>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let bodies = std::sync::Arc::new(std::sync::Mutex::new(bodies));
            for _ in 0..10 {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let body = {
                    let mut b = bodies.lock().unwrap();
                    if b.len() > 1 { b.remove(0) } else { b[0].clone() }
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        format!("http://{addr}/sources.json")
    }

    /// 缓存管理 API：getCacheInfo 统计 + clearCache 按 type 清空 + 参数校验
    #[tokio::test]
    async fn test_cache_api() {
        let (state, dir) = test_state("cacheapi").await;
        state
            .storage
            .cache_toc("https://book.com/a", "https://book.com/toc", "[]")
            .await
            .unwrap();
        state
            .storage
            .save_chapters(
                "local://book1",
                &[
                    ("第一章".to_string(), "正文一甲乙".to_string()),
                    ("第二章".to_string(), "正文二丙丁戊".to_string()),
                ],
            )
            .await
            .unwrap();

        // getCacheInfo：统计字段（camelCase）
        let ret = get_cache_info(AxumState(state.clone())).await;
        assert!(ret.0.is_success);
        assert_eq!(ret.0.data["tocCacheCount"], 1);
        assert_eq!(ret.0.data["tocCacheSize"], 2, "SQLite length() 按字符计");
        assert_eq!(ret.0.data["chapterCount"], 2);
        assert_eq!(ret.0.data["chapterSize"], 11, "5+6 字符");
        assert_eq!(ret.0.data["totalSize"], 13);

        // clearCache：type=toc（body）
        let body = Bytes::from(r#"{"type":"toc"}"#);
        let ret = clear_cache(AxumState(state.clone()), Query(HashMap::new()), Some(body)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert_eq!(ret.0.data["deletedToc"], 1);
        assert_eq!(ret.0.data["deletedChapters"], 0);
        let info = state.storage.get_cache_info().await.unwrap();
        assert_eq!(info.toc_cache_count, 0);
        assert_eq!(info.chapter_count, 2);

        // clearCache：type=chapters（query）
        let params: HashMap<String, String> = [("type".into(), "chapters".into())]
            .into_iter()
            .collect();
        let ret = clear_cache(AxumState(state.clone()), Query(params), None).await;
        assert!(ret.0.is_success);
        assert_eq!(ret.0.data["deletedChapters"], 2);

        // 非法 type → 参数错误
        let body = Bytes::from(r#"{"type":"books"}"#);
        let ret = clear_cache(AxumState(state.clone()), Query(HashMap::new()), Some(body)).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "参数错误");
        // 空 body + 空 query → 参数错误
        let ret = clear_cache(AxumState(state.clone()), Query(HashMap::new()), None).await;
        assert_eq!(ret.0.error_msg, "参数错误");

        cleanup(state, dir).await;
    }

    /// 全书搜索 API：本地书命中（chapterIndex/title/snippet）/ 书源书提示 / 参数校验
    #[tokio::test]
    async fn test_search_book_content_api() {
        let (state, dir) = test_state("searchapi").await;
        state
            .storage
            .save_chapters(
                "local://book1",
                &[
                    ("第一章".to_string(), "这是第一章的正文，关键词出现了。".to_string()),
                    ("第二章".to_string(), "没有匹配。".to_string()),
                ],
            )
            .await
            .unwrap();
        state
            .storage
            .upsert_book(
                "default",
                &crate::model::Book {
                    book_url: "local://book1".into(),
                    name: "本地书".into(),
                    origin: "local".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        state
            .storage
            .upsert_book(
                "default",
                &crate::model::Book {
                    book_url: "https://book.com/web".into(),
                    name: "网文书".into(),
                    origin: "https://source.com".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // GET：key + bookUrl → 命中列表
        let params: HashMap<String, String> = [
            ("key".into(), "关键词".into()),
            ("bookUrl".into(), "local://book1".into()),
        ]
        .into_iter()
        .collect();
        let ret = search_book_content(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        let hits = ret.0.data.as_array().unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["chapterIndex"], 0);
        assert_eq!(hits[0]["title"], "第一章");
        assert!(hits[0]["snippet"].as_str().unwrap().contains("关键词"));

        // POST body 变体
        let body = Bytes::from(r#"{"key":"关键词","bookUrl":"local://book1"}"#);
        let ret = search_book_content(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success);
        assert_eq!(ret.0.data.as_array().unwrap().len(), 1);

        // 书源书 → 仅支持本地书内容搜索
        let params: HashMap<String, String> = [
            ("key".into(), "关键词".into()),
            ("bookUrl".into(), "https://book.com/web".into()),
        ]
        .into_iter()
        .collect();
        let ret = search_book_content(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "仅支持本地书内容搜索");

        // 不存在的书 → 书籍不存在；缺 key / 缺 bookUrl → 参数错误
        let params: HashMap<String, String> = [
            ("key".into(), "关键词".into()),
            ("bookUrl".into(), "local://ghost".into()),
        ]
        .into_iter()
        .collect();
        let ret = search_book_content(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert_eq!(ret.0.error_msg, "书籍不存在");
        let params: HashMap<String, String> = [("bookUrl".into(), "local://book1".into())]
            .into_iter()
            .collect();
        let ret = search_book_content(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert_eq!(ret.0.error_msg, "请输入搜索关键字");
        let params: HashMap<String, String> = [("key".into(), "关键词".into())]
            .into_iter()
            .collect();
        let ret = search_book_content(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert_eq!(ret.0.error_msg, "参数错误");

        cleanup(state, dir).await;
    }

    /// 书源订阅 API：saveSourceSub 抓取校验+批量导入 / getSourceSubs / refreshSourceSub 覆盖 /
    /// deleteSourceSub / 格式错误与上限校验
    #[tokio::test]
    async fn test_source_sub_api() {
        let (state, dir) = test_state("subsapi").await;
        let v1 = r#"[{"bookSourceUrl":"https://s1.com","bookSourceName":"源1"},{"bookSourceUrl":"https://s2.com","bookSourceName":"源2"}]"#;
        let v2 = r#"[{"bookSourceUrl":"https://s1.com","bookSourceName":"源1v2"},{"bookSourceUrl":"https://s2.com","bookSourceName":"源2"},{"bookSourceUrl":"https://s3.com","bookSourceName":"源3"}]"#;
        let sub_url = serve_bodies(vec![v1.to_string(), v2.to_string()]).await;

        // saveSourceSub：抓取 → 校验 → 订阅入库 + 批量导入书源
        let body = Bytes::from(format!(r#"{{"url":"{sub_url}","name":"全量书源"}}"#));
        let ret = save_source_sub(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert_eq!(ret.0.data["count"], 2);
        let subs = state.storage.get_source_subs("default").await.unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].url, sub_url);
        assert_eq!(subs[0].name, "全量书源");
        assert_eq!(subs[0].raw_json.as_deref(), Some(v1), "raw_json 存抓取原文");
        assert_eq!(state.storage.count_book_sources("default").await.unwrap(), 2, "书源已批量导入");
        let s1 = state.storage.find_book_source("default", "https://s1.com").await.unwrap().unwrap();
        assert_eq!(s1.book_source_name, "源1");

        // getSourceSubs：列表（url/name/enabled）
        let ret = get_source_subs(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert!(ret.0.is_success);
        let list = ret.0.data.as_array().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["url"], sub_url.as_str());
        assert_eq!(list[0]["name"], "全量书源");
        assert_eq!(list[0]["enabled"], true);

        // refreshSourceSub：重新拉取 → 覆盖订阅 raw_json + 覆盖/新增书源
        let body = Bytes::from(format!(r#"{{"url":"{sub_url}"}}"#));
        let ret = refresh_source_sub(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert_eq!(ret.0.data["count"], 3);
        let subs = state.storage.get_source_subs("default").await.unwrap();
        assert_eq!(subs[0].name, "全量书源", "刷新保留原订阅名");
        assert_eq!(subs[0].raw_json.as_deref(), Some(v2));
        assert_eq!(state.storage.count_book_sources("default").await.unwrap(), 3);
        let s1 = state.storage.find_book_source("default", "https://s1.com").await.unwrap().unwrap();
        assert_eq!(s1.book_source_name, "源1v2", "已存在书源覆盖更新");

        // refresh 不存在的订阅 → 订阅不存在
        let body = Bytes::from(r#"{"url":"http://127.0.0.1:1/nope.json"}"#);
        let ret = refresh_source_sub(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "订阅不存在");

        // deleteSourceSub：删除订阅，不影响已导入书源
        let body = Bytes::from(format!(r#"{{"url":"{sub_url}"}}"#));
        let ret = delete_source_sub(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert!(state.storage.get_source_subs("default").await.unwrap().is_empty());
        assert_eq!(state.storage.count_book_sources("default").await.unwrap(), 3, "书源保留");

        // 缺 url → 参数校验
        let ret = save_source_sub(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert_eq!(ret.0.error_msg, "请输入订阅链接");
        let ret = delete_source_sub(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert_eq!(ret.0.error_msg, "参数错误");

        // 抓取失败（连接拒绝）→ 远程书源链接错误
        let body = Bytes::from(r#"{"url":"http://127.0.0.1:1/x.json","name":"坏链接"}"#);
        let ret = save_source_sub(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "远程书源链接错误");

        // 非 JSON / 非书源数组 / 空数组 / 缺 bookSourceUrl → 书源数据格式错误
        let bad_url = serve_bodies(vec!["not json".to_string()]).await;
        let body = Bytes::from(format!(r#"{{"url":"{bad_url}"}}"#));
        let ret = save_source_sub(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert_eq!(ret.0.error_msg, "书源数据格式错误");
        let bad_url = serve_bodies(vec!["[{\"foo\":1}]".to_string()]).await;
        let body = Bytes::from(format!(r#"{{"url":"{bad_url}"}}"#));
        let ret = save_source_sub(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert_eq!(ret.0.error_msg, "书源数据格式错误");
        let bad_url = serve_bodies(vec!["[]".to_string()]).await;
        let body = Bytes::from(format!(r#"{{"url":"{bad_url}"}}"#));
        let ret = save_source_sub(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert_eq!(ret.0.error_msg, "书源数据格式错误");

        // 书源数上限：limit=1 时导入 2 个新源 → 超过书源数上限
        state
            .storage
            .insert_user(&User {
                username: "default".into(),
                book_source_limit: 1,
                ..Default::default()
            })
            .await
            .unwrap();
        let ok_url = serve_bodies(vec![v1.to_string()]).await;
        let body = Bytes::from(format!(r#"{{"url":"{ok_url}"}}"#));
        let ret = save_source_sub(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "超过书源数上限");
        assert!(state.storage.get_source_subs("default").await.unwrap().is_empty(), "超限时订阅不落库");

        cleanup(state, dir).await;
    }

    /// 分组收尾：getBookGroups 输出 {id,name,order,orderNum,bookCount}（COUNT 子查询）
    #[tokio::test]
    async fn test_book_groups_with_count_api() {
        let (state, dir) = test_state("grpcnt").await;
        let g1 = state
            .storage
            .save_book_group("default", &crate::model::BookGroup { name: "玄幻".into(), order: 1, ..Default::default() })
            .await
            .unwrap();
        let g2 = state
            .storage
            .save_book_group("default", &crate::model::BookGroup { name: "言情".into(), order: 2, ..Default::default() })
            .await
            .unwrap();
        for url in ["https://b.com/1", "https://b.com/2", "https://b.com/3"] {
            state
                .storage
                .upsert_book("default", &crate::model::Book { book_url: url.into(), name: url.into(), ..Default::default() })
                .await
                .unwrap();
        }
        state.storage.update_book_group_id("default", "https://b.com/1", g1.id).await.unwrap();
        state.storage.update_book_group_id("default", "https://b.com/2", g1.id).await.unwrap();
        state.storage.update_book_group_id("default", "https://b.com/3", g2.id).await.unwrap();

        let ret = get_book_groups(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        let arr = ret.0.data.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["id"], g1.id);
        assert_eq!(arr[0]["name"], "玄幻");
        assert_eq!(arr[0]["order"], 1, "legacy order 字段保留");
        assert_eq!(arr[0]["orderNum"], 1, "orderNum 别名同值");
        assert_eq!(arr[0]["bookCount"], 2, "组内书数");
        assert_eq!(arr[1]["name"], "言情");
        assert_eq!(arr[1]["bookCount"], 1);
        assert_eq!(arr[1]["orderNum"], 2);

        cleanup(state, dir).await;
    }

    /// 分组收尾：saveBookGroup 仅 {id,name} → 重命名保留 order；deleteBookGroup 组内书置 0
    #[tokio::test]
    async fn test_save_book_group_rename_and_delete_api() {
        let (state, dir) = test_state("grpren").await;

        // 新建 {name, order}
        let body = Bytes::from(r#"{"name":"玄幻","order":3}"#);
        let ret = save_book_group(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        let gid = ret.0.data["id"].as_i64().expect("新建应返回 id");

        // 仅 {id,name} → 重命名（order 保留）
        let body = Bytes::from(format!(r#"{{"id":{gid},"name":"玄幻v2"}}"#));
        let ret = save_book_group(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        let list = state.storage.list_book_groups_with_count("default").await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "玄幻v2");
        assert_eq!(list[0].order, 3, "重命名应保留排序");
        assert_eq!(list[0].id, gid);

        // 重命名不存在的分组 → 分组不存在；空名称 → 分组名称不能为空；非 JSON → 参数错误
        let body = Bytes::from(r#"{"id":9999,"name":"幽灵"}"#);
        let ret = save_book_group(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "分组不存在");
        let body = Bytes::from(format!(r#"{{"id":{gid},"name":""}}"#));
        let ret = save_book_group(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert_eq!(ret.0.error_msg, "分组名称不能为空");
        let ret = save_book_group(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(Bytes::from("nope"))).await;
        assert_eq!(ret.0.error_msg, "参数错误");

        // 带 order 的 {id,name,order} → 仍走全量覆盖（兼容旧行为）
        let body = Bytes::from(format!(r#"{{"id":{gid},"name":"玄幻v3","order":9}}"#));
        let ret = save_book_group(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        let list = state.storage.list_book_groups_with_count("default").await.unwrap();
        assert_eq!(list[0].name, "玄幻v3");
        assert_eq!(list[0].order, 9);

        // 删除：组内书 group 置 0，分组移除
        state
            .storage
            .upsert_book("default", &crate::model::Book { book_url: "https://b.com/1".into(), name: "书1".into(), ..Default::default() })
            .await
            .unwrap();
        state.storage.update_book_group_id("default", "https://b.com/1", gid).await.unwrap();
        let body = Bytes::from(format!(r#"{{"id":{gid}}}"#));
        let ret = delete_book_group(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert_eq!(
            state.storage.find_book("default", "https://b.com/1").await.unwrap().unwrap().group,
            0,
            "组内书应置 0"
        );
        assert!(state.storage.list_book_groups("default").await.unwrap().is_empty());

        // 再删 → 分组不存在；缺 id → 参数错误；query 形式 id 同样生效
        let body = Bytes::from(format!(r#"{{"id":{gid}}}"#));
        let ret = delete_book_group(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert_eq!(ret.0.error_msg, "分组不存在");
        let ret = delete_book_group(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(Bytes::from(r#"{"name":"x"}"#))).await;
        assert_eq!(ret.0.error_msg, "参数错误");
        let params: HashMap<String, String> = [("id".into(), gid.to_string())].into_iter().collect();
        let ret = delete_book_group(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert_eq!(ret.0.error_msg, "分组不存在", "query 形式 id 应被识别");

        cleanup(state, dir).await;
    }

    /// F-10 正文缓存：getBookContent 先查 book_chapters（chapterUrl md5 键）命中直读，
    /// 抓取成功后写回；local:// 与缺 bookUrl 不参与缓存
    #[tokio::test]
    async fn test_get_book_content_cache_api() {
        let (state, dir) = test_state("contentcache").await;
        let base_url = serve_bodies(vec![
            r#"<html><body><div class="content">正文一。</div></body></html>"#.to_string(),
            r#"<html><body><div class="content">正文二。</div></body></html>"#.to_string(),
        ])
        .await;
        let base = base_url.trim_end_matches("/sources.json").to_string();
        state
            .storage
            .save_book_source("default", &crate::model::BookSource {
                book_source_url: base.clone(),
                book_source_name: "缓存测试源".into(),
                rule_content: Some(serde_json::json!({ "content": "div.content@text" })),
                ..Default::default()
            })
            .await
            .unwrap();
        let book_url = "https://book.com/a";
        let ch1 = format!("{base}/ch1.html");
        let ch2 = format!("{base}/ch2.html");
        let idx1 = crate::util::md5::chapter_url_hash(&ch1);
        let params = |chapter_url: &str| -> HashMap<String, String> {
            [
                ("chapterUrl".into(), chapter_url.to_string()),
                ("bookUrl".into(), book_url.to_string()),
                ("bookSource".into(), base.clone()),
            ]
            .into_iter()
            .collect()
        };

        // 首次：抓取成功 → 返回 + 写回缓存
        let ret = get_book_content(AxumState(state.clone()), Query(params(&ch1)), HeaderMap::new(), None).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert_eq!(ret.0.data["content"], "正文一。");
        assert_eq!(
            state.storage.get_chapter_content(book_url, idx1).await.unwrap().as_deref(),
            Some("正文一。"),
            "抓取成功应写回 book_chapters"
        );

        // 二次同 chapterUrl：命中缓存直读（若再抓取会拿到正文二。→ 断言失败即回归）
        let ret = get_book_content(AxumState(state.clone()), Query(params(&ch1)), HeaderMap::new(), None).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert_eq!(ret.0.data["content"], "正文一。", "缓存命中应直读不回源");

        // 不同 chapterUrl → 未命中，抓取正文二。并写回
        let ret = get_book_content(AxumState(state.clone()), Query(params(&ch2)), HeaderMap::new(), None).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert_eq!(ret.0.data["content"], "正文二。");
        let idx2 = crate::util::md5::chapter_url_hash(&ch2);
        assert_eq!(state.storage.get_chapter_content(book_url, idx2).await.unwrap().as_deref(), Some("正文二。"));

        // 缺 bookUrl：照常抓取返回，但不落缓存
        let p: HashMap<String, String> = [
            ("chapterUrl".into(), format!("{base}/ch3.html")),
            ("bookSource".into(), base.clone()),
        ]
        .into_iter()
        .collect();
        let ret = get_book_content(AxumState(state.clone()), Query(p), HeaderMap::new(), None).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert!(state.storage.get_chapter_content("", crate::util::md5::chapter_url_hash(&format!("{base}/ch3.html"))).await.unwrap().is_none());

        // local:// 章节不走缓存（不命中、不写回）
        let p: HashMap<String, String> = [
            ("chapterUrl".into(), "local://book1/0".into()),
            ("bookUrl".into(), "local://book1".into()),
        ]
        .into_iter()
        .collect();
        let ret = get_book_content(AxumState(state.clone()), Query(p), HeaderMap::new(), None).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "本地书章节不存在");
        assert_eq!(state.storage.count_chapters("local://book1").await.unwrap(), 0, "local:// 不落正文缓存");

        cleanup(state, dir).await;
    }

    /// 命名兼容批：6 条 legacy 别名路由端到端（真实 router + HTTP 请求）
    #[tokio::test]
    async fn test_alias_routes_end_to_end() {
        let (state, dir) = test_state("alias").await;
        let app = router(state.storage.config.clone(), state.storage.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let base = format!("http://{addr}");
        let client = reqwest::Client::new();

        // getChapterList（= getBookToc）：缺参 → 业务错误（路由可达，非 404）
        let resp = client.get(format!("{base}/reader3/getChapterList")).send().await.unwrap();
        assert_eq!(resp.status(), 200);
        let json: Value = resp.json().await.unwrap();
        assert!(!json["isSuccess"].as_bool().unwrap());
        assert_eq!(json["errorMsg"], "请输入目录链接");

        // getRssContent（= getRssArticle）：缺 url → 业务错误
        let resp = client.get(format!("{base}/reader3/getRssContent")).send().await.unwrap();
        assert_eq!(resp.status(), 200);
        let json: Value = resp.json().await.unwrap();
        assert!(!json["isSuccess"].as_bool().unwrap());

        // getUserList（= getUsers）：非 secure 模式管理接口拒绝（而非 404）
        let resp = client.get(format!("{base}/reader3/getUserList")).send().await.unwrap();
        let json: Value = resp.json().await.unwrap();
        assert!(!json["isSuccess"].as_bool().unwrap());

        // getBookGroupList（= getBookGroups）：空列表
        let resp = client.get(format!("{base}/reader3/getBookGroupList")).send().await.unwrap();
        let json: Value = resp.json().await.unwrap();
        assert!(json["isSuccess"].as_bool().unwrap());
        assert_eq!(json["data"].as_array().unwrap().len(), 0);

        // saveBookGroupName（= saveBookGroup）：新建
        let resp = client
            .post(format!("{base}/reader3/saveBookGroupName"))
            .json(&serde_json::json!({ "name": "分组A" }))
            .send()
            .await
            .unwrap();
        let json: Value = resp.json().await.unwrap();
        assert!(json["isSuccess"].as_bool().unwrap(), "{}", json);
        let gid = json["data"]["id"].as_i64().expect("新建应返回 id");

        // updateBookGroup（= saveBookGroup）：重命名 {id,name}
        let resp = client
            .post(format!("{base}/reader3/updateBookGroup"))
            .json(&serde_json::json!({ "id": gid, "name": "分组A2" }))
            .send()
            .await
            .unwrap();
        let json: Value = resp.json().await.unwrap();
        assert!(json["isSuccess"].as_bool().unwrap(), "{}", json);

        // getBookGroupList 复核：改名生效 + 双字段 + 书数
        let resp = client.get(format!("{base}/reader3/getBookGroupList")).send().await.unwrap();
        let json: Value = resp.json().await.unwrap();
        let arr = json["data"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "分组A2");
        assert_eq!(arr[0]["orderNum"], arr[0]["order"], "order/orderNum 同值");
        assert_eq!(arr[0]["bookCount"], 0);

        // deleteBookGroup：删除成功
        let resp = client
            .post(format!("{base}/reader3/deleteBookGroup"))
            .json(&serde_json::json!({ "id": gid }))
            .send()
            .await
            .unwrap();
        let json: Value = resp.json().await.unwrap();
        assert!(json["isSuccess"].as_bool().unwrap(), "{}", json);

        cleanup(state, dir).await;
    }
}
