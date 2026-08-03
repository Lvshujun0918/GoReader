//! JSON → SQLite 一次性迁移（legacy storage/data → SQLite）
//!
//! 触发条件：`storage/data/users.json` 存在 且 users 表为空。
//! 迁移前自动备份 `storage/data/` → `storage/backup-before-migrate-{ts}/`。
//!
//! 迁移内容：
//! - `storage/data/users.json`（Map<username, User>）→ users 表（user_namespace = username）
//! - `storage/data/{ns}/bookshelf.json`（ns = default 或各用户名）→ books 表（user_namespace = ns）
//! - bookSource.json / rssSource.json 等（后续切片）暂不迁移

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use sqlx::SqlitePool;

use crate::model::{Book, User};
use crate::storage::Storage;

/// 启动时检测并执行 JSON → SQLite 迁移（幂等：users 表非空即跳过）
pub async fn migrate_if_needed(storage: &Storage) -> Result<()> {
    let data_dir = storage.config.storage_dir().join("data");
    let users_path = data_dir.join("users.json");
    if !users_path.exists() {
        tracing::info!("未发现 legacy JSON 数据（{} 不存在），跳过迁移", users_path.display());
        return Ok(());
    }
    let user_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&storage.pool)
        .await?;
    if user_count > 0 {
        tracing::info!("users 表已有 {} 条记录，跳过 JSON 迁移", user_count);
        return Ok(());
    }

    // 1. 迁移前备份 storage/data → storage/backup-before-migrate-{ts}/
    let ts = Utc::now().format("%Y%m%d%H%M%S");
    let backup_dir = storage.config.storage_dir().join(format!("backup-before-migrate-{ts}"));
    copy_dir_recursive(&data_dir, &backup_dir)
        .with_context(|| format!("备份 storage/data → {} 失败", backup_dir.display()))?;
    tracing::info!("已备份 storage/data → {}", backup_dir.display());

    // 2. users.json → users 表
    let usernames = migrate_users(&storage.pool, &users_path).await?;

    // 3. 各命名空间 bookshelf.json → books 表（ns = default + 各用户名）
    let mut namespaces: Vec<String> = Vec::with_capacity(usernames.len() + 1);
    namespaces.push("default".to_string());
    namespaces.extend(usernames.iter().cloned());
    let book_count = migrate_bookshelves(&storage.pool, &data_dir, &namespaces).await?;

    tracing::info!(
        "JSON→SQLite 迁移完成：{} 个用户，{} 本书（备份：{}）",
        usernames.len(),
        book_count,
        backup_dir.display()
    );
    Ok(())
}

/// users.json（Map<username, User>）→ users 表；返回迁移的用户名列表
async fn migrate_users(pool: &SqlitePool, path: &Path) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("读取 {} 失败", path.display()))?;
    let user_map: HashMap<String, User> = serde_json::from_str(&text)
        .with_context(|| format!("解析 {} 失败", path.display()))?;

    let mut usernames = Vec::with_capacity(user_map.len());
    let mut tx = pool.begin().await?;
    for (key, mut user) in user_map {
        if user.username.is_empty() {
            user.username = key;
        }
        // user_namespace = username（用户数据命名空间）
        user.user_namespace = user.username.clone();
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
        .execute(&mut *tx)
        .await?;
        usernames.push(user.username.clone());
        tracing::info!("迁移用户: {}", user.username);
    }
    tx.commit().await?;
    Ok(usernames)
}

/// 各命名空间 bookshelf.json → books 表；返回迁移的书籍总数
async fn migrate_bookshelves(pool: &SqlitePool, data_dir: &Path, namespaces: &[String]) -> Result<usize> {
    let mut total = 0usize;
    for ns in namespaces {
        let path = data_dir.join(ns).join("bookshelf.json");
        if !path.exists() {
            tracing::debug!("{ns} 无 bookshelf.json，跳过");
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("读取 {} 失败（{}），跳过该命名空间", path.display(), e);
                continue;
            }
        };
        let books: Vec<Book> = match serde_json::from_str(&text) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("解析 {} 失败（{}），跳过该命名空间", path.display(), e);
                continue;
            }
        };
        let mut count = 0usize;
        let mut tx = pool.begin().await?;
        for book in books {
            if book.book_url.trim().is_empty() {
                continue; // 无主键的脏数据跳过
            }
            sqlx::query(
                r#"
                INSERT OR REPLACE INTO books
                    (book_url, name, author, origin, origin_name, kind, cover_url, intro,
                     toc_url, charset, custom_cover_url, can_update, dur_chapter_index,
                     dur_chapter_pos, dur_chapter_time, dur_chapter_title, group_name,
                     type, last_check_error, user_namespace, created_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                        ?16, ?17, ?18, ?19, ?20, ?21)
                "#,
            )
            .bind(&book.book_url)
            .bind(&book.name)
            .bind(&book.author)
            .bind(&book.origin)
            .bind(&book.origin_name)
            .bind(&book.kind)
            .bind(&book.cover_url)
            .bind(&book.intro)
            .bind(&book.toc_url)
            .bind(&book.charset)
            .bind(&book.custom_cover_url)
            .bind(book.can_update)
            .bind(book.dur_chapter_index)
            .bind(book.dur_chapter_pos)
            .bind(book.dur_chapter_time)
            .bind(&book.dur_chapter_title)
            .bind(book.group)
            .bind(book.book_type)
            .bind(&book.last_check_error)
            .bind(ns)
            .bind(0i64) // created_at：迁移数据时间未知，置 0（顺序由 rowid 保持）
            .execute(&mut *tx)
            .await?;
            count += 1;
        }
        tx.commit().await?;
        tracing::info!("迁移书架 [{ns}]：{} 本", count);
        total += count;
    }
    Ok(total)
}

/// 递归拷贝目录（备份用）
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}
