//! JSON → SQLite 一次性迁移（legacy storage/data → SQLite）
//!
//! 触发条件：`storage/data/users.json` 存在 且 users 表为空。
//! 迁移前自动备份 `storage/data/` → `storage/backup-before-migrate-{ts}/`。
//!
//! 迁移内容：
//! - `storage/data/users.json`（Map<username, User>）→ users 表（user_namespace = username）
//! - `storage/data/{ns}/bookshelf.json`（ns = default 或各用户名）→ books 表（user_namespace = ns）
//! - bookSource.json / rssSource.json → book_sources / rss_sources 表
//! - bookmark.json / replaceRule.json / txtTocRule.json / httpTTS.json / bookGroup.json / userConfig.json
//!   → bookmarks / replace_rules / txt_toc_rules / http_tts_list / book_groups / user_config 表
//! 每类幂等：目标表非空即跳过（表空才迁）；bookmarks 带 raw_json 原文保底（legacy content 不丢）。

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
        tracing::info!(
            "未发现 legacy JSON 数据（{} 不存在），跳过迁移",
            users_path.display()
        );
        return Ok(());
    }
    let user_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&storage.pool)
        .await?;
    if user_count > 0 {
        tracing::info!("users 表已有 {} 条记录，跳过 JSON 迁移", user_count);
        // 补迁书源：book_sources 空且 data 目录有 bookSource.json 时导入（生产数据同步场景）
        let src_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM book_sources")
            .fetch_one(&storage.pool)
            .await?;
        if src_count == 0 {
            let namespaces = scan_source_namespaces(&data_dir);
            if !namespaces.is_empty() {
                match migrate_book_sources(&storage.pool, &data_dir, &namespaces).await {
                    Ok(n) => tracing::info!("补迁书源：{} 个（命名空间 {:?}）", n, namespaces),
                    Err(e) => tracing::warn!("补迁书源失败：{e}"),
                }
            }
        }
        // 补迁 RSS 源：rss_sources 空且 data 目录有 rssSource.json 时导入
        let rss_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rss_sources")
            .fetch_one(&storage.pool)
            .await?;
        if rss_count == 0 {
            let namespaces = scan_rss_namespaces(&data_dir);
            if !namespaces.is_empty() {
                match migrate_rss_sources(&storage.pool, &data_dir, &namespaces).await {
                    Ok(n) => tracing::info!("补迁 RSS 源：{} 个（命名空间 {:?}）", n, namespaces),
                    Err(e) => tracing::warn!("补迁 RSS 源失败：{e}"),
                }
            }
        }
        // 补迁书签：bookmarks 空且 data 目录有 bookmark.json 时导入（函数内幂等：表非空跳过）
        let namespaces = scan_namespaces_for(&data_dir, "bookmark.json");
        if !namespaces.is_empty() {
            match migrate_bookmarks(&storage.pool, &data_dir, &namespaces).await {
                Ok(n) => tracing::info!("补迁书签：{} 条（命名空间 {:?}）", n, namespaces),
                Err(e) => tracing::warn!("补迁书签失败：{e}"),
            }
        }
        // 补迁替换规则：replace_rules 空且 data 目录有 replaceRule.json 时导入
        let namespaces = scan_namespaces_for(&data_dir, "replaceRule.json");
        if !namespaces.is_empty() {
            match migrate_replace_rules(&storage.pool, &data_dir, &namespaces).await {
                Ok(n) => tracing::info!("补迁替换规则：{} 条（命名空间 {:?}）", n, namespaces),
                Err(e) => tracing::warn!("补迁替换规则失败：{e}"),
            }
        }
        // 补迁 TXT 目录规则：txt_toc_rules 空且 data 目录有 txtTocRule.json 时导入
        let namespaces = scan_namespaces_for(&data_dir, "txtTocRule.json");
        if !namespaces.is_empty() {
            match migrate_txt_toc_rules(&storage.pool, &data_dir, &namespaces).await {
                Ok(n) => tracing::info!("补迁 TXT 目录规则：{} 条（命名空间 {:?}）", n, namespaces),
                Err(e) => tracing::warn!("补迁 TXT 目录规则失败：{e}"),
            }
        }
        // 补迁 HttpTTS：http_tts_list 空且 data 目录有 httpTTS.json 时导入
        let namespaces = scan_namespaces_for(&data_dir, "httpTTS.json");
        if !namespaces.is_empty() {
            match migrate_http_tts(&storage.pool, &data_dir, &namespaces).await {
                Ok(n) => tracing::info!("补迁 HttpTTS：{} 个（命名空间 {:?}）", n, namespaces),
                Err(e) => tracing::warn!("补迁 HttpTTS 失败：{e}"),
            }
        }
        // 补迁分组：book_groups 空且 data 目录有 bookGroup.json 时导入
        let namespaces = scan_namespaces_for(&data_dir, "bookGroup.json");
        if !namespaces.is_empty() {
            match migrate_book_groups(&storage.pool, &data_dir, &namespaces).await {
                Ok(n) => tracing::info!("补迁分组：{} 个（命名空间 {:?}）", n, namespaces),
                Err(e) => tracing::warn!("补迁分组失败：{e}"),
            }
        }
        // 补迁用户配置：user_config 空且 data 目录有 userConfig.json 时导入
        let namespaces = scan_namespaces_for(&data_dir, "userConfig.json");
        if !namespaces.is_empty() {
            match migrate_user_configs(&storage.pool, &data_dir, &namespaces).await {
                Ok(n) => tracing::info!("补迁用户配置：{} 项（命名空间 {:?}）", n, namespaces),
                Err(e) => tracing::warn!("补迁用户配置失败：{e}"),
            }
        }
        return Ok(());
    }

    // 1. 迁移前备份 storage/data → storage/backup-before-migrate-{ts}/
    let ts = Utc::now().format("%Y%m%d%H%M%S");
    let backup_dir = storage
        .config
        .storage_dir()
        .join(format!("backup-before-migrate-{ts}"));
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

    // 5. 各命名空间 rssSource.json → rss_sources 表（ns = default + 各用户名）
    let rss_count = migrate_rss_sources(&storage.pool, &data_dir, &namespaces).await?;

    // 6. 各命名空间 bookmark.json → bookmarks 表（legacy 字段 content/time 映射 + raw_json 保底）
    let bookmark_count = migrate_bookmarks(&storage.pool, &data_dir, &namespaces).await?;

    // 7. 各命名空间 replaceRule.json → replace_rules 表（legacy 无 id → uuid）
    let replace_count = migrate_replace_rules(&storage.pool, &data_dir, &namespaces).await?;

    // 8. 各命名空间 txtTocRule.json → txt_toc_rules 表（legacy id 为 Long → 字符串化）
    let txt_toc_count = migrate_txt_toc_rules(&storage.pool, &data_dir, &namespaces).await?;

    // 9. 各命名空间 httpTTS.json → http_tts_list 表（legacy Long id 忽略，url 为主键）
    let http_tts_count = migrate_http_tts(&storage.pool, &data_dir, &namespaces).await?;

    // 10. 各命名空间 bookGroup.json → book_groups 表（legacy id 保留）
    let group_count = migrate_book_groups(&storage.pool, &data_dir, &namespaces).await?;

    // 11. 各命名空间 userConfig.json → user_config 表（{键:值} 对象 或 [{key,value}] 数组）
    let config_count = migrate_user_configs(&storage.pool, &data_dir, &namespaces).await?;

    tracing::info!(
        "JSON→SQLite 迁移完成：{} 个用户，{} 本书，{} 个书源，{} 个 RSS 源，{} 个书签，{} 条替换规则，{} 条 TXT 目录规则，{} 个 HttpTTS，{} 个分组，{} 项用户配置（备份：{}）",
        usernames.len(),
        book_count,
        source_count,
        rss_count,
        bookmark_count,
        replace_count,
        txt_toc_count,
        http_tts_count,
        group_count,
        config_count,
        backup_dir.display()
    );
    Ok(())
}

/// users.json（Map<username, User>）→ users 表（全字段 + raw_json 原文保底）；返回迁移的用户名列表
async fn migrate_users(pool: &SqlitePool, path: &Path) -> Result<Vec<String>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("读取 {} 失败", path.display()))?;
    let user_map: HashMap<String, serde_json::Value> =
        serde_json::from_str(&text).with_context(|| format!("解析 {} 失败", path.display()))?;

    let mut usernames = Vec::with_capacity(user_map.len());
    let mut tx = pool.begin().await?;
    for (key, value) in user_map {
        let mut user: User = match serde_json::from_value(value.clone()) {
            Ok(u) => u,
            Err(e) => {
                tracing::warn!("解析用户 {} 失败（{}），保留 raw_json", key, e);
                User {
                    username: key.clone(),
                    ..Default::default()
                }
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
async fn migrate_bookshelves(
    pool: &SqlitePool,
    data_dir: &Path,
    namespaces: &[String],
) -> Result<usize> {
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
            .bind(book.read_config.as_ref().map(|v| v.to_string()))
            .bind(book.is_in_shelf)
            .bind(book.cbz)
            .bind(&book.display_cover)
            .bind(&book.display_intro)
            .bind(book.local_epub)
            .bind(book.local_pdf)
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
                     weight, explore_url, search_url, rule_explore, rule_search, rule_book_info,
                     rule_toc, rule_content, rule_related, search_rule, explore_rule, book_info_rule, toc_rule,
                     content_rule, key, tag, logger, variable, user_namespace, raw_json)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                        ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29,
                        ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37, ?38, ?39)
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
            .bind(&src.search_url)
            .bind(val(&src.rule_explore))
            .bind(val(&src.rule_search))
            .bind(val(&src.rule_book_info))
            .bind(val(&src.rule_toc))
            .bind(val(&src.rule_content))
            .bind(val(&src.rule_related))
            .bind(val(&src.search_rule))
            .bind(val(&src.explore_rule))
            .bind(val(&src.book_info_rule))
            .bind(val(&src.toc_rule))
            .bind(val(&src.content_rule))
            .bind(&src.key)
            .bind(&src.tag)
            .bind(val(&src.logger))
            .bind(val(&src.variable))
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

/// 扫描 data 目录中含 bookSource.json 的命名空间
fn scan_source_namespaces(data_dir: &Path) -> Vec<String> {
    let mut namespaces = Vec::new();
    if let Ok(entries) = std::fs::read_dir(data_dir) {
        for e in entries.flatten() {
            if !e.path().is_dir() {
                continue;
            }
            if e.path().join("bookSource.json").exists() {
                if let Some(name) = e.file_name().to_str() {
                    namespaces.push(name.to_string());
                }
            }
        }
    }
    namespaces
}

/// 扫描 data 目录中含 rssSource.json 的命名空间
fn scan_rss_namespaces(data_dir: &Path) -> Vec<String> {
    let mut namespaces = Vec::new();
    if let Ok(entries) = std::fs::read_dir(data_dir) {
        for e in entries.flatten() {
            if !e.path().is_dir() {
                continue;
            }
            if e.path().join("rssSource.json").exists() {
                if let Some(name) = e.file_name().to_str() {
                    namespaces.push(name.to_string());
                }
            }
        }
    }
    namespaces
}

/// 各命名空间 rssSource.json → rss_sources 表（raw_json 原文保底）；返回迁移的 RSS 源总数
async fn migrate_rss_sources(
    pool: &SqlitePool,
    data_dir: &Path,
    namespaces: &[String],
) -> Result<usize> {
    let mut total = 0usize;
    for ns in namespaces {
        let path = data_dir.join(ns).join("rssSource.json");
        if !path.exists() {
            tracing::debug!("{ns} 无 rssSource.json，跳过");
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
            let mut src: crate::model::RssSource = match serde_json::from_value(value.clone()) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("解析 RSS 源失败（{}），跳过", e);
                    continue;
                }
            };
            if src.source_url.trim().is_empty() {
                continue; // 无主键的脏数据跳过
            }
            if src.source_name.is_empty() {
                src.source_name = src.source_url.clone();
            }
            src.raw_json = Some(value.to_string());
            src.user_namespace = ns.clone();
            sqlx::query(
                r#"
                INSERT OR REPLACE INTO rss_sources
                    (rss_source_url, rss_source_name, rss_source_group, enabled,
                     user_namespace, raw_json)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
            )
            .bind(&src.source_url)
            .bind(&src.source_name)
            .bind(&src.source_group)
            .bind(src.enabled)
            .bind(&src.user_namespace)
            .bind(&src.raw_json)
            .execute(&mut *tx)
            .await?;
            count += 1;
        }
        tx.commit().await?;
        tracing::info!("迁移 RSS 源 [{ns}]：{} 个", count);
        total += count;
    }
    Ok(total)
}

/// 扫描 data 目录中含指定 legacy 文件的命名空间
fn scan_namespaces_for(data_dir: &Path, file: &str) -> Vec<String> {
    let mut namespaces = Vec::new();
    if let Ok(entries) = std::fs::read_dir(data_dir) {
        for e in entries.flatten() {
            if !e.path().is_dir() {
                continue;
            }
            if e.path().join(file).exists() {
                if let Some(name) = e.file_name().to_str() {
                    namespaces.push(name.to_string());
                }
            }
        }
    }
    namespaces
}

/// 各命名空间 bookmark.json（legacy：[{bookUrl,title,chapterIndex,content,time}]）
/// → bookmarks 表；raw_json 原文保底（legacy content 无对应列，不丢）。
/// 幂等：bookmarks 表非空即跳过（表空才迁）。
async fn migrate_bookmarks(
    pool: &SqlitePool,
    data_dir: &Path,
    namespaces: &[String],
) -> Result<usize> {
    let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bookmarks")
        .fetch_one(pool)
        .await?;
    if existing > 0 {
        tracing::info!("bookmarks 表已有 {} 条记录，跳过书签迁移", existing);
        return Ok(0);
    }
    let mut total = 0usize;
    for ns in namespaces {
        let path = data_dir.join(ns).join("bookmark.json");
        if !path.exists() {
            tracing::debug!("{ns} 无 bookmark.json，跳过");
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("读取 {} 失败（{}），跳过该命名空间", path.display(), e);
                continue;
            }
        };
        let items: Vec<serde_json::Value> = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("解析 {} 失败（{}），跳过该命名空间", path.display(), e);
                continue;
            }
        };
        let mut count = 0usize;
        let mut tx = pool.begin().await?;
        for value in items {
            let get_str = |k: &str| {
                value
                    .get(k)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let book_url = get_str("bookUrl");
            let title = get_str("title");
            if book_url.trim().is_empty() || title.trim().is_empty() {
                tracing::warn!("书签缺 bookUrl/title，跳过");
                continue;
            }
            // legacy：content(文本)/time(时间戳)；新格式：paragraphIndex/createdAt——两者兼容
            let paragraph_index = value
                .get("paragraphIndex")
                .and_then(|v| v.as_i64())
                .or_else(|| value.get("content").and_then(|v| v.as_i64()))
                .unwrap_or(0);
            let chapter_index = value
                .get("chapterIndex")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let created_at = value
                .get("createdAt")
                .and_then(|v| v.as_i64())
                .or_else(|| value.get("time").and_then(|v| v.as_i64()))
                .unwrap_or(0);
            sqlx::query(
                r#"
                INSERT OR REPLACE INTO bookmarks
                    (book_url, title, paragraph_index, chapter_index, created_at, user_namespace, raw_json)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                "#,
            )
            .bind(&book_url)
            .bind(&title)
            .bind(paragraph_index)
            .bind(chapter_index)
            .bind(created_at)
            .bind(ns)
            .bind(value.to_string())
            .execute(&mut *tx)
            .await?;
            count += 1;
        }
        tx.commit().await?;
        tracing::info!("迁移书签 [{ns}]：{} 条", count);
        total += count;
    }
    Ok(total)
}

/// 各命名空间 replaceRule.json（legacy：[{name,find,replace,enabled,order}]，无 id）
/// → replace_rules 表；legacy 无 id → 补 uuid（对齐 saveReplaceRule/restore 语义）。
/// 幂等：replace_rules 表非空即跳过（表空才迁）。
async fn migrate_replace_rules(
    pool: &SqlitePool,
    data_dir: &Path,
    namespaces: &[String],
) -> Result<usize> {
    let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM replace_rules")
        .fetch_one(pool)
        .await?;
    if existing > 0 {
        tracing::info!("replace_rules 表已有 {} 条记录，跳过替换规则迁移", existing);
        return Ok(0);
    }
    let mut total = 0usize;
    for ns in namespaces {
        let path = data_dir.join(ns).join("replaceRule.json");
        if !path.exists() {
            tracing::debug!("{ns} 无 replaceRule.json，跳过");
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("读取 {} 失败（{}），跳过该命名空间", path.display(), e);
                continue;
            }
        };
        let items: Vec<serde_json::Value> = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("解析 {} 失败（{}），跳过该命名空间", path.display(), e);
                continue;
            }
        };
        let mut count = 0usize;
        let mut tx = pool.begin().await?;
        for value in items {
            let get_str = |k: &str| {
                value
                    .get(k)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let name = get_str("name");
            let find = get_str("find");
            if name.trim().is_empty() || find.trim().is_empty() {
                tracing::warn!("替换规则缺 name/find，跳过");
                continue;
            }
            // legacy 无 id（或有数字 id）→ 统一字符串 id，缺失补 uuid
            let mut id = match value.get("id") {
                Some(v) => match v.as_str() {
                    Some(s) => s.to_string(),
                    None => v.to_string(),
                },
                None => String::new(),
            };
            if id.trim().is_empty() {
                id = uuid::Uuid::new_v4().simple().to_string();
            }
            // enabled/enable、order/orderNum 双兼容（legacy 变体）
            let enabled = value
                .get("enabled")
                .or_else(|| value.get("enable"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let order = value
                .get("order")
                .or_else(|| value.get("orderNum"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            sqlx::query(
                r#"
                INSERT OR REPLACE INTO replace_rules
                    (id, name, find, replace, enable, order_num, user_namespace)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                "#,
            )
            .bind(&id)
            .bind(&name)
            .bind(&find)
            .bind(get_str("replace"))
            .bind(enabled)
            .bind(order)
            .bind(ns)
            .execute(&mut *tx)
            .await?;
            count += 1;
        }
        tx.commit().await?;
        tracing::info!("迁移替换规则 [{ns}]：{} 条", count);
        total += count;
    }
    Ok(total)
}

/// 各命名空间 txtTocRule.json（legacy：[{id(Long),name,rule,serialNumber,enable}]）
/// → txt_toc_rules 表；legacy id 为 Long → 字符串化，缺失补 uuid。
/// 幂等：txt_toc_rules 表非空即跳过（表空才迁）。
async fn migrate_txt_toc_rules(
    pool: &SqlitePool,
    data_dir: &Path,
    namespaces: &[String],
) -> Result<usize> {
    let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM txt_toc_rules")
        .fetch_one(pool)
        .await?;
    if existing > 0 {
        tracing::info!(
            "txt_toc_rules 表已有 {} 条记录，跳过 TXT 目录规则迁移",
            existing
        );
        return Ok(0);
    }
    let mut total = 0usize;
    for ns in namespaces {
        let path = data_dir.join(ns).join("txtTocRule.json");
        if !path.exists() {
            tracing::debug!("{ns} 无 txtTocRule.json，跳过");
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("读取 {} 失败（{}），跳过该命名空间", path.display(), e);
                continue;
            }
        };
        let items: Vec<serde_json::Value> = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("解析 {} 失败（{}），跳过该命名空间", path.display(), e);
                continue;
            }
        };
        let mut count = 0usize;
        let mut tx = pool.begin().await?;
        for value in items {
            let get_str = |k: &str| {
                value
                    .get(k)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let name = get_str("name");
            let rule = get_str("rule");
            if name.trim().is_empty() || rule.trim().is_empty() {
                tracing::warn!("TXT 目录规则缺 name/rule，跳过");
                continue;
            }
            // legacy id 为 Long（数字）→ 字符串化；缺失补 uuid
            let mut id = match value.get("id") {
                Some(v) => match v.as_str() {
                    Some(s) => s.to_string(),
                    None => v.to_string(),
                },
                None => String::new(),
            };
            if id.trim().is_empty() {
                id = uuid::Uuid::new_v4().simple().to_string();
            }
            // enable/enabled 双兼容（legacy 变体）
            let enable = value
                .get("enable")
                .or_else(|| value.get("enabled"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let serial_number = value
                .get("serialNumber")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            sqlx::query(
                r#"
                INSERT OR REPLACE INTO txt_toc_rules
                    (id, name, rule, enable, serial_number, user_namespace)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
            )
            .bind(&id)
            .bind(&name)
            .bind(&rule)
            .bind(enable)
            .bind(serial_number)
            .bind(ns)
            .execute(&mut *tx)
            .await?;
            count += 1;
        }
        tx.commit().await?;
        tracing::info!("迁移 TXT 目录规则 [{ns}]：{} 条", count);
        total += count;
    }
    Ok(total)
}

/// 各命名空间 httpTTS.json（legacy：[{id(Long),name,url,type}]）
/// → http_tts_list 表（url 主键；legacy Long id 忽略——与模型 HttpTts 一致）。
/// 幂等：http_tts_list 表非空即跳过（表空才迁）。
async fn migrate_http_tts(
    pool: &SqlitePool,
    data_dir: &Path,
    namespaces: &[String],
) -> Result<usize> {
    let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM http_tts_list")
        .fetch_one(pool)
        .await?;
    if existing > 0 {
        tracing::info!(
            "http_tts_list 表已有 {} 条记录，跳过 HttpTTS 迁移",
            existing
        );
        return Ok(0);
    }
    let mut total = 0usize;
    for ns in namespaces {
        let path = data_dir.join(ns).join("httpTTS.json");
        if !path.exists() {
            tracing::debug!("{ns} 无 httpTTS.json，跳过");
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("读取 {} 失败（{}），跳过该命名空间", path.display(), e);
                continue;
            }
        };
        let items: Vec<serde_json::Value> = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("解析 {} 失败（{}），跳过该命名空间", path.display(), e);
                continue;
            }
        };
        let mut count = 0usize;
        let mut tx = pool.begin().await?;
        for value in items {
            let get_str = |k: &str| {
                value
                    .get(k)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let url = get_str("url");
            let name = get_str("name");
            if url.trim().is_empty() || name.trim().is_empty() {
                tracing::warn!("HttpTTS 缺 url/name，跳过");
                continue;
            }
            let tts_type = value.get("type").and_then(|v| v.as_i64()).unwrap_or(0);
            sqlx::query(
                r#"
                INSERT OR REPLACE INTO http_tts_list
                    (url, name, type, user_namespace)
                VALUES (?1, ?2, ?3, ?4)
                "#,
            )
            .bind(&url)
            .bind(&name)
            .bind(tts_type)
            .bind(ns)
            .execute(&mut *tx)
            .await?;
            count += 1;
        }
        tx.commit().await?;
        tracing::info!("迁移 HttpTTS [{ns}]：{} 个", count);
        total += count;
    }
    Ok(total)
}

/// 各命名空间 bookGroup.json（legacy：[{id,name,order}]）
/// → book_groups 表（legacy id 保留，books.group_name 引用不变；id<=0 时自增）。
/// 幂等：book_groups 表非空即跳过（表空才迁）。
async fn migrate_book_groups(
    pool: &SqlitePool,
    data_dir: &Path,
    namespaces: &[String],
) -> Result<usize> {
    let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM book_groups")
        .fetch_one(pool)
        .await?;
    if existing > 0 {
        tracing::info!("book_groups 表已有 {} 条记录，跳过分组迁移", existing);
        return Ok(0);
    }
    let mut total = 0usize;
    for ns in namespaces {
        let path = data_dir.join(ns).join("bookGroup.json");
        if !path.exists() {
            tracing::debug!("{ns} 无 bookGroup.json，跳过");
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("读取 {} 失败（{}），跳过该命名空间", path.display(), e);
                continue;
            }
        };
        let items: Vec<serde_json::Value> = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("解析 {} 失败（{}），跳过该命名空间", path.display(), e);
                continue;
            }
        };
        let mut count = 0usize;
        let mut tx = pool.begin().await?;
        for value in items {
            let name = value
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if name.trim().is_empty() {
                tracing::warn!("分组缺 name，跳过");
                continue;
            }
            let id = value.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            let order = value.get("order").and_then(|v| v.as_i64()).unwrap_or(0);
            sqlx::query(
                r#"
                INSERT OR REPLACE INTO book_groups
                    (id, name, order_num, user_namespace)
                VALUES (?1, ?2, ?3, ?4)
                "#,
            )
            .bind(id)
            .bind(&name)
            .bind(order)
            .bind(ns)
            .execute(&mut *tx)
            .await?;
            count += 1;
        }
        tx.commit().await?;
        tracing::info!("迁移分组 [{ns}]：{} 个", count);
        total += count;
    }
    Ok(total)
}

/// 各命名空间 userConfig.json（{键: 值} 对象 或 [{key,value}] 数组）
/// → user_config 表（(user_namespace, ns) 双主键；字符串值原样存，其余 JSON 序列化）。
/// 幂等：user_config 表非空即跳过（表空才迁）。
async fn migrate_user_configs(
    pool: &SqlitePool,
    data_dir: &Path,
    namespaces: &[String],
) -> Result<usize> {
    let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_config")
        .fetch_one(pool)
        .await?;
    if existing > 0 {
        tracing::info!("user_config 表已有 {} 条记录，跳过用户配置迁移", existing);
        return Ok(0);
    }
    let mut total = 0usize;
    for ns in namespaces {
        let path = data_dir.join(ns).join("userConfig.json");
        if !path.exists() {
            tracing::debug!("{ns} 无 userConfig.json，跳过");
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("读取 {} 失败（{}），跳过该命名空间", path.display(), e);
                continue;
            }
        };
        let value: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("解析 {} 失败（{}），跳过该命名空间", path.display(), e);
                continue;
            }
        };
        let mut count = 0usize;
        let mut tx = pool.begin().await?;
        let raw_of = |v: &serde_json::Value| match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        match value {
            serde_json::Value::Object(map) => {
                for (key, v) in map {
                    sqlx::query(
                        r#"
                        INSERT OR REPLACE INTO user_config (user_namespace, ns, config, updated_at)
                        VALUES (?1, ?2, ?3, 0)
                        "#,
                    )
                    .bind(ns)
                    .bind(&key)
                    .bind(raw_of(&v))
                    .execute(&mut *tx)
                    .await?;
                    count += 1;
                }
            }
            serde_json::Value::Array(arr) => {
                for item in arr {
                    let Some(key) = item.get("key").and_then(|k| k.as_str()) else {
                        tracing::warn!("用户配置数组项缺 key，跳过");
                        continue;
                    };
                    let raw = match item.get("value") {
                        Some(v) => raw_of(v),
                        None => String::new(),
                    };
                    sqlx::query(
                        r#"
                        INSERT OR REPLACE INTO user_config (user_namespace, ns, config, updated_at)
                        VALUES (?1, ?2, ?3, 0)
                        "#,
                    )
                    .bind(ns)
                    .bind(key)
                    .bind(raw)
                    .execute(&mut *tx)
                    .await?;
                    count += 1;
                }
            }
            _ => {
                tracing::warn!("{} 既非对象也非数组，跳过该命名空间", path.display());
                tx.rollback().await?;
                continue;
            }
        }
        tx.commit().await?;
        tracing::info!("迁移用户配置 [{ns}]：{} 项", count);
        total += count;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Bookmark;
    use crate::storage::init;
    use crate::AppConfig;

    /// 独立临时目录（避免污染真实 storage/reader.db）
    fn test_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("reader-migrate-test-{}-{tag}", std::process::id()))
    }

    /// 构造最小 legacy data 目录（users.json + 各命名空间 legacy 文件）
    fn write_legacy_files(data_dir: &Path) {
        let default = data_dir.join("default");
        std::fs::create_dir_all(&default).unwrap();
        std::fs::create_dir_all(data_dir.join("alice")).unwrap();
        // 书签：legacy 字段 bookUrl/title/chapterIndex/content/time（任务规格）
        std::fs::write(
            default.join("bookmark.json"),
            r#"[
                {"bookUrl":"https://a.com/book/1","title":"第一章 起点","chapterIndex":1,"content":"这是书签内容","time":1700000000000},
                {"bookUrl":"https://a.com/book/1","title":"第二章 转折","chapterIndex":2,"content":"第二个书签","time":1700000001000}
            ]"#,
        )
        .unwrap();
        // 替换规则：legacy 无 id
        std::fs::write(
            default.join("replaceRule.json"),
            r#"[
                {"name":"去广告","find":"广告","replace":"","enabled":true,"order":1},
                {"name":"净化","find":"旧排版","replace":"新排版","enabled":false,"order":2}
            ]"#,
        )
        .unwrap();
        // TXT 目录规则：legacy id 为 Long（数字）
        std::fs::write(
            default.join("txtTocRule.json"),
            r#"[
                {"id":1,"name":"章节","rule":"^第.+章$","serialNumber":0,"enable":true},
                {"id":2,"name":"卷","rule":"^卷.+","serialNumber":1,"enable":false}
            ]"#,
        )
        .unwrap();
        // HttpTTS：legacy 含 Long id（模型忽略，url 为主键）
        std::fs::write(
            default.join("httpTTS.json"),
            r#"[
                {"id":1,"name":"在线TTS","url":"https://tts.example.com/synth","type":0},
                {"id":2,"name":"本地引擎","url":"local://engine","type":1}
            ]"#,
        )
        .unwrap();
        // 分组：legacy id 保留
        std::fs::write(
            default.join("bookGroup.json"),
            r#"[
                {"id":1,"name":"玄幻","order":0},
                {"id":2,"name":"言情","order":1}
            ]"#,
        )
        .unwrap();
        // 用户配置：default 用 {键:值} 对象；alice 用 [{key,value}] 数组
        std::fs::write(
            default.join("userConfig.json"),
            r#"{"readerSetting":"{\"fontSize\":18}","theme":"dark"}"#,
        )
        .unwrap();
        std::fs::write(
            data_dir.join("alice/userConfig.json"),
            r#"[{"key":"font","value":"16"}]"#,
        )
        .unwrap();
    }

    /// 初始化存储（init 会自动执行 migrate_if_needed）
    async fn setup(tag: &str, with_legacy: bool) -> Storage {
        let dir = test_dir(tag);
        let _ = std::fs::remove_dir_all(&dir);
        let data = dir.join("storage/data");
        std::fs::create_dir_all(data.join("default")).unwrap();
        std::fs::write(
            data.join("users.json"),
            r#"{"alice":{"username":"alice","enableLocalStore":true}}"#,
        )
        .unwrap();
        if with_legacy {
            write_legacy_files(&data);
        }
        let mut config = AppConfig::from_env();
        config.work_dir = dir.to_string_lossy().into_owned();
        init(&config).await.expect("存储初始化失败")
    }

    async fn cleanup(storage: Storage, tag: &str) {
        storage.pool.close().await;
        let _ = std::fs::remove_dir_all(test_dir(tag));
    }

    async fn count(pool: &SqlitePool, table: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// 六类 legacy 文件全量迁移 + 字段映射断言
    #[tokio::test]
    async fn test_migrate_legacy_all_types() {
        let storage = setup("all", true).await;
        let pool = &storage.pool;

        // 用户 + 各表行数
        assert_eq!(count(pool, "users").await, 1);
        assert_eq!(count(pool, "bookmarks").await, 2);
        assert_eq!(count(pool, "replace_rules").await, 2);
        assert_eq!(count(pool, "txt_toc_rules").await, 2);
        assert_eq!(count(pool, "http_tts_list").await, 2);
        assert_eq!(count(pool, "book_groups").await, 2);
        assert_eq!(count(pool, "user_config").await, 3); // default 2 + alice 1

        // 书签：legacy content/time → raw_json 保底；paragraph_index 无对应 → 0
        let (chapter_index, created_at, paragraph_index): (i64, i64, i64) = sqlx::query_as(
            "SELECT chapter_index, created_at, paragraph_index FROM bookmarks WHERE book_url = ?1 AND title = ?2",
        )
        .bind("https://a.com/book/1")
        .bind("第一章 起点")
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(chapter_index, 1);
        assert_eq!(created_at, 1700000000000);
        assert_eq!(paragraph_index, 0);
        let raw: String = sqlx::query_scalar("SELECT raw_json FROM bookmarks WHERE title = ?1")
            .bind("第一章 起点")
            .fetch_one(pool)
            .await
            .unwrap();
        assert!(
            raw.contains("这是书签内容"),
            "legacy content 应保底在 raw_json: {raw}"
        );
        let ns: String =
            sqlx::query_scalar("SELECT user_namespace FROM bookmarks WHERE title = ?1")
                .bind("第一章 起点")
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(ns, "default");

        // 替换规则：legacy 无 id → 补 uuid；enabled/order 映射 enable/order_num
        let id: String = sqlx::query_scalar("SELECT id FROM replace_rules WHERE name = ?1")
            .bind("去广告")
            .fetch_one(pool)
            .await
            .unwrap();
        assert!(!id.is_empty(), "legacy 无 id 应补 uuid");
        let (find, enable, order_num): (String, i64, i64) =
            sqlx::query_as("SELECT find, enable, order_num FROM replace_rules WHERE name = ?1")
                .bind("净化")
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(find, "旧排版");
        assert_eq!((enable, order_num), (0, 2));

        // TXT 目录规则：legacy Long id → 字符串化
        let (id, serial_number, enable): (String, i64, i64) =
            sqlx::query_as("SELECT id, serial_number, enable FROM txt_toc_rules WHERE name = ?1")
                .bind("章节")
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(id, "1");
        assert_eq!((serial_number, enable), (0, 1));

        // HttpTTS：url 主键 + type 映射
        let (name, tts_type): (String, i64) =
            sqlx::query_as("SELECT name, type FROM http_tts_list WHERE url = ?1")
                .bind("https://tts.example.com/synth")
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(name, "在线TTS");
        assert_eq!(tts_type, 0);

        // 分组：legacy id 保留
        let (id, order_num): (i64, i64) =
            sqlx::query_as("SELECT id, order_num FROM book_groups WHERE name = ?1")
                .bind("玄幻")
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!((id, order_num), (1, 0));

        // 用户配置：对象 map（default）+ 数组（alice）
        let config: String = sqlx::query_scalar(
            "SELECT config FROM user_config WHERE user_namespace = ?1 AND ns = ?2",
        )
        .bind("default")
        .bind("readerSetting")
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(config, r#"{"fontSize":18}"#);
        let config: String = sqlx::query_scalar(
            "SELECT config FROM user_config WHERE user_namespace = ?1 AND ns = ?2",
        )
        .bind("alice")
        .bind("font")
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(config, "16");

        cleanup(storage, "all").await;
    }

    /// 幂等：重复执行迁移不产生重复数据（表非空即跳过）
    #[tokio::test]
    async fn test_migrate_idempotent() {
        let storage = setup("idem", true).await;
        migrate_if_needed(&storage).await.unwrap();
        migrate_if_needed(&storage).await.unwrap();
        let pool = &storage.pool;
        assert_eq!(count(pool, "users").await, 1);
        assert_eq!(count(pool, "bookmarks").await, 2);
        assert_eq!(count(pool, "replace_rules").await, 2);
        assert_eq!(count(pool, "txt_toc_rules").await, 2);
        assert_eq!(count(pool, "http_tts_list").await, 2);
        assert_eq!(count(pool, "book_groups").await, 2);
        assert_eq!(count(pool, "user_config").await, 3);
        cleanup(storage, "idem").await;
    }

    /// 表非空才迁：bookmarks 已有数据 → 跳过书签迁移，其余类型照常补迁
    #[tokio::test]
    async fn test_migrate_skips_nonempty_table() {
        let storage = setup("skip", false).await; // 无 legacy 文件：迁移空转
        storage
            .save_bookmark(
                "default",
                &Bookmark {
                    book_url: "https://b.com/book".into(),
                    title: "已有书签".into(),
                    created_at: 1,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        // 写入 legacy 文件后手动再跑迁移（users 非空 → 补迁分支）
        write_legacy_files(&storage.config.storage_dir().join("data"));
        migrate_if_needed(&storage).await.unwrap();
        let pool = &storage.pool;
        assert_eq!(
            count(pool, "bookmarks").await,
            1,
            "bookmarks 非空应跳过迁移"
        );
        let title: String = sqlx::query_scalar("SELECT title FROM bookmarks")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(title, "已有书签", "不应混入 legacy 书签");
        assert_eq!(count(pool, "replace_rules").await, 2);
        assert_eq!(count(pool, "txt_toc_rules").await, 2);
        assert_eq!(count(pool, "http_tts_list").await, 2);
        assert_eq!(count(pool, "book_groups").await, 2);
        assert_eq!(count(pool, "user_config").await, 3);
        cleanup(storage, "skip").await;
    }
}
