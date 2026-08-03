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

use crate::model::{Book, BookSource, User};
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

    // 4. 各命名空间 bookSource.json → book_sources 表（ns = default + 各用户名）
    let source_count = migrate_book_sources(&storage.pool, &data_dir, &namespaces).await?;

    tracing::info!(
        "JSON→SQLite 迁移完成：{} 个用户，{} 本书，{} 个书源（备份：{}）",
        usernames.len(),
        book_count,
        source_count,
        backup_dir.display()
    );
    Ok(())
}

/// users.json（Map<username, User>）→ users 表（全字段 + raw_json 原文保底）；返回迁移的用户名列表
async fn migrate_users(pool: &SqlitePool, path: &Path) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("读取 {} 失败", path.display()))?;
    let user_map: HashMap<String, serde_json::Value> = serde_json::from_str(&text)
        .with_context(|| format!("解析 {} 失败", path.display()))?;

    let mut usernames = Vec::with_capacity(user_map.len());
    let mut tx = pool.begin().await?;
    for (key, value) in user_map {
        let mut user: User = match serde_json::from_value(value.clone()) {
            Ok(u) => u,
            Err(e) => {
                tracing::warn!("解析用户 {} 失败（{}），保留 raw_json", key, e);
                User { username: key.clone(), ..Default::default() }
            }
        };
        if user.username.is_empty() {
            user.username = key;
        }
        // user_namespace = username（用户数据命名空间）
        user.user_namespace = user.username.clone();
        // raw_json：原始 JSON 全量保底（未知字段不丢）
        user.raw_json = Some(value.to_string());
        // token_map：JSON 字符串（legacy Map<String, Long>）
        let token_map_json = user.token_map.as_ref().map(|v| v.to_string());
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO users
                (username, password, salt, token, token_map, enable_webdav, enable_local_store,
                 enable_book_source, enable_rss_source, book_source_limit, book_limit,
                 last_login_at, created_at, user_namespace, raw_json)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
            "#,
        )
        .bind(&user.username)
        .bind(&user.password)
        .bind(&user.salt)
        .bind(&user.token)
        .bind(&token_map_json)
        .bind(user.enable_webdav)
        .bind(user.enable_local_store)
        .bind(user.enable_book_source)
        .bind(user.enable_rss_source)
        .bind(user.book_source_limit)
        .bind(user.book_limit)
        .bind(user.last_login_at)
        .bind(user.created_at)
        .bind(&user.user_namespace)
        .bind(&user.raw_json)
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
        let books: Vec<serde_json::Value> = match serde_json::from_str(&text) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("解析 {} 失败（{}），跳过该命名空间", path.display(), e);
                continue;
            }
        };
        let mut count = 0usize;
        let mut tx = pool.begin().await?;
        for value in books {
            let mut book: Book = match serde_json::from_value(value.clone()) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!("解析书籍失败（{}），跳过", e);
                    continue;
                }
            };
            if book.book_url.trim().is_empty() {
                continue; // 无主键的脏数据跳过
            }
            // raw_json：每本书原始 JSON 全量保底（未知字段不丢）
            book.raw_json = Some(value.to_string());
            book.user_namespace = ns.clone();
            sqlx::query(
                r#"
                INSERT OR REPLACE INTO books
                    (book_url, name, author, origin, origin_name, kind, custom_tag, cover_url,
                     custom_cover_url, intro, custom_intro, charset, type, group_name,
                     latest_chapter_title, latest_chapter_time, last_check_time, last_check_count,
                     total_chapter_num, dur_chapter_title, dur_chapter_index, dur_chapter_pos,
                     dur_chapter_time, word_count, can_update, order_num, origin_order,
                     use_replace_rule, variable, read_config, is_in_shelf, cbz, display_cover,
                     display_intro, local_epub, local_pdf, pdf, split_long_chapter,
                     last_check_error, info_html, toc_html, user_namespace, created_at, raw_json)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                        ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27,
                        ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37, ?38, ?39, ?40,
                        ?41, ?42, ?43, ?44)
                "#,
            )
            .bind(&book.book_url)
            .bind(&book.name)
            .bind(&book.author)
            .bind(&book.origin)
            .bind(&book.origin_name)
            .bind(&book.kind)
            .bind(&book.custom_tag)
            .bind(&book.cover_url)
            .bind(&book.custom_cover_url)
            .bind(&book.intro)
            .bind(&book.custom_intro)
            .bind(&book.charset)
            .bind(book.book_type)
            .bind(book.group)
            .bind(&book.latest_chapter_title)
            .bind(book.latest_chapter_time)
            .bind(book.last_check_time)
            .bind(book.last_check_count)
            .bind(book.total_chapter_num)
            .bind(&book.dur_chapter_title)
            .bind(book.dur_chapter_index)
            .bind(book.dur_chapter_pos)
            .bind(book.dur_chapter_time)
            .bind(&book.word_count)
            .bind(book.can_update)
            .bind(book.order)
            .bind(book.origin_order)
            .bind(book.use_replace_rule)
            .bind(&book.variable)
            .bind(&book.read_config.as_ref().map(|v| v.to_string()))
            .bind(book.is_in_shelf)
            .bind(book.cbz)
            .bind(&book.display_cover)
            .bind(&book.display_intro)
            .bind(&book.local_epub)
            .bind(&book.local_pdf)
            .bind(book.pdf)
            .bind(book.split_long_chapter)
            .bind(&book.last_check_error)
            .bind(&book.info_html)
            .bind(&book.toc_html)
            .bind(&book.user_namespace)
            .bind(0i64) // created_at：迁移数据时间未知，置 0（顺序由 rowid 保持）
            .bind(&book.raw_json)
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

/// 各命名空间 bookSource.json → book_sources 表（全字段 + raw_json 保底）；返回迁移的书源总数
async fn migrate_book_sources(
    pool: &SqlitePool,
    data_dir: &Path,
    namespaces: &[String],
) -> Result<usize> {
    let mut total = 0usize;
    for ns in namespaces {
        let path = data_dir.join(ns).join("bookSource.json");
        if !path.exists() {
            tracing::debug!("{ns} 无 bookSource.json，跳过");
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("读取 {} 失败（{}），跳过该命名空间", path.display(), e);
                continue;
            }
        };
        let sources: Vec<serde_json::Value> = match serde_json::from_str(&text) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("解析 {} 失败（{}），跳过该命名空间", path.display(), e);
                continue;
            }
        };
        let mut count = 0usize;
        let mut tx = pool.begin().await?;
        for value in sources {
            let mut src: BookSource = match serde_json::from_value(value.clone()) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("解析书源失败（{}），跳过", e);
                    continue;
                }
            };
            if src.book_source_url.trim().is_empty() {
                continue; // 无主键的脏数据跳过
            }
            src.raw_json = Some(value.to_string());
            src.user_namespace = ns.clone();
            let val = |v: &Option<serde_json::Value>| v.as_ref().map(|x| x.to_string());
            sqlx::query(
                r#"
                INSERT OR REPLACE INTO book_sources
                    (book_source_url, book_source_name, book_source_group, book_source_type,
                     book_url_pattern, custom_order, enabled, enabled_explore, enabled_cookie_jar,
                     concurrent_rate, header, login_url, login_ui, login_check_js, login_js,
                     book_source_comment, variable_comment, last_update_time, respond_time,
                     weight, explore_url, rule_explore, rule_search, rule_book_info, rule_toc,
                     rule_content, search_rule, explore_rule, book_info_rule, toc_rule,
                     content_rule, key, tag, logger, variable, user_namespace, raw_json)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                        ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29,
                        ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37)
                "#,
            )
            .bind(&src.book_source_url)
            .bind(&src.book_source_name)
            .bind(&src.book_source_group)
            .bind(src.book_source_type)
            .bind(&src.book_url_pattern)
            .bind(src.custom_order)
            .bind(src.enabled)
            .bind(src.enabled_explore)
            .bind(src.enabled_cookie_jar)
            .bind(&src.concurrent_rate)
            .bind(&src.header)
            .bind(&src.login_url)
            .bind(&src.login_ui)
            .bind(&src.login_check_js)
            .bind(&src.login_js)
            .bind(&src.book_source_comment)
            .bind(&src.variable_comment)
            .bind(src.last_update_time)
            .bind(src.respond_time)
            .bind(src.weight)
            .bind(&src.explore_url)
            .bind(&val(&src.rule_explore))
            .bind(&val(&src.rule_search))
            .bind(&val(&src.rule_book_info))
            .bind(&val(&src.rule_toc))
            .bind(&val(&src.rule_content))
            .bind(&val(&src.search_rule))
            .bind(&val(&src.explore_rule))
            .bind(&val(&src.book_info_rule))
            .bind(&val(&src.toc_rule))
            .bind(&val(&src.content_rule))
            .bind(&src.key)
            .bind(&src.tag)
            .bind(&val(&src.logger))
            .bind(&val(&src.variable))
            .bind(&src.user_namespace)
            .bind(&src.raw_json)
            .execute(&mut *tx)
            .await?;
            count += 1;
        }
        tx.commit().await?;
        tracing::info!("迁移书源 [{ns}]：{} 个", count);
        total += count;
    }
    Ok(total)
}
