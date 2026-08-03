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

    // 初始化路由
    let app = api::router(config.clone(), storage);

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    tracing::info!("reader-dev (Rust) listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
