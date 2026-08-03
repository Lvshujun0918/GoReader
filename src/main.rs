//! reader-dev — 阅读3服务器版 Rust 重构
//!
//! API 兼容 legacy 分支（Kotlin）的 `/reader3/*` 接口，数据兼容（JSON storage → SQLite 迁移）。

mod api;
mod model;
mod parser;
mod service;
mod storage;
mod util;

use std::net::SocketAddr;

use anyhow::Result;

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
    /// 注册邀请码（空 = 不要求）
    pub invite_code: String,
    /// 密码最短位数
    pub min_user_password_length: i64,
    /// 新用户默认权限
    pub default_user_enable_webdav: bool,
    pub default_user_enable_local_store: bool,
    pub default_user_enable_book_source: bool,
    pub default_user_enable_rss_source: bool,
    pub default_user_book_source_limit: i64,
    pub default_user_book_limit: i64,
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
            default_user_enable_webdav: env_flag("READER_APP_DEFAULTUSERENABLEWEBDAV"),
            default_user_enable_local_store: env_flag("READER_APP_DEFAULTUSERENABLELOCALSTORE"),
            default_user_enable_book_source: env_flag_opt("READER_APP_DEFAULTUSERENABLEBOOKSOURCE")
                .unwrap_or(true),
            default_user_enable_rss_source: env_flag_opt("READER_APP_DEFAULTUSERENABLERSSSOURCE")
                .unwrap_or(true),
            default_user_book_source_limit: env_i64("READER_APP_DEFAULTUSERBOOKSOURCELIMIT", 200),
            default_user_book_limit: env_i64("READER_APP_DEFAULTUSERBOOKLIMIT", 200),
        }
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
}

fn env_flag(key: &str) -> bool {
    env_flag_opt(key).unwrap_or(false)
}

/// 读取布尔 env：未设置返回 None（可区分“缺省”与“显式 false”
fn env_flag_opt(key: &str) -> Option<bool> {
    std::env::var(key).ok().map(|v| {
        matches!(
            v.to_ascii_lowercase().as_str(),
            "true" | "1" | "yes" | "on"
        )
    })
}

fn env_i64(key: &str, default: i64) -> i64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    dotenvy::dotenv().ok();

    let config = AppConfig::from_env();
    let storage = storage::init(&config).await?;

    // JSON → SQLite 自动迁移（检测到 legacy storage/data 且 users 表为空时执行）
    storage::migrate::migrate_if_needed(&storage).await?;

    // 初始化路由
    let app = api::router(config.clone(), storage);

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    tracing::info!("reader-dev (Rust) listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
