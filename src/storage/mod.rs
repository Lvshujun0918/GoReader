//! 存储层：SQLite（兼容迁移自 legacy 的 JSON storage）

use anyhow::Result;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;

use crate::model::{Book, User};
use crate::AppConfig;

pub mod migrate;

/// 存储句柄
#[derive(Clone)]
pub struct Storage {
    pub pool: SqlitePool,
    pub config: AppConfig,
}

/// 初始化：建目录 + 打开/建库 + 建表
pub async fn init(config: &AppConfig) -> Result<Storage> {
    let storage_dir = config.storage_dir();
    std::fs::create_dir_all(&storage_dir)?;
    let db_path = storage_dir.join("reader.db");

    let options = SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await?;

    // 建表（兼容 legacy 实体）
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            username TEXT PRIMARY KEY,
            password TEXT NOT NULL,
            salt TEXT NOT NULL,
            token TEXT DEFAULT '',
            enable_webdav INTEGER DEFAULT 0,
            enable_local_store INTEGER DEFAULT 0,
            enable_book_source INTEGER DEFAULT 1,
            enable_rss_source INTEGER DEFAULT 1,
            book_source_limit INTEGER DEFAULT 0,
            book_limit INTEGER DEFAULT 0,
            last_login_at INTEGER DEFAULT 0,
            created_at INTEGER DEFAULT 0,
            user_namespace TEXT DEFAULT ''
        );
        "#,
    )
    .execute(&pool)
    .await?;

    // 兼容旧库：users 表缺 user_namespace 列时补列
    let cols: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('users')")
        .fetch_all(&pool)
        .await?;
    if !cols.iter().any(|c| c == "user_namespace") {
        sqlx::query("ALTER TABLE users ADD COLUMN user_namespace TEXT DEFAULT ''")
            .execute(&pool)
            .await?;
        tracing::info!("users 表补充 user_namespace 列");
    }

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS books (
            book_url TEXT,
            name TEXT DEFAULT '',
            author TEXT DEFAULT '',
            origin TEXT DEFAULT '',
            origin_name TEXT DEFAULT '',
            kind TEXT,
            cover_url TEXT,
            intro TEXT,
            toc_url TEXT DEFAULT '',
            charset TEXT,
            custom_cover_url TEXT,
            can_update INTEGER DEFAULT 1,
            dur_chapter_index INTEGER DEFAULT 0,
            dur_chapter_pos INTEGER DEFAULT 0,
            dur_chapter_time INTEGER DEFAULT 0,
            dur_chapter_title TEXT,
            group_name INTEGER DEFAULT 0,
            type INTEGER DEFAULT 0,
            last_check_error TEXT,
            user_namespace TEXT DEFAULT '',
            created_at INTEGER DEFAULT 0,
            PRIMARY KEY (book_url, user_namespace)
        );
        "#,
    )
    .execute(&pool)
    .await?;

    tracing::info!("storage initialized at {}", db_path.display());
    Ok(Storage {
        pool,
        config: config.clone(),
    })
}

impl Storage {
    /// 按用户名查用户（登录 / token 校验）
    pub async fn find_user(&self, username: &str) -> Result<Option<User>> {
        let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE username = ?")
            .bind(username)
            .fetch_optional(&self.pool)
            .await?;
        Ok(user)
    }

    /// 用户总数（注册上限校验）
    pub async fn count_users(&self) -> Result<i64> {
        let count = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    /// 新建用户
    pub async fn insert_user(&self, user: &User) -> Result<()> {
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO users
                (username, password, salt, token, enable_webdav, enable_local_store,
                 enable_book_source, enable_rss_source, book_source_limit, book_limit,
                 last_login_at, created_at, user_namespace)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            "#,
        )
        .bind(&user.username)
        .bind(&user.password)
        .bind(&user.salt)
        .bind(&user.token)
        .bind(user.enable_webdav)
        .bind(user.enable_local_store)
        .bind(user.enable_book_source)
        .bind(user.enable_rss_source)
        .bind(user.book_source_limit)
        .bind(user.book_limit)
        .bind(user.last_login_at)
        .bind(user.created_at)
        .bind(&user.user_namespace)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 登录成功：刷新 token + last_login_at
    pub async fn update_user_session(&self, username: &str, token: &str, last_login_at: i64) -> Result<()> {
        sqlx::query("UPDATE users SET token = ?1, last_login_at = ?2 WHERE username = ?3")
            .bind(token)
            .bind(last_login_at)
            .bind(username)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// 查询某命名空间的书架（按插入顺序，兼容 legacy bookshelf.json 数组顺序）
    pub async fn list_books(&self, namespace: &str) -> Result<Vec<Book>> {
        let books = sqlx::query_as::<_, Book>(
            r#"
            SELECT book_url, name, author, origin, origin_name, kind, cover_url, intro,
                   toc_url, charset, custom_cover_url, can_update, dur_chapter_index,
                   dur_chapter_pos, dur_chapter_time, dur_chapter_title, group_name,
                   type, last_check_error
            FROM books
            WHERE user_namespace = ?1
            ORDER BY rowid ASC
            "#,
        )
        .bind(namespace)
        .fetch_all(&self.pool)
        .await?;
        Ok(books)
    }
}
