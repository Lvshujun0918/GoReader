//! reader-dev 核心库（Rust 重构）
//!
//! API 兼容 legacy 分支（Kotlin）的 `/reader3/*` 接口，数据兼容（JSON storage → SQLite 迁移）。

pub mod api;
pub mod middleware;
pub mod model;
pub mod parser;
pub mod service;
pub mod storage;
pub mod util;

use std::net::SocketAddr;

/// 应用配置（env / .env，兼容 READER_APP_* 前缀）
#[derive(Debug, Clone)]
pub struct AppConfig {
    /// 工作目录（storage 根，兼容 READER_APP_WORKDIR）
    pub work_dir: String,
    /// 服务端口
    pub port: u16,
    /// 是否启用登录鉴权（多用户）
    pub secure: bool,
    /// 管理密码
    pub secure_key: String,
    /// 用户上限
    pub user_limit: i64,
    /// 用户书籍上限
    pub user_book_limit: i64,
    /// 邀请码
    pub invite_code: String,
    /// 最小密码长度
    pub min_user_password_length: i64,
    /// token 有效期（天，GAP 118；<=0 表示永不过期）
    pub token_ttl_days: i64,
    /// 前端静态资源根（构建产物 dist 目录）
    pub web_root: String,
    /// 默认新用户权限（legacy AppConfig 默认）
    pub default_user_enable_webdav: bool,
    pub default_user_enable_local_store: bool,
    pub default_user_enable_book_source: bool,
    pub default_user_enable_rss_source: bool,
    pub default_user_book_source_limit: i64,
    pub default_user_book_limit: i64,
    /// GAP 62 上传大小上限（MB，env READER_UPLOAD_MAX_MB，默认 100）
    pub upload_max_mb: i64,
}

impl AppConfig {
    pub fn from_env() -> Self {
        let work_dir = std::env::var("READER_APP_WORKDIR").unwrap_or_default();
        let port = std::env::var("READER_SERVER_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8080);
        Self {
            work_dir,
            port,
            secure: env_flag("READER_APP_SECURE"),
            secure_key: std::env::var("READER_APP_SECUREKEY").unwrap_or_default(),
            user_limit: env_i64("READER_APP_USERLIMIT", 500_000),
            user_book_limit: env_i64("READER_APP_USERBOOKLIMIT", 500_000),
            invite_code: std::env::var("READER_APP_INVITECODE").unwrap_or_default(),
            min_user_password_length: env_i64("READER_APP_MINUSERPASSWORDLENGTH", 8),
            token_ttl_days: env_i64("READER_TOKEN_TTL_DAYS", 30),
            web_root: std::env::var("READER_APP_WEB_ROOT")
                .unwrap_or_else(|_| "web-ui/dist".to_string()),
            default_user_enable_webdav: env_flag("READER_APP_DEFAULTUSERENABLEWEBDAV"),
            default_user_enable_local_store: env_flag("READER_APP_DEFAULTUSERENABLELOCALSTORE"),
            default_user_enable_book_source: if env_flag("READER_APP_DEFAULTUSERENABLEBOOKSOURCE") {
                true
            } else {
                true
            },
            default_user_enable_rss_source: if env_flag("READER_APP_DEFAULTUSERENABLERSSSOURCE") {
                true
            } else {
                true
            },
            default_user_book_source_limit: env_i64("READER_APP_DEFAULTUSERBOOKSOURCELIMIT", 100),
            default_user_book_limit: env_i64("READER_APP_DEFAULTUSERBOOKLIMIT", 200),
            upload_max_mb: env_i64("READER_UPLOAD_MAX_MB", 100),
        }
    }

    /// GAP 62：上传大小上限字节数（multipart DefaultBodyLimit 用）
    pub fn upload_max_bytes(&self) -> usize {
        (self.upload_max_mb.max(1) * 1024 * 1024) as usize
    }

    /// storage 根目录（workDir 下的 storage）
    pub fn storage_dir(&self) -> std::path::PathBuf {
        let base = if self.work_dir.is_empty() {
            std::path::PathBuf::from(".")
        } else {
            std::path::PathBuf::from(&self.work_dir)
        };
        base.join("storage")
    }

    /// 启动服务（axum）
    pub async fn serve(self) -> anyhow::Result<()> {
        let storage = storage::init(&self).await?;
        // GAP 170 本地书双轨同步仓：启动对账 + 书仓目录文件监听（notify，300ms 去抖）
        service::local_sync::spawn_local_sync(storage.clone());
        // F-35 定时书架更新检查（每 10 分钟）+ GAP #101 订阅/RSS 自动刷新
        service::schedule::spawn_schedule_jobs(storage.clone());
        // GAP #57 自动备份（每天 READER_AUTO_BACKUP_HOUR 03:00 默认）
        service::schedule::spawn_auto_backup_job(storage.clone());
        let app = api::router(self.clone(), storage);
        // GAP 60：静态资源 Cache-Control（hash 文件名 30 天 / index.html no-cache）。
        // 挂载点说明：router.rs 被并行修改（git status 未提交改动）——避免冲突，
        // 在 app 构造处（lib.rs serve()，main.rs 无 app 构造代码）挂载，效果等同 router 内层。
        let app = app.layer(crate::middleware::cache_control::CacheControlLayer);
        // GAP 60 备注（实测确认）：router.rs 内的 CompressionLayer 只覆盖其调用前注册的路由
        // （/assets、/health 等）——fallback 静态资源（web-ui/dist）与后注册的 /reader3、/opds
        // 路由未覆盖（axum Router::layer 只包装调用时已注册的路由）。此处外层再挂一层兜底：
        // 已带 Content-Encoding 的响应自动跳过，SSE（text/event-stream）/音视频默认排除，无副作用。
        let app = app.layer(tower_http::compression::CompressionLayer::new());
        // GAP 62：multipart 超限（DefaultBodyLimit 413）→ 替换为明确的 JSON 错误（最外层兜底）
        let app = app.layer(crate::middleware::upload_limit::UploadLimitLayer {
            max_mb: self.upload_max_mb,
        });
        // 服务监控：请求计数（最外层——413/404/静态资源同样计入）
        let app = app.layer(crate::middleware::stats::StatsLayer);
        let addr = SocketAddr::from(([0, 0, 0, 0], self.port));
        tracing::info!("reader-dev (Rust) listening on {addr}");
        let listener = tokio::net::TcpListener::bind(addr).await?;
        // M3：启用 ConnectInfo（直连对端 IP）——登录限流不再信任可伪造的 X-Forwarded-For
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await?;
        Ok(())
    }
}

fn env_flag(key: &str) -> bool {
    std::env::var(key)
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes" | "on"))
        .unwrap_or(false)
}

fn env_i64(key: &str, default: i64) -> i64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}
