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
    // 书源 cookie 存取注册（crawler 抓取/登录按用户命名空间读表）
    crate::service::crawler::register_cookie_storage(storage.clone());
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
        .route("/opds", get(opds_dispatch))
        .route("/opds-save", post(opds_save_post).get(opds_save_post))
        .route("/opds/*rest", get(opds_dispatch))
        // OPDS 独立账号设置（secure 模式外亦可配置，作用于 OPDS Basic 认证）
        .route("/reader3/getOpdsSettings", get(get_opds_settings).post(get_opds_settings))
        .route("/reader3/saveOpdsSettings", post(save_opds_settings))
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
        // 书源登录态（cookie 按用户隔离）
        .route("/reader3/loginBookSource", get(login_book_source).post(login_book_source))
        .route("/reader3/setBookSourceCookie", post(set_book_source_cookie))
        .route("/reader3/getCaptcha", post(get_captcha))
        .route("/reader3/submitCaptcha", post(submit_captcha))
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
        .route("/reader3/markRssArticleRead", post(mark_rss_article_read))
        .route("/reader3/getRssArticle", get(get_rss_article).post(get_rss_article))
        .route("/reader3/searchBook", get(search_book).post(search_book))
        .route("/reader3/searchBookMulti", get(search_book_multi).post(search_book_multi))
        // 换源搜索：同书其他书源列表（url + bookSource）
        .route("/reader3/searchBookSource", get(search_book_source).post(search_book_source))
        .route("/reader3/getBookInfo", get(get_book_info).post(get_book_info))
        .route("/reader3/getBookToc", get(get_book_toc).post(get_book_toc))
        .route("/reader3/getBookContent", get(get_book_content).post(get_book_content))
        // 差距补全批：多格式导出 / 书源调试 / 整书缓存 / 用户配置 / 本地书刷新 / 批量接口 / 书源健康 / 阅读统计
        .route("/reader3/exportBook", get(export_book))
        .route("/reader3/bookSourceDebugSSE", get(book_source_debug_sse).post(book_source_debug_sse))
        .route("/reader3/cacheBookOnServer", post(cache_book_on_server))
        .route("/reader3/cacheBookSSE", get(cache_book_sse).post(cache_book_sse))
        .route("/reader3/cancelCacheBook", get(cancel_cache_book).post(cancel_cache_book))
        .route("/reader3/getUserConfig", get(get_user_config).post(get_user_config))
        .route("/reader3/saveUserConfig", post(save_user_config))
        .route("/reader3/refreshLocalBook", post(refresh_local_book))
        .route("/reader3/deleteBooks", post(delete_books))
        .route("/reader3/deleteBookmarks", post(delete_bookmarks))
        .route("/reader3/saveRssSources", post(save_rss_sources))
        .route("/reader3/saveBookmarks", post(save_bookmarks))
        .route("/reader3/addBookGroupMulti", post(add_book_group_multi))
        .route("/reader3/removeBookGroupMulti", post(remove_book_group_multi))
        .route("/reader3/saveBookGroupOrder", post(save_book_group_order))
        .route("/reader3/getAvailableBookSource", get(get_available_book_source).post(get_available_book_source))
        .route("/reader3/getInvalidBookSources", get(get_invalid_book_sources).post(get_invalid_book_sources))
        .route("/reader3/setAsDefaultBookSources", post(set_as_default_book_sources))
        .route("/reader3/searchBookSourceSSE", get(search_book_source_sse).post(search_book_source_sse))
        .route("/reader3/getReadingStats", get(get_reading_stats).post(get_reading_stats))
        // 小项补全批：单书缓存删除 / 书架缓存信息 / 导入预览 / 书源文件读取 / 正文缓存写回 /
        // 用户书源删除 / 分组别名 / 目录规则单页调试
        .route("/reader3/deleteBookCache", get(delete_book_cache))
        .route(
            "/reader3/getShelfBookWithCacheInfo",
            get(get_shelf_book_with_cache_info).post(get_shelf_book_with_cache_info),
        )
        .route(
            "/reader3/importBookPreview",
            post(import_book_preview)
                .layer(axum::extract::DefaultBodyLimit::max(100 * 1024 * 1024)),
        )
        .route("/reader3/readSourceFile", post(read_source_file))
        .route("/reader3/saveBookContent", post(save_book_content))
        .route("/reader3/deleteUserBookSource", post(delete_user_book_source))
        .route("/reader3/saveBookGroupId", post(save_book_group_id))
        .route(
            "/reader3/getChapterListByRule",
            get(get_chapter_list_by_rule).post(get_chapter_list_by_rule),
        )
        // 命名兼容批 2（legacy 别名路由——外部客户端兼容）
        .route("/reader3/resetPassword", post(reset_user_password))
        .route("/reader3/httpTTS", get(tts_synthesize).post(tts_synthesize))
        .route(
            "/reader3/uploadFile",
            post(crate::api::files::upload)
                .layer(axum::extract::DefaultBodyLimit::max(100 * 1024 * 1024)),
        )
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

    // 生成新 token 并更新会话（uuid v4 随机——防预测；legacy 的 md5 时间戳 token 不再使用）
    let now = now_millis();
    let token = uuid::Uuid::new_v4().simple().to_string();
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

// ---------------- 书源登录态（cookie 按用户隔离） ----------------

/// 登录参数合并：query + body（JSON 优先，form-urlencoded 兑底）——纯函数，可测
fn merge_login_params(
    query: &HashMap<String, String>,
    body: Option<&[u8]>,
) -> HashMap<String, String> {
    let mut m = query.clone();
    let Some(body) = body else { return m };
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) {
        if let Some(obj) = v.as_object() {
            for (k, val) in obj {
                if let Some(s) = val.as_str() {
                    m.insert(k.clone(), s.to_string());
                } else {
                    m.insert(k.clone(), val.to_string());
                }
            }
            return m;
        }
    }
    for (k, v) in url::form_urlencoded::parse(body) {
        m.insert(k.into_owned(), v.into_owned());
    }
    m
}

/// 解析 bookSource 参数（书源 URL 或完整 JSON）——复用 resolve_book_source 语义
async fn resolve_login_source(
    state: &AppState,
    ns: &str,
    book_source_param: &str,
) -> Option<crate::model::BookSource> {
    if book_source_param.trim_start().starts_with('{') {
        return serde_json::from_str(book_source_param).ok();
    }
    state
        .storage
        .find_book_source(ns, book_source_param)
        .await
        .ok()
        .flatten()
}

/// POST/GET /reader3/loginBookSource：书源登录（登录态独立于系统用户，cookie 按用户存库）
///
/// 参数（query 或 body JSON/form）：bookSource（书源 URL 或完整 JSON）、username、password、
/// captcha（图片验证码文本）、mode=browser（强制浏览器自动登录）。
///
/// 返回：
/// - 成功：{success: true, cookie}
/// - 图片验证码：{success: false, needCaptcha: true, captchaUrl, captchaId, message}
///   （前端显示验证码 → 输入后重新调用本接口（captcha 参数）或 POST submitCaptcha）
/// - 点击类验证码（滑块/点选）无法自动处理：{success: false, needManualCaptcha: true, message}
///   （引导：浏览器登录书源后，在书源设置粘贴 Cookie）
/// - 登录失败：{success: false, message}
#[axum::debug_handler]
async fn login_book_source(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let merged = merge_login_params(&params, body.as_deref());
    let book_source_param = merged.get("bookSource").cloned().unwrap_or_default();
    if book_source_param.is_empty() {
        return Json(ReturnData::err("缺少 bookSource 参数"));
    }
    let Some(source) = resolve_login_source(&state, &namespace, &book_source_param).await else {
        return Json(ReturnData::err("书源不存在（请先导入书源）"));
    };
    if source.login_url.as_deref().unwrap_or("").trim().is_empty() {
        return Json(ReturnData::err("书源未配置 loginUrl"));
    }
    let req = crate::service::login::LoginRequest {
        username: merged.get("username").cloned().unwrap_or_default(),
        password: merged.get("password").cloned().unwrap_or_default(),
        captcha: merged.get("captcha").cloned().unwrap_or_default(),
    };
    let mode = merged.get("mode").cloned().unwrap_or_default();
    let outcome = if mode == "browser" {
        if !crate::service::browser::is_browser_available() {
            return Json(ReturnData::err(
                "未安装浏览器（Chrome/Edge）——无法使用浏览器自动登录，请在书源设置粘贴 Cookie",
            ));
        }
        crate::service::login::login_browser(&state.storage, &namespace, &source, &req).await
    } else {
        crate::service::login::login_http(&state.storage, &namespace, &source, &req).await
    };
    match outcome {
        Ok(crate::service::login::LoginOutcome::Success { cookie }) => Json(ReturnData::ok(
            serde_json::json!({ "success": true, "needCaptcha": false, "cookie": cookie }),
        )),
        Ok(crate::service::login::LoginOutcome::NeedImageCaptcha { captcha_url, captcha_id, message }) => {
            Json(ReturnData::ok(serde_json::json!({
                "success": false,
                "needCaptcha": true,
                "captchaUrl": captcha_url,
                "captchaId": captcha_id,
                "message": message,
            })))
        }
        Ok(crate::service::login::LoginOutcome::NeedManualCookie { message }) => Json(ReturnData::ok(
            serde_json::json!({ "success": false, "needManualCaptcha": true, "message": message }),
        )),
        Ok(crate::service::login::LoginOutcome::Failed { message }) => {
            Json(ReturnData::ok(serde_json::json!({ "success": false, "message": message })))
        }
        Err(e) => {
            tracing::error!("loginBookSource 失败 [{}]: {e}", source.book_source_name);
            Json(ReturnData::err(e.to_string()))
        }
    }
}

/// POST /reader3/setBookSourceCookie：手动设置书源 cookie（按当前用户存库）
///
/// body/query：bookSource（书源 URL）+ cookie（cookie 串；空值 = 清除）
/// 场景：点击类验证码无法自动处理时，用户在浏览器登录书源后把 cookie 粘贴到书源设置
async fn set_book_source_cookie(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let merged = merge_login_params(&params, body.as_deref());
    let book_source = merged.get("bookSource").cloned().unwrap_or_default();
    if book_source.is_empty() {
        return Json(ReturnData::err("缺少 bookSource 参数"));
    }
    let cookie = merged.get("cookie").cloned().unwrap_or_default();
    if cookie.trim().is_empty() {
        match state.storage.clear_cookie(&namespace, &book_source).await {
            Ok(_) => Json(ReturnData::ok(serde_json::json!({ "success": true, "cleared": true }))),
            Err(e) => {
                tracing::error!("setBookSourceCookie 清除失败: {e}");
                Json(ReturnData::err("清除失败"))
            }
        }
    } else {
        match state.storage.set_cookie(&namespace, &book_source, &cookie).await {
            Ok(_) => Json(ReturnData::ok(serde_json::json!({ "success": true }))),
            Err(e) => {
                tracing::error!("setBookSourceCookie 写入失败: {e}");
                Json(ReturnData::err("保存失败"))
            }
        }
    }
}

/// POST /reader3/getCaptcha：重新触发登录页 → 检测验证码 → 返回验证码资源
///
/// body：bookSource。返回 {captchaType: image|slider|click|none, captchaUrl(data URI), captchaId, pageUrl}
/// - image：验证码图片（服务端截图，前端可直接显示）→ 前端输入后 POST submitCaptcha
/// - slider/click：浏览器自动处理/降级（见 loginBookSource 契约）
async fn get_captcha(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let merged = merge_login_params(&params, body.as_deref());
    let book_source_param = merged.get("bookSource").cloned().unwrap_or_default();
    if book_source_param.is_empty() {
        return Json(ReturnData::err("缺少 bookSource 参数"));
    }
    let Some(source) = resolve_login_source(&state, &namespace, &book_source_param).await else {
        return Json(ReturnData::err("书源不存在"));
    };
    match crate::service::login::get_captcha(&state.storage, &namespace, &source).await {
        Ok(data) => Json(ReturnData::ok(data)),
        Err(e) => {
            tracing::error!("getCaptcha 失败 [{}]: {e}", source.book_source_name);
            Json(ReturnData::err(e.to_string()))
        }
    }
}

/// POST /reader3/submitCaptcha：图片验证码文本回填（浏览器流）→ 登录 → {isLogin}
///
/// body：bookSource + captchaId + captchaText（+ 可选 username/password 覆盖会话值）
async fn submit_captcha(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let merged = merge_login_params(&params, body.as_deref());
    let book_source_param = merged.get("bookSource").cloned().unwrap_or_default();
    if book_source_param.is_empty() {
        return Json(ReturnData::err("缺少 bookSource 参数"));
    }
    let Some(source) = resolve_login_source(&state, &namespace, &book_source_param).await else {
        return Json(ReturnData::err("书源不存在"));
    };
    let captcha_id = merged.get("captchaId").cloned().unwrap_or_default();
    let captcha_text = merged.get("captchaText").cloned().unwrap_or_default();
    if captcha_id.is_empty() {
        return Json(ReturnData::err("缺少 captchaId 参数"));
    }
    match crate::service::login::submit_captcha(
        &state.storage,
        &namespace,
        &source,
        &captcha_id,
        &captcha_text,
        merged.get("username").map(String::as_str),
        merged.get("password").map(String::as_str),
    )
    .await
    {
        Ok(data) => Json(ReturnData::ok(data)),
        Err(e) => {
            tracing::error!("submitCaptcha 失败 [{}]: {e}", source.book_source_name);
            Json(ReturnData::err(e.to_string()))
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
    if cache_type.is_empty() {
        cache_type = "all".to_string();
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
    // 文件型本地书（legacy loc_book：正文不入章节表）——解析文件逐章匹配
    if let Some(book) = &shelf {
        if book.origin == "loc_book" && book.book_url.starts_with("storage/") {
            return match search_file_book_content(&state, &namespace, book, &key).await {
                Ok(hits) => Json(ReturnData::ok(serde_json::to_value(hits).unwrap_or(serde_json::Value::Null))),
                Err(e) => {
                    tracing::warn!("searchBookContent 文件书失败 [{book_url}]: {e}");
                    Json(ReturnData::err("搜索失败"))
                }
            };
        }
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

/// 文件型本地书全书搜索：解析文件 → 逐章匹配（key 大小写不敏感，snippet 取命中上下文）
async fn search_file_book_content(
    state: &AppState,
    namespace: &str,
    book: &crate::model::book::Book,
    key: &str,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let path = resolve_loc_book_file(&state.storage.config.storage_dir(), &book.book_url)
        .ok_or_else(|| anyhow::anyhow!("文件不存在"))?;
    let path_lower = path.to_string_lossy().to_lowercase();
    let imported = if path_lower.ends_with(".epub") {
        let bytes = std::fs::read(&path)?;
        crate::service::local_book::parse_epub(&bytes)?
    } else {
        let user_rules = txt_toc_rule_regexes(state, namespace).await;
        crate::service::local_book::parse_txt_file_with_rules(&path, &user_rules)?
    };
    let key_lower = key.to_lowercase();
    let mut hits: Vec<serde_json::Value> = Vec::new();
    for (i, ch) in imported.chapters.iter().enumerate() {
        if hits.len() >= 100 {
            break;
        }
        let title = if ch.title.is_empty() {
            format!("第{}章", i + 1)
        } else {
            ch.title.clone()
        };
        let matched_in_title = ch.title.to_lowercase().contains(&key_lower);
        let content_lower = ch.content.to_lowercase();
        let pos = if matched_in_title {
            Some(0usize)
        } else {
            content_lower.find(&key_lower)
        };
        if let Some(p) = pos {
            let content = &ch.content;
            let start = content.floor_char_boundary(p.saturating_sub(30));
            let end = content.floor_char_boundary((p + key.len() + 50).min(content.len()));
            let snippet = if start > 0 { "…" } else { "" }.to_string()
                + &ch.content[start..end]
                + if end < ch.content.len() { "…" } else { "" };
            hits.push(serde_json::json!({
                "chapterIndex": i,
                "title": title,
                "snippet": snippet,
            }));
        }
    }
    Ok(hits)
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
            // 已读标记合并：入库后按 url 回读 read 列 → 序列化为 hasRead
            let flags = state
                .storage
                .get_rss_article_read_flags(&namespace, &source_url)
                .await
                .unwrap_or_default();
            let articles: Vec<crate::model::RssArticle> = articles
                .into_iter()
                .map(|mut a| {
                    a.read = flags.get(&a.url).copied().unwrap_or(false);
                    a
                })
                .collect();
            Json(ReturnData::ok(serde_json::to_value(&articles).unwrap_or(Value::Null)))
        }
        Err(e) => {
            tracing::error!("getRssArticles 抓取失败 [{}]: {e}", source.source_url);
            Json(ReturnData::err("抓取失败"))
        }
    }
}

/// POST /reader3/markRssArticleRead：标记 RSS 文章已读/未读（body { articleUrl, read }）
async fn mark_rss_article_read(
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
    let url = param_of(&params, body_json.as_ref(), "articleUrl");
    if url.is_empty() {
        return Json(ReturnData::err("RSS文章链接不能为空"));
    }
    let read = body_json
        .as_ref()
        .and_then(|b| b.get("read").and_then(|v| v.as_bool()))
        .unwrap_or(true);
    match state.storage.set_rss_article_read(&url, read).await {
        Ok(_) => Json(ReturnData::ok(Value::Null)),
        Err(e) => {
            tracing::error!("markRssArticleRead 失败: {e}");
            Json(ReturnData::err("标记失败"))
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

    match crate::service::search::search_one_source(&namespace, &source, &key, page).await {
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
    let ns = namespace.clone();
    for source in sources {
        let sem = semaphore.clone();
        let key = key.clone();
        let ns = ns.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await;
            crate::service::search::search_one_source(&ns, &source, &key, page).await.unwrap_or_default()
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

/// POST/GET /reader3/searchBookSource：换源搜索
///
/// 参数：url（当前书 bookUrl）+ bookSource（当前源 URL/名称）
/// 逻辑：取当前书名 → 全部启用可搜索书源（排除当前源）并发搜索 → 书名匹配过滤 → 按书源去重
/// 返回：SearchBook[]（每项含 origin/originName/tocUrl，前端点击后 saveBook 切源）
async fn search_book_source(
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
    let book_source_param = param_of(&params, body_json.as_ref(), "bookSource");
    if url.is_empty() {
        return Json(ReturnData::err("请输入书籍链接"));
    }
    if book_source_param.is_empty() {
        return Json(ReturnData::err("未配置书源"));
    }

    // ① 当前书名：书架优先；未入架走详情解析（同 getBookInfo）
    let name = match state.storage.find_book(&namespace, &url).await {
        Ok(Some(b)) => b.name,
        _ => {
            let Some(source) = resolve_book_source(&state, &namespace, &book_source_param).await
            else {
                return Json(ReturnData::err("书源不存在"));
            };
            match crate::service::book::fetch_url(&namespace, &url, &source).await {
                Ok(resp) => {
                    let info = crate::service::book::analyze_book_info(
                        &resp.body,
                        &resp.url,
                        &source,
                        &url,
                    );
                    if info.name.is_empty() {
                        return Json(ReturnData::err("获取书籍信息失败"));
                    }
                    info.name
                }
                Err(e) => {
                    tracing::error!("searchBookSource 获取书名失败 [{url}]: {e}");
                    return Json(ReturnData::err("获取书籍信息失败"));
                }
            }
        }
    };
    let key = name.trim();
    if key.is_empty() {
        return Json(ReturnData::err("无法获取书名"));
    }

    // ② 全部启用可搜索书源（排除当前源：URL 或名称匹配）
    let current = book_source_param.trim();
    let sources: Vec<crate::model::BookSource> = match state.storage.get_book_sources(&namespace).await {
        Ok(s) => s
            .into_iter()
            .filter(|s| {
                s.enabled
                    && s.search_url.is_some()
                    && s.book_source_url != current
                    && s.book_source_name != current
            })
            .collect(),
        Err(_) => return Json(ReturnData::err("系统错误")),
    };
    if sources.is_empty() {
        return Json(ReturnData::ok(serde_json::Value::Null));
    }

    // ③ 并发搜索（限制并发 8，同 searchBookMulti）
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
    let mut handles = Vec::with_capacity(sources.len());
    let ns = namespace.clone();
    for source in sources {
        let sem = semaphore.clone();
        let key = key.to_string();
        let ns = ns.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await;
            crate::service::search::search_one_source(&ns, &source, &key, 1)
                .await
                .unwrap_or_default()
        }));
    }

    // ④ 汇总：书名匹配过滤（忽略大小写，双向包含）+ 按书源去重（保留首条）
    let mut all: Vec<crate::service::search::SearchBook> = Vec::new();
    for h in handles {
        if let Ok(books) = h.await {
            all.extend(books);
        }
    }
    let ql = key.to_lowercase();
    let mut seen = std::collections::HashSet::new();
    let matched: Vec<_> = all
        .into_iter()
        .filter(|b| {
            let bl = b.name.to_lowercase();
            bl.contains(&ql) || ql.contains(&bl)
        })
        .filter(|b| seen.insert(b.origin.clone()))
        .collect();
    tracing::info!(
        "searchBookSource [{namespace}] 《{key}》：命中 {} 条",
        matched.len()
    );
    Json(ReturnData::ok(serde_json::to_value(matched).unwrap_or(serde_json::Value::Null)))
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
    match crate::service::book::fetch_url(&namespace, &url, &source).await {
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
    // 文件型本地书（legacy：bookUrl = storage/data/.../xx.txt 或任意白名单扩展名）——按扩展名解析分章
    if crate::service::local_book::SUPPORTED_EXTENSIONS
        .iter()
        .any(|e| toc_url.to_lowercase().ends_with(&format!(".{e}")))
    {
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
    match crate::service::book::analyze_toc(&namespace, &toc_url, &source, 20).await {
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
    // legacy 本地书：bookUrl#index（bookUrl 是 storage/ 路径或任意白名单扩展名文件）
    if chapter_url.contains("#") && is_loc_book_file_chapter(&chapter_url) {
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
    match crate::service::book::analyze_content(&namespace, &chapter_url, &source, 5).await {
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

// ==================== 差距补全批：导出 / 调试 / 缓存 / 配置 / 刷新 / 批量 / 健康 / 统计 ====================

/// GET /reader3/exportBook：多格式导出（url 单本 + format=txt|epub|html）
/// txt=章节拼接、epub=zip 构造（mimetype/container.xml/OPF/spine）、html=单页
async fn export_book(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Response {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret).into_response(),
    };
    let body_json = body.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let url = param_of(&params, body_json.as_ref(), "url");
    if url.is_empty() {
        return Json(ReturnData::err("请输入书籍链接")).into_response();
    }
    let format = param_of(&params, body_json.as_ref(), "format");
    let format = if format.is_empty() { "txt" } else { format.as_str() };
    if !matches!(format, "txt" | "epub" | "html") {
        return Json(ReturnData::err("不支持的导出格式（txt|epub|html）")).into_response();
    }
    let (title, author, chapters) =
        match collect_export_chapters(&state, &namespace, &url, &params, body_json.as_ref()).await
        {
            Ok(v) => v,
            Err(msg) => return Json(ReturnData::err(msg)).into_response(),
        };
    if chapters.is_empty() {
        return Json(ReturnData::err("没有可导出的章节")).into_response();
    }
    let export_chapters: Vec<crate::service::export_book::ExportChapter> = chapters
        .iter()
        .map(|(t, c)| crate::service::export_book::ExportChapter {
            title: t.clone(),
            content: c.clone(),
        })
        .collect();
    let (bytes, mime, ext) = match format {
        "epub" => (
            crate::service::export_book::build_epub(&title, &author, &export_chapters),
            "application/epub+zip",
            "epub",
        ),
        "html" => (
            crate::service::export_book::build_html(&title, &export_chapters).into_bytes(),
            "text/html; charset=utf-8",
            "html",
        ),
        _ => (
            crate::service::export_book::build_txt(&title, &export_chapters).into_bytes(),
            "text/plain; charset=utf-8",
            "txt",
        ),
    };
    let filename = sanitize_filename(&title);
    let filename = if filename.is_empty() { "export".to_string() } else { filename };
    // RFC 5987：非 ASCII 文件名百分号编码（HeaderValue 需可见 ASCII）
    let encoded = percent_encode_filename(&filename);
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", mime)
        .header(
            "Content-Disposition",
            format!("attachment; filename=\"{encoded}.{ext}\""),
        )
        .body(Body::from(bytes))
        .unwrap()
}

/// 文件名百分号编码（保留 ASCII 字母数字与 -_. 空格，其余 UTF-8 字节 %XX）
fn percent_encode_filename(name: &str) -> String {
    let mut out = String::new();
    for &b in name.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b' ') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// 收集导出章节：(书名, 作者, [(章节标题, 正文)])——本地书/文件书/书源书统一入口
async fn collect_export_chapters(
    state: &AppState,
    ns: &str,
    url: &str,
    params: &HashMap<String, String>,
    body_json: Option<&serde_json::Value>,
) -> Result<(String, String, Vec<(String, String)>), String> {
    let shelf = state.storage.find_book(ns, url).await.ok().flatten();
    let is_local = url.starts_with("local://");
    let is_file = url.starts_with("storage/")
        || crate::service::local_book::SUPPORTED_EXTENSIONS
            .iter()
            .any(|e| url.to_lowercase().ends_with(&format!(".{e}")));

    // ① 本地书（local://）：章节表直读
    if is_local {
        let book = shelf.ok_or_else(|| "书籍不存在（请先加入书架）".to_string())?;
        let rows = state
            .storage
            .list_chapters(url)
            .await
            .map_err(|e| format!("读取章节失败: {e}"))?;
        let mut chapters = Vec::with_capacity(rows.len());
        for (idx, title) in rows {
            let content = state
                .storage
                .get_chapter_content(url, idx)
                .await
                .map_err(|e| format!("读取章节失败: {e}"))?
                .unwrap_or_default();
            chapters.push((title, content));
        }
        return Ok((book.name, book.author, chapters));
    }
    // ② 文件型本地书：解析原文件（TXT 用用户规则）
    if is_file {
        let path = resolve_export_file_path(&state.storage.config.storage_dir(), url)
            .ok_or_else(|| "本地书文件不存在".to_string())?;
        let user_rules = txt_toc_rule_regexes(state, ns).await;
        let imported = crate::service::local_book::parse_loc_book_path(&path, &user_rules)
            .map_err(|e| format!("解析失败: {e}"))?;
        let name = shelf
            .as_ref()
            .map(|b| b.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| imported.meta.title.clone());
        let chapters: Vec<(String, String)> = imported
            .chapters
            .iter()
            .map(|c| (c.title.clone(), c.content.clone()))
            .collect();
        return Ok((name, imported.meta.author, chapters));
    }
    // ③ 书源书：书架 origin 定位书源（兜底 bookSource 参数）→ 目录 → 逐章正文（优先缓存）
    let book = shelf.ok_or_else(|| "书籍不存在（请先加入书架）".to_string())?;
    let mut source = if !book.origin.is_empty() {
        state
            .storage
            .find_book_source(ns, &book.origin)
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    if source.is_none() {
        let bs = param_of(params, body_json, "bookSource");
        if !bs.is_empty() {
            source = resolve_book_source(state, ns, &bs).await;
        }
    }
    let Some(source) = source else {
        return Err("书源不存在".to_string());
    };
    let toc_url = if book.toc_url.is_empty() {
        url.to_string()
    } else {
        book.toc_url.clone()
    };
    let toc = crate::service::book::analyze_toc(ns, &toc_url, &source, 20)
        .await
        .map_err(|e| format!("获取目录失败: {e}"))?;
    let mut chapters = Vec::with_capacity(toc.len());
    for ch in toc {
        if ch.is_volume {
            continue;
        }
        let idx = crate::util::md5::chapter_url_hash(&ch.url);
        let content = match state.storage.get_chapter_content(url, idx).await.ok().flatten() {
            Some(c) if !c.trim().is_empty() => c,
            _ => crate::service::book::analyze_content(ns, &ch.url, &source, 5)
                .await
                .map_err(|e| format!("获取正文失败: {e}"))?,
        };
        chapters.push((ch.title, content));
    }
    Ok((book.name, book.author, chapters))
}

/// 文件名净化（去路径分隔符/非法字符，截断 80 字符）
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .chars()
        .take(80)
        .collect()
}

/// 文件型本地书路径解析（严格防穿越 + legacy 目录式兜底）
pub(crate) fn resolve_export_file_path(
    storage_dir: &std::path::Path,
    book_url: &str,
) -> Option<std::path::PathBuf> {
    resolve_storage_path(storage_dir, book_url)
        .or_else(|| resolve_loc_book_file(storage_dir, book_url))
}

/// GET/POST /reader3/bookSourceDebugSSE：逐规则执行测试（SSE 事件流）
/// 参数：bookSource（必填）+ action=search|explore|toc|content + key + url/chapterUrl
/// 输出：{type:step,message:{ruleName,url,elapsedMs,resultLen,error,detail}} → {type:result,data}
async fn book_source_debug_sse(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Response {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return sse_error(ret),
    };
    let body_json = body.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let action = param_of(&params, body_json.as_ref(), "action");
    let key = param_of(&params, body_json.as_ref(), "key");
    let mut target = param_of(&params, body_json.as_ref(), "chapterUrl");
    if target.is_empty() {
        target = param_of(&params, body_json.as_ref(), "url");
    }
    if !matches!(action.as_str(), "search" | "explore" | "toc" | "content") {
        return sse_error(ReturnData::err("请输入调试动作（search|explore|toc|content）"));
    }
    if action == "search" && key.is_empty() {
        return sse_error(ReturnData::err("请输入搜索关键字"));
    }
    let bs_param = param_of(&params, body_json.as_ref(), "bookSource");
    let Some(source) = resolve_book_source(&state, &namespace, &bs_param).await else {
        return sse_error(ReturnData::err("书源不存在"));
    };

    let (tx, rx) =
        tokio::sync::mpsc::unbounded_channel::<Result<Bytes, std::convert::Infallible>>();
    let ns = namespace.clone();
    tokio::spawn(async move {
        let send = |tx: &tokio::sync::mpsc::UnboundedSender<Result<Bytes, std::convert::Infallible>>,
                    payload: &serde_json::Value| {
            let text = format!("data: {payload}\n\n");
            let _ = tx.send(Ok(Bytes::from(text)));
        };
        send(
            &tx,
            &json!({
                "type": "start",
                "message": { "action": action, "bookSource": source.book_source_name },
            }),
        );
        let result = crate::service::debug::run_debug(
            &ns,
            &source,
            &action,
            &key,
            &target,
            |step| {
                send(&tx, &json!({ "type": "step", "message": step }));
            },
        )
        .await;
        match result {
            Ok(data) => send(&tx, &json!({ "type": "result", "data": data })),
            Err(e) => send(&tx, &json!({ "type": "error", "message": e.to_string() })),
        }
    });

    let stream = futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream; charset=utf-8")
        .header("Cache-Control", "no-cache")
        .body(Body::from_stream(stream))
        .unwrap()
}

/// POST /reader3/cacheBookOnServer：后台整书缓存（目录 → 逐章正文 → 缓存表，并发 3）
async fn cache_book_on_server(
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
    if state.storage.find_book(&namespace, &url).await.ok().flatten().is_none() {
        return Json(ReturnData::err("书籍不存在（请先加入书架）"));
    }
    let progress = crate::service::cache_job::start(&namespace, &url, state.storage.clone());
    let p = progress.lock().unwrap_or_else(|e| e.into_inner());
    Json(ReturnData::ok(json!({
        "started": !p.finished,
        "url": url,
        "cached": p.cached,
        "total": p.total,
        "title": p.title,
    })))
}

/// GET/POST /reader3/cacheBookSSE：缓存进度流 {cached,total,title,finished,error,cancelled}
async fn cache_book_sse(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Response {
    let _namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return sse_error(ret),
    };
    let body_json = body.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let url = param_of(&params, body_json.as_ref(), "url");
    if url.is_empty() {
        return sse_error(ReturnData::err("参数错误"));
    }
    let Some(progress) = crate::service::cache_job::progress_of(&url) else {
        return sse_error(ReturnData::err("缓存任务不存在"));
    };

    let (tx, rx) =
        tokio::sync::mpsc::unbounded_channel::<Result<Bytes, std::convert::Infallible>>();
    let progress_for_task = progress.clone();
    tokio::spawn(async move {
        loop {
            let (payload, finished) = {
                let p = progress_for_task.lock().unwrap_or_else(|e| e.into_inner());
                (
                    json!({
                        "cached": p.cached,
                        "total": p.total,
                        "title": p.title,
                        "finished": p.finished,
                        "cancelled": p.cancelled,
                        "error": p.error,
                    }),
                    p.finished,
                )
            };
            let text = format!("data: {payload}\n\n");
            if tx.send(Ok(Bytes::from(text))).is_err() {
                return; // 客户端断开
            }
            if finished {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
    });

    let stream = futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream; charset=utf-8")
        .header("Cache-Control", "no-cache")
        .body(Body::from_stream(stream))
        .unwrap()
}

/// GET/POST /reader3/cancelCacheBook：取消后台缓存任务（内存任务表）
async fn cancel_cache_book(
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
    let url = param_of(&params, body_json.as_ref(), "url");
    if url.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    let cancelled = crate::service::cache_job::cancel(&url);
    Json(ReturnData::ok(json!({ "cancelled": cancelled })))
}

/// GET/POST /reader3/getUserConfig：用户配置读取（按用户 + 配置命名空间；key/ns 参数，默认 global）
async fn get_user_config(
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
    let mut key = param_of(&params, body_json.as_ref(), "key");
    if key.is_empty() {
        key = param_of(&params, body_json.as_ref(), "ns");
    }
    if key.is_empty() {
        key = "global".to_string();
    }
    match state.storage.get_user_config(&namespace, &key).await {
        Ok(Some(raw)) => {
            // 配置为 JSON 文本 → 解析返回（解析失败原样返回字符串）
            let data = serde_json::from_str::<serde_json::Value>(&raw)
                .unwrap_or(serde_json::Value::String(raw));
            Json(ReturnData::ok(json!({ "ns": key, "config": data })))
        }
        Ok(None) => Json(ReturnData::ok(json!({ "ns": key, "config": serde_json::Value::Null }))),
        Err(e) => {
            tracing::error!("getUserConfig [{namespace}/{key}] 失败: {e}");
            Json(ReturnData::err("系统错误"))
        }
    }
}

/// POST /reader3/saveUserConfig：用户配置保存（body：{ns?, config: JSON} 或裸 JSON 整体）
async fn save_user_config(
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
    // 键：body.ns/key → query → global
    let mut key = json
        .get("ns")
        .or_else(|| json.get("key"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if key.is_empty() {
        key = params.get("ns").cloned().unwrap_or_else(|| "global".to_string());
    }
    // 配置：body.config（任意 JSON）→ 序列化；无 config 键则整体为配置
    let config = json.get("config").cloned().unwrap_or(json);
    let raw = match &config {
        serde_json::Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| "{}".to_string()),
    };
    match state.storage.save_user_config(&namespace, &key, &raw).await {
        Ok(_) => Json(ReturnData::ok(serde_json::Value::Null)),
        Err(e) => {
            tracing::error!("saveUserConfig [{namespace}/{key}] 失败: {e}");
            Json(ReturnData::err("保存失败"))
        }
    }
}

/// POST /reader3/refreshLocalBook：重扫本地书（local:// 重解析原文件；文件书重解析）
async fn refresh_local_book(
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
    let book = match state.storage.find_book(&namespace, &url).await {
        Ok(Some(b)) => b,
        Ok(None) => return Json(ReturnData::err("书籍不存在")),
        Err(e) => {
            tracing::error!("refreshLocalBook 查询失败: {e}");
            return Json(ReturnData::err("系统错误"));
        }
    };
    let user_rules = txt_toc_rule_regexes(&state, &namespace).await;
    let is_file = url.starts_with("storage/")
        || crate::service::local_book::SUPPORTED_EXTENSIONS
            .iter()
            .any(|e| url.to_lowercase().ends_with(&format!(".{e}")));
    let imported = if url.starts_with("local://") {
        // 原文件：storage/data/{ns}/opds_files/{id}.{ext}
        let id = url.trim_start_matches("local://").split('/').next().unwrap_or("").to_string();
        let dir = state
            .storage
            .config
            .storage_dir()
            .join("data")
            .join(&namespace)
            .join("opds_files");
        let mut found = None;
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                let stem = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let ext_ok = crate::service::local_book::SUPPORTED_EXTENSIONS.iter().any(|ext| {
                    p.to_string_lossy().to_lowercase().ends_with(&format!(".{ext}"))
                });
                if stem == id && ext_ok {
                    found = Some(p);
                    break;
                }
            }
        }
        match found {
            Some(path) => match crate::service::local_book::parse_loc_book_path(&path, &user_rules) {
                Ok(b) => b,
                Err(e) => return Json(ReturnData::err(format!("解析失败：{e}"))),
            },
            None => return Json(ReturnData::err("本地书原文件不存在")),
        }
    } else if is_file {
        let path = match resolve_export_file_path(&state.storage.config.storage_dir(), &url) {
            Some(p) => p,
            None => return Json(ReturnData::err("本地书文件不存在")),
        };
        match crate::service::local_book::parse_loc_book_path(&path, &user_rules) {
            Ok(b) => b,
            Err(e) => return Json(ReturnData::err(format!("解析失败：{e}"))),
        }
    } else {
        return Json(ReturnData::err("仅支持本地书刷新"));
    };
    if imported.chapters.is_empty() {
        return Json(ReturnData::err("未解析到章节内容"));
    }
    let pairs: Vec<(String, String)> = imported
        .chapters
        .iter()
        .map(|c| (c.title.clone(), c.content.clone()))
        .collect();
    if let Err(e) = state.storage.save_chapters(&url, &pairs).await {
        tracing::error!("refreshLocalBook 章节入库失败: {e}");
        return Json(ReturnData::err("刷新失败"));
    }
    // 更新 total_chapter_num（书名缺失时用解析出的标题补）
    let mut patch = serde_json::Map::new();
    patch.insert("totalChapterNum".to_string(), json!(pairs.len() as i64));
    if book.name.is_empty() && !imported.meta.title.is_empty() {
        patch.insert("name".to_string(), json!(imported.meta.title));
    }
    let _ = state.storage.patch_book(&namespace, &url, &patch).await;
    tracing::info!("refreshLocalBook [{namespace}] {url}: {} 章", pairs.len());
    Json(ReturnData::ok(json!({
        "bookUrl": url,
        "name": book.name,
        "chapterCount": pairs.len(),
    })))
}

/// POST /reader3/deleteBooks：批量删除（body：{bookUrls:[]}）
async fn delete_books(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let _ = params;
    let Some(body) = body else {
        return Json(ReturnData::err("参数错误"));
    };
    let json: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return Json(ReturnData::err("参数错误")),
    };
    let urls: Vec<String> = json
        .get("bookUrls")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if urls.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    match state.storage.delete_books(&namespace, &urls).await {
        Ok(count) => Json(ReturnData::ok(json!({ "count": count }))),
        Err(e) => {
            tracing::error!("deleteBooks 失败: {e}");
            Json(ReturnData::err("删除失败"))
        }
    }
}

/// POST /reader3/deleteBookmarks：批量删书签（body：{bookUrl, ids:[]}——ids 为书签标题）
async fn delete_bookmarks(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let _ = params;
    let Some(body) = body else {
        return Json(ReturnData::err("参数错误"));
    };
    let json: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return Json(ReturnData::err("参数错误")),
    };
    let book_url = json.get("bookUrl").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let ids: Vec<String> = json
        .get("ids")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if book_url.is_empty() || ids.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    match state.storage.delete_bookmarks(&namespace, &book_url, &ids).await {
        Ok(count) => Json(ReturnData::ok(json!({ "count": count }))),
        Err(e) => {
            tracing::error!("deleteBookmarks 失败: {e}");
            Json(ReturnData::err("删除失败"))
        }
    }
}

/// POST /reader3/saveRssSources：批量保存 RSS 源（body = 数组）
async fn save_rss_sources(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let _ = params;
    let Some(body) = body else {
        return Json(ReturnData::err("参数错误"));
    };
    let mut sources: Vec<crate::model::RssSource> = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return Json(ReturnData::err("参数错误")),
    };
    if sources.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    for s in &mut sources {
        if s.source_url.trim().is_empty() || s.source_name.trim().is_empty() {
            return Json(ReturnData::err("参数错误"));
        }
        s.user_namespace = namespace.clone();
    }
    match state.storage.save_rss_sources(&namespace, &sources).await {
        Ok(_) => Json(ReturnData::ok(json!({ "count": sources.len() }))),
        Err(e) => {
            tracing::error!("saveRssSources 失败: {e}");
            Json(ReturnData::err("保存失败"))
        }
    }
}

/// POST /reader3/saveBookmarks：批量保存书签（body = 数组）
async fn save_bookmarks(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let _ = params;
    let Some(body) = body else {
        return Json(ReturnData::err("参数错误"));
    };
    let mut bookmarks: Vec<crate::model::Bookmark> = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return Json(ReturnData::err("参数错误")),
    };
    if bookmarks.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    for b in &mut bookmarks {
        if b.book_url.trim().is_empty() || b.title.trim().is_empty() {
            return Json(ReturnData::err("参数错误"));
        }
        b.user_namespace = namespace.clone();
        if b.created_at == 0 {
            b.created_at = now_millis();
        }
    }
    match state.storage.save_bookmarks(&namespace, &bookmarks).await {
        Ok(_) => Json(ReturnData::ok(json!({ "count": bookmarks.len() }))),
        Err(e) => {
            tracing::error!("saveBookmarks 失败: {e}");
            Json(ReturnData::err("保存失败"))
        }
    }
}

/// POST /reader3/addBookGroupMulti：批量设分组（body：{bookUrls, groupId}）
async fn add_book_group_multi(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let _ = params;
    let Some(body) = body else {
        return Json(ReturnData::err("参数错误"));
    };
    let json: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return Json(ReturnData::err("参数错误")),
    };
    let urls: Vec<String> = json
        .get("bookUrls")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let group_id = json.get("groupId").and_then(|v| v.as_i64()).unwrap_or(-1);
    if urls.is_empty() || group_id < 0 {
        return Json(ReturnData::err("参数错误"));
    }
    match state.storage.add_book_group_multi(&namespace, &urls, group_id).await {
        Ok(count) => Json(ReturnData::ok(json!({ "count": count }))),
        Err(e) => {
            tracing::error!("addBookGroupMulti 失败: {e}");
            Json(ReturnData::err("保存失败"))
        }
    }
}

/// POST /reader3/removeBookGroupMulti：批量移出分组（body：{bookUrls}）
async fn remove_book_group_multi(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let _ = params;
    let Some(body) = body else {
        return Json(ReturnData::err("参数错误"));
    };
    let json: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return Json(ReturnData::err("参数错误")),
    };
    let urls: Vec<String> = json
        .get("bookUrls")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    if urls.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    match state.storage.remove_book_group_multi(&namespace, &urls).await {
        Ok(count) => Json(ReturnData::ok(json!({ "count": count }))),
        Err(e) => {
            tracing::error!("removeBookGroupMulti 失败: {e}");
            Json(ReturnData::err("保存失败"))
        }
    }
}

/// POST /reader3/saveBookGroupOrder：分组排序（body：{order:[{id,orderNum}]}）
async fn save_book_group_order(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let _ = params;
    let Some(body) = body else {
        return Json(ReturnData::err("参数错误"));
    };
    let json: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return Json(ReturnData::err("参数错误")),
    };
    let order: Vec<(i64, i64)> = json
        .get("order")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let id = item.get("id")?.as_i64()?;
                    let order_num = item
                        .get("orderNum")
                        .or_else(|| item.get("order"))
                        .and_then(|v| v.as_i64())?;
                    Some((id, order_num))
                })
                .collect()
        })
        .unwrap_or_default();
    if order.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    match state.storage.save_book_group_order(&namespace, &order).await {
        Ok(count) => Json(ReturnData::ok(json!({ "count": count }))),
        Err(e) => {
            tracing::error!("saveBookGroupOrder 失败: {e}");
            Json(ReturnData::err("保存失败"))
        }
    }
}

/// GET/POST /reader3/getAvailableBookSource：可用书源（key 要求可搜索；url 按 bookUrlPattern 规则过滤）
async fn get_available_book_source(
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
    let url = param_of(&params, body_json.as_ref(), "url");
    let group = param_of(&params, body_json.as_ref(), "bookSourceGroup");
    let sources = match state.storage.get_book_sources(&namespace).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("getAvailableBookSource [{namespace}] 失败: {e}");
            return Json(ReturnData::err("系统错误"));
        }
    };
    let mut out: Vec<serde_json::Value> = Vec::new();
    for s in sources {
        if !s.enabled {
            continue;
        }
        if !group.is_empty()
            && !s.book_source_group
                .as_deref()
                .map(|g| g.split(' ').any(|part| part == group))
                .unwrap_or(false)
        {
            continue;
        }
        if !key.is_empty() && s.search_url.is_none() {
            continue;
        }
        if !url.is_empty() {
            if let Some(pattern) = &s.book_url_pattern {
                if !pattern.is_empty() {
                    // 正则编译失败视为放行（书源可用性不受坏规则影响）
                    let matched =
                        regex::Regex::new(pattern).map(|re| re.is_match(&url)).unwrap_or(true);
                    if !matched {
                        continue;
                    }
                }
            }
        }
        out.push(serde_json::to_value(s).unwrap_or(serde_json::Value::Null));
    }
    Json(ReturnData::ok(serde_json::Value::Array(out)))
}

/// GET/POST /reader3/getInvalidBookSources：并发 HEAD/首页检测失效书源（轻量超时 8s）
async fn get_invalid_book_sources(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let _ = (params, body);
    let sources = match state.storage.get_book_sources(&namespace).await {
        Ok(s) => s.into_iter().filter(|s| s.enabled).collect::<Vec<_>>(),
        Err(e) => {
            tracing::error!("getInvalidBookSources [{namespace}] 失败: {e}");
            return Json(ReturnData::err("系统错误"));
        }
    };
    let invalid = crate::service::health::find_invalid(&namespace, &sources).await;
    let arr: Vec<serde_json::Value> = invalid
        .into_iter()
        .map(|(s, reason)| {
            json!({
                "bookSourceUrl": s.book_source_url,
                "bookSourceName": s.book_source_name,
                "error": reason,
            })
        })
        .collect();
    Json(ReturnData::ok(serde_json::Value::Array(arr)))
}

/// POST /reader3/setAsDefaultBookSources：默认书源标记（body：{bookSources:[url...] 或 [对象...]}）
async fn set_as_default_book_sources(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let _ = params;
    let Some(body) = body else {
        return Json(ReturnData::err("参数错误"));
    };
    let json: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return Json(ReturnData::err("参数错误")),
    };
    let urls: Vec<String> = json
        .get("bookSources")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .or_else(|| item.get("bookSourceUrl").and_then(|v| v.as_str()).map(str::to_string))
                })
                .collect()
        })
        .unwrap_or_default();
    if urls.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    match state.storage.set_default_book_sources(&namespace, &urls).await {
        Ok(_) => Json(ReturnData::ok(json!({ "count": urls.len() }))),
        Err(e) => {
            tracing::error!("setAsDefaultBookSources 失败: {e}");
            Json(ReturnData::err("保存失败"))
        }
    }
}

/// GET/POST /reader3/searchBookSourceSSE：流式换源结果（逐书源事件 + end）
async fn search_book_source_sse(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Response {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return sse_error(ret),
    };
    let body_json = body.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let url = param_of(&params, body_json.as_ref(), "url");
    let book_source_param = param_of(&params, body_json.as_ref(), "bookSource");
    if url.is_empty() {
        return sse_error(ReturnData::err("请输入书籍链接"));
    }
    if book_source_param.is_empty() {
        return sse_error(ReturnData::err("未配置书源"));
    }

    // ① 当前书名（书架优先；未入架走详情解析）
    let name = match state.storage.find_book(&namespace, &url).await {
        Ok(Some(b)) => b.name,
        _ => {
            let Some(source) = resolve_book_source(&state, &namespace, &book_source_param).await
            else {
                return sse_error(ReturnData::err("书源不存在"));
            };
            match crate::service::book::fetch_url(&namespace, &url, &source).await {
                Ok(resp) => {
                    let info = crate::service::book::analyze_book_info(
                        &resp.body,
                        &resp.url,
                        &source,
                        &url,
                    );
                    if info.name.is_empty() {
                        return sse_error(ReturnData::err("获取书籍信息失败"));
                    }
                    info.name
                }
                Err(e) => {
                    tracing::error!("searchBookSourceSSE 获取书名失败 [{url}]: {e}");
                    return sse_error(ReturnData::err("获取书籍信息失败"));
                }
            }
        }
    };
    let key = name.trim().to_string();
    if key.is_empty() {
        return sse_error(ReturnData::err("无法获取书名"));
    }

    // ② 全部启用可搜索书源（排除当前源）
    let current = book_source_param.trim();
    let sources: Vec<crate::model::BookSource> = match state.storage.get_book_sources(&namespace).await
    {
        Ok(s) => s
            .into_iter()
            .filter(|s| {
                s.enabled
                    && s.search_url.is_some()
                    && s.book_source_url != current
                    && s.book_source_name != current
            })
            .collect(),
        Err(_) => return sse_error(ReturnData::err("系统错误")),
    };

    let (tx, rx) =
        tokio::sync::mpsc::unbounded_channel::<Result<Bytes, std::convert::Infallible>>();
    let ns = namespace.clone();
    tokio::spawn(async move {
        if sources.is_empty() {
            let payload = json!({ "lastIndex": -1, "isEnd": true });
            let _ = tx.send(Ok(Bytes::from(format!("event: end\ndata: {payload}\n\n"))));
            return;
        }
        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
        let mut tasks = futures::stream::FuturesUnordered::new();
        for (i, source) in sources.into_iter().enumerate() {
            let sem = semaphore.clone();
            let key = key.clone();
            let ns = ns.clone();
            tasks.push(Box::pin(async move {
                let _permit = sem.acquire().await;
                let books = crate::service::search::search_one_source(&ns, &source, &key, 1)
                    .await
                    .unwrap_or_default();
                (i as i64, books)
            }));
        }
        // 汇总：书名匹配过滤 + 按书源去重，逐源推送
        let mut last = -1i64;
        let mut seen = std::collections::HashSet::new();
        let ql = key.to_lowercase();
        while let Some((i, books)) = tasks.next().await {
            last = i;
            let matched: Vec<_> = books
                .into_iter()
                .filter(|b| {
                    let bl = b.name.to_lowercase();
                    bl.contains(&ql) || ql.contains(&bl)
                })
                .filter(|b| seen.insert(b.origin.clone()))
                .collect();
            let payload = json!({ "lastIndex": i, "data": matched });
            if tx
                .send(Ok(Bytes::from(format!("event: book\ndata: {payload}\n\n"))))
                .is_err()
            {
                return; // 客户端断开
            }
        }
        let end_payload = json!({ "lastIndex": last, "isEnd": true });
        let _ = tx.send(Ok(Bytes::from(format!("event: end\ndata: {end_payload}\n\n"))));
    });

    let stream = futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream; charset=utf-8")
        .header("Cache-Control", "no-cache")
        .body(Body::from_stream(stream))
        .unwrap()
}

/// GET/POST /reader3/getReadingStats：阅读统计（today/week/total 秒数 + 单书 books[]）
async fn get_reading_stats(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    let _ = (params, body);
    match state.storage.get_reading_stats(&namespace).await {
        Ok(stats) => Json(ReturnData::ok(
            serde_json::to_value(stats).unwrap_or(serde_json::Value::Null),
        )),
        Err(e) => {
            tracing::error!("getReadingStats [{namespace}] 失败: {e}");
            Json(ReturnData::err("系统错误"))
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

/// 命名空间解析（OPDS 认证）：非 secure 模式一律 default；secure 模式支持：
/// ① Basic——独立 OPDS 账号优先（system_settings opds_username/opds_password，sha256+salt），
///    未配置或校验失败回退系统用户账号（users 表，legacy 双 md5 校验）；
/// ② accessToken（query/header，username:token，与 /reader3 一致）。
async fn opds_ns(
    state: &AppState,
    headers: &HeaderMap,
    params: &HashMap<String, String>,
) -> Result<String, Response> {
    if !state.storage.config.secure {
        return Ok("default".to_string());
    }
    // ① Basic 认证
    if let Some(creds) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
    {
        let decoded = String::from_utf8(opds_base64_decode(creds)).unwrap_or_default();
        if let Some((username, password)) = decoded.split_once(':') {
            // 独立 OPDS 账号（配置后优先）
            if let Ok(Some((opds_user, stored))) = state.storage.get_opds_account().await {
                if username == opds_user && crate::util::sha256::verify_password(password, &stored) {
                    return Ok(opds_user);
                }
            }
            // 系统用户账号（users 表；密码为 legacy 双 md5 哈希存储，复用 gen_encrypted_password 校验）
            if let Ok(Some(user)) = state.storage.find_user(username).await {
                let expect = gen_encrypted_password(password, &user.salt);
                if expect == user.password {
                    return Ok(user.username);
                }
            }
        }
    }
    // ② accessToken（query/header，username:token，与 /reader3 一致）
    match resolve_namespace(state, params, headers).await {
        Ok(ns) => Ok(ns),
        Err(_) => Err(opds_unauthorized()),
    }
}

/// OPDS 401 响应（WWW-Authenticate: Basic）
fn opds_unauthorized() -> Response {
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header("WWW-Authenticate", "Basic realm=\"reader\"")
        .body(Body::empty())
        .unwrap()
}

/// Basic 凭证解码（标准 base64）
fn opds_base64_decode(s: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .unwrap_or_default()
}

/// OPDS 统一分发（/opds/*rest）：OPDS 1.2 目录/搜索/获取/下载/保存 + OPDS 2.0 JSON
async fn opds_dispatch(
    State(state): State<AppState>,
    path: Option<axum::extract::Path<String>>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let rest = path.as_deref().map(String::as_str).unwrap_or("");
    let ns = match opds_ns(&state, &headers, &params).await {
        Ok(ns) => ns,
        Err(resp) => return resp,
    };
    let (start, max) = crate::api::opds::parse_page(&params);
    let segs: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();

    // OPDS 1.2（Atom）
    let atom = "application/atom+xml;profile=opds-catalog;charset=utf-8";
    // OPDS 2.0（JSON）
    let opds2 = "application/opds+json";

    let make = |r: Result<String, anyhow::Error>, ct: &str| -> Response {
        match r {
            Ok(body) => Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", ct)
                .body(Body::from(body))
                .unwrap(),
            Err(e) => {
                tracing::error!("OPDS 请求失败 [/opds/{rest}]: {e}");
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Body::empty())
                    .unwrap()
            }
        }
    };

    let resp = match segs.as_slice() {
        // ---------------- OPDS 1.2 ----------------
        [] => make(
            crate::api::opds::root(&state.storage, &ns).await,
            "application/atom+xml;profile=opds-catalog;kind=navigation;charset=utf-8",
        ),
        ["opensearch.xml"] => make(
            Ok(crate::api::opds::open_search_xml()),
            "application/opensearchdescription+xml;charset=utf-8",
        ),
        ["shelf"] => make(crate::api::opds::shelf(&state.storage, &ns, start, max).await, atom),
        ["recent"] => make(crate::api::opds::recent(&state.storage, &ns, start, max).await, atom),
        ["local"] => make(crate::api::opds::local(&state.storage, &ns, start, max).await, atom),
        ["groups"] => make(crate::api::opds::groups(&state.storage, &ns).await, atom),
        ["group", id] => match id.parse::<i64>() {
            Ok(gid) => make(crate::api::opds::group(&state.storage, &ns, gid, start, max).await, atom),
            Err(_) => opds_404(),
        },
        ["source"] => make(crate::api::opds::sources(&state.storage, &ns).await, atom),
        ["source", name] => make(
            crate::api::opds::source(&state.storage, &ns, name, start, max).await,
            atom,
        ),
        ["search"] => {
            let q = params.get("q").cloned().unwrap_or_default();
            make(crate::api::opds::search(&state.storage, &ns, &q, start, max).await, atom)
        }
        // 获取/下载
        ["acquire", id] => {
            match crate::api::opds::acquire(&state.storage, &ns, id).await {
                Ok((name, bytes)) => Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "text/plain; charset=utf-8")
                    .header("Content-Disposition", format!("inline; filename=\"{}\"", name))
                    .body(Body::from(bytes))
                    .unwrap(),
                Err(e) => {
                    tracing::warn!("OPDS 正文获取失败: {e}");
                    opds_404()
                }
            }
        }
        ["download", id] => {
            let format = params.get("format").cloned().unwrap_or_else(|| "txt".to_string());
            let max_chapters = params.get("maxChapters").and_then(|v| v.parse::<usize>().ok());
            match crate::api::opds::download(&state.storage, &ns, id, &format, max_chapters).await {
                Ok((name, bytes, ct)) => Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", ct)
                    .header("Content-Disposition", format!("attachment; filename=\"{}\"", name))
                    .body(Body::from(bytes))
                    .unwrap(),
                Err(e) => {
                    tracing::warn!("OPDS 下载失败: {e}");
                    opds_404()
                }
            }
        }
        // OPDS-PSE：GET 进度 entry
        ["save", id] => {
            let want_json = params.get("format").map(|v| v == "json").unwrap_or(false)
                || headers
                    .get(axum::http::header::ACCEPT)
                    .and_then(|v| v.to_str().ok())
                    .map(|v| v.contains("application/json") && !v.contains("atom"))
                    .unwrap_or(false);
            if want_json {
                match crate::api::opds::save_entry_json(&state.storage, &ns, id).await {
                    Ok(v) => Response::builder()
                        .status(StatusCode::OK)
                        .header("Content-Type", "application/json; charset=utf-8")
                        .body(Body::from(v.to_string()))
                        .unwrap(),
                    Err(_) => opds_404(),
                }
            } else {
                match crate::api::opds::save_entry_xml(&state.storage, &ns, id).await {
                    Ok(xml) => Response::builder()
                        .status(StatusCode::OK)
                        .header("Content-Type", "application/atom+xml;type=entry;charset=utf-8")
                        .body(Body::from(xml))
                        .unwrap(),
                    Err(_) => opds_404(),
                }
            }
        }
        // ---------------- OPDS 2.0 ----------------
        ["catalog"] => make(crate::api::opds::catalog_json(&state.storage, &ns).await, opds2),
        ["catalog", "shelf"] => make(
            crate::api::opds::shelf_json(&state.storage, &ns, start, max).await,
            opds2,
        ),
        ["catalog", "recent"] => make(
            crate::api::opds::recent_json(&state.storage, &ns, start, max).await,
            opds2,
        ),
        ["catalog", "local"] => make(
            crate::api::opds::local_json(&state.storage, &ns, start, max).await,
            opds2,
        ),
        ["catalog", "groups"] => make(crate::api::opds::groups_json(&state.storage, &ns).await, opds2),
        ["catalog", "group", id] => match id.parse::<i64>() {
            Ok(gid) => make(
                crate::api::opds::group_json(&state.storage, &ns, gid, start, max).await,
                opds2,
            ),
            Err(_) => opds_404(),
        },
        ["catalog", "source"] => make(crate::api::opds::sources_json(&state.storage, &ns).await, opds2),
        ["catalog", "source", name] => make(
            crate::api::opds::source_json(&state.storage, &ns, name, start, max).await,
            opds2,
        ),
        ["catalog", "search"] => {
            let q = params.get("q").cloned().unwrap_or_default();
            make(crate::api::opds::search_json(&state.storage, &ns, &q, start, max).await, opds2)
        }
        _ => opds_404(),
    };
    resp
}

/// OPDS 404
fn opds_404() -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::empty())
        .unwrap()
}

/// POST /opds/save/{bookId}：OPDS-PSE 保存进度（body/query：progress/position/total/chapterIndex/chapterTitle/timestamp）
async fn opds_save_post(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<axum::body::Bytes>,
) -> Response {
    let ns = match opds_ns(&state, &headers, &params).await {
        Ok(ns) => ns,
        Err(resp) => return resp,
    };
    let body_json = body.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let id = param_of(&params, body_json.as_ref(), "bookId");
    let f64_of = |keys: &[&str]| -> Option<f64> {
        for k in keys {
            if let Some(v) = params.get(*k).and_then(|v| v.parse::<f64>().ok()) {
                return Some(v);
            }
            if let Some(b) = body_json.as_ref() {
                if let Some(v) = b.get(*k).and_then(|v| v.as_f64()) {
                    return Some(v);
                }
            }
        }
        None
    };
    let i64_of = |keys: &[&str]| -> Option<i64> {
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
    let str_of = |keys: &[&str]| -> Option<String> {
        for k in keys {
            if let Some(v) = params.get(*k) {
                return Some(v.clone());
            }
            if let Some(b) = body_json.as_ref() {
                if let Some(v) = b.get(*k).and_then(|v| v.as_str()) {
                    return Some(v.to_string());
                }
            }
        }
        None
    };
    let chapter_title = str_of(&["chapterTitle", "durChapterTitle"]);
    match crate::api::opds::apply_save(
        &state.storage,
        &ns,
        &id,
        f64_of(&["progress"]),
        i64_of(&["position", "durChapterPos"]),
        i64_of(&["total"]),
        i64_of(&["chapterIndex", "durChapterIndex"]),
        chapter_title,
        i64_of(&["timestamp", "durChapterTime"]),
    )
    .await
    {
        Ok(v) => Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json; charset=utf-8")
            .body(Body::from(v.to_string()))
            .unwrap(),
        Err(e) => {
            tracing::warn!("OPDS-PSE 保存失败: {e}");
            opds_404()
        }
    }
}

/// GET /reader3/getOpdsSettings：OPDS 独立账号配置（enabled/username/passwordSet；不回传密码）
async fn get_opds_settings(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    match state.storage.get_opds_account().await {
        Ok(Some((username, _))) => Json(ReturnData::ok(json!({
            "enabled": true,
            "username": username,
            "passwordSet": true,
            "namespace": namespace,
        }))),
        Ok(None) => Json(ReturnData::ok(json!({
            "enabled": false,
            "username": "",
            "passwordSet": false,
            "namespace": namespace,
        }))),
        Err(e) => {
            tracing::error!("getOpdsSettings 失败: {e}");
            Json(ReturnData::err("系统错误"))
        }
    }
}

/// POST /reader3/saveOpdsSettings：配置 OPDS 独立账号（body {username, password}；username 空 = 禁用）
async fn save_opds_settings(
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
    let username = body_json
        .as_ref()
        .and_then(|b| b.get("username").and_then(|v| v.as_str()))
        .unwrap_or("")
        .trim()
        .to_string();
    let password = body_json
        .as_ref()
        .and_then(|b| b.get("password").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();
    if username.is_empty() {
        // 禁用独立账号（回退系统账号/token）
        match state.storage.clear_opds_account().await {
            Ok(_) => Json(ReturnData::ok(json!({"enabled": false}))),
            Err(e) => {
                tracing::error!("saveOpdsSettings(禁用) 失败: {e}");
                Json(ReturnData::err("系统错误"))
            }
        }
    } else if password.len() < 4 {
        Json(ReturnData::err("密码至少 4 位"))
    } else {
        let stored = crate::util::sha256::store_password(&password);
        match state.storage.set_opds_account(&username, &stored).await {
            Ok(_) => Json(ReturnData::ok(json!({
                "enabled": true,
                "username": username,
            }))),
            Err(e) => {
                tracing::error!("saveOpdsSettings 失败: {e}");
                Json(ReturnData::err("系统错误"))
            }
        }
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
    // 阅读统计：先取旧进度（增量时长/字数），再更新
    let old = match state.storage.find_book(&namespace, &book_url).await {
        Ok(Some(b)) => Some((b.dur_chapter_time, b.dur_chapter_pos)),
        _ => None,
    };
    match state
        .storage
        .update_book_progress(&namespace, &book_url, title.as_deref(), index, pos, time)
        .await
    {
        Ok(0) => Json(ReturnData::err("书籍未加入书架")),
        Ok(_) => {
            // 增量累计阅读时长/字数到 reading_stats（今日行）
            if let Some((old_time, old_pos)) = old {
                let delta_seconds = if old_time > 0 && time > old_time {
                    (time - old_time) / 1000
                } else {
                    0
                };
                let delta_chars = if pos > old_pos { pos - old_pos } else { 0 };
                if delta_seconds > 0 || delta_chars > 0 {
                    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
                    if let Err(e) = state
                        .storage
                        .record_reading_stats(&namespace, &book_url, &date, delta_seconds, delta_chars)
                        .await
                    {
                        tracing::warn!("记录阅读统计失败 [{book_url}]: {e}");
                    }
                }
            }
            Json(ReturnData::ok(serde_json::Value::Null))
        }
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
    match crate::service::explore::explore_url(&namespace, &target, &source).await {
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
    let ns = namespace.clone();
    tokio::spawn(async move {
        // 并发受控（semaphore），结果到达即推送（FuturesUnordered 完成顺序）
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrent_count));
        let mut tasks = futures::stream::FuturesUnordered::new();
        for i in start..end {
            let sem = sem.clone();
            let key = key.clone();
            let ns = ns.clone();
            let source = sources[i].clone();
            tasks.push(Box::pin(async move {
                let _permit = sem.acquire().await;
                let books =
                    crate::service::search::search_one_source(&ns, &source, &key, 1).await.unwrap_or_default();
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

// ---------------- 小项补全批 ----------------

/// GET /reader3/deleteBookCache：删除单书缓存（book_chapters 该 book_url 行——
/// 本地书章节 + 书源书正文缓存）；不影响书架 books 行
async fn delete_book_cache(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<ReturnData> {
    let url = params.get("url").cloned().unwrap_or_default();
    if url.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    match state.storage.delete_book_cache(&url).await {
        Ok(deleted) => Json(ReturnData::ok(json!({ "deleted": deleted }))),
        Err(e) => {
            tracing::error!("deleteBookCache 失败 [{url}]: {e}");
            Json(ReturnData::err("删除失败"))
        }
    }
}

/// GET/POST /reader3/getShelfBookWithCacheInfo：书架书 + 缓存信息（缓存章数/正文大小）
async fn get_shelf_book_with_cache_info(
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
    let book = match state.storage.find_book(&namespace, &url).await {
        Ok(Some(b)) => b,
        Ok(None) => return Json(ReturnData::err("书籍不存在")),
        Err(e) => {
            tracing::error!("getShelfBookWithCacheInfo 失败 [{url}]: {e}");
            return Json(ReturnData::err("系统错误"));
        }
    };
    let (cache_chapter_count, cache_size) = state
        .storage
        .book_cache_info(&url)
        .await
        .unwrap_or((0, 0));
    let mut data = serde_json::to_value(book).unwrap_or(serde_json::Value::Null);
    if let Some(obj) = data.as_object_mut() {
        obj.insert("cacheChapterCount".to_string(), json!(cache_chapter_count));
        obj.insert("cacheSize".to_string(), json!(cache_size));
    }
    Json(ReturnData::ok(data))
}

/// POST /reader3/importBookPreview：导入预览（multipart file——解析但不入库）
/// 返回 {name, author, format, chapterCount, preview: [前 10 章标题]}
async fn import_book_preview(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Json<ReturnData> {
    let namespace = match resolve_namespace(&state, &params, &headers).await {
        Ok(ns) => ns,
        Err(ret) => return Json(ret),
    };
    // 取 file 字段（首块）
    let mut file_name = String::new();
    let mut bytes: Vec<u8> = Vec::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            file_name = field.file_name().unwrap_or("file").to_string();
            if let Ok(b) = field.bytes().await {
                bytes = b.to_vec();
            }
            break;
        }
    }
    if bytes.is_empty() {
        return Json(ReturnData::err("请上传文件"));
    }
    let safe_name = std::path::Path::new(&file_name)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = crate::service::local_book::file_ext(&safe_name);
    if ext.is_empty()
        || !crate::service::local_book::SUPPORTED_EXTENSIONS
            .iter()
            .any(|e| *e == ext)
    {
        return Json(ReturnData::err("不支持的格式"));
    }
    // 解析（parse_loc_book_path 按扩展名分派；核心逻辑在可测的纯函数中）
    let user_rules = txt_toc_rule_regexes(&state, &namespace).await;
    match import_preview_from_bytes(&bytes, &ext, &user_rules) {
        Ok(json) => Json(ReturnData::ok(json)),
        Err(e) => Json(ReturnData::err(format!("解析失败：{e}"))),
    }
}

/// 导入预览核心（纯函数，可测）：字节 → 临时文件 → parse_loc_book_path 解析
/// （复用本地书解析链路）→ {name, author, format, chapterCount, preview: [前 10 章标题]}；不入库
fn import_preview_from_bytes(
    bytes: &[u8],
    ext: &str,
    user_rules: &[String],
) -> anyhow::Result<serde_json::Value> {
    let tmp_path =
        std::env::temp_dir().join(format!("reader-preview-{}.{ext}", uuid::Uuid::new_v4()));
    std::fs::write(&tmp_path, bytes)?;
    let result = (|| -> anyhow::Result<serde_json::Value> {
        let imported = crate::service::local_book::parse_loc_book_path(&tmp_path, user_rules)?;
        let preview: Vec<String> = imported
            .chapters
            .iter()
            .take(10)
            .map(|c| c.title.clone())
            .collect();
        Ok(json!({
            "name": imported.meta.title,
            "author": imported.meta.author,
            "format": imported.format,
            "chapterCount": imported.chapters.len(),
            "preview": preview,
        }))
    })();
    let _ = std::fs::remove_file(&tmp_path);
    result
}

/// POST /reader3/readSourceFile：读取书源文件文本（body {path}）
/// secure 模式限 storage 目录内（resolve_storage_path 防穿越）；非 secure 限工作目录内
async fn read_source_file(
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
    let path = param_of(&params, body_json.as_ref(), "path");
    if path.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    let file = if state.storage.config.secure {
        resolve_storage_path(&state.storage.config.storage_dir(), &path)
    } else {
        let base = std::path::PathBuf::from(&state.storage.config.work_dir);
        crate::api::files::resolve_secure_path(&base, &path)
    };
    let Some(file) = file else {
        return Json(ReturnData::err("路径不存在"));
    };
    if !file.is_file() {
        return Json(ReturnData::err("路径不存在"));
    }
    match tokio::fs::read_to_string(&file).await {
        Ok(content) => Json(ReturnData::ok(serde_json::Value::String(content))),
        Err(e) => {
            tracing::error!("readSourceFile 读取失败 [{}]: {e}", file.display());
            Json(ReturnData::err("读取失败"))
        }
    }
}

/// POST /reader3/saveBookContent：写章节正文缓存（body {bookUrl, chapterUrl, title, content}）
/// chapter_index = chapterUrl md5 哈希（与 getBookContent 正文缓存同键）
async fn save_book_content(
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
    let book_url = param_of(&params, body_json.as_ref(), "bookUrl");
    let chapter_url = param_of(&params, body_json.as_ref(), "chapterUrl");
    let title = param_of(&params, body_json.as_ref(), "title");
    let content = param_of(&params, body_json.as_ref(), "content");
    if book_url.is_empty() || chapter_url.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    if content.is_empty() {
        return Json(ReturnData::err("正文不能为空"));
    }
    let idx = crate::util::md5::chapter_url_hash(&chapter_url);
    match state
        .storage
        .cache_chapter_content(&book_url, idx, &title, &content)
        .await
    {
        Ok(_) => Json(ReturnData::ok(serde_json::Value::Null)),
        Err(e) => {
            tracing::error!("saveBookContent 失败 [{book_url}]: {e}");
            Json(ReturnData::err("保存失败"))
        }
    }
}

/// POST /reader3/deleteUserBookSource：删除当前用户书源（body {bookSource}；
/// 兼容 bookSourceUrl/url 参数名）
async fn delete_user_book_source(
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
    let mut url = param_of(&params, body_json.as_ref(), "bookSource");
    if url.is_empty() {
        url = param_of(&params, body_json.as_ref(), "bookSourceUrl");
    }
    if url.is_empty() {
        url = param_of(&params, body_json.as_ref(), "url");
    }
    if url.is_empty() {
        return Json(ReturnData::err("参数错误"));
    }
    match state.storage.delete_book_source(&namespace, &url).await {
        Ok(_) => Json(ReturnData::ok(serde_json::Value::Null)),
        Err(e) => {
            tracing::error!("deleteUserBookSource 失败 [{url}]: {e}");
            Json(ReturnData::err("删除失败"))
        }
    }
}

/// POST /reader3/saveBookGroupId：设置书分组（body {bookUrl, groupId}）——
/// updateBookGroupId 别名（参数名兼容 group/groupId）
async fn save_book_group_id(
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
        .get("groupId")
        .and_then(|v| v.parse::<i64>().ok())
        .or_else(|| {
            body_json
                .as_ref()
                .and_then(|b| b.get("groupId").and_then(|v| v.as_i64()))
        })
        .or_else(|| {
            params
                .get("group")
                .and_then(|v| v.parse::<i64>().ok())
                .or_else(|| {
                    body_json
                        .as_ref()
                        .and_then(|b| b.get("group").and_then(|v| v.as_i64()))
                })
        })
        .unwrap_or(-1);
    if book_url.is_empty() || group < 0 {
        return Json(ReturnData::err("参数错误"));
    }
    match state
        .storage
        .update_book_group_id(&namespace, &book_url, group)
        .await
    {
        Ok(_) => Json(ReturnData::ok(serde_json::Value::Null)),
        Err(e) => {
            tracing::error!("saveBookGroupId 失败: {e}");
            Json(ReturnData::err("保存失败"))
        }
    }
}

/// GET/POST /reader3/getChapterListByRule：书源 ruleToc 单页解析调试
/// 参数：url（目录页，缺省用 chapterUrl）+ bookSource（书源 URL 或完整 JSON）
/// 返回章节数组（同 getBookToc 结构：title/url/isVolume/index）
async fn get_chapter_list_by_rule(
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
        url = param_of(&params, body_json.as_ref(), "chapterUrl");
    }
    if url.is_empty() {
        return Json(ReturnData::err("请输入目录链接"));
    }
    let bs_param = param_of(&params, body_json.as_ref(), "bookSource");
    if bs_param.is_empty() {
        // find_book_source 对空串走 LIKE '%' 会命中首源——显式拦截
        return Json(ReturnData::err("书源不存在"));
    }
    let Some(source) = resolve_book_source(&state, &namespace, &bs_param).await else {
        return Json(ReturnData::err("书源不存在"));
    };
    match crate::service::book::parse_toc_page(&namespace, &url, &source).await {
        Ok(chapters) => Json(ReturnData::ok(
            serde_json::to_value(chapters).unwrap_or(serde_json::Value::Null),
        )),
        Err(e) => {
            tracing::error!("getChapterListByRule 失败 [{url}]: {e}");
            Json(ReturnData::err("获取目录失败"))
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

    let ext = crate::service::local_book::file_ext(&file_name);
    if !crate::service::local_book::SUPPORTED_EXTENSIONS.contains(&ext.as_str()) {
        return Json(ReturnData::err("仅支持 EPUB/TXT/MOBI/AZW3/PDF/FB2/DOCX"));
    }
    // 用户自定义 TXT 目录规则（启用 + 按 serialNumber 排序）；无则用内置默认规则（仅 TXT 使用）
    let user_rules = txt_toc_rule_regexes(&state, &namespace).await;
    let imported = if ext == "txt" {
        // TXT 解析失败保持静默回退（与旧行为一致：空书 → “未解析到章节内容”）
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
        match crate::service::local_book::parse_file_bytes(&bytes, &ext, &user_rules) {
            Ok(b) => b,
            Err(e) => return Json(ReturnData::err(format!("{} 解析失败：{e}", ext.to_uppercase()))),
        }
    };

    if imported.chapters.is_empty() {
        return Json(ReturnData::err("未解析到章节内容"));
    }

    let book_url = format!("local://{}", uuid::Uuid::new_v4());
    let book = crate::model::book_chapter::BookInfo {
        name: if imported.meta.title.is_empty() {
            std::path::Path::new(&file_name)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| file_name.clone())
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

    // OPDS 原文件下载：原始文件落盘 data/{ns}/opds_files/{uuid}.{ext}（供 /opds/download 直下）
    let opds_dir = state
        .storage
        .config
        .storage_dir()
        .join("data")
        .join(&namespace)
        .join("opds_files");
    if let Err(e) = std::fs::create_dir_all(&opds_dir) {
        tracing::warn!("OPDS 原文件目录创建失败: {e}");
    } else {
        let file_id = book.book_url.trim_start_matches("local://");
        if let Err(e) = std::fs::write(opds_dir.join(format!("{file_id}.{ext}")), &bytes) {
            tracing::warn!("OPDS 原文件落盘失败: {e}");
        }
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

/// chapterUrl 是否文件型本地书章节（bookPart 是 storage/ 路径或白名单扩展名文件）
fn is_loc_book_file_chapter(chapter_url: &str) -> bool {
    let Some((book_part, _)) = chapter_url.rsplit_once('#') else {
        return false;
    };
    if book_part.starts_with("storage/") {
        return true;
    }
    let lower = book_part.to_lowercase();
    crate::service::local_book::SUPPORTED_EXTENSIONS
        .iter()
        .any(|e| lower.ends_with(&format!(".{e}")))
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
    // 按扩展名分派（复用 resolve_loc_book_file 定位结果；TXT 用默认规则分章）
    let imported = match crate::service::local_book::parse_loc_book_path(&path, &[]) {
        Ok(b) => b,
        Err(e) => {
            tracing::debug!("loc_book toc: 解析失败 [{path_lower}] {e}");
            return None;
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

/// 文件型本地书目录：按扩展名解析（TXT 用用户规则）→ 章节列表（chapterUrl = bookUrl#index）
async fn get_book_toc_file(state: &AppState, ns: &str, book_url: &str) -> Option<Json<ReturnData>> {
    // 优先严格路径（防穿越），失败回退 legacy 目录式 index.epub 定位
    let path = resolve_storage_path(&state.storage.config.storage_dir(), book_url)
        .or_else(|| resolve_loc_book_file(&state.storage.config.storage_dir(), book_url))?;
    let user_rules = txt_toc_rule_regexes(state, ns).await;
    let imported = crate::service::local_book::parse_loc_book_path(&path, &user_rules).ok()?;
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

/// 文件型本地书正文：bookUrl#index → 定位文件（白名单扩展名）→ 提取章节
async fn get_book_content_file(state: &AppState, ns: &str, chapter_url: &str) -> Option<Json<ReturnData>> {
    let (book_part, idx_part) = chapter_url.rsplit_once('#')?;
    let index: usize = idx_part.parse().ok()?;
    let path = resolve_loc_book_file(&state.storage.config.storage_dir(), book_part)?;
    // 按扩展名分派（TXT 用用户规则，其余格式用各自解析器）
    let user_rules = txt_toc_rule_regexes(state, ns).await;
    let imported = crate::service::local_book::parse_loc_book_path(&path, &user_rules).ok()?;
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

    /// deleteBookCache：删单书缓存（book_chapters 行）——只删目标书、书架不受影响
    #[tokio::test]
    async fn test_delete_book_cache_api() {
        let (state, dir) = test_state("delbcache").await;
        state.storage.save_chapters("https://book.com/a", &[("第一章".to_string(), "正文A".to_string())]).await.unwrap();
        state.storage.save_chapters("https://book.com/b", &[("第一章".to_string(), "正文B".to_string())]).await.unwrap();
        state.storage.upsert_book("default", &crate::model::Book { book_url: "https://book.com/a".into(), name: "书A".into(), ..Default::default() }).await.unwrap();

        let params: HashMap<String, String> = [("url".into(), "https://book.com/a".into())].into_iter().collect();
        let ret = delete_book_cache(AxumState(state.clone()), Query(params)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert_eq!(ret.0.data["deleted"], 1);
        assert_eq!(state.storage.count_chapters("https://book.com/a").await.unwrap(), 0);
        assert_eq!(state.storage.count_chapters("https://book.com/b").await.unwrap(), 1, "其他书缓存不受影响");
        assert!(state.storage.find_book("default", "https://book.com/a").await.unwrap().is_some(), "删除缓存不应动书架");

        // 缺 url → 参数错误
        let ret = delete_book_cache(AxumState(state.clone()), Query(HashMap::new())).await;
        assert_eq!(ret.0.error_msg, "参数错误");
        cleanup(state, dir).await;
    }

    /// getShelfBookWithCacheInfo：书架书 + cacheChapterCount/cacheSize
    #[tokio::test]
    async fn test_get_shelf_book_with_cache_info_api() {
        let (state, dir) = test_state("shelfcache").await;
        state.storage.upsert_book("default", &crate::model::Book { book_url: "https://book.com/a".into(), name: "缓存书".into(), ..Default::default() }).await.unwrap();
        state.storage.save_chapters("https://book.com/a", &[
            ("第一章".to_string(), "正文一二三".to_string()),
            ("第二章".to_string(), "正文四五六".to_string()),
        ]).await.unwrap();

        let params: HashMap<String, String> = [("url".into(), "https://book.com/a".into())].into_iter().collect();
        let ret = get_shelf_book_with_cache_info(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert_eq!(ret.0.data["bookUrl"], "https://book.com/a");
        assert_eq!(ret.0.data["name"], "缓存书");
        assert_eq!(ret.0.data["cacheChapterCount"], 2);
        assert_eq!(ret.0.data["cacheSize"], 10, "5+5 字符×2 章");

        // 不存在 → 书籍不存在；缺 url → 书源链接不能为空
        let params: HashMap<String, String> = [("url".into(), "https://book.com/none".into())].into_iter().collect();
        let ret = get_shelf_book_with_cache_info(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "书籍不存在");
        let ret = get_shelf_book_with_cache_info(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert_eq!(ret.0.error_msg, "书源链接不能为空");
        cleanup(state, dir).await;
    }

    /// saveBookContent：写正文缓存（chapterUrl md5 键）→ 可读回
    #[tokio::test]
    async fn test_save_book_content_api() {
        let (state, dir) = test_state("savecontent").await;
        let chapter_url = "https://book.com/c/1";
        let body = Bytes::from(format!(
            r#"{{"bookUrl":"https://book.com/a","chapterUrl":"{chapter_url}","title":"第一章","content":"手动写入的正文"}}"#
        ));
        let ret = save_book_content(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        let idx = crate::util::md5::chapter_url_hash(chapter_url);
        let cached = state.storage.get_chapter_content("https://book.com/a", idx).await.unwrap();
        assert_eq!(cached.as_deref(), Some("手动写入的正文"));
        assert_eq!(state.storage.list_chapters("https://book.com/a").await.unwrap()[0].1, "第一章", "标题一并入库");

        // 缺 bookUrl/chapterUrl → 参数错误；空正文 → 正文不能为空
        let ret = save_book_content(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(Bytes::from(r#"{"bookUrl":"x"}"#))).await;
        assert_eq!(ret.0.error_msg, "参数错误");
        let ret = save_book_content(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(Bytes::from(r#"{"bookUrl":"x","chapterUrl":"y","content":""}"#))).await;
        assert_eq!(ret.0.error_msg, "正文不能为空");
        cleanup(state, dir).await;
    }

    /// deleteUserBookSource：删当前用户书源（body {bookSource}）
    #[tokio::test]
    async fn test_delete_user_book_source_api() {
        let (state, dir) = test_state("delusersrc").await;
        state.storage.save_book_source("default", &crate::model::BookSource { book_source_url: "https://s1.com".into(), book_source_name: "源1".into(), ..Default::default() }).await.unwrap();
        state.storage.save_book_source("default", &crate::model::BookSource { book_source_url: "https://s2.com".into(), book_source_name: "源2".into(), ..Default::default() }).await.unwrap();

        let body = Bytes::from(r#"{"bookSource":"https://s1.com"}"#);
        let ret = delete_user_book_source(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert!(state.storage.find_book_source("default", "https://s1.com").await.unwrap().is_none());
        assert!(state.storage.find_book_source("default", "https://s2.com").await.unwrap().is_some(), "其他书源保留");

        // 缺参 → 参数错误；query bookSource 形式生效
        let ret = delete_user_book_source(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert_eq!(ret.0.error_msg, "参数错误");
        let params: HashMap<String, String> = [("bookSource".into(), "https://s2.com".into())].into_iter().collect();
        let ret = delete_user_book_source(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(ret.0.is_success);
        assert!(state.storage.find_book_source("default", "https://s2.com").await.unwrap().is_none());
        cleanup(state, dir).await;
    }

    /// saveBookGroupId：updateBookGroupId 别名（groupId/group 参数名兼容）
    #[tokio::test]
    async fn test_save_book_group_id_api() {
        let (state, dir) = test_state("savegrpid").await;
        let gid = state.storage.save_book_group("default", &crate::model::BookGroup { name: "玄幻".into(), order: 1, ..Default::default() }).await.unwrap().id;
        state.storage.upsert_book("default", &crate::model::Book { book_url: "https://b.com/1".into(), name: "书1".into(), ..Default::default() }).await.unwrap();

        // body groupId
        let body = Bytes::from(format!(r#"{{"bookUrl":"https://b.com/1","groupId":{gid}}}"#));
        let ret = save_book_group_id(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert_eq!(state.storage.find_book("default", "https://b.com/1").await.unwrap().unwrap().group, gid);

        // query group 参数名（兼容旧 updateBookGroupId 命名）
        let params: HashMap<String, String> = [("bookUrl".into(), "https://b.com/1".into()), ("group".into(), "0".into())].into_iter().collect();
        let ret = save_book_group_id(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(ret.0.is_success);
        assert_eq!(state.storage.find_book("default", "https://b.com/1").await.unwrap().unwrap().group, 0);

        // 缺 bookUrl / 非法 groupId → 参数错误
        let ret = save_book_group_id(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(Bytes::from(r#"{"groupId":1}"#))).await;
        assert_eq!(ret.0.error_msg, "参数错误");
        let body = Bytes::from(r#"{"bookUrl":"https://b.com/1","groupId":-5}"#);
        let ret = save_book_group_id(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert_eq!(ret.0.error_msg, "参数错误");
        cleanup(state, dir).await;
    }

    /// readSourceFile：读书源文件文本；secure 限 storage 内，非 secure 限工作目录内（防穿越）
    #[tokio::test]
    async fn test_read_source_file_api() {
        let (mut state, dir) = test_state("readsrc").await;
        // 非 secure：工作目录内可读
        std::fs::write(dir.join("bookSource.json"), r#"{"bookSourceUrl":"https://x.com"}"#).unwrap();
        let body = Bytes::from(r#"{"path":"bookSource.json"}"#);
        let ret = read_source_file(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert_eq!(ret.0.data, json!(r#"{"bookSourceUrl":"https://x.com"}"#));

        // 穿越/绝对路径拒绝（解析不出 → 路径不存在）
        let body = Bytes::from(r#"{"path":"../escape.json"}"#);
        let ret = read_source_file(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "路径不存在");
        // 不存在 → 路径不存在；缺 path → 参数错误
        let body = Bytes::from(r#"{"path":"ghost.json"}"#);
        let ret = read_source_file(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert_eq!(ret.0.error_msg, "路径不存在");
        let ret = read_source_file(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(Bytes::from(r#"{}"#))).await;
        assert_eq!(ret.0.error_msg, "参数错误");

        // secure：storage 内可读、storage 外拒绝（需登录）
        state.storage.config.secure = true;
        state.storage.insert_user(&User {
            username: "alice".into(),
            token: "tok9".into(),
            ..Default::default()
        }).await.unwrap();
        let auth_params: HashMap<String, String> =
            [("accessToken".into(), "alice:tok9".into())].into_iter().collect();
        let storage_dir = state.storage.config.storage_dir();
        std::fs::create_dir_all(&storage_dir).unwrap();
        std::fs::write(storage_dir.join("bookSource.json"), "[secure]").unwrap();
        let body = Bytes::from(r#"{"path":"storage/bookSource.json"}"#);
        let ret = read_source_file(AxumState(state.clone()), Query(auth_params.clone()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "secure storage 内应可读: {}", ret.0.error_msg);
        assert_eq!(ret.0.data, json!("[secure]"));
        let body = Bytes::from(r#"{"path":"work-only.txt"}"#);
        std::fs::write(dir.join("work-only.txt"), "outside").unwrap();
        let ret = read_source_file(AxumState(state.clone()), Query(auth_params), HeaderMap::new(), Some(body)).await;
        assert!(!ret.0.is_success, "工作目录文件在 secure 下不可达");
        assert_eq!(ret.0.error_msg, "路径不存在");
        cleanup(state, dir).await;
    }

    /// getChapterListByRule：书源 ruleToc 单页解析（url 抓取 → 章节数组）
    #[tokio::test]
    async fn test_get_chapter_list_by_rule_api() {
        let (state, dir) = test_state("chplist").await;
        let base_url = serve_bodies(vec![r#"{"chapters":[
            {"t":"第一章 开始","h":"/c/1.html"},
            {"t":"第二章 继续","h":"/c/2.html"}
        ]}"#.to_string()]).await;
        let base = base_url.trim_end_matches("/sources.json").to_string();
        state.storage.save_book_source("default", &crate::model::BookSource {
            book_source_url: base.clone(),
            book_source_name: "目录源".into(),
            rule_toc: Some(serde_json::json!({
                "chapterList": "$.chapters[*]",
                "chapterName": "$.t",
                "chapterUrl": "$.h",
            })),
            ..Default::default()
        }).await.unwrap();

        // url + bookSource（书源 URL）
        let toc_url = format!("{base}/toc");
        let params: HashMap<String, String> = [
            ("url".into(), toc_url.clone()),
            ("bookSource".into(), base.clone()),
        ].into_iter().collect();
        let ret = get_chapter_list_by_rule(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        let arr = ret.0.data.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["title"], "第一章 开始");
        assert_eq!(arr[0]["url"], format!("{base}/c/1.html"), "相对 URL 应转绝对");
        assert_eq!(arr[0]["index"], 0);
        assert_eq!(arr[1]["title"], "第二章 继续");

        // chapterUrl 参数兜底
        let params: HashMap<String, String> = [
            ("chapterUrl".into(), toc_url.clone()),
            ("bookSource".into(), base.clone()),
        ].into_iter().collect();
        let ret = get_chapter_list_by_rule(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(ret.0.is_success, "chapterUrl 兜底应生效: {}", ret.0.error_msg);

        // 缺 url/书源不存在 → 参数校验
        let ret = get_chapter_list_by_rule(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert_eq!(ret.0.error_msg, "请输入目录链接");
        let params: HashMap<String, String> = [("url".into(), toc_url)].into_iter().collect();
        let ret = get_chapter_list_by_rule(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert_eq!(ret.0.error_msg, "书源不存在");
        cleanup(state, dir).await;
    }

    /// importBookPreview：multipart file → 解析预览（不入库）——handler 全链路 + 纯函数
    #[tokio::test]
    async fn test_import_book_preview_api() {
        // 纯函数核心：TXT 三章 → {name/format/chapterCount/preview 前 10 章}
        let txt = "第一章 起点\n内容一。\n第二章 成长\n内容二。\n第三章 终局\n内容三。";
        let json = import_preview_from_bytes(txt.as_bytes(), "txt", &[]).unwrap();
        assert_eq!(json["format"], "txt");
        assert_eq!(json["chapterCount"], 3);
        let preview = json["preview"].as_array().unwrap();
        assert_eq!(preview.len(), 3);
        assert_eq!(preview[0], "第一章 起点");
        assert_eq!(preview[2], "第三章 终局");
        // 不支持的格式
        assert!(import_preview_from_bytes(b"x", "exe", &[]).is_err());

        // handler 全链路：构造 multipart 请求体 → Multipart 提取器 → 响应
        let (state, dir) = test_state("importprev").await;
        let boundary = "reader-test-boundary";
        let multipart_body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\nContent-Type: text/plain\r\n\r\n{txt}\r\n--{boundary}--\r\n"
        );
        let req = axum::http::Request::builder()
            .method("POST")
            .header("content-type", format!("multipart/form-data; boundary={boundary}"))
            .body(axum::body::Body::from(multipart_body))
            .unwrap();
        use axum::extract::FromRequest;
        let multipart = axum::extract::Multipart::from_request(req, &()).await.unwrap();
        let ret = import_book_preview(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), multipart).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert_eq!(ret.0.data["format"], "txt");
        assert_eq!(ret.0.data["chapterCount"], 3);
        assert_eq!(ret.0.data["preview"][0], "第一章 起点");
        // 不入库：书架/章节表无痕
        assert!(state.storage.list_books("default").await.unwrap().is_empty());
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
        // 空 body + 空 query → 默认 all（成功）
        let ret = clear_cache(AxumState(state.clone()), Query(HashMap::new()), None).await;
        assert!(ret.0.is_success);

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

    /// 换源：searchBookSource——书架书取书名、排除当前源、其余源搜索失败优雅降级为空数组
    #[tokio::test]
    async fn test_search_book_source_api() {
        let (state, dir) = test_state("srcswitch").await;
        // 两个书源：s1=当前源（有搜索规则），s2=其他源（search_url 指向不可达域名，爬取失败→空）
        for (url, name) in [("https://s1.com", "源1"), ("https://s2.com", "源2")] {
            state
                .storage
                .save_book_source(
                    "default",
                    &crate::model::BookSource {
                        book_source_url: url.into(),
                        book_source_name: name.into(),
                        enabled: true,
                        search_url: Some(format!("{url}/search?q={{key}}")),
                        rule_search: Some(serde_json::json!({
                            "bookList": "@js:JSON.parse(result).data",
                            "name": "$.name",
                            "author": "$.author",
                            "bookUrl": "$.url",
                            "tocUrl": "$.toc"
                        })),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
        }
        // 书架书（当前源 s1）
        state
            .storage
            .upsert_book(
                "default",
                &crate::model::Book {
                    book_url: "https://s1.com/book/1".into(),
                    name: "测试书".into(),
                    origin: "https://s1.com".into(),
                    origin_name: "源1".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // 缺参 → 业务错误
        let ret = search_book_source(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "请输入书籍链接");

        // 正常调用：排除当前源 s1，仅 s2 搜索（不可达 → 空数组，不报错）
        let params: HashMap<String, String> = [
            ("url".into(), "https://s1.com/book/1".into()),
            ("bookSource".into(), "https://s1.com".into()),
        ]
        .into_iter()
        .collect();
        let ret = search_book_source(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(ret.0.is_success, "应成功: {}", ret.0.error_msg);
        let data = ret.0.data.as_array().expect("应返回数组（源搜索失败降级为空）");
        assert!(data.is_empty());

        // 仅当前源 → 无其他源 → data null
        state.storage.delete_book_source("default", "https://s2.com").await.unwrap();
        let params: HashMap<String, String> = [
            ("url".into(), "https://s1.com/book/1".into()),
            ("bookSource".into(), "https://s1.com".into()),
        ]
        .into_iter()
        .collect();
        let ret = search_book_source(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(ret.0.is_success);
        assert!(ret.0.data.is_null(), "无其他源应返回 null");

        cleanup(state, dir).await;
    }

    /// OPDS：本地书 acquire 正文 / download 下载（存库章节重建），目录条目含两个 acquisition 链接
    #[tokio::test]
    async fn test_opds_local_book_acquire_and_download() {
        let (state, dir) = test_state("opdslocal").await;
        state
            .storage
            .upsert_book(
                "default",
                &crate::model::Book {
                    book_url: "local://abc".into(),
                    name: "本地书".into(),
                    author: "作者".into(),
                    origin: "loc_book".into(),
                    origin_name: "本地".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        state
            .storage
            .save_chapters(
                "local://abc",
                &[
                    ("第一章".to_string(), "第一段内容".to_string()),
                    ("第二章".to_string(), "第二段内容".to_string()),
                ],
            )
            .await
            .unwrap();
        let id = crate::api::opds::encode_id("local://abc");

        // acquire：正文（新 API 返回首章正文，不含标题）
        let (name, bytes) = crate::api::opds::acquire(&state.storage, "default", &id)
            .await
            .expect("acquire 应成功");
        assert_eq!(name, "本地书.txt");
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("第一段内容"), "acquire 应返回首章正文: {text}");

        // download：章节重建（带文件名）
        let (fname, bytes, _ct) = crate::api::opds::download(&state.storage, "default", &id, "", None)
            .await
            .unwrap();
        assert_eq!(fname, "本地书.txt");
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("第二段内容"));

        // 书架目录（OPDS 2.0 JSON）：含两个 acquisition 链接（download + acquire）
        let json = crate::api::opds::shelf_json(&state.storage, "default", 0, 50).await.unwrap();
        assert!(json.contains(&format!("/opds/download/{id}")), "目录应含下载链接");
        assert!(json.contains(&format!("/opds/acquire/{id}")), "目录应含正文链接");
        assert!(json.contains("本地书"));

        cleanup(state, dir).await;
    }

    /// OPDS：accessToken 查询参数认证（secure 模式），与 /reader3 一致
    #[tokio::test]
    async fn test_opds_access_token_query_auth() {
        let (state, dir) = test_state("opdsauth").await;
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

        // 正确 accessToken → alice
        let params: HashMap<String, String> =
            [("accessToken".into(), "alice:tok123".into())].into_iter().collect();
        let ns = opds_ns(&state, &HeaderMap::new(), &params).await.expect("应认证通过");
        assert_eq!(ns, "alice");

        // 错误 token → 401
        let params: HashMap<String, String> =
            [("accessToken".into(), "alice:bad".into())].into_iter().collect();
        let ret = opds_ns(&state, &HeaderMap::new(), &params).await;
        assert!(ret.is_err());
        assert_eq!(ret.unwrap_err().status(), StatusCode::UNAUTHORIZED);

        // 缺 accessToken → 401（secure）
        let ret = opds_ns(&state, &HeaderMap::new(), &HashMap::new()).await;
        assert!(ret.is_err());

        // 非 secure → 恒 default（accessToken 不参与）
        let mut state2 = state.clone();
        state2.storage.config.secure = false;
        let ret = opds_ns(&state2, &HeaderMap::new(), &HashMap::new()).await;
        assert_eq!(ret.unwrap(), "default");

        cleanup(state, dir).await;
    }

    /// OPDS：Basic 认证——独立 OPDS 账号优先（system_settings），回退系统用户（users 表）；
    /// 密码存储：独立账号 sha256(salt||pwd)（salt$hash）；系统用户 legacy 双 md5（gen_encrypted_password）
    #[tokio::test]
    async fn test_opds_basic_auth_accounts() {
        let (state, dir) = test_state("opdsbasic").await;
        let mut state = state;
        state.storage.config.secure = true;
        // 系统用户（users 表，legacy 双 md5 哈希存储）
        let salt = "s1".to_string();
        state
            .storage
            .insert_user(&User {
                username: "alice".into(),
                password: gen_encrypted_password("pw123456", &salt),
                salt: salt.clone(),
                token: "tok123".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        // 独立 OPDS 账号（sha256+salt 存储，不回传明文）
        let stored = crate::util::sha256::store_password("opds-pass");
        state.storage.set_opds_account("reader", &stored).await.unwrap();

        let basic = |u: &str, p: &str| {
            use base64::Engine;
            let mut h = HeaderMap::new();
            let cred = base64::engine::general_purpose::STANDARD.encode(format!("{u}:{p}"));
            h.insert(
                axum::http::header::AUTHORIZATION,
                format!("Basic {cred}").parse().unwrap(),
            );
            h
        };

        // 独立 OPDS 账号通过
        let ns = opds_ns(&state, &basic("reader", "opds-pass"), &HashMap::new())
            .await
            .expect("独立 OPDS 账号应通过");
        assert_eq!(ns, "reader");

        // 独立账号密码错误 → 401（不回退系统账号的同名用户）
        let ret = opds_ns(&state, &basic("reader", "wrong"), &HashMap::new()).await;
        assert!(ret.is_err());
        assert_eq!(ret.unwrap_err().status(), StatusCode::UNAUTHORIZED);

        // 系统用户 Basic 通过（密码为哈希存储，复用 gen_encrypted_password 校验）
        let ns = opds_ns(&state, &basic("alice", "pw123456"), &HashMap::new())
            .await
            .expect("系统用户 Basic 应通过");
        assert_eq!(ns, "alice");

        // 系统用户密码错误 → 401
        let ret = opds_ns(&state, &basic("alice", "bad"), &HashMap::new()).await;
        assert!(ret.is_err());

        // 禁用独立账号后：系统账号仍可用
        state.storage.clear_opds_account().await.unwrap();
        let ns = opds_ns(&state, &basic("alice", "pw123456"), &HashMap::new())
            .await
            .expect("禁用独立账号后系统用户应通过");
        assert_eq!(ns, "alice");

        cleanup(state, dir).await;
    }

    /// OPDS 分发路由：根导航 / shelf / opensearch / 404 / acquire / save
    #[tokio::test]
    async fn test_opds_dispatch_routes() {
        let (state, dir) = test_state("opdsdispatch").await;
        state
            .storage
            .upsert_book(
                "default",
                &crate::model::Book {
                    book_url: "https://a.com/1".into(),
                    name: "测试书".into(),
                    author: "作者".into(),
                    origin: "https://s.com".into(),
                    origin_name: "源".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let dispatch = |rest: &str, params: HashMap<String, String>| {
            opds_dispatch(
                AxumState(state.clone()),
                Some(axum::extract::Path(rest.to_string())),
                Query(params),
                HeaderMap::new(),
            )
        };

        // 根：导航 feed
        let resp = dispatch("", HashMap::new()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = String::from_utf8(axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap().to_vec()).unwrap();
        assert!(body.contains("书架") && body.contains("/opds/shelf"));

        // shelf：acquisition feed 含条目
        let resp = dispatch("shelf", HashMap::new()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = String::from_utf8(axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap().to_vec()).unwrap();
        assert!(body.contains("测试书"));
        assert!(body.contains("opds:totalResults"));

        // opensearch.xml
        let resp = dispatch("opensearch.xml", HashMap::new()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("opensearchdescription"));

        // OPDS 2.0 根
        let resp = dispatch("catalog", HashMap::new()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("application/opds+json"));

        // 未知路径 → 404
        let resp = dispatch("nonexistent", HashMap::new()).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // 不存在书籍：acquire/save → 404
        let id = crate::api::opds::encode_id("https://nope.com");
        let resp = dispatch(&format!("acquire/{id}"), HashMap::new()).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let resp = dispatch(&format!("save/{id}"), HashMap::new()).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // 搜索
        let resp = dispatch("search", [("q".to_string(), "测试".to_string())].into_iter().collect()).await;
        assert_eq!(resp.status(), StatusCode::OK);

        cleanup(state, dir).await;
    }

    /// OPDS 独立账号设置端点：saveOpdsSettings / getOpdsSettings（密码不回传）
    #[tokio::test]
    async fn test_opds_settings_endpoints() {
        let (state, dir) = test_state("opdsset").await;
        // 默认关闭
        let ret = get_opds_settings(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new()).await;
        assert!(ret.0.is_success);
        assert_eq!(ret.0.data["enabled"], false);
        assert_eq!(ret.0.data["passwordSet"], false);

        // 配置账号
        let body = Bytes::from(json!({"username": "reader", "password": "secret123"}).to_string());
        let ret = save_opds_settings(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "保存失败: {}", ret.0.error_msg);
        assert_eq!(ret.0.data["enabled"], true);

        let ret = get_opds_settings(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new()).await;
        assert!(ret.0.is_success);
        assert_eq!(ret.0.data["username"], "reader");
        assert_eq!(ret.0.data["passwordSet"], true);
        assert!(ret.0.data.get("password").is_none(), "密码不得回传");
        // 落库为 salt$hash，非明文
        let (_, stored) = state.storage.get_opds_account().await.unwrap().unwrap();
        assert!(stored.contains('$'));
        assert_ne!(stored, "secret123");

        // 短密码拒绝
        let body = Bytes::from(json!({"username": "reader", "password": "123"}).to_string());
        let ret = save_opds_settings(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(!ret.0.is_success);

        // 空用户名 → 禁用
        let body = Bytes::from(json!({"username": "", "password": ""}).to_string());
        let ret = save_opds_settings(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success);
        let ret = get_opds_settings(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new()).await;
        assert_eq!(ret.0.data["enabled"], false);

        cleanup(state, dir).await;
    }

    /// 路由构建冒烟：全部 OPDS 路由注册无冲突（matchit 冲突会在构建时 panic）
    #[tokio::test]
    async fn test_router_constructs_with_opds_routes() {
        let (state, dir) = test_state("opdsrouter").await;
        let app = router(state.storage.config.clone(), state.storage.clone());
        // 不 panic 即通过（axum 0.7 Router 无 routes() 自省——构建冲突会在 router() 时 panic）
        let _ = app;
        cleanup(state, dir).await;
    }

    // ---------------- 书源登录态（loginBookSource 参数解析） ----------------

    /// 登录参数合并：query 兜底 + body JSON 优先 / form-urlencoded 兑底
    #[test]
    fn test_merge_login_params_query_only() {
        let mut q = HashMap::new();
        q.insert("bookSource".to_string(), "https://a.com".to_string());
        q.insert("username".to_string(), "u1".to_string());
        let m = merge_login_params(&q, None);
        assert_eq!(m.get("bookSource").map(String::as_str), Some("https://a.com"));
        assert_eq!(m.get("username").map(String::as_str), Some("u1"));
        assert_eq!(m.get("password"), None);
    }

    #[test]
    fn test_merge_login_params_json_body() {
        let mut q = HashMap::new();
        q.insert("bookSource".to_string(), "https://query.com".to_string());
        let body = br#"{"bookSource":"https://body.com","username":"u1","password":"p1","captcha":"c1","mode":"browser"}"#;
        let m = merge_login_params(&q, Some(body));
        // body JSON 优先于 query
        assert_eq!(m.get("bookSource").map(String::as_str), Some("https://body.com"));
        assert_eq!(m.get("username").map(String::as_str), Some("u1"));
        assert_eq!(m.get("password").map(String::as_str), Some("p1"));
        assert_eq!(m.get("captcha").map(String::as_str), Some("c1"));
        assert_eq!(m.get("mode").map(String::as_str), Some("browser"));
    }

    #[test]
    fn test_merge_login_params_form_body() {
        let mut q = HashMap::new();
        q.insert("bookSource".to_string(), "https://a.com".to_string());
        let body = b"username=u1&password=p1&captcha=c1";
        let m = merge_login_params(&q, Some(body));
        assert_eq!(m.get("bookSource").map(String::as_str), Some("https://a.com"));
        assert_eq!(m.get("username").map(String::as_str), Some("u1"));
        assert_eq!(m.get("password").map(String::as_str), Some("p1"));
        assert_eq!(m.get("captcha").map(String::as_str), Some("c1"));
    }

    #[test]
    fn test_merge_login_params_invalid_body_falls_back_to_query() {
        let mut q = HashMap::new();
        q.insert("bookSource".to_string(), "https://a.com".to_string());
        // 非 JSON 非表单（二进制）→ 保留 query
        let m = merge_login_params(&q, Some(b"\x00\x01\x02"));
        assert_eq!(m.get("bookSource").map(String::as_str), Some("https://a.com"));
    }
    // ==================== 差距补全批：导出 / 调试 / 缓存 / 配置 / 刷新 / 批量 / 健康 / 统计 ====================

    /// 微型 HTTP 服务器：支持 HEAD 与 GET（健康检测用）
    async fn serve_head_get() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            for _ in 0..10 {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let req = String::from_utf8_lossy(&buf);
                let body = if req.starts_with("HEAD ") { "" } else { "<html>ok</html>" };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        format!("http://{addr}")
    }

    /// exportBook：本地书 txt/epub/html 三格式导出 + 参数校验
    #[tokio::test]
    async fn test_export_book_api() {
        let (state, dir) = test_state("exportbook").await;
        state
            .storage
            .upsert_book(
                "default",
                &crate::model::Book {
                    book_url: "local://exp1".into(),
                    name: "导出测试书".into(),
                    author: "作者甲".into(),
                    origin: "local".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        state
            .storage
            .save_chapters(
                "local://exp1",
                &[
                    ("第一章".to_string(), "正文一 <甲> & 乙。".to_string()),
                    ("第二章".to_string(), "正文二。".to_string()),
                ],
            )
            .await
            .unwrap();
        let params = |format: &str| -> HashMap<String, String> {
            [("url".into(), "local://exp1".into()), ("format".into(), format.into())]
                .into_iter()
                .collect()
        };

        // txt
        let resp = export_book(AxumState(state.clone()), Query(params("txt")), HeaderMap::new(), None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("Content-Type").and_then(|v| v.to_str().ok()),
            Some("text/plain; charset=utf-8")
        );
        let cd = resp.headers().get("Content-Disposition").and_then(|v| v.to_str().ok()).expect("应含 Content-Disposition");
        assert!(cd.starts_with("attachment; filename="), "{cd}");
        assert!(cd.ends_with(".txt\""), "{cd}");
        assert!(cd.contains("%E5%AF%BC"), "非 ASCII 应百分号编码: {cd}");
        let txt = String::from_utf8(axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap().to_vec()).unwrap();
        assert!(txt.contains("导出测试书"));
        assert!(txt.contains("第一章"));
        assert!(txt.contains("正文一 <甲> & 乙。"));
        assert!(txt.contains("正文二。"));

        // epub（zip 构造验证：mimetype/container/OPF/spine 章节）
        let resp = export_book(AxumState(state.clone()), Query(params("epub")), HeaderMap::new(), None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("Content-Type").and_then(|v| v.to_str().ok()),
            Some("application/epub+zip")
        );
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap().to_vec();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("EPUB 应为合法 zip");
        let mut mime = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("mimetype").unwrap(), &mut mime).unwrap();
        assert_eq!(mime, "application/epub+zip");
        let mut container = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("META-INF/container.xml").unwrap(), &mut container).unwrap();
        assert!(container.contains("OEBPS/content.opf"));
        let mut opf = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("OEBPS/content.opf").unwrap(), &mut opf).unwrap();
        assert!(opf.contains("<dc:title>导出测试书</dc:title>"));
        assert!(opf.contains("<dc:creator>作者甲</dc:creator>"));
        assert_eq!(opf.matches("<itemref").count(), 2, "spine 两章");
        let mut ch0 = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("OEBPS/chap_0000.xhtml").unwrap(), &mut ch0).unwrap();
        assert!(ch0.contains("正文一 &lt;甲&gt; &amp; 乙。"), "XML 转义: {ch0}");

        // html（单页：标题 + 章节）
        let resp = export_book(AxumState(state.clone()), Query(params("html")), HeaderMap::new(), None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("Content-Type").and_then(|v| v.to_str().ok()),
            Some("text/html; charset=utf-8")
        );
        let html = String::from_utf8(axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap().to_vec()).unwrap();
        assert!(html.contains("<h1>导出测试书</h1>"));
        assert!(html.contains("<h2>第一章</h2>"));
        assert!(html.contains("<p>正文二。</p>"));

        // 非法格式 / 缺 url
        let resp = export_book(AxumState(state.clone()), Query(params("pdf")), HeaderMap::new(), None).await;
        let json: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap()).unwrap();
        assert!(!json["isSuccess"].as_bool().unwrap());
        assert_eq!(json["errorMsg"], "不支持的导出格式（txt|epub|html）");
        let resp = export_book(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        let json: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap()).unwrap();
        assert!(!json["isSuccess"].as_bool().unwrap());

        cleanup(state, dir).await;
    }

    /// exportBook：书源书（目录 → 逐章正文，复用规则引擎）
    #[tokio::test]
    async fn test_export_book_web_api() {
        let (state, dir) = test_state("exportweb").await;
        let base_url = serve_bodies(vec![
            r#"<ul class="chapters"><li><a href="/ch1.html">第一章</a></li><li><a href="/ch2.html">第二章</a></li></ul>"#.to_string(),
            r#"<html><body><div class="content">正文一。</div></body></html>"#.to_string(),
            r#"<html><body><div class="content">正文二。</div></body></html>"#.to_string(),
        ])
        .await;
        let base = base_url.trim_end_matches("/sources.json").to_string();
        state
            .storage
            .save_book_source(
                "default",
                &crate::model::BookSource {
                    book_source_url: base.clone(),
                    book_source_name: "导出源".into(),
                    rule_toc: Some(serde_json::json!({
                        "chapterList": "ul.chapters@li", "chapterName": "a@text", "chapterUrl": "a@href"
                    })),
                    rule_content: Some(serde_json::json!({ "content": "div.content@text" })),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let book_url = format!("{base}/book/1");
        state
            .storage
            .upsert_book(
                "default",
                &crate::model::Book {
                    book_url: book_url.clone(),
                    name: "网文书".into(),
                    origin: base.clone(),
                    toc_url: format!("{base}/toc"),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let params: HashMap<String, String> = [("url".into(), book_url.clone())].into_iter().collect();
        let resp = export_book(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert_eq!(resp.status(), StatusCode::OK, "书源书应可导出");
        let txt = String::from_utf8(axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap().to_vec()).unwrap();
        assert!(txt.contains("网文书"));
        assert!(txt.contains("第一章"));
        assert!(txt.contains("正文一。"));
        assert!(txt.contains("正文二。"));

        cleanup(state, dir).await;
    }

    /// bookSourceDebugSSE：search 动作逐步骤事件（规则解析/URL 构造/请求/规则应用）→ result
    #[tokio::test]
    async fn test_book_source_debug_sse_search() {
        let (state, dir) = test_state("dbgsearch").await;
        let base_url = serve_bodies(vec![
            r#"{"data":[{"name":"调试书","author":"甲","url":"/book/1"}]}"#.to_string(),
        ])
        .await;
        let base = base_url.trim_end_matches("/sources.json").to_string();
        state
            .storage
            .save_book_source(
                "default",
                &crate::model::BookSource {
                    book_source_url: base.clone(),
                    book_source_name: "调试源".into(),
                    search_url: Some(format!("{base}/search?q={{key}}")),
                    rule_search: Some(serde_json::json!({
                        "bookList": "$.data[*]",
                        "name": "$.name", "author": "$.author", "bookUrl": "$.url"
                    })),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let params: HashMap<String, String> = [
            ("bookSource".into(), base.clone()),
            ("action".into(), "search".into()),
            ("key".into(), "调试书".into()),
        ]
        .into_iter()
        .collect();
        let resp = book_source_debug_sse(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = String::from_utf8(axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024).await.unwrap().to_vec()).unwrap();
        assert!(body.contains("\"type\":\"start\""), "{body}");
        assert!(body.contains("\"type\":\"step\""), "应含 step 事件: {body}");
        assert!(body.contains("规则解析（ruleSearch）"), "应含规则解析步骤: {body}");
        assert!(body.contains("URL 构造"), "应含 URL 构造步骤: {body}");
        assert!(body.contains("请求 URL"), "应含请求步骤: {body}");
        assert!(body.contains("规则应用（bookList 字段）"), "应含规则应用步骤: {body}");
        assert!(body.contains("\"type\":\"result\""), "应含 result 事件: {body}");
        assert!(body.contains("\"name\":\"调试书\""), "result 应含搜索结果: {body}");
        assert!(body.contains("\"ruleName\":\"规则解析（ruleSearch）\""), "step 应含 ruleName 字段: {body}");
        assert!(body.contains("bookListKind"), "step 应含规则解析明细: {body}");

        // 缺 key → error 事件
        let params: HashMap<String, String> = [("bookSource".into(), base.clone()), ("action".into(), "search".into())]
            .into_iter()
            .collect();
        let resp = book_source_debug_sse(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        let body = String::from_utf8(axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap().to_vec()).unwrap();
        assert!(body.contains("请输入搜索关键字"));

        // 非法动作 → error 事件
        let params: HashMap<String, String> = [("bookSource".into(), base.clone()), ("action".into(), "bad".into())]
            .into_iter()
            .collect();
        let resp = book_source_debug_sse(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        let body = String::from_utf8(axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap().to_vec()).unwrap();
        assert!(body.contains("请输入调试动作"));

        cleanup(state, dir).await;
    }

    /// bookSourceDebugSSE：toc / content 动作
    #[tokio::test]
    async fn test_book_source_debug_sse_toc_content() {
        let (state, dir) = test_state("dbgtoc").await;
        let base_url = serve_bodies(vec![
            r#"<ul class="chapters"><li><a href="/ch.html">第一章</a></li></ul>"#.to_string(),
        ])
        .await;
        let base = base_url.trim_end_matches("/sources.json").to_string();
        state
            .storage
            .save_book_source(
                "default",
                &crate::model::BookSource {
                    book_source_url: base.clone(),
                    book_source_name: "目录源".into(),
                    rule_toc: Some(serde_json::json!({
                        "chapterList": "ul.chapters@li", "chapterName": "a@text", "chapterUrl": "a@href"
                    })),
                    rule_content: Some(serde_json::json!({ "content": "div.content@text" })),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let params: HashMap<String, String> = [
            ("bookSource".into(), base.clone()),
            ("action".into(), "toc".into()),
            ("url".into(), format!("{base}/toc.html")),
        ]
        .into_iter()
        .collect();
        let resp = book_source_debug_sse(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        let body = String::from_utf8(axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024).await.unwrap().to_vec()).unwrap();
        assert!(body.contains("规则解析（ruleToc）"), "{body}");
        assert!(body.contains("chapterList 提取"), "应含 chapterList 提取步骤: {body}");
        assert!(body.contains("字段规则（chapterName/chapterUrl）"), "{body}");
        assert!(body.contains("\"title\":\"第一章\""), "result 应含章节: {body}");

        // content
        let base_url = serve_bodies(vec![
            r#"<html><body><div class="content">正文一。</div></body></html>"#.to_string(),
        ])
        .await;
        let base2 = base_url.trim_end_matches("/sources.json").to_string();
        state
            .storage
            .save_book_source(
                "default",
                &crate::model::BookSource {
                    book_source_url: base2.clone(),
                    book_source_name: "正文源".into(),
                    rule_content: Some(serde_json::json!({ "content": "div.content@text" })),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let params: HashMap<String, String> = [
            ("bookSource".into(), base2.clone()),
            ("action".into(), "content".into()),
            ("chapterUrl".into(), format!("{base2}/ch.html")),
        ]
        .into_iter()
        .collect();
        let resp = book_source_debug_sse(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        let body = String::from_utf8(axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024).await.unwrap().to_vec()).unwrap();
        assert!(body.contains("规则解析（ruleContent）"), "{body}");
        assert!(body.contains("content 规则应用"), "{body}");
        assert!(body.contains("\"content\":\"正文一。\""), "result 应含正文: {body}");

        // 缺 url → error 事件（toc）
        let params: HashMap<String, String> = [("bookSource".into(), base.clone()), ("action".into(), "toc".into())]
            .into_iter()
            .collect();
        let resp = book_source_debug_sse(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        let body = String::from_utf8(axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap().to_vec()).unwrap();
        assert!(body.contains("请输入目录链接"));

        cleanup(state, dir).await;
    }

    /// cacheBookOnServer / cacheBookSSE / cancelCacheBook：本地书（无网络）
    #[tokio::test]
    async fn test_cache_book_api() {
        let (state, dir) = test_state("cachebook").await;
        let book_url = "local://cache1";
        state
            .storage
            .upsert_book(
                "default",
                &crate::model::Book {
                    book_url: book_url.into(),
                    name: "缓存书".into(),
                    origin: "local".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        state
            .storage
            .save_chapters(
                book_url,
                &[("第一章".to_string(), "正文一".to_string()), ("第二章".to_string(), "正文二".to_string())],
            )
            .await
            .unwrap();
        let params: HashMap<String, String> = [("url".into(), book_url.into())].into_iter().collect();

        // 启动任务
        let ret = cache_book_on_server(AxumState(state.clone()), Query(params.clone()), HeaderMap::new(), None).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert!(ret.0.data["started"].as_bool().unwrap());
        // 等待完成
        assert!(
            crate::service::cache_job::wait_finished(book_url, std::time::Duration::from_secs(5)).await,
            "任务应在 5s 内完成"
        );
        let p = crate::service::cache_job::progress_of(book_url).unwrap();
        let p = p.lock().unwrap_or_else(|e| e.into_inner());
        assert!(p.finished);
        assert_eq!(p.total, 2);
        assert_eq!(p.cached, 2);
        assert_eq!(p.title, "缓存书");
        drop(p);

        // SSE 进度流
        let resp = cache_book_sse(AxumState(state.clone()), Query(params.clone()), HeaderMap::new(), None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = String::from_utf8(axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024).await.unwrap().to_vec()).unwrap();
        assert!(body.contains("\"cached\":2"), "{body}");
        assert!(body.contains("\"total\":2"), "{body}");
        assert!(body.contains("\"finished\":true"), "{body}");

        // cancel：任务已完成但仍在表内 → true；再 cancel → false
        let ret = cancel_cache_book(AxumState(state.clone()), Query(params.clone()), HeaderMap::new(), None).await;
        assert!(ret.0.is_success);
        assert!(ret.0.data["cancelled"].as_bool().unwrap());
        let ret = cancel_cache_book(AxumState(state.clone()), Query(params.clone()), HeaderMap::new(), None).await;
        assert!(!ret.0.data["cancelled"].as_bool().unwrap());

        // 未知任务 SSE → error 事件
        let ghost: HashMap<String, String> = [("url".into(), "local://ghost".into())].into_iter().collect();
        let resp = cache_book_sse(AxumState(state.clone()), Query(ghost), HeaderMap::new(), None).await;
        let body = String::from_utf8(axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap().to_vec()).unwrap();
        assert!(body.contains("缓存任务不存在"));

        // 书不存在 → 不启动
        let ghost: HashMap<String, String> = [("url".into(), "local://ghost2".into())].into_iter().collect();
        let ret = cache_book_on_server(AxumState(state.clone()), Query(ghost), HeaderMap::new(), None).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "书籍不存在（请先加入书架）");

        cleanup(state, dir).await;
    }

    /// cacheBookOnServer：书源书后台缓存（目录 → 并发 3 逐章 → 缓存表）
    #[tokio::test]
    async fn test_cache_book_web_api() {
        let (state, dir) = test_state("cacheweb").await;
        let base_url = serve_bodies(vec![
            r#"<ul class="chapters"><li><a href="/ch1.html">第一章</a></li><li><a href="/ch2.html">第二章</a></li></ul>"#.to_string(),
            r#"<html><body><div class="content">正文一。</div></body></html>"#.to_string(),
            r#"<html><body><div class="content">正文二。</div></body></html>"#.to_string(),
        ])
        .await;
        let base = base_url.trim_end_matches("/sources.json").to_string();
        state
            .storage
            .save_book_source(
                "default",
                &crate::model::BookSource {
                    book_source_url: base.clone(),
                    book_source_name: "缓存源".into(),
                    rule_toc: Some(serde_json::json!({
                        "chapterList": "ul.chapters@li", "chapterName": "a@text", "chapterUrl": "a@href"
                    })),
                    rule_content: Some(serde_json::json!({ "content": "div.content@text" })),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let book_url = format!("{base}/book/cache");
        state
            .storage
            .upsert_book(
                "default",
                &crate::model::Book {
                    book_url: book_url.clone(),
                    name: "缓存网文书".into(),
                    origin: base.clone(),
                    toc_url: format!("{base}/toc"),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let params: HashMap<String, String> = [("url".into(), book_url.clone())].into_iter().collect();
        let ret = cache_book_on_server(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert!(
            crate::service::cache_job::wait_finished(&book_url, std::time::Duration::from_secs(10)).await,
            "书源书缓存应在 10s 内完成"
        );
        let p = crate::service::cache_job::progress_of(&book_url).unwrap();
        let p = p.lock().unwrap_or_else(|e| e.into_inner());
        assert!(p.finished, "任务应结束");
        assert_eq!(p.total, 2);
        assert_eq!(p.cached, 2, "两章都应缓存成功: {p:?}");
        assert_eq!(p.title, "缓存网文书");
        drop(p);
        // 缓存表已写入（chapterUrl md5 键）
        let idx1 = crate::util::md5::chapter_url_hash(&format!("{base}/ch1.html"));
        let idx2 = crate::util::md5::chapter_url_hash(&format!("{base}/ch2.html"));
        assert_eq!(
            state.storage.get_chapter_content(&book_url, idx1).await.unwrap().as_deref(),
            Some("正文一。")
        );
        assert_eq!(
            state.storage.get_chapter_content(&book_url, idx2).await.unwrap().as_deref(),
            Some("正文二。")
        );
        // 清理任务表
        crate::service::cache_job::cancel(&book_url);
        cleanup(state, dir).await;
    }

    /// getUserConfig / saveUserConfig：读写覆盖 + 用户隔离（secure 模式）
    #[tokio::test]
    async fn test_user_config_api() {
        let (state, dir) = test_state("userconf").await;

        // 保存 {ns, config}
        let body = Bytes::from(r#"{"ns":"reader","config":{"fontSize":18,"theme":"dark"}}"#);
        let ret = save_user_config(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        // 读取
        let params: HashMap<String, String> = [("key".into(), "reader".into())].into_iter().collect();
        let ret = get_user_config(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(ret.0.is_success);
        assert_eq!(ret.0.data["ns"], "reader");
        assert_eq!(ret.0.data["config"]["fontSize"], 18);
        assert_eq!(ret.0.data["config"]["theme"], "dark");
        // 覆盖
        let body = Bytes::from(r#"{"ns":"reader","config":{"fontSize":20}}"#);
        let ret = save_user_config(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success);
        let params: HashMap<String, String> = [("key".into(), "reader".into())].into_iter().collect();
        let ret = get_user_config(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert_eq!(ret.0.data["config"]["fontSize"], 20);
        // 未设置的 key → null
        let params: HashMap<String, String> = [("key".into(), "ghost".into())].into_iter().collect();
        let ret = get_user_config(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(ret.0.data["config"].is_null());
        // 裸 JSON 整体保存（无 config 键；默认 ns=global）
        let body = Bytes::from(r#"{"fontSize":16}"#);
        let ret = save_user_config(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success);
        let ret = get_user_config(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert_eq!(ret.0.data["config"]["fontSize"], 16);
        // 非 JSON → 参数错误
        let ret = save_user_config(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(Bytes::from("nope"))).await;
        assert_eq!(ret.0.error_msg, "参数错误");

        // secure 模式用户隔离
        let mut state = state;
        state.storage.config.secure = true;
        state
            .storage
            .insert_user(&User { username: "alice".into(), token: "t1".into(), ..Default::default() })
            .await
            .unwrap();
        state
            .storage
            .insert_user(&User { username: "bob".into(), token: "t2".into(), ..Default::default() })
            .await
            .unwrap();
        let body = Bytes::from(r#"{"ns":"pref","config":{"a":1}}"#);
        let params: HashMap<String, String> = [("accessToken".into(), "alice:t1".into())].into_iter().collect();
        let ret = save_user_config(AxumState(state.clone()), Query(params.clone()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success);
        let mut q = params.clone();
        q.insert("key".into(), "pref".into());
        let ret = get_user_config(AxumState(state.clone()), Query(q), HeaderMap::new(), None).await;
        assert_eq!(ret.0.data["config"]["a"], 1);
        // bob 读不到 alice 的配置
        let mut qb: HashMap<String, String> = [
            ("accessToken".into(), "bob:t2".into()),
            ("key".into(), "pref".into()),
        ]
        .into_iter()
        .collect();
        let ret = get_user_config(AxumState(state.clone()), Query(qb.clone()), HeaderMap::new(), None).await;
        assert!(ret.0.data["config"].is_null(), "bob 不应看到 alice 配置: {ret:?}");
        // bob 覆盖自己的配置不影响 alice
        let body = Bytes::from(r#"{"ns":"pref","config":{"a":2}}"#);
        let ret = save_user_config(AxumState(state.clone()), Query(qb), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success);
        let mut q = params;
        q.insert("key".into(), "pref".into());
        let ret = get_user_config(AxumState(state.clone()), Query(q), HeaderMap::new(), None).await;
        assert_eq!(ret.0.data["config"]["a"], 1, "alice 配置不受 bob 影响");

        cleanup(state, dir).await;
    }

    /// refreshLocalBook：local:// 重解析原文件 + 文件书重解析 + 非本地书拒绝
    #[tokio::test]
    async fn test_refresh_local_book_api() {
        let (state, dir) = test_state("refreshlocal").await;
        // local:// 书：原文件在 opds_files/{id}.txt
        let id = "book-abc";
        let opds_dir = state.storage.config.storage_dir().join("data/default/opds_files");
        std::fs::create_dir_all(&opds_dir).unwrap();
        std::fs::write(
            opds_dir.join(format!("{id}.txt")),
            "第一章 起点\n内容一。\n第二章 成长\n内容二。",
        )
        .unwrap();
        let book_url = format!("local://{id}");
        state
            .storage
            .upsert_book(
                "default",
                &crate::model::Book {
                    book_url: book_url.clone(),
                    name: "刷新书".into(),
                    origin: "local".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let params: HashMap<String, String> = [("url".into(), book_url.clone())].into_iter().collect();
        let ret = refresh_local_book(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert_eq!(ret.0.data["chapterCount"], 2);
        assert_eq!(ret.0.data["name"], "刷新书");
        assert_eq!(state.storage.list_chapters(&book_url).await.unwrap().len(), 2, "章节已重扫入库");
        assert_eq!(
            state.storage.find_book("default", &book_url).await.unwrap().unwrap().total_chapter_num,
            2,
            "totalChapterNum 已更新"
        );

        // 文件型本地书（storage/ 路径）
        let file_dir = state.storage.config.storage_dir().join("data/default/books");
        std::fs::create_dir_all(&file_dir).unwrap();
        std::fs::write(
            file_dir.join("示例2.txt"),
            "第一章 起点\n内容一。\n第二章 成长\n内容二。\n第三章 终章\n内容三。",
        )
        .unwrap();
        let fbook = "storage/data/default/books/示例2.txt";
        state
            .storage
            .upsert_book(
                "default",
                &crate::model::Book {
                    book_url: fbook.into(),
                    name: "文件书".into(),
                    origin: "loc_book".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let params: HashMap<String, String> = [("url".into(), fbook.into())].into_iter().collect();
        let ret = refresh_local_book(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert_eq!(ret.0.data["chapterCount"], 3);

        // 非本地书 → 拒绝；不存在 → 书籍不存在；缺 url → 参数错误
        state
            .storage
            .upsert_book(
                "default",
                &crate::model::Book { book_url: "https://web.com/a".into(), name: "网文".into(), ..Default::default() },
            )
            .await
            .unwrap();
        let params: HashMap<String, String> = [("url".into(), "https://web.com/a".into())].into_iter().collect();
        let ret = refresh_local_book(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "仅支持本地书刷新");
        let params: HashMap<String, String> = [("url".into(), "local://ghost".into())].into_iter().collect();
        let ret = refresh_local_book(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert_eq!(ret.0.error_msg, "书籍不存在");
        let ret = refresh_local_book(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert_eq!(ret.0.error_msg, "参数错误");

        cleanup(state, dir).await;
    }

    /// deleteBooks：批量删除（含章节清理）与参数校验
    #[tokio::test]
    async fn test_delete_books_api() {
        let (state, dir) = test_state("delbooks").await;
        for (url, name) in [("https://b.com/1", "书1"), ("https://b.com/2", "书2"), ("https://b.com/3", "书3")] {
            state
                .storage
                .upsert_book("default", &crate::model::Book { book_url: url.into(), name: name.into(), ..Default::default() })
                .await
                .unwrap();
        }
        state.storage.save_chapters("https://b.com/1", &[("第一章".into(), "正文".into())]).await.unwrap();

        let body = Bytes::from(r#"{"bookUrls":["https://b.com/1","https://b.com/2"]}"#);
        let ret = delete_books(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert_eq!(ret.0.data["count"], 2);
        assert!(state.storage.find_book("default", "https://b.com/1").await.unwrap().is_none());
        assert!(state.storage.find_book("default", "https://b.com/2").await.unwrap().is_none());
        assert!(state.storage.find_book("default", "https://b.com/3").await.unwrap().is_some());
        assert_eq!(state.storage.count_chapters("https://b.com/1").await.unwrap(), 0, "章节连带删除");

        // 参数校验
        let ret = delete_books(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert_eq!(ret.0.error_msg, "参数错误");
        let body = Bytes::from(r#"{"bookUrls":[]}"#);
        let ret = delete_books(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert_eq!(ret.0.error_msg, "参数错误");

        cleanup(state, dir).await;
    }

    /// deleteBookmarks：批量删书签（{bookUrl, ids}）与参数校验
    #[tokio::test]
    async fn test_delete_bookmarks_api() {
        let (state, dir) = test_state("delbms").await;
        for (i, title) in ["m1", "m2", "m3"].iter().enumerate() {
            state
                .storage
                .save_bookmark(
                    "default",
                    &crate::model::Bookmark {
                        book_url: "https://b.com/1".into(),
                        title: (*title).into(),
                        paragraph_index: i as i64,
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
        }
        // 删两条
        let body = Bytes::from(r#"{"bookUrl":"https://b.com/1","ids":["m1","m3"]}"#);
        let ret = delete_bookmarks(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert_eq!(ret.0.data["count"], 2);
        let rest = state.storage.list_bookmarks("default", "https://b.com/1").await.unwrap();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].title, "m2");
        // 参数校验
        let body = Bytes::from(r#"{"bookUrl":"https://b.com/1","ids":[]}"#);
        let ret = delete_bookmarks(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert_eq!(ret.0.error_msg, "参数错误");
        let ret = delete_bookmarks(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert_eq!(ret.0.error_msg, "参数错误");

        cleanup(state, dir).await;
    }

    /// saveRssSources：批量保存 RSS 源（覆盖 + 校验）
    #[tokio::test]
    async fn test_save_rss_sources_api() {
        let (state, dir) = test_state("save_rss").await;
        let body = Bytes::from(
            r#"[{"sourceUrl":"https://r1.com/feed","sourceName":"源1","sourceGroup":"科技","enabled":true},
                {"sourceUrl":"https://r2.com/feed","sourceName":"源2","enabled":false}]"#,
        );
        let ret = save_rss_sources(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert_eq!(ret.0.data["count"], 2);
        let list = state.storage.get_rss_sources("default").await.unwrap();
        assert_eq!(list.len(), 2);
        let s1 = list.iter().find(|s| s.source_url == "https://r1.com/feed").unwrap();
        assert_eq!(s1.source_name, "源1");
        assert_eq!(s1.source_group.as_deref(), Some("科技"));
        assert!(s1.enabled);
        let s2 = list.iter().find(|s| s.source_url == "https://r2.com/feed").unwrap();
        assert!(!s2.enabled);
        // 覆盖同 url 不新增
        let body = Bytes::from(r#"[{"sourceUrl":"https://r1.com/feed","sourceName":"源1v2"}]"#);
        let ret = save_rss_sources(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success);
        assert_eq!(state.storage.get_rss_sources("default").await.unwrap().len(), 2);
        // 校验
        let ret = save_rss_sources(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert_eq!(ret.0.error_msg, "参数错误");
        let body = Bytes::from(r#"[]"#);
        let ret = save_rss_sources(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert_eq!(ret.0.error_msg, "参数错误");
        let body = Bytes::from(r#"[{"sourceUrl":"https://r3.com/feed"}]"#);
        let ret = save_rss_sources(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert_eq!(ret.0.error_msg, "参数错误", "缺 sourceName 应拒绝");

        cleanup(state, dir).await;
    }

    /// markRssArticleRead：标记已读/未读（body {articleUrl, read}）
    #[tokio::test]
    async fn test_mark_rss_article_read_api() {
        let (mut state, dir) = test_state("mark_read").await;
        let article = crate::model::RssArticle {
            url: "https://feed.example.com/x".into(),
            source_url: "https://feed.example.com/rss".into(),
            title: "X".into(),
            ..Default::default()
        };
        state.storage.save_rss_articles("default", &[article]).await.unwrap();
        // 已读
        let body = Bytes::from(r#"{"articleUrl":"https://feed.example.com/x","read":true}"#);
        let ret = mark_rss_article_read(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        let got = state.storage.get_rss_article("https://feed.example.com/x").await.unwrap().unwrap();
        assert!(got.read);
        // 标回未读
        let body = Bytes::from(r#"{"articleUrl":"https://feed.example.com/x","read":false}"#);
        let ret = mark_rss_article_read(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success);
        assert!(!state.storage.get_rss_article("https://feed.example.com/x").await.unwrap().unwrap().read);
        // 参数校验：缺 articleUrl
        let ret = mark_rss_article_read(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert_eq!(ret.0.error_msg, "RSS文章链接不能为空");
        // 未登录（secure 模式）拒绝
        state.storage.config.secure = true;
        let body = Bytes::from(r#"{"articleUrl":"https://feed.example.com/x","read":true}"#);
        let ret = mark_rss_article_read(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(!ret.0.is_success);

        cleanup(state, dir).await;
    }

    /// saveBookmarks：批量保存书签（createdAt 自动补）
    #[tokio::test]
    async fn test_save_bookmarks_api() {
        let (state, dir) = test_state("savebms").await;
        let body = Bytes::from(
            r#"[{"bookUrl":"https://b.com/1","title":"书签甲","paragraphIndex":3,"chapterIndex":1},
                {"bookUrl":"https://b.com/1","title":"书签乙"}]"#,
        );
        let ret = save_bookmarks(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert_eq!(ret.0.data["count"], 2);
        let list = state.storage.list_bookmarks("default", "https://b.com/1").await.unwrap();
        assert_eq!(list.len(), 2);
        let jia = list.iter().find(|b| b.title == "书签甲").unwrap();
        assert_eq!(jia.paragraph_index, 3);
        assert!(jia.created_at > 0, "createdAt 应自动补");
        // 校验
        let body = Bytes::from(r#"[{"bookUrl":"https://b.com/1"}]"#);
        let ret = save_bookmarks(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert_eq!(ret.0.error_msg, "参数错误");
        let ret = save_bookmarks(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert_eq!(ret.0.error_msg, "参数错误");

        cleanup(state, dir).await;
    }

    /// addBookGroupMulti / removeBookGroupMulti：批量设分组/移出分组
    #[tokio::test]
    async fn test_book_group_multi_api() {
        let (state, dir) = test_state("grpmulti").await;
        let g = state
            .storage
            .save_book_group("default", &crate::model::BookGroup { name: "玄幻".into(), order: 1, ..Default::default() })
            .await
            .unwrap();
        for url in ["https://b.com/1", "https://b.com/2", "https://b.com/3"] {
            state
                .storage
                .upsert_book("default", &crate::model::Book { book_url: url.into(), name: url.into(), ..Default::default() })
                .await
                .unwrap();
        }
        // 批量设分组
        let body = Bytes::from(format!(r#"{{"bookUrls":["https://b.com/1","https://b.com/2"],"groupId":{}}}"#, g.id));
        let ret = add_book_group_multi(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert_eq!(ret.0.data["count"], 2);
        assert_eq!(state.storage.find_book("default", "https://b.com/1").await.unwrap().unwrap().group, g.id);
        assert_eq!(state.storage.find_book("default", "https://b.com/2").await.unwrap().unwrap().group, g.id);
        assert_eq!(state.storage.find_book("default", "https://b.com/3").await.unwrap().unwrap().group, 0);
        // 参数校验
        let body = Bytes::from(r#"{"bookUrls":["https://b.com/1"],"groupId":-1}"#);
        let ret = add_book_group_multi(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert_eq!(ret.0.error_msg, "参数错误");
        // 批量移出
        let body = Bytes::from(r#"{"bookUrls":["https://b.com/1","https://b.com/3"]}"#);
        let ret = remove_book_group_multi(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success);
        assert_eq!(ret.0.data["count"], 2, "匹配行数（b1 置 0 + b3 本为 0 也计入匹配）");
        assert_eq!(state.storage.find_book("default", "https://b.com/1").await.unwrap().unwrap().group, 0);
        assert_eq!(state.storage.find_book("default", "https://b.com/2").await.unwrap().unwrap().group, g.id, "未涉及的保持");
        let ret = remove_book_group_multi(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert_eq!(ret.0.error_msg, "参数错误");

        cleanup(state, dir).await;
    }

    /// saveBookGroupOrder：分组排序批量保存
    #[tokio::test]
    async fn test_save_book_group_order_api() {
        let (state, dir) = test_state("grporder").await;
        let g1 = state
            .storage
            .save_book_group("default", &crate::model::BookGroup { name: "甲".into(), order: 1, ..Default::default() })
            .await
            .unwrap();
        let g2 = state
            .storage
            .save_book_group("default", &crate::model::BookGroup { name: "乙".into(), order: 2, ..Default::default() })
            .await
            .unwrap();
        let body = Bytes::from(format!(
            r#"{{"order":[{{"id":{},"orderNum":2}},{{"id":{},"orderNum":1}}]}}"#,
            g1.id, g2.id
        ));
        let ret = save_book_group_order(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert_eq!(ret.0.data["count"], 2);
        let list = state.storage.list_book_groups("default").await.unwrap();
        assert_eq!(list[0].id, g2.id, "乙应排第一");
        assert_eq!(list[0].order, 1);
        assert_eq!(list[1].id, g1.id);
        assert_eq!(list[1].order, 2);
        // 参数校验
        let body = Bytes::from(r#"{"order":[]}"#);
        let ret = save_book_group_order(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert_eq!(ret.0.error_msg, "参数错误");

        cleanup(state, dir).await;
    }

    /// getAvailableBookSource：启用过滤 + key 可搜索过滤 + bookUrlPattern 规则过滤
    #[tokio::test]
    async fn test_get_available_book_source_api() {
        let (state, dir) = test_state("availsrc").await;
        // s1：可搜索 + bookUrlPattern 匹配 a.com
        state
            .storage
            .save_book_source(
                "default",
                &crate::model::BookSource {
                    book_source_url: "https://s1.com".into(),
                    book_source_name: "源1".into(),
                    enabled: true,
                    search_url: Some("https://s1.com/s".into()),
                    book_url_pattern: Some(r#"^https://a\.com/"#.into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        // s2：可探索不可搜索、无 pattern
        state
            .storage
            .save_book_source(
                "default",
                &crate::model::BookSource {
                    book_source_url: "https://s2.com".into(),
                    book_source_name: "源2".into(),
                    enabled: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        // s3：禁用
        state
            .storage
            .save_book_source(
                "default",
                &crate::model::BookSource {
                    book_source_url: "https://s3.com".into(),
                    book_source_name: "源3".into(),
                    enabled: false,
                    search_url: Some("https://s3.com/s".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // key → 仅可搜索源
        let params: HashMap<String, String> = [("key".into(), "测试".into())].into_iter().collect();
        let ret = get_available_book_source(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert!(ret.0.is_success);
        let arr = ret.0.data.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["bookSourceUrl"], "https://s1.com");
        // url 匹配 pattern → s1 + s2（s2 无 pattern 放行）
        let params: HashMap<String, String> = [("url".into(), "https://a.com/book/1".into())].into_iter().collect();
        let ret = get_available_book_source(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        let arr = ret.0.data.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        // url 不匹配 pattern → 仅 s2
        let params: HashMap<String, String> = [("url".into(), "https://b.com/book/1".into())].into_iter().collect();
        let ret = get_available_book_source(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        let arr = ret.0.data.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["bookSourceUrl"], "https://s2.com");

        cleanup(state, dir).await;
    }

    /// getInvalidBookSources：HEAD 200 判定可用；连接拒绝判定失效
    #[tokio::test]
    async fn test_get_invalid_book_sources_api() {
        let (state, dir) = test_state("invalidsrc").await;
        let good_url = serve_head_get().await;
        state
            .storage
            .save_book_source(
                "default",
                &crate::model::BookSource {
                    book_source_url: good_url.clone(),
                    book_source_name: "好源".into(),
                    enabled: true,
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
                    book_source_url: "http://127.0.0.1:1".into(),
                    book_source_name: "坏源".into(),
                    enabled: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        // 禁用的不参与检测
        state
            .storage
            .save_book_source(
                "default",
                &crate::model::BookSource {
                    book_source_url: "http://127.0.0.1:2".into(),
                    book_source_name: "停用源".into(),
                    enabled: false,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let ret = get_invalid_book_sources(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        let arr = ret.0.data.as_array().unwrap();
        assert_eq!(arr.len(), 1, "仅坏源应判定失效: {arr:?}");
        assert_eq!(arr[0]["bookSourceUrl"], "http://127.0.0.1:1");
        assert!(arr[0]["error"].as_str().unwrap().contains("连接失败"));

        cleanup(state, dir).await;
    }

    /// setAsDefaultBookSources：默认书源标记（字符串数组 / 对象数组）
    #[tokio::test]
    async fn test_set_as_default_book_sources_api() {
        let (state, dir) = test_state("defaultsrc").await;
        let body = Bytes::from(r#"{"bookSources":["https://s1.com",{"bookSourceUrl":"https://s2.com"}]}"#);
        let ret = set_as_default_book_sources(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        assert_eq!(ret.0.data["count"], 2);
        let list = state.storage.get_default_book_sources("default").await.unwrap();
        assert_eq!(list, vec!["https://s1.com".to_string(), "https://s2.com".to_string()]);
        // 覆盖
        let body = Bytes::from(r#"{"bookSources":["https://s3.com"]}"#);
        let ret = set_as_default_book_sources(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success);
        assert_eq!(state.storage.get_default_book_sources("default").await.unwrap(), vec!["https://s3.com".to_string()]);
        // 校验
        let body = Bytes::from(r#"{"bookSources":[]}"#);
        let ret = set_as_default_book_sources(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert_eq!(ret.0.error_msg, "参数错误");

        cleanup(state, dir).await;
    }

    /// searchBookSourceSSE：流式换源（逐书源 event: book → event: end）
    #[tokio::test]
    async fn test_search_book_source_sse_api() {
        let (state, dir) = test_state("srcsse").await;
        // 当前源 s1（排除）
        state
            .storage
            .save_book_source(
                "default",
                &crate::model::BookSource {
                    book_source_url: "https://s1.com".into(),
                    book_source_name: "源1".into(),
                    enabled: true,
                    search_url: Some("https://s1.com/s?q={{key}}".into()),
                    rule_search: Some(serde_json::json!({
                        "bookList": "$.data[*]",
                        "name": "$.name", "author": "$.author", "bookUrl": "$.url", "tocUrl": "$.toc"
                    })),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        // s2 → 本地测试服务器（返回命中结果）
        let base_url = serve_bodies(vec![
            r#"{"data":[{"name":"测试书","author":"甲","url":"/book/9","toc":"/toc"}]}"#.to_string(),
        ])
        .await;
        let base = base_url.trim_end_matches("/sources.json").to_string();
        state
            .storage
            .save_book_source(
                "default",
                &crate::model::BookSource {
                    book_source_url: base.clone(),
                    book_source_name: "源2".into(),
                    enabled: true,
                    search_url: Some(format!("{base}/search?q={{key}}")),
                    rule_search: Some(serde_json::json!({
                        "bookList": "$.data[*]",
                        "name": "$.name", "author": "$.author", "bookUrl": "$.url", "tocUrl": "$.toc"
                    })),
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
                    book_url: "https://s1.com/book/1".into(),
                    name: "测试书".into(),
                    origin: "https://s1.com".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let params: HashMap<String, String> = [
            ("url".into(), "https://s1.com/book/1".into()),
            ("bookSource".into(), "https://s1.com".into()),
        ]
        .into_iter()
        .collect();
        let resp = search_book_source_sse(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = String::from_utf8(axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024).await.unwrap().to_vec()).unwrap();
        assert!(body.contains("event: book"), "应含 book 事件: {body}");
        assert!(body.contains("\"name\":\"测试书\""), "命中书应推送: {body}");
        assert!(body.contains("event: end"), "应含 end 事件: {body}");
        assert!(body.contains("\"isEnd\":true"), "{body}");

        // 缺 url → error 事件
        let params: HashMap<String, String> = [("bookSource".into(), "https://s1.com".into())].into_iter().collect();
        let resp = search_book_source_sse(AxumState(state.clone()), Query(params), HeaderMap::new(), None).await;
        let body = String::from_utf8(axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap().to_vec()).unwrap();
        assert!(body.contains("请输入书籍链接"));

        cleanup(state, dir).await;
    }

    /// getReadingStats：saveBookProgress 增量累计时长/字数 → today/week/total/books
    #[tokio::test]
    async fn test_reading_stats_api() {
        let (state, dir) = test_state("readstats").await;
        let now = now_millis();
        state
            .storage
            .upsert_book(
                "default",
                &crate::model::Book {
                    book_url: "https://stats.com/b".into(),
                    name: "统计书".into(),
                    dur_chapter_time: now - 10_000,
                    dur_chapter_pos: 0,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // 第一次进度：+10s / +500 字
        let body = Bytes::from(
            json!({"bookUrl":"https://stats.com/b","durChapterIndex":1,"durChapterPos":500,"durChapterTime":now}).to_string(),
        );
        let ret = save_book_progress(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);
        // 第二次：+5s / +200 字
        let body = Bytes::from(
            json!({"bookUrl":"https://stats.com/b","durChapterIndex":1,"durChapterPos":700,"durChapterTime":now + 5000}).to_string(),
        );
        let ret = save_book_progress(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(ret.0.is_success, "{}", ret.0.error_msg);

        // storage 层汇总
        let stats = state.storage.get_reading_stats("default").await.unwrap();
        assert_eq!(stats.today, 15, "10s + 5s");
        assert_eq!(stats.total, 15);
        assert!(stats.week >= 15);
        assert_eq!(stats.books.len(), 1);
        assert_eq!(stats.books[0].book_url, "https://stats.com/b");
        assert_eq!(stats.books[0].name, "统计书");
        assert_eq!(stats.books[0].seconds, 15);
        assert_eq!(stats.books[0].chars, 700, "500 + 200 字");

        // handler 输出（camelCase）
        let ret = get_reading_stats(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), None).await;
        assert!(ret.0.is_success);
        assert_eq!(ret.0.data["today"], 15);
        assert_eq!(ret.0.data["total"], 15);
        assert_eq!(ret.0.data["books"][0]["chars"], 700);
        assert_eq!(ret.0.data["books"][0]["bookUrl"], "https://stats.com/b");

        // 未入架书 → 书籍未加入书架（不记统计）
        let body = Bytes::from(r#"{"bookUrl":"https://ghost.com/b","durChapterIndex":0,"durChapterPos":10}"#);
        let ret = save_book_progress(AxumState(state.clone()), Query(HashMap::new()), HeaderMap::new(), Some(body)).await;
        assert!(!ret.0.is_success);
        assert_eq!(ret.0.error_msg, "书籍未加入书架");

        cleanup(state, dir).await;
    }

    /// 命名兼容批 2（端到端）：resetPassword / httpTTS / uploadFile 别名路由
    #[tokio::test]
    async fn test_alias_routes_batch2() {
        let (state, dir) = test_state("alias2").await;
        let mut state = state;
        state.storage.config.secure = true;
        state.storage.config.secure_key = "sk".into();
        state
            .storage
            .insert_user(&User {
                username: "alice".into(),
                token: "t1".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        let app = router(state.storage.config.clone(), state.storage.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let base = format!("http://{addr}");
        let client = reqwest::Client::new();

        // uploadFile（= file/upload）：multipart txt 上传（手构 multipart body）
        let boundary = "----reader-test-boundary";
        let multipart_body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"示例.txt\"\r\nContent-Type: text/plain\r\n\r\n第一章 起点\n内容一。\r\n--{boundary}--\r\n"
        );
        let resp = client
            .post(format!("{base}/reader3/uploadFile"))
            .query(&[("accessToken", "alice:t1")])
            .header("Content-Type", format!("multipart/form-data; boundary={boundary}"))
            .body(multipart_body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let json: Value = resp.json().await.unwrap();
        assert!(json["isSuccess"].as_bool().unwrap(), "uploadFile 应成功: {json}");
        let arr = json["data"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "示例.txt");
        assert!(
            std::path::Path::new(&state.storage.config.storage_dir().join("data/alice/示例.txt")).exists(),
            "文件应落盘"
        );

        // httpTTS（= tts）：未知引擎 → 业务错误（无网络请求）
        let resp = client
            .get(format!("{base}/reader3/httpTTS"))
            .query(&[("accessToken", "alice:t1"), ("text", "你好"), ("engine", "nope")])
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let json: Value = resp.json().await.unwrap();
        assert_eq!(json["errorMsg"], "不支持的TTS引擎");

        // resetPassword（= resetUserPassword）：重置后旧 token 失效
        let resp = client
            .post(format!("{base}/reader3/resetPassword"))
            .query(&[("accessToken", "alice:t1"), ("secureKey", "sk")])
            .json(&json!({"username": "alice", "newPassword": "新密码123"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let json: Value = resp.json().await.unwrap();
        assert!(json["isSuccess"].as_bool().unwrap(), "resetPassword 应成功: {json}");
        let alice = state.storage.find_user("alice").await.unwrap().unwrap();
        assert_eq!(gen_encrypted_password("新密码123", &alice.salt), alice.password, "新密码应可校验");
        assert!(alice.token.is_empty(), "旧 token 应失效");

        cleanup(state, dir).await;
    }

}
