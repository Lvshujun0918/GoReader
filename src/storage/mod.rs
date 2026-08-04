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

/// 缓存统计（getCacheInfo）
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheInfo {
    /// toc_cache 行数
    pub toc_cache_count: i64,
    /// toc_cache 近似大小（sum length(chapters_json)）
    pub toc_cache_size: i64,
    /// book_chapters 行数
    pub chapter_count: i64,
    /// 章节缓存近似大小（sum length(content)）
    pub chapter_size: i64,
    /// 总大小（目录缓存 + 章节缓存）
    pub total_size: i64,
}

/// 全书搜索命中（searchBookContent）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookContentHit {
    pub chapter_index: i64,
    pub title: String,
    /// 命中段落前后截取的摘要
    pub snippet: String,
}

/// 阅读统计单书汇总（getReadingStats.books[]）
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadingStatBook {
    pub book_url: String,
    /// 书名（书架 books 表关联；不在书架时用 bookUrl）
    pub name: String,
    /// 累计阅读秒数
    pub seconds: i64,
    /// 累计阅读字数
    pub chars: i64,
}

/// 阅读统计（getReadingStats：今日/本周/总计 + 单书汇总）
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadingStats {
    /// 今日阅读秒数
    pub today: i64,
    /// 近 7 天阅读秒数
    pub week: i64,
    /// 累计阅读秒数
    pub total: i64,
    /// 单书汇总（按秒数降序）
    pub books: Vec<ReadingStatBook>,
}

/// 命中摘要：定位 key 首次出现位置（大小写不敏感），取所在段落 + 前后各 radius 字符，
/// 截断处补省略号、换行压平为空格
fn make_snippet(content: &str, key: &str, radius: usize) -> String {
    let lower = content.to_lowercase();
    let key_lower = key.to_lowercase();
    let Some(pos) = lower.find(&key_lower) else {
        return String::new();
    };
    // 对齐 UTF-8 字符边界（lowercase 极端情形下字节偏移可能漂移）
    let pos = floor_char_boundary(content, pos);
    // 段落边界（最近的前后换行）
    let para_start = content[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let para_end = content[pos..]
        .find('\n')
        .map(|i| pos + i)
        .unwrap_or(content.len());
    let start = para_start.max(pos.saturating_sub(radius));
    let end = (pos + key.len() + radius).min(para_end);
    let start = floor_char_boundary(content, start);
    let end = floor_char_boundary(content, end);
    let mut s = String::new();
    if start > para_start {
        s.push('…');
    }
    s.push_str(&content[start..end]);
    if end < para_end {
        s.push('…');
    }
    s.replace('\n', " ")
}

/// 向左对齐到最近的 UTF-8 字符边界（O(3) 步内收敛）
fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}


/// 初始化：建目录 + 打开/建库 + 建表
pub async fn init(config: &AppConfig) -> Result<Storage> {
    let storage_dir = config.storage_dir();
    std::fs::create_dir_all(&storage_dir)?;
    let db_path = storage_dir.join("reader.db");

    let options = SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(true)
        // GAP 96：WAL 模式（并发读写不互斥——默认 DELETE journal 下读会阻塞写/写会阻塞读）
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        // GAP 96：busy_timeout（锁等待 5s，避免并发写瞬时 SQLITE_BUSY 报错）
        .busy_timeout(std::time::Duration::from_secs(5))
        // sqlx-sqlite 0.7.4 已知缺陷：语句缓存 + 建表/ALTER 类 DDL 并发时，
        // sqlite 自动重准备后的列数（column_count）与缓存的列元数据不一致 →
        // SqliteRow::current 越界 panic（row.rs:43，见 storage 测试偶发失败）。
        // 本库规模下准备开销可忽略，禁用缓存彻底规避。
        .statement_cache_capacity(0);
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
            token_map TEXT,
            enable_webdav INTEGER DEFAULT 0,
            enable_local_store INTEGER DEFAULT 0,
            enable_book_source INTEGER DEFAULT 1,
            enable_rss_source INTEGER DEFAULT 1,
            book_source_limit INTEGER DEFAULT 0,
            book_limit INTEGER DEFAULT 0,
            last_login_at INTEGER DEFAULT 0,
            created_at INTEGER DEFAULT 0,
            user_namespace TEXT DEFAULT '',
            raw_json TEXT
        );
        "#,
    )
    .execute(&pool)
    .await?;

    // OPDS 独立账号等系统设置（键值表）
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS system_settings (
            key TEXT PRIMARY KEY,
            value TEXT,
            updated_at INTEGER DEFAULT 0
        );
        "#,
    )
    .execute(&pool)
    .await?;

    // 书源登录态 cookie（按用户隔离：user_namespace + source_url 联合主键）
    // user_agent 列：FlareSolverr 返回的 userAgent 与库中不同时一并记录（部分站点 UA 绑定 cookie）
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS book_source_cookies (
            user_namespace TEXT NOT NULL,
            source_url TEXT NOT NULL,
            cookie TEXT NOT NULL DEFAULT '',
            user_agent TEXT NOT NULL DEFAULT '',
            updated_at INTEGER DEFAULT 0,
            PRIMARY KEY (user_namespace, source_url)
        );
        "#,
    )
    .execute(&pool)
    .await?;

    // 兼容旧库：books 表缺 user_namespace 列时补列
    let cols: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('users')")
        .fetch_all(&pool)
        .await?;
    if !cols.iter().any(|c| c == "user_namespace") {
        sqlx::query("ALTER TABLE users ADD COLUMN user_namespace TEXT DEFAULT ''")
            .execute(&pool)
            .await?;
        tracing::info!("users 表补充 user_namespace 列");
    }

    // 兼容旧库：books.local_epub/local_pdf 曾被声明为 TEXT（Book 模型为 bool），
    // TEXT 亲和性会把 bool 存成文本 '0'/'1'，读取时 bool 解码失败（saveBook/进度/书架读回依赖）。
    // 检测到 TEXT 类型则重建表为 INTEGER（幂等，仅执行一次）。
    let epub_col_type: Option<String> = sqlx::query_scalar(
        "SELECT type FROM pragma_table_info('books') WHERE name = 'local_epub'",
    )
    .fetch_optional(&pool)
    .await?;
    if epub_col_type.as_deref() == Some("TEXT") {
        rebuild_books_bool_columns(&pool).await?;
        tracing::info!("books 表重建：local_epub/local_pdf TEXT → INTEGER");
    }

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS books (
            book_url TEXT,
            name TEXT DEFAULT '',
            author TEXT DEFAULT '',
            origin TEXT DEFAULT '',
            origin_name TEXT DEFAULT '',
            toc_url TEXT DEFAULT '',
            kind TEXT,
            custom_tag TEXT,
            cover_url TEXT,
            custom_cover_url TEXT,
            intro TEXT,
            custom_intro TEXT,
            charset TEXT,
            type INTEGER DEFAULT 0,
            group_name INTEGER DEFAULT 0,
            latest_chapter_title TEXT,
            latest_chapter_time INTEGER DEFAULT 0,
            last_check_time INTEGER DEFAULT 0,
            last_check_count INTEGER DEFAULT 0,
            total_chapter_num INTEGER DEFAULT 0,
            dur_chapter_title TEXT,
            dur_chapter_index INTEGER DEFAULT 0,
            dur_chapter_pos INTEGER DEFAULT 0,
            dur_chapter_time INTEGER DEFAULT 0,
            word_count TEXT,
            can_update INTEGER DEFAULT 1,
            order_num INTEGER DEFAULT 0,
            origin_order INTEGER DEFAULT 0,
            use_replace_rule INTEGER DEFAULT 1,
            variable TEXT,
            read_config TEXT,
            is_in_shelf INTEGER DEFAULT 1,
            cbz INTEGER DEFAULT 0,
            display_cover TEXT,
            display_intro TEXT,
            local_epub INTEGER DEFAULT 0,
            local_pdf INTEGER DEFAULT 0,
            pdf INTEGER DEFAULT 0,
            split_long_chapter INTEGER DEFAULT 0,
            last_check_error TEXT,
            info_html TEXT,
            toc_html TEXT,
            user_namespace TEXT DEFAULT '',
            created_at INTEGER DEFAULT 0,
            raw_json TEXT,
            local_file TEXT,
            local_file_mtime INTEGER DEFAULT 0,
            local_file_size INTEGER DEFAULT 0,
            local_file_deleted INTEGER DEFAULT 0,
            PRIMARY KEY (book_url, user_namespace)
        );
        "#,
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS book_sources (
            book_source_url TEXT PRIMARY KEY,
            book_source_name TEXT DEFAULT '',
            book_source_group TEXT,
            book_source_type INTEGER DEFAULT 0,
            book_url_pattern TEXT,
            custom_order INTEGER DEFAULT 0,
            enabled INTEGER DEFAULT 1,
            enabled_explore INTEGER DEFAULT 1,
            enabled_cookie_jar INTEGER,
            concurrent_rate TEXT,
            header TEXT,
            login_url TEXT,
            login_ui TEXT,
            login_check_js TEXT,
            login_js TEXT,
            book_source_comment TEXT,
            variable_comment TEXT,
            last_update_time INTEGER DEFAULT 0,
            respond_time INTEGER DEFAULT 0,
            weight INTEGER DEFAULT 0,
            explore_url TEXT,
            search_url TEXT,
            rule_explore TEXT,
            rule_search TEXT,
            rule_book_info TEXT,
            rule_toc TEXT,
            rule_content TEXT,
            rule_related TEXT,
            search_rule TEXT,
            explore_rule TEXT,
            book_info_rule TEXT,
            toc_rule TEXT,
            content_rule TEXT,
            key TEXT,
            tag TEXT,
            logger TEXT,
            variable TEXT,
            user_namespace TEXT DEFAULT '',
            raw_json TEXT
        );
        "#,
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS rss_sources (
            rss_source_url TEXT PRIMARY KEY,
            rss_source_name TEXT DEFAULT '',
            rss_source_group TEXT,
            enabled INTEGER DEFAULT 1,
            user_namespace TEXT DEFAULT '',
            raw_json TEXT
        );
        "#,
    )
    .execute(&pool)
    .await?;

    // RSS 文章缓存（url 主键；content 为 feed 正文/摘要或抓取网页提取的正文）
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS rss_articles (
            url TEXT PRIMARY KEY,
            source_url TEXT DEFAULT '',
            title TEXT DEFAULT '',
            author TEXT DEFAULT '',
            time INTEGER DEFAULT 0,
            content TEXT,
            cover TEXT,
            read INTEGER DEFAULT 0,
            user_namespace TEXT DEFAULT ''
        );
        "#,
    )
    .execute(&pool)
    .await?;

    // 兼容旧库：rss_articles 缺 read 列时补列（幂等——已存在则跳过）
    let rss_cols: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('rss_articles')")
        .fetch_all(&pool)
        .await?;
    if !rss_cols.iter().any(|c| c == "read") {
        sqlx::query("ALTER TABLE rss_articles ADD COLUMN read INTEGER DEFAULT 0")
            .execute(&pool)
            .await?;
        tracing::info!("rss_articles 表补充 read 列");
    }

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS book_chapters (
            book_url TEXT NOT NULL,
            chapter_index INTEGER NOT NULL,
            title TEXT DEFAULT '',
            content TEXT,
            PRIMARY KEY (book_url, chapter_index)
        );
        "#,
    )
    .execute(&pool)
    .await?;

    // F-10：目录缓存（getBookToc 成功落盘，TTL 5 分钟；book_url 为主键，toc_url 供“同 tocUrl 直读缓存”查找）
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS toc_cache (
            book_url TEXT PRIMARY KEY,
            toc_url TEXT DEFAULT '',
            chapters_json TEXT,
            updated_at INTEGER DEFAULT 0
        );
        "#,
    )
    .execute(&pool)
    .await?;

    // 书签（任务规格：PRIMARY KEY (book_url, title)）
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS bookmarks (
            book_url TEXT NOT NULL,
            title TEXT NOT NULL DEFAULT '',
            paragraph_index INTEGER DEFAULT 0,
            chapter_index INTEGER DEFAULT 0,
            created_at INTEGER DEFAULT 0,
            user_namespace TEXT DEFAULT '',
            PRIMARY KEY (book_url, title)
        );
        "#,
    )
    .execute(&pool)
    .await?;

    // 书架分组（books.group_name 存分组 id）
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS book_groups (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL DEFAULT '',
            order_num INTEGER DEFAULT 0,
            user_namespace TEXT DEFAULT ''
        );
        "#,
    )
    .execute(&pool)
    .await?;

    // F-28 替换规则（前端生成字符串 id；order 为 SQLite 关键字 → order_num）
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS replace_rules (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL DEFAULT '',
            find TEXT NOT NULL DEFAULT '',
            replace TEXT NOT NULL DEFAULT '',
            enable INTEGER DEFAULT 1,
            order_num INTEGER DEFAULT 0,
            user_namespace TEXT DEFAULT ''
        );
        "#,
    )
    .execute(&pool)
    .await?;

    // F-26 HttpTTS 听书源（url 主键；type 0=在线合成 / 1=本地引擎）
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS http_tts_list (
            url TEXT PRIMARY KEY,
            name TEXT NOT NULL DEFAULT '',
            type INTEGER DEFAULT 0,
            user_namespace TEXT DEFAULT ''
        );
        "#,
    )
    .execute(&pool)
    .await?;

    // 书源订阅（url 主键；raw_json 为抓取到的书源数组 JSON 原文）
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS source_subs (
            url TEXT PRIMARY KEY,
            name TEXT NOT NULL DEFAULT '',
            enabled INTEGER DEFAULT 1,
            user_namespace TEXT DEFAULT '',
            raw_json TEXT
        );
        "#,
    )
    .execute(&pool)
    .await?;

    // 自定义 TXT 目录规则（对齐 legado TxtTocRule：name/rule/serialNumber/enable）
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS txt_toc_rules (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL DEFAULT '',
            rule TEXT NOT NULL DEFAULT '',
            enable INTEGER DEFAULT 1,
            serial_number INTEGER DEFAULT 0,
            user_namespace TEXT DEFAULT ''
        );
        "#,
    )
    .execute(&pool)
    .await?;

    // 用户配置（前端设置同步：user_config 表按用户命名空间 + 配置命名空间双主键）
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS user_config (
            user_namespace TEXT NOT NULL,
            ns TEXT NOT NULL,
            config TEXT,
            updated_at INTEGER DEFAULT 0,
            PRIMARY KEY (user_namespace, ns)
        );
        "#,
    )
    .execute(&pool)
    .await?;

    // 阅读统计（reading_stats：按用户 + 书 + 日期累计阅读时长/字数）
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS reading_stats (
            user_namespace TEXT NOT NULL,
            book_url TEXT NOT NULL,
            date TEXT NOT NULL,
            seconds INTEGER DEFAULT 0,
            chars INTEGER DEFAULT 0,
            PRIMARY KEY (user_namespace, book_url, date)
        );
        "#,
    )
    .execute(&pool)
    .await?;

    // 幂等补列（兼容旧库：缺列则 ALTER TABLE 补上）
    let columns = [
        ("users", &["token_map", "raw_json"][..]),
        ("book_sources", &["rule_related"][..]),
        // GAP 44：旧库缺 rss_source_group 列时补上（新库 CREATE TABLE 已含）
        ("rss_sources", &["rss_source_group"][..]),
        (
            "books",
            &[
                "toc_url", "custom_tag", "custom_intro", "latest_chapter_title", "latest_chapter_time",
                "last_check_time", "last_check_count", "total_chapter_num", "word_count",
                "order_num", "origin_order", "use_replace_rule", "variable", "read_config",
                "is_in_shelf", "cbz", "display_cover", "display_intro", "local_epub", "local_pdf", "pdf", "split_long_chapter", "info_html", "toc_html", "language", "publisher", "published_at", "raw_json",
            ][..],
        ),
    ];
    for (table, cols) in columns {
        for col in cols {
            ensure_column(&pool, table, col).await?;
        }
    }

    // GAP 170 本地书双轨同步列（幂等补列；local_file TEXT + 变更检测/删除标记 INTEGER）
    ensure_column_typed(&pool, "books", "local_file", "TEXT").await?;
    ensure_column_typed(&pool, "books", "local_file_mtime", "INTEGER DEFAULT 0").await?;
    ensure_column_typed(&pool, "books", "local_file_size", "INTEGER DEFAULT 0").await?;
    ensure_column_typed(&pool, "books", "local_file_deleted", "INTEGER DEFAULT 0").await?;

    tracing::info!("storage initialized at {}", db_path.display());

    // JSON → SQLite 迁移（幂等：users 表非空跳过）
    let storage = Storage {
        pool,
        config: config.clone(),
    };
    if let Err(e) = crate::storage::migrate::migrate_if_needed(&storage).await {
        tracing::error!("JSON→SQLite 迁移失败（服务继续启动，数据仍保留在 JSON）：{e}");
    }
    // WAL 快照刷新：WAL 模式下隐式读事务跨语句保持——建表/ALTER/重建期间
    // 执行过 pragma 检查的池连接会持有 DDL 提交前的旧读快照，后续查询可能
    // 读不到新表结构/新数据（sqlx-sqlite 0.7.4 下表现为 row.rs 越界 panic 或
    // 查询返回空——storage 测试偶发）。BEGIN;COMMIT; 结束各连接的隐式读事务，
    // 下次读取强制取新快照（无事务的连接上为安全 no-op；裸 COMMIT 会报错）。
    let max_conns = 8; // 与上方 SqlitePoolOptions::max_connections(8) 一致
    for _ in 0..max_conns {
        let mut conn = storage.pool.acquire().await?;
        sqlx::query("BEGIN; COMMIT;").execute(&mut *conn).await?;
    }
    Ok(storage)
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

    // ---------------- OPDS 设置（system_settings 键值表） ----------------

    /// 系统设置读取（无则 None）
    pub async fn get_system_setting(&self, key: &str) -> Result<Option<String>> {
        let r: Option<(String,)> = sqlx::query_as("SELECT value FROM system_settings WHERE key = ?1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(r.map(|x| x.0))
    }

    /// 系统设置写入（INSERT OR REPLACE）
    pub async fn set_system_setting(&self, key: &str, value: &str) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO system_settings (key, value, updated_at) VALUES (?1, ?2, ?3)",
        )
        .bind(key)
        .bind(value)
        .bind(chrono::Utc::now().timestamp_millis())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 系统设置删除（返回删除行数）
    pub async fn delete_system_setting(&self, key: &str) -> Result<u64> {
        let r = sqlx::query("DELETE FROM system_settings WHERE key = ?1")
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected())
    }

    /// OPDS 独立账号：读取 (username, 存储串 `salt$hash`)。未配置返回 None。
    pub async fn get_opds_account(&self) -> Result<Option<(String, String)>> {
        let username = self.get_system_setting("opds_username").await?;
        let password = self.get_system_setting("opds_password").await?;
        match (username, password) {
            (Some(u), Some(p)) if !u.is_empty() && !p.is_empty() => Ok(Some((u, p))),
            _ => Ok(None),
        }
    }

    /// OPDS 独立账号写入（password 为已生成的 `salt$hash` 存储串）
    pub async fn set_opds_account(&self, username: &str, stored_password: &str) -> Result<()> {
        self.set_system_setting("opds_username", username).await?;
        self.set_system_setting("opds_password", stored_password).await?;
        Ok(())
    }

    /// OPDS 独立账号清除（禁用；回退系统账号/token 认证）
    pub async fn clear_opds_account(&self) -> Result<()> {
        self.delete_system_setting("opds_username").await?;
        self.delete_system_setting("opds_password").await?;
        Ok(())
    }

    /// 书源列表（按命名空间；无则回退 default）
    pub async fn get_book_sources(&self, ns: &str) -> Result<Vec<crate::model::BookSource>> {
        let rows = sqlx::query_as::<_, crate::model::BookSource>(
            "SELECT * FROM book_sources WHERE user_namespace = ?1 ORDER BY custom_order, book_source_name",
        )
        .bind(ns)
        .fetch_all(&self.pool)
        .await?;
        if !rows.is_empty() || ns == "default" {
            return Ok(rows);
        }
        // 回退 default 命名空间（legacy 语义：用户无书源时用系统书源）
        sqlx::query_as::<_, crate::model::BookSource>(
            "SELECT * FROM book_sources WHERE user_namespace = 'default' ORDER BY custom_order, book_source_name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Into::into)
    }

    /// 按 URL 查书源（精确或前缀匹配，兼容 ##@ 后缀；用户命名空间 + fallback default）
    pub async fn find_book_source(
        &self,
        ns: &str,
        book_source_url: &str,
    ) -> Result<Option<crate::model::BookSource>> {
        let like = format!("{book_source_url}%");
        let r = sqlx::query_as::<_, crate::model::BookSource>(
            "SELECT * FROM book_sources WHERE user_namespace = ?1 AND (book_source_url = ?2 OR book_source_url LIKE ?3)",
        )
        .bind(ns)
        .bind(book_source_url)
        .bind(&like)
        .fetch_optional(&self.pool)
        .await?;
        if r.is_some() || ns == "default" {
            return Ok(r);
        }
        sqlx::query_as::<_, crate::model::BookSource>(
            "SELECT * FROM book_sources WHERE user_namespace = 'default' AND (book_source_url = ?1 OR book_source_url LIKE ?2)",
        )
        .bind(book_source_url)
        .bind(&like)
        .fetch_optional(&self.pool)
        .await
        .map_err(Into::into)
    }

    /// 保存书源（INSERT OR REPLACE；raw_json 按 camelCase 重新序列化，与 bookSource.json 字段名一致）
    pub async fn save_book_source(&self, ns: &str, source: &crate::model::BookSource) -> Result<()> {
        upsert_book_source(&self.pool, ns, source).await
    }

    /// 批量保存书源（单事务：全部成功或全部回滚）
    pub async fn save_book_sources(&self, ns: &str, sources: &[crate::model::BookSource]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for source in sources {
            upsert_book_source(&mut *tx, ns, source).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// 删除书源（按 URL 精确匹配，仅限本命名空间）；返回受影响行数
    /// 连带删除该书源的登录态 cookie（按用户）
    pub async fn delete_book_source(&self, ns: &str, url: &str) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        let r = sqlx::query(
            "DELETE FROM book_sources WHERE user_namespace = ?1 AND book_source_url = ?2",
        )
        .bind(ns)
        .bind(url)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM book_source_cookies WHERE user_namespace = ?1 AND source_url = ?2")
            .bind(ns)
            .bind(url)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(r.rows_affected())
    }

    /// 按 URL 查单个书源（管理 API 用；复用 find_book_source 的精确/前缀匹配 + default 回退语义）
    pub async fn get_book_source(
        &self,
        ns: &str,
        url: &str,
    ) -> Result<Option<crate::model::BookSource>> {
        self.find_book_source(ns, url).await
    }

    /// 去重分组列表（兼容 legacy getBookSourceGroups：bookSourceGroup 空格分隔，保序去重；无书源回退 default）
    pub async fn list_book_source_groups(&self, ns: &str) -> Result<Vec<String>> {
        let sources = self.get_book_sources(ns).await?;
        let mut groups: Vec<String> = Vec::new();
        for s in sources {
            let Some(group) = s.book_source_group else { continue };
            for part in group.split_whitespace() {
                if !groups.iter().any(|g| g == part) {
                    groups.push(part.to_string());
                }
            }
        }
        Ok(groups)
    }

    /// 启停书源（按 URL 精确匹配，仅限本命名空间）；返回受影响行数
    pub async fn update_book_source_enabled(&self, ns: &str, url: &str, enabled: bool) -> Result<u64> {
        let r = sqlx::query(
            "UPDATE book_sources SET enabled = ?1 WHERE user_namespace = ?2 AND book_source_url = ?3",
        )
        .bind(enabled)
        .bind(ns)
        .bind(url)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }

    /// 清空命名空间全部书源（连带清理书源 cookie）
    pub async fn delete_all_book_sources(&self, ns: &str) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        let r = sqlx::query("DELETE FROM book_sources WHERE user_namespace = ?1")
            .bind(ns)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM book_source_cookies WHERE user_namespace = ?1")
            .bind(ns)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(r.rows_affected())
    }

    // ---------------- 书源登录态 cookie（按用户隔离） ----------------

    /// 读取书源 cookie（精确 source_url 键；无则 None）
    pub async fn get_cookie(&self, ns: &str, source_url: &str) -> Result<Option<String>> {
        let r: Option<(String,)> = sqlx::query_as(
            "SELECT cookie FROM book_source_cookies WHERE user_namespace = ?1 AND source_url = ?2",
        )
        .bind(ns)
        .bind(source_url)
        .fetch_optional(&self.pool)
        .await?;
        Ok(r.map(|x| x.0).filter(|c| !c.is_empty()))
    }

    /// 写入书源 cookie（INSERT OR REPLACE；空值等价清除）
    pub async fn set_cookie(&self, ns: &str, source_url: &str, cookie: &str) -> Result<()> {
        if cookie.trim().is_empty() {
            self.clear_cookie(ns, source_url).await?;
            return Ok(());
        }
        sqlx::query(
            "INSERT OR REPLACE INTO book_source_cookies (user_namespace, source_url, cookie, user_agent, updated_at)
             VALUES (?1, ?2, ?3, COALESCE((SELECT user_agent FROM book_source_cookies WHERE user_namespace = ?1 AND source_url = ?2), ''), ?4)",
        )
        .bind(ns)
        .bind(source_url)
        .bind(cookie)
        .bind(chrono::Utc::now().timestamp_millis())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 清除书源 cookie（返回删除行数）
    pub async fn clear_cookie(&self, ns: &str, source_url: &str) -> Result<u64> {
        let r = sqlx::query(
            "DELETE FROM book_source_cookies WHERE user_namespace = ?1 AND source_url = ?2",
        )
        .bind(ns)
        .bind(source_url)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }

    /// 记录书源 user_agent（FlareSolverr 返回 UA 与库中不同时更新——部分站点 UA 绑定 cookie）
    pub async fn set_cookie_user_agent(&self, ns: &str, source_url: &str, user_agent: &str) -> Result<()> {
        if user_agent.trim().is_empty() {
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO book_source_cookies (user_namespace, source_url, cookie, user_agent, updated_at)
             VALUES (?1, ?2, '', ?3, ?4)
             ON CONFLICT(user_namespace, source_url) DO UPDATE SET user_agent = ?3",
        )
        .bind(ns)
        .bind(source_url)
        .bind(user_agent)
        .bind(chrono::Utc::now().timestamp_millis())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 读取书源登录态会话（cookie + user_agent）
    pub async fn get_source_session(
        &self,
        ns: &str,
        source_url: &str,
    ) -> Result<Option<(String, String)>> {
        let r: Option<(String, String)> = sqlx::query_as(
            "SELECT cookie, user_agent FROM book_source_cookies WHERE user_namespace = ?1 AND source_url = ?2",
        )
        .bind(ns)
        .bind(source_url)
        .fetch_optional(&self.pool)
        .await?;
        Ok(r.map(|(c, ua)| (c, ua)))
    }

    /// 按 baseUrl 匹配书源 cookie（crawler 抓取用：请求 URL 的 base 与书源
    /// source_url 的 base 一致即命中——source_url 可能带 `##` 备用地址后缀）。
    /// 仅查本命名空间（书源 cookie 按用户隔离）。
    pub async fn get_cookie_by_base(&self, ns: &str, base_url: &str) -> Result<Option<String>> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT source_url, cookie FROM book_source_cookies WHERE user_namespace = ?1",
        )
        .bind(ns)
        .fetch_all(&self.pool)
        .await?;
        let target = normalize_base(base_url);
        for (source_url, cookie) in rows {
            // `##` 后缀：主地址/备用地址都算同源（与 book_sources 语义一致）——任一段命中即可
            if cookie.is_empty() {
                continue;
            }
            let any_match = source_url.split("##").any(|part| normalize_base(part) == target);
            if any_match {
                return Ok(Some(cookie));
            }
        }
        Ok(None)
    }

    // ---------------- RSS ----------------

    /// RSS 源列表（按命名空间；无则回退 default，同 get_book_sources 语义）
    pub async fn get_rss_sources(&self, ns: &str) -> Result<Vec<crate::model::RssSource>> {
        let rows = sqlx::query_as::<_, crate::model::RssSource>(
            "SELECT * FROM rss_sources WHERE user_namespace = ?1 ORDER BY rss_source_name",
        )
        .bind(ns)
        .fetch_all(&self.pool)
        .await?;
        if !rows.is_empty() || ns == "default" {
            return Ok(rows);
        }
        sqlx::query_as::<_, crate::model::RssSource>(
            "SELECT * FROM rss_sources WHERE user_namespace = 'default' ORDER BY rss_source_name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Into::into)
    }

    /// 按 URL 查 RSS 源（用户命名空间 + default 回退）
    pub async fn find_rss_source(
        &self,
        ns: &str,
        source_url: &str,
    ) -> Result<Option<crate::model::RssSource>> {
        let r = sqlx::query_as::<_, crate::model::RssSource>(
            "SELECT * FROM rss_sources WHERE user_namespace = ?1 AND rss_source_url = ?2",
        )
        .bind(ns)
        .bind(source_url)
        .fetch_optional(&self.pool)
        .await?;
        if r.is_some() || ns == "default" {
            return Ok(r);
        }
        sqlx::query_as::<_, crate::model::RssSource>(
            "SELECT * FROM rss_sources WHERE user_namespace = 'default' AND rss_source_url = ?1",
        )
        .bind(source_url)
        .fetch_optional(&self.pool)
        .await
        .map_err(Into::into)
    }

    /// 保存 RSS 源（INSERT OR REPLACE；raw_json 存完整 JSON 原文）
    pub async fn save_rss_source(&self, ns: &str, source: &crate::model::RssSource) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO rss_sources            (rss_source_url, rss_source_name, rss_source_group, enabled, user_namespace, raw_json)            VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(&source.source_url)
        .bind(&source.source_name)
        .bind(&source.source_group)
        .bind(source.enabled)
        .bind(ns)
        .bind(&source.raw_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 删除 RSS 源（按 URL，仅限本命名空间）；返回受影响行数
    pub async fn delete_rss_source(&self, ns: &str, source_url: &str) -> Result<u64> {
        let r = sqlx::query(
            "DELETE FROM rss_sources WHERE user_namespace = ?1 AND rss_source_url = ?2",
        )
        .bind(ns)
        .bind(source_url)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }

    /// 批量保存 RSS 文章（单事务，INSERT ... ON CONFLICT 按 url 主键去重更新；
    /// 更新时不触碰 read 列——feed 刷新重新入库不清除已读标记）
    pub async fn save_rss_articles(&self, ns: &str, articles: &[crate::model::RssArticle]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for a in articles {
            sqlx::query(
                "INSERT INTO rss_articles            (url, source_url, title, author, time, content, cover, user_namespace, read)            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)            ON CONFLICT(url) DO UPDATE SET                source_url = excluded.source_url,                title = excluded.title,                author = excluded.author,                time = excluded.time,                content = excluded.content,                cover = excluded.cover,                user_namespace = excluded.user_namespace",
            )
            .bind(&a.url)
            .bind(&a.source_url)
            .bind(&a.title)
            .bind(&a.author)
            .bind(a.time)
            .bind(&a.content)
            .bind(&a.cover)
            .bind(ns)
            .bind(a.read)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// 标记 RSS 文章已读/未读（url 主键；返回受影响行数）
    pub async fn set_rss_article_read(&self, url: &str, read: bool) -> Result<u64> {
        let r = sqlx::query("UPDATE rss_articles SET read = ?2 WHERE url = ?1")
            .bind(url)
            .bind(read)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected())
    }

    /// 某 RSS 源全部文章的已读标记（url → read），getRssArticles 返回时合并用
    pub async fn get_rss_article_read_flags(
        &self,
        ns: &str,
        source_url: &str,
    ) -> Result<std::collections::HashMap<String, bool>> {
        let rows: Vec<(String, bool)> = sqlx::query_as(
            "SELECT url, read FROM rss_articles WHERE user_namespace = ?1 AND source_url = ?2",
        )
        .bind(ns)
        .bind(source_url)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().collect())
    }

    /// 按 URL 查 RSS 文章（getRssArticle 正文/缓存用）
    pub async fn get_rss_article(&self, url: &str) -> Result<Option<crate::model::RssArticle>> {
        let r = sqlx::query_as::<_, crate::model::RssArticle>(
            "SELECT * FROM rss_articles WHERE url = ?1",
        )
        .bind(url)
        .fetch_optional(&self.pool)
        .await?;
        Ok(r)
    }

    // ---------------- 缓存管理 ----------------

    /// 缓存统计：toc_cache 行数 / book_chapters 行数 / 章节正文近似大小（sum length(content)）/
    /// 目录缓存大小（sum length(chapters_json)）/ 总大小（两者之和）
    pub async fn get_cache_info(&self) -> Result<CacheInfo> {
        let toc_cache_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM toc_cache")
            .fetch_one(&self.pool)
            .await?;
        let toc_cache_size: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(length(chapters_json)), 0) FROM toc_cache",
        )
        .fetch_one(&self.pool)
        .await?;
        let chapter_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM book_chapters")
            .fetch_one(&self.pool)
            .await?;
        let chapter_size: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(length(content)), 0) FROM book_chapters",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(CacheInfo {
            toc_cache_count,
            toc_cache_size,
            chapter_count,
            chapter_size,
            total_size: toc_cache_size + chapter_size,
        })
    }

    /// 清空缓存（type: "toc" 清目录缓存 / "chapters" 清章节缓存 / "all" 全清）；
    /// 返回 (toc 删除行数, 章节删除行数)
    pub async fn clear_cache(&self, cache_type: &str) -> Result<(u64, u64)> {
        let mut toc_deleted = 0u64;
        let mut chapters_deleted = 0u64;
        if cache_type == "toc" || cache_type == "all" {
            let r = sqlx::query("DELETE FROM toc_cache")
                .execute(&self.pool)
                .await?;
            toc_deleted = r.rows_affected();
        }
        if cache_type == "chapters" || cache_type == "all" {
            let r = sqlx::query("DELETE FROM book_chapters")
                .execute(&self.pool)
                .await?;
            chapters_deleted = r.rows_affected();
        }
        Ok((toc_deleted, chapters_deleted))
    }

    // ---------------- 全书搜索（本地书） ----------------

    /// 某书在 book_chapters 表中的章节数（本地书判定用）
    pub async fn count_chapters(&self, book_url: &str) -> Result<i64> {
        let count = sqlx::query_scalar("SELECT COUNT(*) FROM book_chapters WHERE book_url = ?1")
            .bind(book_url)
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    /// 删除单书缓存（book_chapters 该 book_url 行——本地书章节 + 书源书正文缓存）；
    /// 不影响书架 books 行。返回删除行数
    pub async fn delete_book_cache(&self, book_url: &str) -> Result<u64> {
        let r = sqlx::query("DELETE FROM book_chapters WHERE book_url = ?1")
            .bind(book_url)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected())
    }

    /// 单书缓存信息：(章节数, 正文近似大小 sum length(content))
    pub async fn book_cache_info(&self, book_url: &str) -> Result<(i64, i64)> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM book_chapters WHERE book_url = ?1")
            .bind(book_url)
            .fetch_one(&self.pool)
            .await?;
        let size: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(length(content)), 0) FROM book_chapters WHERE book_url = ?1",
        )
        .bind(book_url)
        .fetch_one(&self.pool)
        .await?;
        Ok((count, size))
    }

    /// 全书搜索：book_chapters 正文 LIKE 匹配（key 中 %/_ 转义为字面量），按章节序返回
    /// 命中章节（chapterIndex/title/snippet——命中段落前后截取），最多 limit 条
    pub async fn search_book_content(
        &self,
        book_url: &str,
        key: &str,
        limit: i64,
    ) -> Result<Vec<BookContentHit>> {
        let escaped = key
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("%{escaped}%");
        let rows = sqlx::query_as::<_, (i64, String, String)>(
            "SELECT chapter_index, title, content FROM book_chapters             WHERE book_url = ?1 AND content LIKE ?2 ESCAPE '\\'             ORDER BY chapter_index LIMIT ?3",
        )
        .bind(book_url)
        .bind(&pattern)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        let mut hits = Vec::with_capacity(rows.len());
        for (chapter_index, title, content) in rows {
            hits.push(BookContentHit {
                chapter_index,
                title,
                snippet: make_snippet(&content, key, 40),
            });
        }
        Ok(hits)
    }

    // ---------------- 书源订阅 ----------------

    /// 订阅列表（按名称排序；用户无订阅回退 default，同书源语义）
    pub async fn get_source_subs(&self, ns: &str) -> Result<Vec<crate::model::SourceSub>> {
        let rows = sqlx::query_as::<_, crate::model::SourceSub>(
            "SELECT * FROM source_subs WHERE user_namespace = ?1 ORDER BY name, url",
        )
        .bind(ns)
        .fetch_all(&self.pool)
        .await?;
        if !rows.is_empty() || ns == "default" {
            return Ok(rows);
        }
        sqlx::query_as::<_, crate::model::SourceSub>(
            "SELECT * FROM source_subs WHERE user_namespace = 'default' ORDER BY name, url",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Into::into)
    }

    /// 按 URL 查订阅（用户命名空间 + default 回退）
    pub async fn find_source_sub(
        &self,
        ns: &str,
        url: &str,
    ) -> Result<Option<crate::model::SourceSub>> {
        let r = sqlx::query_as::<_, crate::model::SourceSub>(
            "SELECT * FROM source_subs WHERE user_namespace = ?1 AND url = ?2",
        )
        .bind(ns)
        .bind(url)
        .fetch_optional(&self.pool)
        .await?;
        if r.is_some() || ns == "default" {
            return Ok(r);
        }
        sqlx::query_as::<_, crate::model::SourceSub>(
            "SELECT * FROM source_subs WHERE user_namespace = 'default' AND url = ?1",
        )
        .bind(url)
        .fetch_optional(&self.pool)
        .await
        .map_err(Into::into)
    }

    /// 保存订阅（INSERT OR REPLACE，按 url 主键覆盖；raw_json 存书源数组 JSON 原文）
    pub async fn save_source_sub(
        &self,
        ns: &str,
        url: &str,
        name: &str,
        raw_json: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO source_subs (url, name, enabled, user_namespace, raw_json)             VALUES (?1, ?2, 1, ?3, ?4)",
        )
        .bind(url)
        .bind(name)
        .bind(ns)
        .bind(raw_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 删除订阅（按 url，仅限本命名空间）；返回受影响行数
    pub async fn delete_source_sub(&self, ns: &str, url: &str) -> Result<u64> {
        let r = sqlx::query("DELETE FROM source_subs WHERE user_namespace = ?1 AND url = ?2")
            .bind(ns)
            .bind(url)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected())
    }

    /// 保存章节（本地书）
    pub async fn save_chapters(&self, book_url: &str, chapters: &[(String, String)]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for (i, (title, content)) in chapters.iter().enumerate() {
            sqlx::query(
                "INSERT OR REPLACE INTO book_chapters (book_url, chapter_index, title, content) VALUES (?1, ?2, ?3, ?4)",
            )
            .bind(book_url)
            .bind(i as i64)
            .bind(title)
            .bind(content)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// 本地书章节列表
    pub async fn list_chapters(&self, book_url: &str) -> Result<Vec<(i64, String)>> {
        let rows = sqlx::query_as::<_, (i64, String)>(
            "SELECT chapter_index, title FROM book_chapters WHERE book_url = ?1 ORDER BY chapter_index",
        )
        .bind(book_url)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// 章节正文
    pub async fn get_chapter_content(&self, book_url: &str, index: i64) -> Result<Option<String>> {
        let r: Option<(String,)> = sqlx::query_as(
            "SELECT content FROM book_chapters WHERE book_url = ?1 AND chapter_index = ?2",
        )
        .bind(book_url)
        .bind(index)
        .fetch_optional(&self.pool)
        .await?;
        Ok(r.map(|x| x.0))
    }

    /// 书源书正文缓存写回（chapter_index = chapterUrl md5 哈希；与本地书顺序索引键域不重叠）
    pub async fn cache_chapter_content(
        &self,
        book_url: &str,
        index: i64,
        title: &str,
        content: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO book_chapters (book_url, chapter_index, title, content) VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(book_url)
        .bind(index)
        .bind(title)
        .bind(content)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 删除本地书（含章节）
    pub async fn delete_local_book(&self, book_url: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM book_chapters WHERE book_url = ?1")
            .bind(book_url)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM books WHERE book_url = ?1")
            .bind(book_url)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 删除书（书源书或本地书——本地书含章节）
    /// GAP 117：删除后清理 assets/{ns}/covers 下对应封面文件（未被其他书引用时）
    pub async fn delete_book(&self, ns: &str, book_url: &str) -> Result<u64> {
        let cover_url = sqlx::query_scalar::<_, Option<String>>(
            "SELECT cover_url FROM books WHERE user_namespace = ?1 AND book_url = ?2",
        )
        .bind(ns)
        .bind(book_url)
        .fetch_optional(&self.pool)
        .await?
        .flatten();
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM book_chapters WHERE book_url = ?1")
            .bind(book_url)
            .execute(&mut *tx)
            .await?;
        let r = sqlx::query("DELETE FROM books WHERE user_namespace = ?1 AND book_url = ?2")
            .bind(ns)
            .bind(book_url)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        if r.rows_affected() > 0 {
            self.cleanup_orphan_cover(ns, cover_url.as_deref()).await;
        }
        Ok(r.rows_affected())
    }

    /// GAP 117：封面文件残留清理——cover_url 形如 /assets/{ns}/covers/{file}，
    /// 且删除后无其他书引用同一路径时删除文件（最佳努力，失败仅告警）
    async fn cleanup_orphan_cover(&self, ns: &str, cover_url: Option<&str>) {
        let Some(cover_url) = cover_url else { return };
        let Some(file) = cover_file_name(ns, cover_url) else { return };
        // 其他书（本命名空间内）是否仍引用同一封面
        let refs: i64 = match sqlx::query_scalar(
            "SELECT COUNT(*) FROM books WHERE user_namespace = ?1 AND cover_url = ?2",
        )
        .bind(ns)
        .bind(cover_url)
        .fetch_one(&self.pool)
        .await
        {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!("cleanup_orphan_cover 查询失败: {e}");
                return;
            }
        };
        if refs > 0 {
            return;
        }
        let path = self
            .config
            .storage_dir()
            .join("assets")
            .join(ns)
            .join("covers")
            .join(&file);
        if let Err(e) = std::fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("封面清理失败 {}: {e}", path.display());
            }
        } else {
            tracing::info!("GAP 117 已清理无引用封面 {}", path.display());
        }
    }

    /// 更新书字段（编辑：name/author/coverUrl/group）
    pub async fn update_book(
        &self,
        ns: &str,
        book_url: &str,
        name: Option<&str>,
        author: Option<&str>,
        cover_url: Option<&str>,
        group: Option<i64>,
    ) -> Result<u64> {
        let r = sqlx::query(
            "UPDATE books SET name = COALESCE(?3, name), author = COALESCE(?4, author),              cover_url = COALESCE(?5, cover_url), group_name = COALESCE(?6, group_name)              WHERE user_namespace = ?1 AND book_url = ?2",
        )
        .bind(ns)
        .bind(book_url)
        .bind(name)
        .bind(author)
        .bind(cover_url)
        .bind(group)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }

    /// 按 URL 查书架书（saveBook 新增/编辑判断；不存在返回 None）
    pub async fn find_book(&self, ns: &str, book_url: &str) -> Result<Option<Book>> {
        let book = sqlx::query_as::<_, Book>(
            "SELECT * FROM books WHERE user_namespace = ?1 AND book_url = ?2",
        )
        .bind(ns)
        .bind(book_url)
        .fetch_optional(&self.pool)
        .await?;
        Ok(book)
    }

    /// saveBook 全量入架/覆盖：INSERT OR REPLACE（不存在则新增，存在则全字段更新）
    pub async fn upsert_book(&self, ns: &str, book: &Book) -> Result<()> {
        let mut b = book.clone();
        b.user_namespace = ns.to_string();
        // GAP 170 双轨同步：local_file 为服务端内部字段（客户端 saveBook 不带）——
        // 旧客户端全量覆盖时保留既有文件关联，避免打断文件↔DB 双轨
        let local_file = if b.local_file.is_some() {
            b.local_file.clone()
        } else {
            self.find_book(ns, &b.book_url)
                .await
                .ok()
                .flatten()
                .and_then(|old| old.local_file)
        };
        sqlx::query(
            r#"INSERT OR REPLACE INTO books
            (book_url, name, author, origin, origin_name, kind, custom_tag, cover_url,
             custom_cover_url, intro, custom_intro, charset, type, group_name,
             latest_chapter_title, latest_chapter_time, last_check_time, last_check_count,
             total_chapter_num, dur_chapter_title, dur_chapter_index, dur_chapter_pos,
             dur_chapter_time, word_count, can_update, order_num, origin_order,
             use_replace_rule, variable, read_config, is_in_shelf, cbz, display_cover,
             display_intro, local_epub, local_pdf, pdf, split_long_chapter,
             last_check_error, info_html, toc_html, language, publisher, published_at,
             user_namespace, created_at, raw_json, local_file, local_file_mtime,
             local_file_size, local_file_deleted)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                    ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27,
                    ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37, ?38, ?39, ?40,
                    ?41, ?42, ?43, ?44, ?45, ?46, ?47, ?48, ?49, ?50, ?51)"#,
        )
        .bind(&b.book_url)
        .bind(&b.name)
        .bind(&b.author)
        .bind(&b.origin)
        .bind(&b.origin_name)
        .bind(&b.kind)
        .bind(&b.custom_tag)
        .bind(&b.cover_url)
        .bind(&b.custom_cover_url)
        .bind(&b.intro)
        .bind(&b.custom_intro)
        .bind(&b.charset)
        .bind(b.book_type)
        .bind(b.group)
        .bind(&b.latest_chapter_title)
        .bind(b.latest_chapter_time)
        .bind(b.last_check_time)
        .bind(b.last_check_count)
        .bind(b.total_chapter_num)
        .bind(&b.dur_chapter_title)
        .bind(b.dur_chapter_index)
        .bind(b.dur_chapter_pos)
        .bind(b.dur_chapter_time)
        .bind(&b.word_count)
        .bind(b.can_update)
        .bind(b.order)
        .bind(b.origin_order)
        .bind(b.use_replace_rule)
        .bind(&b.variable)
        .bind(&b.read_config.as_ref().map(|v| v.to_string()))
        .bind(b.is_in_shelf)
        .bind(b.cbz)
        .bind(&b.display_cover)
        .bind(&b.display_intro)
        .bind(b.local_epub)
        .bind(b.local_pdf)
        .bind(b.pdf)
        .bind(b.split_long_chapter)
        .bind(&b.last_check_error)
        .bind(&b.info_html)
        .bind(&b.toc_html)
        .bind(&b.language)
        .bind(&b.publisher)
        .bind(&b.published_at)
        .bind(&b.user_namespace)
        .bind(b.created_at)
        .bind(&b.raw_json)
        // GAP 170 双轨同步：local_file 为服务端内部字段（客户端 saveBook 不带）——
        // 旧客户端全量覆盖时保留既有文件关联，避免打断文件↔DB 双轨
        .bind(local_file)
        .bind(b.local_file_mtime)
        .bind(b.local_file_size)
        .bind(b.local_file_deleted)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// saveBook 增量更新：按请求 JSON 中出现的字段（camelCase 键）动态 UPDATE（编辑场景，
    /// 未提供的字段保持不变；列名来自固定映射表，无注入风险）
    pub async fn patch_book(
        &self,
        ns: &str,
        book_url: &str,
        patch: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<u64> {
        let mut qb = sqlx::QueryBuilder::new("UPDATE books SET ");
        let mut first = true;
        let mut any = false;
        for (key, value) in patch {
            let Some(col) = BOOK_PATCH_COLUMNS.iter().find(|(k, _)| k == key).map(|(_, c)| *c) else {
                continue;
            };
            if !first {
                qb.push(", ");
            }
            qb.push(col).push(" = ");
            push_book_patch_value(&mut qb, value);
            first = false;
            any = true;
        }
        if !any {
            return Ok(0);
        }
        qb.push(" WHERE user_namespace = ")
            .push_bind(ns)
            .push(" AND book_url = ")
            .push_bind(book_url);
        let r = qb.build().execute(&self.pool).await?;
        Ok(r.rows_affected())
    }

    /// F-8 保存阅读进度（durChapter* 字段；title 为 None 时保持原值）
    pub async fn update_book_progress(
        &self,
        ns: &str,
        book_url: &str,
        title: Option<&str>,
        index: i64,
        pos: i64,
        time: i64,
    ) -> Result<u64> {
        let r = sqlx::query(
            "UPDATE books SET dur_chapter_title = COALESCE(?3, dur_chapter_title),              dur_chapter_index = ?4, dur_chapter_pos = ?5, dur_chapter_time = ?6              WHERE user_namespace = ?1 AND book_url = ?2",
        )
        .bind(ns)
        .bind(book_url)
        .bind(title)
        .bind(index)
        .bind(pos)
        .bind(time)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }

    /// F-10 目录缓存写入（getBookToc 成功后调用）
    pub async fn cache_toc(&self, book_url: &str, toc_url: &str, chapters_json: &str) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO toc_cache (book_url, toc_url, chapters_json, updated_at)              VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(book_url)
        .bind(toc_url)
        .bind(chapters_json)
        .bind(chrono::Utc::now().timestamp_millis())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// F-10 目录缓存读取（同 tocUrl 直读；超过 max_age_ms 视为未命中）
    pub async fn get_toc_cache(&self, toc_url: &str, max_age_ms: i64) -> Result<Option<String>> {
        let cutoff = chrono::Utc::now().timestamp_millis() - max_age_ms;
        let r: Option<(String,)> = sqlx::query_as(
            "SELECT chapters_json FROM toc_cache WHERE toc_url = ?1 AND updated_at >= ?2",
        )
        .bind(toc_url)
        .bind(cutoff)
        .fetch_optional(&self.pool)
        .await?;
        Ok(r.map(|x| x.0))
    }

    /// 保存书签（INSERT OR REPLACE，主键 book_url+title）
    pub async fn save_bookmark(&self, ns: &str, bookmark: &crate::model::Bookmark) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO bookmarks (book_url, title, paragraph_index, chapter_index,              created_at, user_namespace) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(&bookmark.book_url)
        .bind(&bookmark.title)
        .bind(bookmark.paragraph_index)
        .bind(bookmark.chapter_index)
        .bind(bookmark.created_at)
        .bind(ns)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 某书的书签列表（按创建时间倒序）
    pub async fn list_bookmarks(&self, ns: &str, book_url: &str) -> Result<Vec<crate::model::Bookmark>> {
        let rows = sqlx::query_as::<_, crate::model::Bookmark>(
            "SELECT * FROM bookmarks WHERE user_namespace = ?1 AND book_url = ?2              ORDER BY created_at DESC, rowid DESC",
        )
        .bind(ns)
        .bind(book_url)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// 删除书签（book_url + title）；返回受影响行数
    pub async fn delete_bookmark(&self, ns: &str, book_url: &str, title: &str) -> Result<u64> {
        let r = sqlx::query(
            "DELETE FROM bookmarks WHERE user_namespace = ?1 AND book_url = ?2 AND title = ?3",
        )
        .bind(ns)
        .bind(book_url)
        .bind(title)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }

    /// 分组列表（按 order_num, id 排序）
    pub async fn list_book_groups(&self, ns: &str) -> Result<Vec<crate::model::BookGroup>> {
        let rows = sqlx::query_as::<_, crate::model::BookGroup>(
            "SELECT * FROM book_groups WHERE user_namespace = ?1 ORDER BY order_num, id",
        )
        .bind(ns)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// 保存分组：id > 0 按 id 覆盖，否则自增新建；返回带 id 的分组
    pub async fn save_book_group(&self, ns: &str, group: &crate::model::BookGroup) -> Result<crate::model::BookGroup> {
        let mut g = group.clone();
        g.user_namespace = ns.to_string();
        if g.id > 0 {
            sqlx::query(
                "INSERT OR REPLACE INTO book_groups (id, name, order_num, user_namespace)              VALUES (?1, ?2, ?3, ?4)",
            )
            .bind(g.id)
            .bind(&g.name)
            .bind(g.order)
            .bind(ns)
            .execute(&self.pool)
            .await?;
        } else {
            let r = sqlx::query(
                "INSERT INTO book_groups (name, order_num, user_namespace) VALUES (?1, ?2, ?3)",
            )
            .bind(&g.name)
            .bind(g.order)
            .bind(ns)
            .execute(&self.pool)
            .await?;
            g.id = r.last_insert_rowid();
        }
        Ok(g)
    }

    /// 书设分组（books.group_name = 分组 id）；返回受影响行数
    pub async fn update_book_group_id(&self, ns: &str, book_url: &str, group: i64) -> Result<u64> {
        let r = sqlx::query(
            "UPDATE books SET group_name = ?3 WHERE user_namespace = ?1 AND book_url = ?2",
        )
        .bind(ns)
        .bind(book_url)
        .bind(group)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }

    /// 分组列表（带组内书数统计：books.group_name = 分组 id 计数）
    pub async fn list_book_groups_with_count(
        &self,
        ns: &str,
    ) -> Result<Vec<crate::model::BookGroupWithCount>> {
        let rows = sqlx::query_as::<_, (i64, String, i64, i64)>(
            "SELECT g.id, g.name, g.order_num,               (SELECT COUNT(*) FROM books b               WHERE b.user_namespace = g.user_namespace AND b.group_name = g.id) AS book_count               FROM book_groups g WHERE g.user_namespace = ?1 ORDER BY g.order_num, g.id",
        )
        .bind(ns)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, name, order, book_count)| crate::model::BookGroupWithCount {
                id,
                name,
                order,
                order_num: order,
                book_count,
            })
            .collect())
    }

    /// 分组重命名（仅改 name，保留 order 与 id；不存在返回 0 行）
    pub async fn rename_book_group(&self, ns: &str, id: i64, name: &str) -> Result<u64> {
        let r = sqlx::query(
            "UPDATE book_groups SET name = ?3 WHERE user_namespace = ?1 AND id = ?2",
        )
        .bind(ns)
        .bind(id)
        .bind(name)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }

    /// 删除分组（事务：组内书 group_name 置 0 后删分组）；返回删除的分组行数
    pub async fn delete_book_group(&self, ns: &str, id: i64) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE books SET group_name = 0 WHERE user_namespace = ?1 AND group_name = ?2")
            .bind(ns)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        let r = sqlx::query("DELETE FROM book_groups WHERE user_namespace = ?1 AND id = ?2")
            .bind(ns)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(r.rows_affected())
    }

    // ---------------- 批量接口（deleteBooks / 分组批量 / 书签批量 / RSS 批量） ----------------

    /// 批量删除书（事务：每本连带删章节）；返回删除的书行数
    /// 批量删除（GAP 117：删除后清理无引用封面文件）
    pub async fn delete_books(&self, ns: &str, book_urls: &[String]) -> Result<u64> {
        let mut covers: Vec<String> = Vec::new();
        for url in book_urls {
            if let Ok(Some(c)) = sqlx::query_scalar::<_, Option<String>>(
                "SELECT cover_url FROM books WHERE user_namespace = ?1 AND book_url = ?2",
            )
            .bind(ns)
            .bind(url)
            .fetch_one(&self.pool)
            .await
            {
                covers.push(c);
            }
        }
        let mut tx = self.pool.begin().await?;
        let mut deleted = 0u64;
        for url in book_urls {
            sqlx::query("DELETE FROM book_chapters WHERE book_url = ?1")
                .bind(url)
                .execute(&mut *tx)
                .await?;
            let r = sqlx::query("DELETE FROM books WHERE user_namespace = ?1 AND book_url = ?2")
                .bind(ns)
                .bind(url)
                .execute(&mut *tx)
                .await?;
            deleted += r.rows_affected();
        }
        tx.commit().await?;
        for c in covers {
            self.cleanup_orphan_cover(ns, Some(&c)).await;
        }
        Ok(deleted)
    }

    /// 批量书设分组（books.group_name = 分组 id）；返回受影响行数
    pub async fn add_book_group_multi(
        &self,
        ns: &str,
        book_urls: &[String],
        group_id: i64,
    ) -> Result<u64> {
        if book_urls.is_empty() {
            return Ok(0);
        }
        let mut qb = sqlx::QueryBuilder::new("UPDATE books SET group_name = ");
        qb.push_bind(group_id)
            .push(" WHERE user_namespace = ")
            .push_bind(ns)
            .push(" AND book_url IN (");
        let mut sep = qb.separated(", ");
        for url in book_urls {
            sep.push_bind(url);
        }
        qb.push(")");
        let r = qb.build().execute(&self.pool).await?;
        Ok(r.rows_affected())
    }

    /// 批量移出分组（group_name 置 0）；返回受影响行数
    pub async fn remove_book_group_multi(&self, ns: &str, book_urls: &[String]) -> Result<u64> {
        if book_urls.is_empty() {
            return Ok(0);
        }
        let mut qb = sqlx::QueryBuilder::new("UPDATE books SET group_name = 0 WHERE user_namespace = ");
        qb.push_bind(ns).push(" AND book_url IN (");
        let mut sep = qb.separated(", ");
        for url in book_urls {
            sep.push_bind(url);
        }
        qb.push(")");
        let r = qb.build().execute(&self.pool).await?;
        Ok(r.rows_affected())
    }

    /// 分组排序批量保存（order = [(id, order_num)]）；返回更新行数
    pub async fn save_book_group_order(&self, ns: &str, order: &[(i64, i64)]) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        let mut updated = 0u64;
        for (id, order_num) in order {
            let r = sqlx::query(
                "UPDATE book_groups SET order_num = ?3 WHERE user_namespace = ?1 AND id = ?2",
            )
            .bind(ns)
            .bind(id)
            .bind(order_num)
            .execute(&mut *tx)
            .await?;
            updated += r.rows_affected();
        }
        tx.commit().await?;
        Ok(updated)
    }

    /// 批量保存书签（单事务，INSERT OR REPLACE 按 book_url+title 主键去重）
    pub async fn save_bookmarks(&self, ns: &str, bookmarks: &[crate::model::Bookmark]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for b in bookmarks {
            sqlx::query(
                "INSERT OR REPLACE INTO bookmarks (book_url, title, paragraph_index, chapter_index,              created_at, user_namespace) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .bind(&b.book_url)
            .bind(&b.title)
            .bind(b.paragraph_index)
            .bind(b.chapter_index)
            .bind(b.created_at)
            .bind(ns)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// 批量删除书签（book_url + ids=书签标题列表，IN 查询）；返回受影响行数
    pub async fn delete_bookmarks(&self, ns: &str, book_url: &str, ids: &[String]) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let mut qb = sqlx::QueryBuilder::new("DELETE FROM bookmarks WHERE user_namespace = ");
        qb.push_bind(ns)
            .push(" AND book_url = ")
            .push_bind(book_url)
            .push(" AND title IN (");
        let mut sep = qb.separated(", ");
        for id in ids {
            sep.push_bind(id);
        }
        qb.push(")");
        let r = qb.build().execute(&self.pool).await?;
        Ok(r.rows_affected())
    }

    /// 批量保存 RSS 源（单事务，INSERT OR REPLACE 按 rss_source_url 主键覆盖）
    pub async fn save_rss_sources(&self, ns: &str, sources: &[crate::model::RssSource]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for s in sources {
            sqlx::query(
                "INSERT OR REPLACE INTO rss_sources            (rss_source_url, rss_source_name, rss_source_group, enabled, user_namespace, raw_json)            VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .bind(&s.source_url)
            .bind(&s.source_name)
            .bind(&s.source_group)
            .bind(s.enabled)
            .bind(ns)
            .bind(&s.raw_json)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    // ---------------- 用户配置（user_config 表：按用户 + 配置命名空间） ----------------

    /// 读取用户配置（无则 None）
    pub async fn get_user_config(&self, ns: &str, key: &str) -> Result<Option<String>> {
        let r: Option<(String,)> = sqlx::query_as(
            "SELECT config FROM user_config WHERE user_namespace = ?1 AND ns = ?2",
        )
        .bind(ns)
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;
        Ok(r.map(|x| x.0))
    }

    /// 保存用户配置（INSERT OR REPLACE）
    pub async fn save_user_config(&self, ns: &str, key: &str, config: &str) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO user_config (user_namespace, ns, config, updated_at)              VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(ns)
        .bind(key)
        .bind(config)
        .bind(chrono::Utc::now().timestamp_millis())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ---------------- 阅读统计（reading_stats 表：按用户 + 书 + 日期累计） ----------------

    /// 增量累计阅读统计（seconds/chars 累加到当日行；INSERT OR REPLACE 语义不丢历史）
    pub async fn record_reading_stats(
        &self,
        ns: &str,
        book_url: &str,
        date: &str,
        seconds: i64,
        chars: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO reading_stats (user_namespace, book_url, date, seconds, chars)              VALUES (?1, ?2, ?3, ?4, ?5)              ON CONFLICT(user_namespace, book_url, date)              DO UPDATE SET seconds = seconds + excluded.seconds, chars = chars + excluded.chars",
        )
        .bind(ns)
        .bind(book_url)
        .bind(date)
        .bind(seconds)
        .bind(chars)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 阅读统计汇总：今日/近 7 天/累计秒数 + 单书（秒数降序，关联书架书名）
    pub async fn get_reading_stats(&self, ns: &str) -> Result<ReadingStats> {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let week_start = (chrono::Utc::now() - chrono::Duration::days(6))
            .format("%Y-%m-%d")
            .to_string();
        let today_s: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(seconds), 0) FROM reading_stats WHERE user_namespace = ?1 AND date = ?2",
        )
        .bind(ns)
        .bind(&today)
        .fetch_one(&self.pool)
        .await?;
        let week_s: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(seconds), 0) FROM reading_stats WHERE user_namespace = ?1 AND date >= ?2",
        )
        .bind(ns)
        .bind(&week_start)
        .fetch_one(&self.pool)
        .await?;
        let total_s: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(seconds), 0) FROM reading_stats WHERE user_namespace = ?1",
        )
        .bind(ns)
        .fetch_one(&self.pool)
        .await?;
        let rows = sqlx::query_as::<_, (String, i64, i64)>(
            "SELECT book_url, COALESCE(SUM(seconds), 0), COALESCE(SUM(chars), 0)              FROM reading_stats WHERE user_namespace = ?1              GROUP BY book_url ORDER BY 2 DESC",
        )
        .bind(ns)
        .fetch_all(&self.pool)
        .await?;
        let mut books = Vec::with_capacity(rows.len());
        for (book_url, seconds, chars) in rows {
            let name = sqlx::query_scalar::<_, String>(
                "SELECT name FROM books WHERE user_namespace = ?1 AND book_url = ?2",
            )
            .bind(ns)
            .bind(&book_url)
            .fetch_optional(&self.pool)
            .await?
            .unwrap_or_else(|| book_url.clone());
            books.push(ReadingStatBook {
                book_url,
                name,
                seconds,
                chars,
            });
        }
        Ok(ReadingStats {
            today: today_s,
            week: week_s,
            total: total_s,
            books,
        })
    }

    // ---------------- 默认书源标记（system_settings：default_book_sources_{ns}） ----------------

    /// 默认书源列表（JSON 数组存 system_settings；无则空）
    pub async fn get_default_book_sources(&self, ns: &str) -> Result<Vec<String>> {
        let key = format!("default_book_sources_{ns}");
        match self.get_system_setting(&key).await? {
            Some(raw) => Ok(serde_json::from_str::<Vec<String>>(&raw).unwrap_or_default()),
            None => Ok(Vec::new()),
        }
    }

    /// 设置默认书源标记（覆盖式保存书源 URL 列表）
    pub async fn set_default_book_sources(&self, ns: &str, urls: &[String]) -> Result<()> {
        let key = format!("default_book_sources_{ns}");
        let raw = serde_json::to_string(urls).unwrap_or_else(|_| "[]".to_string());
        self.set_system_setting(&key, &raw).await
    }

    // ---------------- F-28 替换规则 ----------------

    /// 替换规则列表（按 order_num, id 排序；无用户规则回退 default，同书源语义）
    pub async fn get_replace_rules(&self, ns: &str) -> Result<Vec<crate::model::ReplaceRule>> {
        let rows = sqlx::query_as::<_, crate::model::ReplaceRule>(
            "SELECT * FROM replace_rules WHERE user_namespace = ?1 ORDER BY order_num, id",
        )
        .bind(ns)
        .fetch_all(&self.pool)
        .await?;
        if !rows.is_empty() || ns == "default" {
            return Ok(rows);
        }
        sqlx::query_as::<_, crate::model::ReplaceRule>(
            "SELECT * FROM replace_rules WHERE user_namespace = 'default' ORDER BY order_num, id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Into::into)
    }

    /// 保存单条替换规则（INSERT OR REPLACE，按 id 主键覆盖）
    pub async fn save_replace_rule(&self, ns: &str, rule: &crate::model::ReplaceRule) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO replace_rules (id, name, find, replace, enable, order_num, user_namespace)              VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .bind(&rule.id)
        .bind(&rule.name)
        .bind(&rule.find)
        .bind(&rule.replace)
        .bind(rule.enabled)
        .bind(rule.order)
        .bind(ns)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 批量保存替换规则（单事务：全部成功或全部回滚）
    pub async fn save_replace_rules(&self, ns: &str, rules: &[crate::model::ReplaceRule]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for rule in rules {
            sqlx::query(
                "INSERT OR REPLACE INTO replace_rules (id, name, find, replace, enable, order_num, user_namespace)                  VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .bind(&rule.id)
            .bind(&rule.name)
            .bind(&rule.find)
            .bind(&rule.replace)
            .bind(rule.enabled)
            .bind(rule.order)
            .bind(ns)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// 删除替换规则（按 id，仅限本命名空间）；返回受影响行数
    pub async fn delete_replace_rule(&self, ns: &str, id: &str) -> Result<u64> {
        let r = sqlx::query("DELETE FROM replace_rules WHERE user_namespace = ?1 AND id = ?2")
            .bind(ns)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected())
    }

    // ---------------- F-26 HttpTTS ----------------

    /// HttpTTS 听书源列表（按名称排序；无用户数据回退 default，同书源语义）
    pub async fn get_http_tts_list(&self, ns: &str) -> Result<Vec<crate::model::HttpTts>> {
        let rows = sqlx::query_as::<_, crate::model::HttpTts>(
            "SELECT * FROM http_tts_list WHERE user_namespace = ?1 ORDER BY name",
        )
        .bind(ns)
        .fetch_all(&self.pool)
        .await?;
        if !rows.is_empty() || ns == "default" {
            return Ok(rows);
        }
        sqlx::query_as::<_, crate::model::HttpTts>(
            "SELECT * FROM http_tts_list WHERE user_namespace = 'default' ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Into::into)
    }

    /// 保存 HttpTTS（INSERT OR REPLACE，按 url 主键覆盖）
    pub async fn save_http_tts(&self, ns: &str, tts: &crate::model::HttpTts) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO http_tts_list (url, name, type, user_namespace)              VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(&tts.url)
        .bind(&tts.name)
        .bind(tts.tts_type)
        .bind(ns)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 删除 HttpTTS（按 url，仅限本命名空间）；返回受影响行数
    pub async fn delete_http_tts(&self, ns: &str, url: &str) -> Result<u64> {
        let r = sqlx::query("DELETE FROM http_tts_list WHERE user_namespace = ?1 AND url = ?2")
            .bind(ns)
            .bind(url)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected())
    }

    // ---------------- 自定义 TXT 目录规则 ----------------

    /// 用户自定义 TXT 目录规则（按 serial_number, id 排序；仅用户自有，无 default 回退）
    pub async fn get_txt_toc_rules(&self, ns: &str) -> Result<Vec<crate::model::TxtTocRule>> {
        let rows = sqlx::query_as::<_, crate::model::TxtTocRule>(
            "SELECT * FROM txt_toc_rules WHERE user_namespace = ?1 ORDER BY serial_number, id",
        )
        .bind(ns)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// 保存单条 TXT 目录规则（INSERT OR REPLACE，按 id 主键覆盖）
    pub async fn save_txt_toc_rule(&self, ns: &str, rule: &crate::model::TxtTocRule) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO txt_toc_rules (id, name, rule, enable, serial_number, user_namespace)              VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(&rule.id)
        .bind(&rule.name)
        .bind(&rule.rule)
        .bind(rule.enable)
        .bind(rule.serial_number)
        .bind(ns)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 删除 TXT 目录规则（按 id，仅限本命名空间）；返回受影响行数
    pub async fn delete_txt_toc_rule(&self, ns: &str, id: &str) -> Result<u64> {
        let r = sqlx::query("DELETE FROM txt_toc_rules WHERE user_namespace = ?1 AND id = ?2")
            .bind(ns)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected())
    }

    /// 导入内置默认规则为用户规则（id 固定 default-{i}，幂等可重复导入）；返回导入条数
    pub async fn import_default_txt_toc_rules(&self, ns: &str) -> Result<usize> {
        let defaults = crate::service::local_book::DEFAULT_TOC_RULES;
        let mut tx = self.pool.begin().await?;
        for (i, rule) in defaults.iter().enumerate() {
            sqlx::query(
                "INSERT OR REPLACE INTO txt_toc_rules (id, name, rule, enable, serial_number, user_namespace)                  VALUES (?1, ?2, ?3, 1, ?4, ?5)",
            )
            .bind(format!("default-{}", i + 1))
            .bind(format!("默认规则{}", i + 1))
            .bind(*rule)
            .bind(i as i64)
            .bind(ns)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(defaults.len())
    }

    // ---------------- getSystemInfo 统计 ----------------

    /// 全部命名空间书籍总数
    pub async fn count_books(&self) -> Result<i64> {
        let count = sqlx::query_scalar("SELECT COUNT(*) FROM books")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    /// 全部命名空间书源总数
    pub async fn count_all_book_sources(&self) -> Result<i64> {
        let count = sqlx::query_scalar("SELECT COUNT(*) FROM book_sources")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    /// 本地书入库（books + 章节）
    pub async fn save_local_book(
        &self,
        ns: &str,
        info: &crate::model::book_chapter::BookInfo,
        imported: &crate::service::local_book::ImportedBook,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"INSERT OR REPLACE INTO books
            (book_url, name, author, kind, intro, language, publisher, published_at,
             cover_url, toc_url, origin, origin_name, group_name, type, user_namespace, created_at)
            VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,0,1,?13,?14)"#,
        )
        .bind(&info.book_url)
        .bind(&info.name)
        .bind(&info.author)
        .bind(&info.kind)
        .bind(&info.intro)
        .bind(&info.language)
        .bind(&info.publisher)
        .bind(&info.published_at)
        .bind(&info.cover_url)
        .bind(&info.toc_url)
        .bind(&info.origin)
        .bind(&info.origin_name)
        .bind(ns)
        .bind(chrono::Utc::now().timestamp_millis())
        .execute(&mut *tx)
        .await?;
        let chapters: Vec<(String, String)> = imported
            .chapters
            .iter()
            .map(|c| (c.title.clone(), c.content.clone()))
            .collect();
        for (i, (title, content)) in chapters.iter().enumerate() {
            sqlx::query(
                "INSERT OR REPLACE INTO book_chapters (book_url, chapter_index, title, content) VALUES (?1,?2,?3,?4)",
            )
            .bind(&info.book_url)
            .bind(i as i64)
            .bind(title)
            .bind(content)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    // ---------------- GAP 171：loc_book → DB 迁移 ----------------

    /// legacy loc_book 文件书迁移：文件解析结果写入 book_chapters（键 = 原 book_url），
    /// books.local_file 记录关联路径（保留原记录 origin 不动，local_file 非空即迁移标记）
    /// 返回是否写入（书不存在返回 false）
    pub async fn migrate_loc_book(
        &self,
        ns: &str,
        book_url: &str,
        local_file: Option<&str>,
        mtime: i64,
        size: i64,
        chapters: &[(String, String)],
        cover: Option<&[u8]>,
    ) -> Result<bool> {
        let exists: bool = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM books WHERE user_namespace = ?1 AND book_url = ?2",
        )
        .bind(ns)
        .bind(book_url)
        .fetch_one(&self.pool)
        .await?
            > 0;
        if !exists {
            return Ok(false);
        }
        let mut tx = self.pool.begin().await?;
        // 覆盖式重建章节（重复迁移幂等）
        sqlx::query("DELETE FROM book_chapters WHERE book_url = ?1")
            .bind(book_url)
            .execute(&mut *tx)
            .await?;
        for (i, (title, content)) in chapters.iter().enumerate() {
            sqlx::query(
                "INSERT OR REPLACE INTO book_chapters (book_url, chapter_index, title, content) VALUES (?1,?2,?3,?4)",
            )
            .bind(book_url)
            .bind(i as i64)
            .bind(title)
            .bind(content)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "UPDATE books SET local_file = ?3, local_file_mtime = ?4, local_file_size = ?5, local_file_deleted = 0              WHERE user_namespace = ?1 AND book_url = ?2",
        )
        .bind(ns)
        .bind(book_url)
        .bind(local_file)
        .bind(mtime)
        .bind(size)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        // 封面落盘（与上传导入一致）
        if let Some(cover) = cover {
            if !cover.is_empty() {
                let cover_dir = self
                    .config
                    .storage_dir()
                    .join("assets")
                    .join(ns)
                    .join("covers");
                let _ = std::fs::create_dir_all(&cover_dir);
                let file_id = format!("{}.jpg", uuid::Uuid::new_v4());
                if std::fs::write(cover_dir.join(&file_id), cover).is_ok() {
                    let _ = self
                        .update_book_cover(ns, book_url, &format!("/assets/{ns}/covers/{file_id}"))
                        .await;
                }
            }
        }
        Ok(true)
    }

    /// GAP 171：查全部 legacy loc_book 文件书（migrateLocBook all 用）
    pub async fn list_loc_book_books(&self, ns: &str) -> Result<Vec<crate::model::Book>> {
        sqlx::query_as::<_, crate::model::Book>(
            "SELECT * FROM books WHERE user_namespace = ?1 AND origin = 'loc_book' ORDER BY name",
        )
        .bind(ns)
        .fetch_all(&self.pool)
        .await
        .map_err(Into::into)
    }

    // ---------------- GAP 170 本地书双轨同步 ----------------

    /// 查询有 local_file 关联的书（含已删除标记的——文件重现时用于重链/重扫）
    pub async fn list_linked_local_books(&self, ns: &str) -> Result<Vec<Book>> {
        let rows = sqlx::query_as::<_, Book>(
            "SELECT * FROM books WHERE user_namespace = ?1 AND local_file IS NOT NULL AND local_file != ''",
        )
        .bind(ns)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// 查询无文件关联的 local:// DB 书（对账任务自动生成 epub 落书仓目录用）
    pub async fn list_local_db_books_without_file(&self, ns: &str) -> Result<Vec<Book>> {
        let rows = sqlx::query_as::<_, Book>(
            "SELECT * FROM books WHERE user_namespace = ?1 AND book_url LIKE 'local://%' AND (local_file IS NULL OR local_file = '')",
        )
        .bind(ns)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// 写入本地书文件关联（local_file=None 时仅更新变更检测/删除标记字段）
    pub async fn link_local_file(
        &self,
        ns: &str,
        book_url: &str,
        local_file: Option<&str>,
        mtime: i64,
        size: i64,
        deleted: bool,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE books SET local_file = ?3, local_file_mtime = ?4, local_file_size = ?5, local_file_deleted = ?6              WHERE user_namespace = ?1 AND book_url = ?2",
        )
        .bind(ns)
        .bind(book_url)
        .bind(local_file)
        .bind(mtime)
        .bind(size)
        .bind(deleted)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 替换本地书全部章节（重扫用：事务内先删后插——旧章残留清理，新章序从 0 开始）
    pub async fn replace_chapters(&self, book_url: &str, chapters: &[(String, String)]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM book_chapters WHERE book_url = ?1")
            .bind(book_url)
            .execute(&mut *tx)
            .await?;
        for (i, (title, content)) in chapters.iter().enumerate() {
            sqlx::query(
                "INSERT INTO book_chapters (book_url, chapter_index, title, content) VALUES (?1, ?2, ?3, ?4)",
            )
            .bind(book_url)
            .bind(i as i64)
            .bind(title)
            .bind(content)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// 本地书章节（含正文，全量——对账任务 epub 生成用）
    pub async fn list_chapters_full(&self, book_url: &str) -> Result<Vec<(i64, String, String)>> {
        let rows = sqlx::query_as::<_, (i64, String, String)>(
            "SELECT chapter_index, title, content FROM book_chapters WHERE book_url = ?1 ORDER BY chapter_index",
        )
        .bind(book_url)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// 更新封面 URL（导入后写封面文件）
    pub async fn update_book_cover(&self, ns: &str, book_url: &str, cover_url: &str) -> Result<u64> {
        let r = sqlx::query("UPDATE books SET cover_url = ?3 WHERE user_namespace = ?1 AND book_url = ?2")
            .bind(ns)
            .bind(book_url)
            .bind(cover_url)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected())
    }

    /// 本地书入库（books + 章节）
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

    // ---------------- GAP 59 多设备 token（users.token_map） ----------------

    /// 登录：追加 token 到 token_map（JSON 数组，上限 5，最旧被丢），同时刷新主 token 与 last_login_at
    pub async fn add_user_token(&self, username: &str, token: &str, last_login_at: i64) -> Result<()> {
        let user = self.find_user(username).await?;
        let map_json = crate::model::user::token_map_push(
            &user.as_ref().and_then(|u| u.token_map.clone()),
            token,
        );
        sqlx::query("UPDATE users SET token = ?1, token_map = ?2, last_login_at = ?3 WHERE username = ?4")
            .bind(token)
            .bind(&map_json)
            .bind(last_login_at)
            .bind(username)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// 登出：从 token_map 移除指定 token；若移除的是主 token 则同时清空主 token
    /// （其他设备 token 不受影响——多设备会话互不干扰）。返回受影响行数。
    pub async fn remove_user_token(&self, username: &str, token: &str) -> Result<u64> {
        let user = self.find_user(username).await?;
        let Some(user) = user else { return Ok(0) };
        let (map_json, removed) =
            crate::model::user::token_map_remove(&user.token_map, token);
        let clear_main = !user.token.is_empty() && user.token == token;
        let main_token = if clear_main { "" } else { user.token.as_str() };
        sqlx::query("UPDATE users SET token = ?1, token_map = ?2 WHERE username = ?3")
            .bind(main_token)
            .bind(&map_json)
            .bind(username)
            .execute(&self.pool)
            .await?;
        Ok(if removed || clear_main { 1 } else { 0 })
    }

    /// 查询某命名空间的书架（按插入顺序，兼容 legacy bookshelf.json 数组顺序）
    pub async fn list_books(&self, namespace: &str) -> Result<Vec<Book>> {
        let books = sqlx::query_as::<_, Book>(
            r#"
            SELECT *
            FROM books
            WHERE user_namespace = ?1
            ORDER BY dur_chapter_time DESC, rowid DESC
            "#,
        )
        .bind(namespace)
        .fetch_all(&self.pool)
        .await?;
        Ok(books)
    }

    // ---------------- F-7 书源数上限 ----------------

    /// 某命名空间现有书源数（仅用户自有书源，不含 default 回退）
    pub async fn count_book_sources(&self, ns: &str) -> Result<i64> {
        let count = sqlx::query_scalar(
            "SELECT COUNT(*) FROM book_sources WHERE user_namespace = ?1",
        )
        .bind(ns)
        .fetch_one(&self.pool)
        .await?;
        Ok(count)
    }

    /// 用户书源上限（users.book_source_limit；用户不存在返回 None——非 secure 模式不限制）
    pub async fn book_source_limit_for(&self, ns: &str) -> Result<Option<i64>> {
        let limit = sqlx::query_scalar(
            "SELECT book_source_limit FROM users WHERE username = ?1",
        )
        .bind(ns)
        .fetch_optional(&self.pool)
        .await?;
        Ok(limit)
    }

    // ---------------- F-25/F-34 用户会话 ----------------

    /// F-25 退出登录：清空用户 token（token 立即失效）
    pub async fn logout_user(&self, username: &str) -> Result<u64> {
        let r = sqlx::query("UPDATE users SET token = '' WHERE username = ?1")
            .bind(username)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected())
    }

    /// F-34 清理不活跃用户：删除 last_login_at < before_ms 的 users 行（简化：仅删用户行，
    /// 用户数据目录/命名空间数据保留；except 用户受保护不删）。返回被删用户名列表
    /// GAP #95：清理不活跃用户（users 行 + 用户级数据行 + 数据目录；except 用户除外）
    pub async fn clear_inactive_users(&self, before_ms: i64, except: Option<&str>) -> Result<Vec<String>> {
        let mut tx = self.pool.begin().await?;
        let rows: Vec<String> =
            sqlx::query_scalar("SELECT username FROM users WHERE last_login_at < ?1")
                .bind(before_ms)
                .fetch_all(&mut *tx)
                .await?;
        let mut deleted = Vec::new();
        for username in rows {
            if except == Some(username.as_str()) {
                continue;
            }
            sqlx::query("DELETE FROM users WHERE username = ?1")
                .bind(&username)
                .execute(&mut *tx)
                .await?;
            delete_user_rows(&mut tx, &username).await?;
            deleted.push(username);
        }
        tx.commit().await?;
        for username in &deleted {
            self.remove_user_data_dir(username);
        }
        Ok(deleted)
    }

    // ---------------- F-32 用户管理 ----------------

    /// 全部用户列表（含权限/启用状态；按创建时间排序）
    pub async fn list_users(&self) -> Result<Vec<User>> {
        let users = sqlx::query_as::<_, User>("SELECT * FROM users ORDER BY created_at, username")
            .fetch_all(&self.pool)
            .await?;
        Ok(users)
    }

    /// 更新用户权限/限额（None 字段不更新；用户不存在返回 0 行）
    #[allow(clippy::too_many_arguments)]
    pub async fn update_user_permissions(
        &self,
        username: &str,
        enable_webdav: Option<bool>,
        enable_local_store: Option<bool>,
        enable_book_source: Option<bool>,
        enable_rss_source: Option<bool>,
        book_source_limit: Option<i64>,
        book_limit: Option<i64>,
    ) -> Result<u64> {
        let r = sqlx::query(
            r#"
            UPDATE users SET
                enable_webdav     = COALESCE(?1, enable_webdav),
                enable_local_store = COALESCE(?2, enable_local_store),
                enable_book_source = COALESCE(?3, enable_book_source),
                enable_rss_source  = COALESCE(?4, enable_rss_source),
                book_source_limit  = COALESCE(?5, book_source_limit),
                book_limit         = COALESCE(?6, book_limit)
            WHERE username = ?7
            "#,
        )
        .bind(enable_webdav)
        .bind(enable_local_store)
        .bind(enable_book_source)
        .bind(enable_rss_source)
        .bind(book_source_limit)
        .bind(book_limit)
        .bind(username)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }

    /// GAP #95：删除用户并清理全部用户数据（用户级表行 + storage/data/{username} 目录）。
    ///
    /// 表：books/book_sources/book_source_cookies/reading_stats/user_config/rss_sources/
    /// rss_articles/bookmarks/book_groups/replace_rules/http_tts_list/source_subs/txt_toc_rules；
    /// 章节/目录缓存仅当该书 URL 不再被其他用户拥有时删除（book_url 为全局主键）；
    /// 目录递归删除（含 webdav/opds_files 等）。用户不存在返回 0。
    pub async fn delete_user(&self, username: &str) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        let r = sqlx::query("DELETE FROM users WHERE username = ?1")
            .bind(username)
            .execute(&mut *tx)
            .await?;
        if r.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(0);
        }
        delete_user_rows(&mut tx, username).await?;
        tx.commit().await?;
        self.remove_user_data_dir(username);
        Ok(1)
    }

    /// 删除 storage/data/{username} 目录（递归——含 webdav/opds_files 等）；失败仅告警
    fn remove_user_data_dir(&self, username: &str) {
        // 防御：注册已限 ^[a-zA-Z0-9]+$，再挡一层路径穿越
        if username.is_empty()
            || username.contains(['/', '\\'])
            || username.contains("..")
            || !username.chars().all(|c| c.is_ascii_alphanumeric())
        {
            tracing::warn!("跳过用户数据目录清理（非法用户名）: {username}");
            return;
        }
        let dir = self.config.storage_dir().join("data").join(username);
        if !dir.exists() {
            return;
        }
        match std::fs::remove_dir_all(&dir) {
            Ok(_) => tracing::info!("已删除用户数据目录: {}", dir.display()),
            Err(e) => tracing::warn!("删除用户数据目录失败（文件占用？）{}: {e}", dir.display()),
        }
    }

    /// 重置用户密码（新 salt + 加密密码；清空 token 使旧会话立即失效）
    pub async fn reset_user_password(
        &self,
        username: &str,
        salt: &str,
        encrypted_password: &str,
    ) -> Result<u64> {
        let r = sqlx::query("UPDATE users SET password = ?1, salt = ?2, token = '' WHERE username = ?3")
            .bind(encrypted_password)
            .bind(salt)
            .bind(username)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected())
    }

    // ---------------- F-35 定时书架更新 ----------------

    /// F-35 可更新书架书（can_update=1，全命名空间）
    pub async fn list_updatable_books(&self) -> Result<Vec<Book>> {
        let books = sqlx::query_as::<_, Book>("SELECT * FROM books WHERE can_update = 1")
            .fetch_all(&self.pool)
            .await?;
        Ok(books)
    }

    /// F-35 回写更新检查结果：最新章节标题/总数/检查时间/检查次数
    pub async fn update_book_update_info(
        &self,
        ns: &str,
        book_url: &str,
        latest_title: Option<&str>,
        total_num: i64,
        checked_at: i64,
    ) -> Result<u64> {
        let r = sqlx::query(
            "UPDATE books SET                 latest_chapter_title = COALESCE(?3, latest_chapter_title),                 latest_chapter_time = CASE WHEN ?3 IS NOT NULL THEN ?4 ELSE latest_chapter_time END,                 total_chapter_num = ?5,                 last_check_time = ?6,                 last_check_count = last_check_count + 1                 WHERE user_namespace = ?1 AND book_url = ?2",
        )
        .bind(ns)
        .bind(book_url)
        .bind(latest_title)
        .bind(checked_at)
        .bind(total_num)
        .bind(checked_at)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }

    // ---------------- F-39 WebDAV 备份 ----------------

    /// F-39 书架数据打包 zip（bookshelf/bookSource/bookmark/bookGroup/rssSources）写入
    /// storage/data/{ns}/webdav/legado/backup-{ts}.zip；返回 zip 文件路径
    pub async fn create_backup_zip(&self, ns: &str) -> Result<String> {
        let ts = chrono::Utc::now().format("%Y-%m-%d-%H%M%S");
        self.write_backup_zip(ns, &format!("backup-{ts}")).await
    }

    /// 打包 zip 写入 storage/data/{ns}/webdav/legado/{stem}.zip（
    /// GAP #57 自动备份与手动备份共用核心）；返回 zip 文件路径
    pub(crate) async fn write_backup_zip(&self, ns: &str, stem: &str) -> Result<String> {
        let legado = self
            .config
            .storage_dir()
            .join("data")
            .join(ns)
            .join("webdav")
            .join("legado");
        std::fs::create_dir_all(&legado)?;
        let zip_path = legado.join(format!("{stem}.zip"));

        // 收集数据（legacy backupFileNames 子集：Rust 有对应表/模型的部分）
        let books = self.list_books(ns).await?;
        let sources = self.get_book_sources(ns).await?;
        let bookmarks = sqlx::query_as::<_, crate::model::Bookmark>(
            "SELECT * FROM bookmarks WHERE user_namespace = ?1 ORDER BY created_at DESC, rowid DESC",
        )
        .bind(ns)
        .fetch_all(&self.pool)
        .await?;
        let groups = self.list_book_groups(ns).await?;
        let rss_sources = self.get_rss_sources(ns).await?;

        let file = std::fs::File::create(&zip_path)?;
        let mut writer = zip::ZipWriter::new(file);
        write_zip_entry(&mut writer, "bookshelf.json", &serde_json::to_vec_pretty(&books)?)?;
        write_zip_entry(&mut writer, "bookSource.json", &serde_json::to_vec_pretty(&sources)?)?;
        write_zip_entry(&mut writer, "bookmark.json", &serde_json::to_vec_pretty(&bookmarks)?)?;
        write_zip_entry(&mut writer, "bookGroup.json", &serde_json::to_vec_pretty(&groups)?)?;
        write_zip_entry(&mut writer, "rssSources.json", &serde_json::to_vec_pretty(&rss_sources)?)?;
        writer.finish()?;

        tracing::info!("备份完成 [{ns}]: {}", zip_path.display());
        Ok(zip_path.to_string_lossy().into_owned())
    }

    // ---------------- GAP #57 自动备份 ----------------

    /// GAP #57：清理自动备份 zip（webdav/legado/auto-*.zip），仅保留最近 keep 份；
    /// 返回删除数（文件名按日期字典序即时间序）
    pub fn prune_auto_backups(&self, ns: &str, keep: usize) -> usize {
        let legado = self
            .config
            .storage_dir()
            .join("data")
            .join(ns)
            .join("webdav")
            .join("legado");
        let Ok(entries) = std::fs::read_dir(&legado) else {
            return 0;
        };
        let mut files: Vec<std::path::PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("auto-") && n.ends_with(".zip"))
                    .unwrap_or(false)
            })
            .collect();
        files.sort();
        let mut removed = 0usize;
        while files.len() > keep {
            let oldest = files.remove(0);
            match std::fs::remove_file(&oldest) {
                Ok(_) => removed += 1,
                Err(e) => tracing::warn!("删除旧自动备份失败 {}: {e}", oldest.display()),
            }
        }
        removed
    }

    /// 定时任务命名空间集合：非 secure → [default]；secure → 全部用户 + 残留 default 目录
    pub async fn schedule_namespaces(&self) -> Vec<String> {
        if !self.config.secure {
            return vec!["default".to_string()];
        }
        let mut nss: Vec<String> = Vec::new();
        if let Ok(users) = self.list_users().await {
            nss.extend(users.iter().map(|u| u.username.clone()));
        }
        // secure 化之前遗留的 default 数据目录也纳入（存在才处理）
        let default_dir = self.config.storage_dir().join("data").join("default");
        if default_dir.is_dir() && !nss.iter().any(|n| n == "default") {
            nss.push("default".to_string());
        }
        nss
    }

    // ---------------- F-55 备份恢复（restoreFromZip / restoreFromWebdav 共用核心） ----------------

    /// F-55：从备份 zip 字节恢复（restoreFromZip/restoreFromWebdav 共用核心）
    ///
    /// 结构探测：条目在 zip 根 或 config/ 目录下（兼容 legacy 备份 zip 两种布局）；
    /// 书架文件兼容 bookshelf.json / books.json 两个命名。
    ///
    /// 逐项幂等：已存在时 overwrite=true 覆盖、否则跳过（计数进 skipped）；
    /// namespace 一律为当前用户（备份内 user_namespace 不生效）。
    pub async fn restore_backup_zip(
        &self,
        ns: &str,
        zip_bytes: &[u8],
        overwrite: bool,
    ) -> Result<RestoreReport> {
        let cursor = std::io::Cursor::new(zip_bytes.to_vec());
        let mut archive = zip::ZipArchive::new(cursor)
            .map_err(|e| anyhow::anyhow!("备份文件不是有效的 zip：{e}"))?;
        // 预读全部条目到内存（zip 读取是同步 IO，避免跨 await 持有 archive）
        let mut entries: std::collections::HashMap<String, Vec<u8>> = std::collections::HashMap::new();
        use std::io::Read;
        for i in 0..archive.len() {
            let mut f = archive.by_index(i)?;
            let name = f.name().to_string();
            let mut bytes = Vec::new();
            f.read_to_end(&mut bytes)?;
            entries.insert(name, bytes);
        }

        // 条目读取：根 或 config/ 前缀（legacy 两种布局）
        let entry = |name: &str| -> Option<&Vec<u8>> {
            entries
                .get(name)
                .or_else(|| entries.get(&format!("config/{name}")))
        };

        // 结构探测：一个识别条目都没有 → 拒绝（避免把任意 zip 当备份）
        let recognized = [
            "bookSource.json",
            "bookshelf.json",
            "books.json",
            "bookGroup.json",
            "replaceRule.json",
            "txtTocRule.json",
            "rssSources.json",
            "userConfig.json",
            "httpTTS.json",
            "bookmark.json",
        ];
        if !recognized.iter().any(|n| entry(n).is_some()) {
            return Err(anyhow::anyhow!("备份文件中没有可恢复的数据"));
        }

        let mut report = RestoreReport::default();

        // 书源（按 bookSourceUrl 覆盖或跳过）
        if let Some(bytes) = entry("bookSource.json") {
            match serde_json::from_slice::<Vec<crate::model::BookSource>>(bytes) {
                Ok(items) => {
                    for s in items {
                        if s.book_source_url.trim().is_empty() {
                            report.skipped.sources += 1;
                            continue;
                        }
                        if !overwrite
                            && self
                                .table_exists(
                                    "SELECT 1 FROM book_sources WHERE user_namespace = ?1 AND book_source_url = ?2",
                                    ns,
                                    &s.book_source_url,
                                )
                                .await?
                        {
                            report.skipped.sources += 1;
                        } else {
                            self.save_book_source(ns, &s).await?;
                            report.restored.sources += 1;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("restore [{ns}] bookSource.json 解析失败: {e}");
                    report.skipped.sources += 1;
                }
            }
        }

        // 书架（按 book_url upsert，namespace=当前用户；bookshelf.json / books.json 兼容）
        if let Some(bytes) = entry("bookshelf.json").or_else(|| entry("books.json")) {
            match serde_json::from_slice::<Vec<Book>>(bytes) {
                Ok(items) => {
                    for mut b in items {
                        if b.book_url.trim().is_empty() {
                            report.skipped.books += 1;
                            continue;
                        }
                        b.user_namespace = ns.to_string();
                        if !overwrite
                            && self
                                .table_exists(
                                    "SELECT 1 FROM books WHERE user_namespace = ?1 AND book_url = ?2",
                                    ns,
                                    &b.book_url,
                                )
                                .await?
                        {
                            report.skipped.books += 1;
                        } else {
                            self.upsert_book(ns, &b).await?;
                            report.restored.books += 1;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("restore [{ns}] bookshelf.json 解析失败: {e}");
                    report.skipped.books += 1;
                }
            }
        }

        // 书架分组（按 id 覆盖或跳过；id<=0 时自增新建）
        if let Some(bytes) = entry("bookGroup.json") {
            match serde_json::from_slice::<Vec<crate::model::BookGroup>>(bytes) {
                Ok(items) => {
                    for g in items {
                        if g.name.trim().is_empty() {
                            report.skipped.groups += 1;
                            continue;
                        }
                        if !overwrite && g.id > 0 {
                            let exists: bool = sqlx::query_scalar::<_, i64>(
                                "SELECT 1 FROM book_groups WHERE user_namespace = ?1 AND id = ?2",
                            )
                            .bind(ns)
                            .bind(g.id)
                            .fetch_optional(&self.pool)
                            .await?
                            .is_some();
                            if exists {
                                report.skipped.groups += 1;
                                continue;
                            }
                        }
                        self.save_book_group(ns, &g).await?;
                        report.restored.groups += 1;
                    }
                }
                Err(e) => {
                    tracing::warn!("restore [{ns}] bookGroup.json 解析失败: {e}");
                    report.skipped.groups += 1;
                }
            }
        }

        // 替换规则（按 id）
        if let Some(bytes) = entry("replaceRule.json") {
            match serde_json::from_slice::<Vec<crate::model::ReplaceRule>>(bytes) {
                Ok(items) => {
                    for mut r in items {
                        if r.id.trim().is_empty() {
                            r.id = uuid::Uuid::new_v4().simple().to_string();
                        }
                        if r.name.trim().is_empty() {
                            report.skipped.rules += 1;
                            continue;
                        }
                        if !overwrite
                            && self
                                .table_exists(
                                    "SELECT 1 FROM replace_rules WHERE user_namespace = ?1 AND id = ?2",
                                    ns,
                                    &r.id,
                                )
                                .await?
                        {
                            report.skipped.rules += 1;
                        } else {
                            self.save_replace_rule(ns, &r).await?;
                            report.restored.rules += 1;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("restore [{ns}] replaceRule.json 解析失败: {e}");
                    report.skipped.rules += 1;
                }
            }
        }

        // 自定义 TXT 目录规则（按 id）
        if let Some(bytes) = entry("txtTocRule.json") {
            match serde_json::from_slice::<Vec<crate::model::TxtTocRule>>(bytes) {
                Ok(items) => {
                    for mut r in items {
                        if r.id.trim().is_empty() {
                            r.id = uuid::Uuid::new_v4().simple().to_string();
                        }
                        if r.name.trim().is_empty() || r.rule.trim().is_empty() {
                            report.skipped.txt_rules += 1;
                            continue;
                        }
                        if !overwrite
                            && self
                                .table_exists(
                                    "SELECT 1 FROM txt_toc_rules WHERE user_namespace = ?1 AND id = ?2",
                                    ns,
                                    &r.id,
                                )
                                .await?
                        {
                            report.skipped.txt_rules += 1;
                        } else {
                            self.save_txt_toc_rule(ns, &r).await?;
                            report.restored.txt_rules += 1;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("restore [{ns}] txtTocRule.json 解析失败: {e}");
                    report.skipped.txt_rules += 1;
                }
            }
        }

        // RSS 源（按 sourceUrl；raw_json 存条目原文，未知字段不丢）
        if let Some(bytes) = entry("rssSources.json") {
            match serde_json::from_slice::<serde_json::Value>(bytes) {
                Ok(serde_json::Value::Array(arr)) => {
                    for v in arr {
                        let mut s: crate::model::RssSource = match serde_json::from_value(v.clone()) {
                            Ok(s) => s,
                            Err(_) => {
                                report.skipped.rss += 1;
                                continue;
                            }
                        };
                        if s.source_url.trim().is_empty() || s.source_name.trim().is_empty() {
                            report.skipped.rss += 1;
                            continue;
                        }
                        s.raw_json = Some(v.to_string());
                        if !overwrite
                            && self
                                .table_exists(
                                    "SELECT 1 FROM rss_sources WHERE user_namespace = ?1 AND rss_source_url = ?2",
                                    ns,
                                    &s.source_url,
                                )
                                .await?
                        {
                            report.skipped.rss += 1;
                        } else {
                            self.save_rss_source(ns, &s).await?;
                            report.restored.rss += 1;
                        }
                    }
                }
                Ok(_) => report.skipped.rss += 1,
                Err(e) => {
                    tracing::warn!("restore [{ns}] rssSources.json 解析失败: {e}");
                    report.skipped.rss += 1;
                }
            }
        }

        // 用户配置（userConfig.json = {键: 值} 对象；或 [{key, value}] 数组）
        if let Some(bytes) = entry("userConfig.json") {
            match serde_json::from_slice::<serde_json::Value>(bytes) {
                Ok(serde_json::Value::Object(map)) => {
                    for (k, v) in map {
                        let raw = match &v {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        if !overwrite && self.get_user_config(ns, &k).await?.is_some() {
                            report.skipped.config += 1;
                        } else {
                            self.save_user_config(ns, &k, &raw).await?;
                            report.restored.config += 1;
                        }
                    }
                }
                Ok(serde_json::Value::Array(arr)) => {
                    for v in arr {
                        let Some(k) = v.get("key").and_then(|x| x.as_str()) else {
                            report.skipped.config += 1;
                            continue;
                        };
                        let raw = match v.get("value") {
                            Some(serde_json::Value::String(s)) => s.clone(),
                            Some(other) => other.to_string(),
                            None => String::new(),
                        };
                        if !overwrite && self.get_user_config(ns, k).await?.is_some() {
                            report.skipped.config += 1;
                        } else {
                            self.save_user_config(ns, k, &raw).await?;
                            report.restored.config += 1;
                        }
                    }
                }
                Ok(_) => report.skipped.config += 1,
                Err(e) => {
                    tracing::warn!("restore [{ns}] userConfig.json 解析失败: {e}");
                    report.skipped.config += 1;
                }
            }
        }

        // HttpTTS 听书源（按 url）
        if let Some(bytes) = entry("httpTTS.json") {
            match serde_json::from_slice::<Vec<crate::model::HttpTts>>(bytes) {
                Ok(items) => {
                    for t in items {
                        if t.url.trim().is_empty() || t.name.trim().is_empty() {
                            report.skipped.tts += 1;
                            continue;
                        }
                        if !overwrite
                            && self
                                .table_exists(
                                    "SELECT 1 FROM http_tts_list WHERE user_namespace = ?1 AND url = ?2",
                                    ns,
                                    &t.url,
                                )
                                .await?
                        {
                            report.skipped.tts += 1;
                        } else {
                            self.save_http_tts(ns, &t).await?;
                            report.restored.tts += 1;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("restore [{ns}] httpTTS.json 解析失败: {e}");
                    report.skipped.tts += 1;
                }
            }
        }

        // 书签（按 bookUrl+title）——备份 zip 含 bookmark.json，一并恢复
        if let Some(bytes) = entry("bookmark.json") {
            match serde_json::from_slice::<Vec<crate::model::Bookmark>>(bytes) {
                Ok(items) => {
                    for bm in items {
                        if bm.book_url.trim().is_empty() || bm.title.trim().is_empty() {
                            report.skipped.bookmarks += 1;
                            continue;
                        }
                        let exists: bool = sqlx::query_scalar::<_, i64>(
                            "SELECT 1 FROM bookmarks WHERE user_namespace = ?1 AND book_url = ?2 AND title = ?3",
                        )
                        .bind(ns)
                        .bind(&bm.book_url)
                        .bind(&bm.title)
                        .fetch_optional(&self.pool)
                        .await?
                        .is_some();
                        if !overwrite && exists {
                            report.skipped.bookmarks += 1;
                        } else {
                            self.save_bookmark(ns, &bm).await?;
                            report.restored.bookmarks += 1;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("restore [{ns}] bookmark.json 解析失败: {e}");
                    report.skipped.bookmarks += 1;
                }
            }
        }

        tracing::info!(
            "恢复完成 [{ns}] overwrite={overwrite}: restored={:?} skipped={:?}",
            report.restored,
            report.skipped
        );
        Ok(report)
    }

    /// 表存在性探测（幂等判断共用；两参版本）
    async fn table_exists(&self, sql: &str, a: &str, b: &str) -> Result<bool> {
        Ok(sqlx::query_scalar::<_, i64>(sql)
            .bind(a)
            .bind(b)
            .fetch_optional(&self.pool)
            .await?
            .is_some())
    }
}

/// 各类目恢复计数（restored/skipped 共用）
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreCounts {
    /// 书源（按 bookSourceUrl）
    pub sources: u64,
    /// 书架书（按 book_url）
    pub books: u64,
    /// 书架分组（按 id）
    pub groups: u64,
    /// 替换规则（按 id）
    pub rules: u64,
    /// 自定义 TXT 目录规则（按 id）
    #[serde(rename = "txtRules")]
    pub txt_rules: u64,
    /// RSS 源（按 sourceUrl）
    pub rss: u64,
    /// 用户配置（按配置键）
    pub config: u64,
    /// HttpTTS 听书源（按 url）
    pub tts: u64,
    /// 书签（按 bookUrl+title）
    pub bookmarks: u64,
}

/// 恢复报告（restoreFromZip / restoreFromWebdav 返回：restored/skipped 各类目计数）
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReport {
    pub restored: RestoreCounts,
    pub skipped: RestoreCounts,
}

/// zip 单条目写入（F-39）
fn write_zip_entry(
    writer: &mut zip::ZipWriter<std::fs::File>,
    name: &str,
    bytes: &[u8],
) -> Result<()> {
    use std::io::Write;
    writer.start_file(name, zip::write::FileOptions::default())?;
    writer.write_all(bytes)?;
    Ok(())
}

/// F-35：扫描 books 表 can_update=1 的书 → analyze_toc → 回写
/// latest_chapter_title / total_chapter_num（单本失败跳过，不影响其余）
pub async fn run_shelf_update(storage: &Storage) -> Result<usize> {
    let books = storage.list_updatable_books().await?;
    let mut updated = 0usize;
    for book in books {
        // 本地书（local:// 或 storage 文件型）无书源可抓，跳过
        if book.origin == "local"
            || book.book_url.starts_with("local://")
            || book.book_url.ends_with(".txt")
        {
            continue;
        }
        if book.toc_url.trim().is_empty() {
            continue;
        }
        // 书源缺失（用户/系统均无）→ 无法抓取，跳过
        let Ok(Some(source)) = storage
            .find_book_source(&book.user_namespace, &book.origin)
            .await
        else {
            continue;
        };
        match crate::service::book::analyze_toc(&book.user_namespace, &book.toc_url, &source, 20).await {
            Ok(chapters) if !chapters.is_empty() => {
                let non_volume: Vec<&crate::model::book_chapter::BookChapter> =
                    chapters.iter().filter(|c| !c.is_volume).collect();
                let latest = non_volume.last().map(|c| c.title.clone());
                let total = non_volume.len() as i64;
                let now = chrono::Utc::now().timestamp_millis();
                match storage
                    .update_book_update_info(
                        &book.user_namespace,
                        &book.book_url,
                        latest.as_deref(),
                        total,
                        now,
                    )
                    .await
                {
                    Ok(_) => updated += 1,
                    Err(e) => tracing::warn!("书架更新回写失败 [{}]: {e:#}", book.book_url),
                }
            }
            Ok(_) => {} // 无章节规则/空目录：无可更新内容，跳过
            Err(e) => tracing::warn!("书架更新跳过 [{}]: {e:#}", book.book_url),
        }
    }
    Ok(updated)
}

/// 幂等补列：列不存在则 ALTER TABLE ADD COLUMN（旧库升级用）
/// 规范化 baseUrl（scheme://host[:port]，去尾斜杠/路径/查询）——
/// 书源 cookie 按 base 匹配：请求 https://a.com/book/1 命中 source_url https://a.com
/// GAP #95：删除用户命名空间下的全部用户级数据行（事务内调用）
///
/// 覆盖全部含 user_namespace 列的表；章节/目录缓存（全局 book_url 主键）仅当
/// 该书 URL 不再被其他用户拥有时才删除，避免误删他用户章节。
async fn delete_user_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    username: &str,
) -> Result<()> {
    sqlx::query(
        "DELETE FROM book_chapters WHERE book_url IN (
            SELECT book_url FROM books WHERE user_namespace = ?1
              AND book_url NOT IN (SELECT book_url FROM books WHERE user_namespace != ?1))",
    )
    .bind(username)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "DELETE FROM toc_cache WHERE book_url IN (
            SELECT book_url FROM books WHERE user_namespace = ?1
              AND book_url NOT IN (SELECT book_url FROM books WHERE user_namespace != ?1))",
    )
    .bind(username)
    .execute(&mut **tx)
    .await?;
    // 表名单为内部硬编码常量，无注入面
    for table in [
        "books",
        "book_sources",
        "book_source_cookies",
        "reading_stats",
        "user_config",
        "rss_sources",
        "rss_articles",
        "bookmarks",
        "book_groups",
        "replace_rules",
        "http_tts_list",
        "source_subs",
        "txt_toc_rules",
    ] {
        sqlx::query(&format!("DELETE FROM {table} WHERE user_namespace = ?1"))
            .bind(username)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

pub(crate) fn normalize_base(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    // 无 scheme 时补 https://（容忍裸 host 写法）
    let with_scheme = if url.contains("://") {
        url.to_string()
    } else {
        format!("https://{url}")
    };
    let parsed = url::Url::parse(&with_scheme).ok()?;
    let host = parsed.host_str()?;
    let port = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
    Some(format!("{}://{host}{port}", parsed.scheme()))
}

async fn ensure_column(pool: &SqlitePool, table: &str, column: &str) -> anyhow::Result<()> {
    ensure_column_typed(pool, table, column, "TEXT").await
}

/// GAP 117：从 cover_url（/assets/{ns}/covers/{file}）提取文件名；
/// 仅接受纯文件名（无路径分隔符，防穿越）且命名空间匹配时返回
fn cover_file_name(ns: &str, cover_url: &str) -> Option<String> {
    let prefix = format!("/assets/{ns}/covers/");
    let file = cover_url.strip_prefix(&prefix)?;
    if file.is_empty()
        || file.contains('/')
        || file.contains('\\')
        || file.contains("..")
    {
        return None;
    }
    Some(file.to_string())
}

/// 幂等补列（带列类型；GAP 170：local_file 等双轨同步列按类型补充）
async fn ensure_column_typed(
    pool: &SqlitePool,
    table: &str,
    column: &str,
    sql_type: &str,
) -> anyhow::Result<()> {
    let row: (i64,) = sqlx::query_as(&format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = '{column}'"))
        .fetch_one(pool)
        .await?;
    if row.0 == 0 {
        let sql = format!("ALTER TABLE {table} ADD COLUMN {column} {sql_type}");
        sqlx::query(&sql).execute(pool).await?;
        tracing::info!("ALTER TABLE {table} ADD COLUMN {column} {sql_type}");
    }
    Ok(())
}

/// 旧库 books 表重建：local_epub/local_pdf 列类型 TEXT → INTEGER（Book 模型为 bool；
/// TEXT 亲和性会把 bool 写成文本，读回时解码失败）。重建在事务内完成：改名 → 建新表 →
/// 按列名交集动态拷数据（兼容任意旧表形态）→ 删旧表。
async fn rebuild_books_bool_columns(pool: &SqlitePool) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("ALTER TABLE books RENAME TO books_old")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        r#"
        CREATE TABLE books (
            book_url TEXT,
            name TEXT DEFAULT '',
            author TEXT DEFAULT '',
            origin TEXT DEFAULT '',
            origin_name TEXT DEFAULT '',
            toc_url TEXT DEFAULT '',
            kind TEXT,
            custom_tag TEXT,
            cover_url TEXT,
            custom_cover_url TEXT,
            intro TEXT,
            custom_intro TEXT,
            charset TEXT,
            type INTEGER DEFAULT 0,
            group_name INTEGER DEFAULT 0,
            latest_chapter_title TEXT,
            latest_chapter_time INTEGER DEFAULT 0,
            last_check_time INTEGER DEFAULT 0,
            last_check_count INTEGER DEFAULT 0,
            total_chapter_num INTEGER DEFAULT 0,
            dur_chapter_title TEXT,
            dur_chapter_index INTEGER DEFAULT 0,
            dur_chapter_pos INTEGER DEFAULT 0,
            dur_chapter_time INTEGER DEFAULT 0,
            word_count TEXT,
            can_update INTEGER DEFAULT 1,
            order_num INTEGER DEFAULT 0,
            origin_order INTEGER DEFAULT 0,
            use_replace_rule INTEGER DEFAULT 1,
            variable TEXT,
            read_config TEXT,
            is_in_shelf INTEGER DEFAULT 1,
            cbz INTEGER DEFAULT 0,
            display_cover TEXT,
            display_intro TEXT,
            local_epub INTEGER DEFAULT 0,
            local_pdf INTEGER DEFAULT 0,
            pdf INTEGER DEFAULT 0,
            split_long_chapter INTEGER DEFAULT 0,
            last_check_error TEXT,
            info_html TEXT,
            toc_html TEXT,
            user_namespace TEXT DEFAULT '',
            created_at INTEGER DEFAULT 0,
            raw_json TEXT,
            local_file TEXT,
            local_file_mtime INTEGER DEFAULT 0,
            local_file_size INTEGER DEFAULT 0,
            local_file_deleted INTEGER DEFAULT 0,
            PRIMARY KEY (book_url, user_namespace)
        );
        "#,
    )
    .execute(&mut *tx)
    .await?;
    // 旧表实际存在的列（与新表按列名交集，保序）→ 动态 INSERT ... SELECT
    let cols: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM pragma_table_info('books_old')              WHERE name IN (SELECT name FROM pragma_table_info('books'))",
    )
    .fetch_all(&mut *tx)
    .await?;
    if !cols.is_empty() {
        let quoted: Vec<String> = cols.iter().map(|c| format!("\"{c}\"")).collect();
        let col_list = quoted.join(", ");
        let sql = format!("INSERT INTO books ({col_list}) SELECT {col_list} FROM books_old");
        sqlx::query(&sql).execute(&mut *tx).await?;
    }
    sqlx::query("DROP TABLE books_old")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// INSERT OR REPLACE 单条书源（save_book_source / save_book_sources 共用；
/// raw_json 由 serde 按 camelCase 重新序列化，序列化时跳过 user_namespace / raw_json 内部字段）
async fn upsert_book_source<'e, E>(executor: E, ns: &str, source: &crate::model::BookSource) -> Result<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let raw_json = serde_json::to_string(source)?;
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
    .bind(&source.book_source_url)
    .bind(&source.book_source_name)
    .bind(&source.book_source_group)
    .bind(source.book_source_type)
    .bind(&source.book_url_pattern)
    .bind(source.custom_order)
    .bind(source.enabled)
    .bind(source.enabled_explore)
    .bind(source.enabled_cookie_jar)
    .bind(&source.concurrent_rate)
    .bind(&source.header)
    .bind(&source.login_url)
    .bind(&source.login_ui)
    .bind(&source.login_check_js)
    .bind(&source.login_js)
    .bind(&source.book_source_comment)
    .bind(&source.variable_comment)
    .bind(source.last_update_time)
    .bind(source.respond_time)
    .bind(source.weight)
    .bind(&source.explore_url)
    .bind(&source.search_url)
    .bind(&source.rule_explore)
    .bind(&source.rule_search)
    .bind(&source.rule_book_info)
    .bind(&source.rule_toc)
    .bind(&source.rule_content)
    .bind(&source.rule_related)
    .bind(&source.search_rule)
    .bind(&source.explore_rule)
    .bind(&source.book_info_rule)
    .bind(&source.toc_rule)
    .bind(&source.content_rule)
    .bind(&source.key)
    .bind(&source.tag)
    .bind(&source.logger)
    .bind(&source.variable)
    .bind(ns)
    .bind(raw_json)
    .execute(executor)
    .await?;
    Ok(())
}

/// saveBook 增量更新字段映射（JSON camelCase 键 → books 表列；固定白名单，防注入）
const BOOK_PATCH_COLUMNS: &[(&str, &str)] = &[
    ("tocUrl", "toc_url"),
    ("origin", "origin"),
    ("originName", "origin_name"),
    ("name", "name"),
    ("author", "author"),
    ("kind", "kind"),
    ("customTag", "custom_tag"),
    ("coverUrl", "cover_url"),
    ("customCoverUrl", "custom_cover_url"),
    ("intro", "intro"),
    ("customIntro", "custom_intro"),
    ("charset", "charset"),
    ("type", "type"),
    ("group", "group_name"),
    ("latestChapterTitle", "latest_chapter_title"),
    ("latestChapterTime", "latest_chapter_time"),
    ("lastCheckTime", "last_check_time"),
    ("lastCheckCount", "last_check_count"),
    ("totalChapterNum", "total_chapter_num"),
    ("durChapterTitle", "dur_chapter_title"),
    ("durChapterIndex", "dur_chapter_index"),
    ("durChapterPos", "dur_chapter_pos"),
    ("durChapterTime", "dur_chapter_time"),
    ("wordCount", "word_count"),
    ("canUpdate", "can_update"),
    ("order", "order_num"),
    ("originOrder", "origin_order"),
    ("useReplaceRule", "use_replace_rule"),
    ("variable", "variable"),
    ("readConfig", "read_config"),
    ("isInShelf", "is_in_shelf"),
    ("lastCheckError", "last_check_error"),
    ("infoHtml", "info_html"),
    ("tocHtml", "toc_html"),
    ("cbz", "cbz"),
    ("displayCover", "display_cover"),
    ("displayIntro", "display_intro"),
    ("localEpub", "local_epub"),
    ("localPdf", "local_pdf"),
    ("pdf", "pdf"),
    ("splitLongChapter", "split_long_chapter"),
    ("language", "language"),
    ("publisher", "publisher"),
    ("publishedAt", "published_at"),
    ("createdAt", "created_at"),
];

/// 按 JSON value 类型绑定（bool→0/1、数字→int、字符串→text、对象/数组→JSON 文本、null→NULL）
fn push_book_patch_value(qb: &mut sqlx::QueryBuilder<'_, sqlx::Sqlite>, value: &serde_json::Value) {
    match value {
        serde_json::Value::Bool(b) => {
            qb.push_bind(if *b { 1i64 } else { 0i64 });
        }
        serde_json::Value::Number(n) => {
            qb.push_bind(n.as_i64().unwrap_or(0));
        }
        serde_json::Value::String(s) => {
            qb.push_bind(s.clone());
        }
        serde_json::Value::Null => {
            qb.push_bind(Option::<String>::None);
        }
        other => {
            qb.push_bind(other.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::BookSource;

    /// 独立临时目录初始化存储（避免污染真实 storage/reader.db）
    async fn test_storage(tag: &str) -> Storage {
        let dir = std::env::temp_dir().join(format!(
            "reader-storage-test-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut config = AppConfig::from_env();
        config.work_dir = dir.to_string_lossy().into_owned();
        init(&config).await.expect("测试存储初始化失败")
    }

    /// 释放连接池并清理临时目录
    async fn cleanup(storage: Storage, tag: &str) {
        storage.pool.close().await;
        let dir = std::env::temp_dir().join(format!(
            "reader-storage-test-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn source(url: &str, name: &str, group: Option<&str>) -> BookSource {
        BookSource {
            book_source_url: url.into(),
            book_source_name: name.into(),
            book_source_group: group.map(|g| g.to_string()),
            search_url: Some(format!("{url}/search?q={{{{key}}}}")),
            rule_search: Some(serde_json::json!({ "bookList": "$.data" })),
            enabled: true,
            enabled_explore: true,
            custom_order: 1,
            ..Default::default()
        }
    }

    /// 保存 → 查询 → 覆盖保存 → 删除 往返；raw_json camelCase 与 bookSource.json 一致
    #[tokio::test]
    async fn test_save_get_delete_roundtrip() {
        let storage = test_storage("roundtrip").await;
        let mut s = source("https://a.com", "A源", Some("小说 玄幻"));
        storage.save_book_source("default", &s).await.unwrap();

        let got = storage
            .get_book_source("default", "https://a.com")
            .await
            .unwrap()
            .expect("保存后应能查到");
        assert_eq!(got.book_source_name, "A源");
        assert_eq!(got.book_source_group.as_deref(), Some("小说 玄幻"));
        assert_eq!(got.user_namespace, "default");
        assert_eq!(got.rule_search, Some(serde_json::json!({ "bookList": "$.data" })));

        // raw_json：camelCase、含规则字段、可反序列化回 BookSource
        let raw = got.raw_json.as_deref().expect("raw_json 应已写入");
        let v: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert!(v.get("bookSourceUrl").is_some(), "raw_json 应为 camelCase: {raw}");
        assert!(v.get("book_source_url").is_none(), "raw_json 不应含 snake_case: {raw}");
        assert!(v.get("bookSourceName").is_some());
        assert_eq!(v["enabled"], serde_json::Value::Bool(true));
        let roundtrip: BookSource = serde_json::from_str(raw).unwrap();
        assert_eq!(roundtrip.book_source_url, "https://a.com");

        // 覆盖保存（改名 + 禁用）→ INSERT OR REPLACE 生效
        s.book_source_name = "A源v2".into();
        s.enabled = false;
        storage.save_book_source("default", &s).await.unwrap();
        let got2 = storage
            .get_book_source("default", "https://a.com")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got2.book_source_name, "A源v2");
        assert!(!got2.enabled);

        // 删除 → 查不到
        let affected = storage.delete_book_source("default", "https://a.com").await.unwrap();
        assert_eq!(affected, 1);
        assert!(storage
            .get_book_source("default", "https://a.com")
            .await
            .unwrap()
            .is_none());

        cleanup(storage, "roundtrip").await;
    }

    /// update_book_source_enabled：单条切换 + 不存在返回 0 行
    #[tokio::test]
    async fn test_update_enabled() {
        let storage = test_storage("enabled").await;
        storage
            .save_book_source("default", &source("https://a.com", "A", None))
            .await
            .unwrap();
        storage
            .save_book_source("default", &source("https://b.com", "B", None))
            .await
            .unwrap();

        let affected = storage
            .update_book_source_enabled("default", "https://a.com", false)
            .await
            .unwrap();
        assert_eq!(affected, 1);
        let a = storage
            .get_book_source("default", "https://a.com")
            .await
            .unwrap()
            .unwrap();
        assert!(!a.enabled, "A 应被禁用");
        let b = storage
            .get_book_source("default", "https://b.com")
            .await
            .unwrap()
            .unwrap();
        assert!(b.enabled, "B 应保持启用");

        // 不存在的 URL → 0 行
        let none = storage
            .update_book_source_enabled("default", "https://nope.com", true)
            .await
            .unwrap();
        assert_eq!(none, 0);

        cleanup(storage, "enabled").await;
    }

    /// 批量事务保存 + 分组去重列表（含 default 回退）
    #[tokio::test]
    async fn test_batch_save_and_groups() {
        let storage = test_storage("batch").await;
        let sources = vec![
            source("https://a.com", "A", Some("小说 玄幻")),
            source("https://b.com", "B", Some("玄幻")),
            source("https://c.com", "C", None),
            source("https://d.com", "D", Some("")),
        ];
        storage.save_book_sources("default", &sources).await.unwrap();

        let all = storage.get_book_sources("default").await.unwrap();
        assert_eq!(all.len(), 4);
        assert!(all.iter().all(|s| s.raw_json.is_some()), "批量保存应写入 raw_json");

        // 保序去重；空串/None 分组不产生条目
        let groups = storage.list_book_source_groups("default").await.unwrap();
        assert_eq!(groups, vec!["小说", "玄幻"]);
        // 无书源命名空间回退 default 的分组
        let groups_fb = storage.list_book_source_groups("ghost").await.unwrap();
        assert_eq!(groups_fb, vec!["小说", "玄幻"]);

        cleanup(storage, "batch").await;
    }

    /// 旧库兼容：books.local_epub/local_pdf 为 TEXT 类型时，init 应重建为 INTEGER 且数据无损读回
    #[tokio::test]
    async fn test_legacy_books_text_bool_rebuild() {
        let dir = std::env::temp_dir().join(format!(
            "reader-storage-test-{}-legacyrebuild",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut config = AppConfig::from_env();
        config.work_dir = dir.to_string_lossy().into_owned();

        // 1. 模拟旧库：books 表 local_epub/local_pdf 为 TEXT，写入一行（含 TEXT '1'）
        let db_path = dir.join("storage").join("reader.db");
        std::fs::create_dir_all(dir.join("storage")).unwrap();
        {
            let opts = SqliteConnectOptions::new().filename(&db_path).create_if_missing(true);
            let pool = SqlitePoolOptions::new().max_connections(1).connect_with(opts).await.unwrap();
            sqlx::query(
                "CREATE TABLE books (
                    book_url TEXT PRIMARY KEY, name TEXT DEFAULT '', author TEXT DEFAULT '',
                    origin TEXT DEFAULT '', origin_name TEXT DEFAULT '', toc_url TEXT DEFAULT '',
                    local_epub TEXT, local_pdf TEXT, pdf INTEGER DEFAULT 0,
                    user_namespace TEXT DEFAULT '')",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO books (book_url, name, local_epub, local_pdf, user_namespace)              VALUES ('https://old.com/a', '旧书', '1', '0', 'default')",
            )
            .execute(&pool)
            .await
            .unwrap();
            pool.close().await;
        }

        // 2. init → 检测 TEXT → 重建
        let storage = init(&config).await.expect("含旧 books 表的库应能初始化");
        let book = storage
            .find_book("default", "https://old.com/a")
            .await
            .unwrap()
            .expect("重建后旧数据应保留");
        assert_eq!(book.name, "旧书");
        assert!(book.local_epub, "TEXT '1' 应迁移为 true");
        assert!(!book.local_pdf, "TEXT '0' 应迁移为 false");
        // 重建后写入/读回 bool 正常
        storage
            .update_book_progress("default", "https://old.com/a", Some("第1章"), 0, 0, 1)
            .await
            .unwrap();
        let again = storage.find_book("default", "https://old.com/a").await.unwrap().unwrap();
        assert_eq!(again.dur_chapter_title.as_deref(), Some("第1章"));
        assert!(again.local_epub);

        // 3. 幂等：再次 init 不报错、不重复重建
        let storage2 = init(&config).await.unwrap();
        assert!(storage2.find_book("default", "https://old.com/a").await.unwrap().is_some());

        storage.pool.close().await;
        storage2.pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 命名空间隔离：列表/删除/启停互不串；空命名空间回退 default
    #[tokio::test]
    async fn test_namespace_isolation() {
        let storage = test_storage("ns").await;
        storage
            .save_book_source("default", &source("https://a.com", "默认源", None))
            .await
            .unwrap();
        storage
            .save_book_source("alice", &source("https://b.com", "爱丽丝源", None))
            .await
            .unwrap();

        let alice = storage.get_book_sources("alice").await.unwrap();
        assert_eq!(alice.len(), 1);
        assert_eq!(alice[0].book_source_name, "爱丽丝源");
        // 无书源命名空间回退 default
        let bob = storage.get_book_sources("bob").await.unwrap();
        assert_eq!(bob.len(), 1);
        assert_eq!(bob[0].book_source_name, "默认源");
        // 删除只影响本命名空间
        assert_eq!(
            storage.delete_book_source("alice", "https://b.com").await.unwrap(),
            1
        );
        assert!(storage
            .get_book_source("alice", "https://b.com")
            .await
            .unwrap()
            .is_none());
        assert!(storage
            .get_book_source("default", "https://a.com")
            .await
            .unwrap()
            .is_some());
        // 启停同样按命名空间隔离：跨命名空间 URL 影响 0 行
        assert_eq!(
            storage
                .update_book_source_enabled("alice", "https://a.com", false)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            storage
                .update_book_source_enabled("default", "https://a.com", false)
                .await
                .unwrap(),
            1
        );
        assert!(!storage
            .get_book_source("default", "https://a.com")
            .await
            .unwrap()
            .unwrap()
            .enabled);

        cleanup(storage, "ns").await;
    }

    /// 构造书架书（默认值 + 关键字段）
    fn shelf_book(url: &str, name: &str) -> Book {
        Book {
            book_url: url.into(),
            name: name.into(),
            author: "作者A".into(),
            origin: "https://src.com".into(),
            origin_name: "源A".into(),
            toc_url: format!("{url}/toc"),
            book_type: 1,
            can_update: true,
            is_in_shelf: true,
            ..Default::default()
        }
    }

    /// F-8/F-9：upsert 新增 → find → patch 增量 → upsert 覆盖 → 进度保存 全链路
    #[tokio::test]
    async fn test_book_save_progress_flow() {
        let storage = test_storage("book").await;
        let url = "https://book.com/a";
        assert!(storage.find_book("default", url).await.unwrap().is_none(), "初始不在书架");

        // F-9 新增入架（全量 INSERT）
        let mut book = shelf_book(url, "书名");
        book.total_chapter_num = 100;
        storage.upsert_book("default", &book).await.unwrap();
        let got = storage.find_book("default", url).await.unwrap().expect("入架后应可查到");
        assert_eq!(got.name, "书名");
        assert_eq!(got.origin, "https://src.com");
        assert_eq!(got.total_chapter_num, 100);
        assert_eq!(got.user_namespace, "default");

        // F-9 编辑：增量 patch（name/coverUrl/group），未提供字段保持不变
        let patch: serde_json::Map<String, serde_json::Value> = serde_json::json!({
            "name": "书名v2",
            "coverUrl": "https://cover.com/x.jpg",
            "group": 3,
        })
        .as_object()
        .unwrap()
        .clone();
        let affected = storage.patch_book("default", url, &patch).await.unwrap();
        assert_eq!(affected, 1);
        let got2 = storage.find_book("default", url).await.unwrap().unwrap();
        assert_eq!(got2.name, "书名v2");
        assert_eq!(got2.cover_url.as_deref(), Some("https://cover.com/x.jpg"));
        assert_eq!(got2.group, 3);
        assert_eq!(got2.total_chapter_num, 100, "未提供的字段应保持原值");
        // 未知键忽略 + 空 patch → 0 行
        let junk: serde_json::Map<String, serde_json::Value> =
            serde_json::json!({ "unknownKey": 1 }).as_object().unwrap().clone();
        assert_eq!(storage.patch_book("default", url, &junk).await.unwrap(), 0);
        // 不存在的书 patch → 0 行
        assert_eq!(storage.patch_book("default", "https://nope.com", &patch).await.unwrap(), 0);

        // F-9 覆盖：upsert 全字段更新
        let mut book2 = shelf_book(url, "书名v3");
        book2.total_chapter_num = 200;
        storage.upsert_book("default", &book2).await.unwrap();
        let got3 = storage.find_book("default", url).await.unwrap().unwrap();
        assert_eq!(got3.name, "书名v3");
        assert_eq!(got3.total_chapter_num, 200);

        // F-8 进度保存
        let affected = storage
            .update_book_progress("default", url, Some("第3章"), 2, 1234, 5678)
            .await
            .unwrap();
        assert_eq!(affected, 1);
        let got4 = storage.find_book("default", url).await.unwrap().unwrap();
        assert_eq!(got4.dur_chapter_title.as_deref(), Some("第3章"));
        assert_eq!(got4.dur_chapter_index, 2);
        assert_eq!(got4.dur_chapter_pos, 1234);
        assert_eq!(got4.dur_chapter_time, 5678);
        // title=None 保持原值
        storage
            .update_book_progress("default", url, None, 3, 0, 9999)
            .await
            .unwrap();
        let got5 = storage.find_book("default", url).await.unwrap().unwrap();
        assert_eq!(got5.dur_chapter_title.as_deref(), Some("第3章"));
        assert_eq!(got5.dur_chapter_index, 3);
        // 书架外的书 → 0 行
        assert_eq!(
            storage
                .update_book_progress("default", "https://nope.com", Some("x"), 0, 0, 0)
                .await
                .unwrap(),
            0
        );

        cleanup(storage, "book").await;
    }

    /// F-10：目录缓存写入 → 命中 → 过期未命中
    #[tokio::test]
    async fn test_toc_cache_roundtrip() {
        let storage = test_storage("toccache").await;
        let toc_url = "https://book.com/toc";
        assert!(storage.get_toc_cache(toc_url, 300_000).await.unwrap().is_none(), "未缓存时应未命中");

        storage.cache_toc(toc_url, toc_url, r#"[{"title":"第一章","url":"https://book.com/1"}]"#).await.unwrap();
        let cached = storage.get_toc_cache(toc_url, 300_000).await.unwrap().expect("缓存后应命中");
        assert!(cached.contains("第一章"));
        // 同 book_url 覆盖写
        storage.cache_toc(toc_url, toc_url, r#"[{"title":"新目录"}]"#).await.unwrap();
        let cached2 = storage.get_toc_cache(toc_url, 300_000).await.unwrap().unwrap();
        assert!(cached2.contains("新目录"));
        // 过期（把 updated_at 置 0）→ 未命中
        sqlx::query("UPDATE toc_cache SET updated_at = 0 WHERE book_url = ?1")
            .bind(toc_url)
            .execute(&storage.pool)
            .await
            .unwrap();
        assert!(storage.get_toc_cache(toc_url, 300_000).await.unwrap().is_none(), "TTL 过期应未命中");

        cleanup(storage, "toccache").await;
    }

    /// 书签：保存 → 列表 → 覆盖保存（同 title）→ 删除
    #[tokio::test]
    async fn test_bookmark_roundtrip() {
        let storage = test_storage("bookmark").await;
        let url = "https://book.com/a";
        let bm = crate::model::Bookmark {
            book_url: url.into(),
            title: "标记1".into(),
            paragraph_index: 42,
            chapter_index: 3,
            created_at: 1000,
            ..Default::default()
        };
        storage.save_bookmark("default", &bm).await.unwrap();
        storage
            .save_bookmark("default", &crate::model::Bookmark {
                book_url: url.into(),
                title: "标记2".into(),
                paragraph_index: 7,
                chapter_index: 1,
                created_at: 2000,
                ..Default::default()
            })
            .await
            .unwrap();

        let list = storage.list_bookmarks("default", url).await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].title, "标记2", "按创建时间倒序");
        assert_eq!(list[1].paragraph_index, 42);
        // 他书/他命名空间隔离
        assert!(storage.list_bookmarks("default", "https://other.com").await.unwrap().is_empty());
        assert!(storage.list_bookmarks("alice", url).await.unwrap().is_empty());

        // 同 title 覆盖保存
        storage
            .save_bookmark("default", &crate::model::Bookmark {
                book_url: url.into(),
                title: "标记1".into(),
                paragraph_index: 99,
                chapter_index: 3,
                created_at: 3000,
                ..Default::default()
            })
            .await
            .unwrap();
        let list2 = storage.list_bookmarks("default", url).await.unwrap();
        assert_eq!(list2.len(), 2);
        assert_eq!(list2[0].paragraph_index, 99);

        // 删除
        assert_eq!(storage.delete_bookmark("default", url, "标记1").await.unwrap(), 1);
        assert_eq!(storage.list_bookmarks("default", url).await.unwrap().len(), 1);
        assert_eq!(storage.delete_bookmark("default", url, "不存在").await.unwrap(), 0);

        cleanup(storage, "bookmark").await;
    }

    /// 分组：新建（自增 id）→ 列表 → 按 id 覆盖 → 书设分组
    #[tokio::test]
    async fn test_book_group_flow() {
        let storage = test_storage("bookgroup").await;
        let g1 = storage
            .save_book_group("default", &crate::model::BookGroup {
                name: "玄幻".into(),
                order: 1,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(g1.id > 0, "新建应返回自增 id");
        let g2 = storage
            .save_book_group("default", &crate::model::BookGroup {
                name: "言情".into(),
                order: 2,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(g2.id > g1.id);

        let list = storage.list_book_groups("default").await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "玄幻", "按 order 排序");
        // 命名空间隔离
        assert!(storage.list_book_groups("alice").await.unwrap().is_empty());

        // 按 id 覆盖（改名 + 排序）
        let updated = storage
            .save_book_group("default", &crate::model::BookGroup {
                id: g1.id,
                name: "玄幻v2".into(),
                order: 5,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(updated.id, g1.id);
        let list2 = storage.list_book_groups("default").await.unwrap();
        assert_eq!(list2.len(), 2);
        assert_eq!(list2[1].name, "玄幻v2");

        // 书设分组（books.group_name）
        let url = "https://book.com/a";
        storage.upsert_book("default", &shelf_book(url, "书名")).await.unwrap();
        assert_eq!(storage.update_book_group_id("default", url, g1.id).await.unwrap(), 1);
        assert_eq!(storage.find_book("default", url).await.unwrap().unwrap().group, g1.id);
        assert_eq!(storage.update_book_group_id("default", "https://nope.com", g1.id).await.unwrap(), 0);

        cleanup(storage, "bookgroup").await;
    }

    /// RSS 源：保存（含 raw_json 原文）→ 查询 → 覆盖保存 → 删除；命名空间回退 default
    #[tokio::test]
    async fn test_rss_source_roundtrip() {
        let storage = test_storage("rsssrc").await;
        let s = crate::model::RssSource {
            source_url: "https://feed.example.com/rss".into(),
            source_name: "示例源".into(),
            source_group: Some("科技".into()),
            enabled: true,
            raw_json: Some(
                r#"{"sourceUrl":"https://feed.example.com/rss","sourceName":"示例源","sourceGroup":"科技","enabled":true,"sortUrl":null,"ruleContent":"css.article"}"#
                    .into(),
            ),
            ..Default::default()
        };
        storage.save_rss_source("default", &s).await.unwrap();

        let got = storage
            .find_rss_source("default", "https://feed.example.com/rss")
            .await
            .unwrap()
            .expect("保存后应能查到");
        assert_eq!(got.source_name, "示例源");
        assert_eq!(got.source_group.as_deref(), Some("科技"));
        assert!(got.enabled);
        assert_eq!(got.user_namespace, "default");
        let raw: serde_json::Value =
            serde_json::from_str(got.raw_json.as_deref().expect("raw_json 应已写入")).unwrap();
        assert_eq!(raw["sourceUrl"], "https://feed.example.com/rss");
        assert_eq!(raw["ruleContent"], "css.article", "raw_json 应保留完整字段");

        // 覆盖保存（改名 + 禁用）→ INSERT OR REPLACE 生效
        let mut s2 = s.clone();
        s2.source_name = "示例源v2".into();
        s2.enabled = false;
        storage.save_rss_source("default", &s2).await.unwrap();
        let got2 = storage
            .find_rss_source("default", "https://feed.example.com/rss")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got2.source_name, "示例源v2");
        assert!(!got2.enabled);

        // 列表（default 直接返回；其他命名空间回退 default）
        let list = storage.get_rss_sources("default").await.unwrap();
        assert_eq!(list.len(), 1);
        let fb = storage.get_rss_sources("ghost").await.unwrap();
        assert_eq!(fb.len(), 1);
        assert_eq!(fb[0].source_name, "示例源v2", "无源命名空间回退 default");

        // 删除 → 查不到
        assert_eq!(
            storage.delete_rss_source("default", "https://feed.example.com/rss").await.unwrap(),
            1
        );
        assert!(storage
            .find_rss_source("default", "https://feed.example.com/rss")
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            storage.delete_rss_source("default", "https://feed.example.com/rss").await.unwrap(),
            0,
            "重复删除影响 0 行"
        );

        cleanup(storage, "rsssrc").await;
    }

    /// RSS 文章：批量保存（按 url 去重）→ 按 url 查询；命名空间隔离
    #[tokio::test]
    async fn test_rss_articles_roundtrip() {
        let storage = test_storage("rssart").await;
        let article = |url: &str, title: &str, time: i64| crate::model::RssArticle {
            url: url.into(),
            source_url: "https://feed.example.com/rss".into(),
            title: title.into(),
            author: "作者".into(),
            time,
            content: Some("正文".into()),
            cover: Some("https://img.example.com/1.jpg".into()),
            ..Default::default()
        };
        let articles = vec![article("https://feed.example.com/a", "甲", 1000), article("https://feed.example.com/b", "乙", 2000)];
        storage.save_rss_articles("default", &articles).await.unwrap();

        let got = storage.get_rss_article("https://feed.example.com/a").await.unwrap().unwrap();
        assert_eq!(got.title, "甲");
        assert_eq!(got.source_url, "https://feed.example.com/rss");
        assert_eq!(got.time, 1000);
        assert_eq!(got.content.as_deref(), Some("正文"));
        assert_eq!(got.cover.as_deref(), Some("https://img.example.com/1.jpg"));
        assert_eq!(got.user_namespace, "default");

        // 同 url 覆盖（刷新 feed 时去重更新）
        storage
            .save_rss_articles("default", &[article("https://feed.example.com/a", "甲v2", 3000)])
            .await
            .unwrap();
        let again = storage.get_rss_article("https://feed.example.com/a").await.unwrap().unwrap();
        assert_eq!(again.title, "甲v2");
        assert_eq!(again.time, 3000);
        assert_eq!(storage.get_rss_article("https://feed.example.com/b").await.unwrap().unwrap().title, "乙");

        // 已读标记：标记已读 → 重新入库（feed 刷新）不清除
        storage.set_rss_article_read("https://feed.example.com/a", true).await.unwrap();
        storage
            .save_rss_articles("default", &[article("https://feed.example.com/a", "甲v3", 4000)])
            .await
            .unwrap();
        let marked = storage.get_rss_article("https://feed.example.com/a").await.unwrap().unwrap();
        assert!(marked.read, "重新入库不应清除已读标记");
        assert_eq!(marked.title, "甲v3");
        // 标回未读
        storage.set_rss_article_read("https://feed.example.com/a", false).await.unwrap();
        assert!(!storage.get_rss_article("https://feed.example.com/a").await.unwrap().unwrap().read);
        // 已读标记批量查询（仅本命名空间 + 本源的 url）
        let flags = storage.get_rss_article_read_flags("default", "https://feed.example.com/rss").await.unwrap();
        assert_eq!(flags.len(), 2);
        assert!(!flags["https://feed.example.com/a"]);
        assert!(!flags["https://feed.example.com/b"]);
        let other = storage.get_rss_article_read_flags("other", "https://feed.example.com/rss").await.unwrap();
        assert!(other.is_empty(), "其他命名空间看不到该源的已读标记");

        // 不存在的 url
        assert!(storage.get_rss_article("https://feed.example.com/nope").await.unwrap().is_none());

        cleanup(storage, "rssart").await;
    }

    /// F-7：书源计数（不含 default 回退）+ 用户书源上限读取
    #[tokio::test]
    async fn test_book_source_limit_helpers() {
        let storage = test_storage("bslimit").await;
        storage
            .insert_user(&User {
                username: "alice".into(),
                book_source_limit: 5,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(storage.book_source_limit_for("alice").await.unwrap(), Some(5));
        assert_eq!(storage.book_source_limit_for("ghost").await.unwrap(), None);
        for i in 0..3 {
            storage
                .save_book_source("alice", &source(&format!("https://s{i}.com"), "S", None))
                .await
                .unwrap();
        }
        assert_eq!(storage.count_book_sources("alice").await.unwrap(), 3);
        assert_eq!(storage.count_book_sources("default").await.unwrap(), 0, "计数不含 default 回退");
        cleanup(storage, "bslimit").await;
    }

    /// F-25：logout 清空 token，重复 logout 影响 0 行
    #[tokio::test]
    async fn test_logout_user() {
        let storage = test_storage("logout").await;
        storage
            .insert_user(&User {
                username: "alice".into(),
                token: "t1".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(storage.logout_user("alice").await.unwrap(), 1);
        assert!(storage.find_user("alice").await.unwrap().unwrap().token.is_empty());
        assert_eq!(storage.logout_user("ghost").await.unwrap(), 0);
        cleanup(storage, "logout").await;
    }

    /// GAP 59：登录追加 token（token_map 上限 5、去重）；登出仅移除当前设备 token
    #[tokio::test]
    async fn test_add_remove_user_token() {
        let storage = test_storage("tokenmap").await;
        storage
            .insert_user(&User {
                username: "alice".into(),
                token: "t0".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        // 追加 6 个 token → 上限 5，最旧被丢
        for i in 1..=6 {
            storage.add_user_token("alice", &format!("tok-{i}"), 1000 + i).await.unwrap();
        }
        let user = storage.find_user("alice").await.unwrap().unwrap();
        assert_eq!(user.token, "tok-6", "主 token 为最新");
        let list = crate::model::user::token_map_list(&user.token_map);
        assert_eq!(list, vec!["tok-2", "tok-3", "tok-4", "tok-5", "tok-6"], "上限 5 且最旧被丢");
        // 重新登录同一 token → 去重（不重复计数）
        storage.add_user_token("alice", "tok-5", 9999).await.unwrap();
        let user = storage.find_user("alice").await.unwrap().unwrap();
        let list = crate::model::user::token_map_list(&user.token_map);
        assert_eq!(list.iter().filter(|t| *t == "tok-5").count(), 1, "去重");
        assert_eq!(list.len(), 5);
        // 登出：移除设备 token（主 token 保持）
        storage.remove_user_token("alice", "tok-4").await.unwrap();
        let user = storage.find_user("alice").await.unwrap().unwrap();
        assert_eq!(user.token, "tok-5", "主 token 不受影响");
        let list = crate::model::user::token_map_list(&user.token_map);
        assert!(!list.contains(&"tok-4".to_string()));
        assert_eq!(list.len(), 4);
        // 登出主 token → 主 token 清空但 map 其余保留
        storage.remove_user_token("alice", "tok-5").await.unwrap();
        let user = storage.find_user("alice").await.unwrap().unwrap();
        assert!(user.token.is_empty(), "主 token 清空");
        let list = crate::model::user::token_map_list(&user.token_map);
        assert_eq!(list.len(), 3, "其余设备 token 保留");
        // 不存在的 token → 无影响
        assert_eq!(storage.remove_user_token("alice", "nope").await.unwrap(), 0);
        // legacy 对象形态 token_map 兼容（键即 token）——直接 UPDATE 注入旧形态数据
        storage
            .insert_user(&User {
                username: "legacy".into(),
                token: "main".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        sqlx::query("UPDATE users SET token_map = ?1 WHERE username = 'legacy'")
            .bind(serde_json::json!({"old-1": 1700000000000i64}).to_string())
            .execute(&storage.pool)
            .await
            .unwrap();
        storage.add_user_token("legacy", "new-1", 2000).await.unwrap();
        let user = storage.find_user("legacy").await.unwrap().unwrap();
        let list = crate::model::user::token_map_list(&user.token_map);
        assert!(list.contains(&"old-1".to_string()) && list.contains(&"new-1".to_string()));
        cleanup(storage, "tokenmap").await;
    }

    /// F-34：不活跃用户清理（GAP #95：users 行 + 用户级数据行 + 数据目录；except 保护）
    #[tokio::test]
    async fn test_clear_inactive_users() {
        let storage = test_storage("inactive").await;
        let mk = |name: &str, last: i64| User {
            username: name.into(),
            last_login_at: last,
            ..Default::default()
        };
        storage.insert_user(&mk("old", 1000)).await.unwrap();
        storage.insert_user(&mk("mid", 5000)).await.unwrap();
        storage.insert_user(&mk("new", 9999)).await.unwrap();
        // old 用户残留数据（书 + 数据目录文件）——应一并清理
        storage
            .upsert_book(
                "old",
                &crate::model::Book {
                    book_url: "https://old.com/b".into(),
                    name: "旧书".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let dir = storage
            .config
            .storage_dir()
            .join("data")
            .join("old")
            .join("webdav");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("x.txt"), "x").unwrap();

        let deleted = storage.clear_inactive_users(6000, None).await.unwrap();
        assert_eq!(deleted, vec!["old", "mid"]);
        assert!(storage.find_user("old").await.unwrap().is_none());
        assert!(storage.find_user("mid").await.unwrap().is_none());
        assert!(storage.find_user("new").await.unwrap().is_some());
        // 用户级数据行 + 目录已清理
        assert!(storage.list_books("old").await.unwrap().is_empty());
        assert!(!storage.config.storage_dir().join("data").join("old").exists());

        // except 用户受保护
        let deleted = storage.clear_inactive_users(99999, Some("new")).await.unwrap();
        assert!(deleted.is_empty());
        assert!(storage.find_user("new").await.unwrap().is_some());
        cleanup(storage, "inactive").await;
    }

    /// GAP #95：deleteUser 全量清理——用户级表行 + 数据目录；共享书章节保留
    #[tokio::test]
    async fn test_delete_user_cleans_all_data() {
        let storage = test_storage("deluser2").await;
        // alice（被删）与 bob（保留）
        storage
            .insert_user(&User {
                username: "alice".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        storage
            .insert_user(&User {
                username: "bob".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        // 共享书（双方都有）+ alice 独有书
        let shared = crate::model::Book {
            book_url: "https://s.com/shared".into(),
            name: "共享".into(),
            ..Default::default()
        };
        let alice_only = crate::model::Book {
            book_url: "https://a.com/only".into(),
            name: "独有".into(),
            ..Default::default()
        };
        storage.upsert_book("alice", &shared).await.unwrap();
        storage.upsert_book("bob", &shared).await.unwrap();
        storage.upsert_book("alice", &alice_only).await.unwrap();
        // 章节（共享书章节两用户共用；独有书章节仅 alice）
        for (url, idx) in [("https://s.com/shared", 0), ("https://a.com/only", 0)] {
            sqlx::query("INSERT INTO book_chapters (book_url, chapter_index, title, content) VALUES (?1, ?2, '章', '文')")
                .bind(url)
                .bind(idx)
                .execute(&storage.pool)
                .await
                .unwrap();
        }
        // 用户级数据铺满各表
        storage
            .save_book_source(
                "alice",
                &crate::model::BookSource {
                    book_source_url: "https://a.com/src".into(),
                    book_source_name: "源".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        storage.set_cookie("alice", "https://a.com/src", "sid=1").await.unwrap();
        storage.save_user_config("alice", "reader", "{}").await.unwrap();
        storage
            .record_reading_stats("alice", "https://a.com/only", "2025-01-01", 60, 1000)
            .await
            .unwrap();
        storage
            .save_bookmark(
                "alice",
                &crate::model::Bookmark {
                    book_url: "https://a.com/only".into(),
                    title: "书签".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        storage
            .save_source_sub("alice", "https://a.com/sub", "订阅", "[]")
            .await
            .unwrap();
        storage
            .save_rss_source(
                "alice",
                &crate::model::RssSource {
                    source_url: "https://a.com/rss".into(),
                    source_name: "RSS".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        storage
            .save_rss_articles(
                "alice",
                &[crate::model::RssArticle {
                    url: "https://a.com/rss/1".into(),
                    title: "文".into(),
                    ..Default::default()
                }],
            )
            .await
            .unwrap();
        // 数据目录（含 webdav 子目录）
        let alice_dir = storage.config.storage_dir().join("data").join("alice");
        std::fs::create_dir_all(alice_dir.join("webdav").join("legado")).unwrap();
        std::fs::write(alice_dir.join("webdav").join("legado").join("backup-1.zip"), "zip").unwrap();
        std::fs::create_dir_all(alice_dir.join("opds_files")).unwrap();
        std::fs::write(alice_dir.join("opds_files").join("f.txt"), "f").unwrap();

        // 删除
        assert_eq!(storage.delete_user("alice").await.unwrap(), 1);
        // users 行
        assert!(storage.find_user("alice").await.unwrap().is_none());
        // 用户级表行清空
        assert!(storage.list_books("alice").await.unwrap().is_empty());
        assert!(storage.get_book_sources("alice").await.unwrap().is_empty());
        assert!(storage.get_cookie("alice", "https://a.com/src").await.unwrap().is_none());
        let cfg: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_config WHERE user_namespace = 'alice'")
            .fetch_one(&storage.pool)
            .await
            .unwrap();
        assert_eq!(cfg, 0);
        let stats: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reading_stats WHERE user_namespace = 'alice'")
            .fetch_one(&storage.pool)
            .await
            .unwrap();
        assert_eq!(stats, 0);
        let bm: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bookmarks WHERE user_namespace = 'alice'")
            .fetch_one(&storage.pool)
            .await
            .unwrap();
        assert_eq!(bm, 0);
        let subs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM source_subs WHERE user_namespace = 'alice'")
            .fetch_one(&storage.pool)
            .await
            .unwrap();
        assert_eq!(subs, 0);
        let rss_src: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rss_sources WHERE user_namespace = 'alice'")
            .fetch_one(&storage.pool)
            .await
            .unwrap();
        assert_eq!(rss_src, 0);
        let rss_art: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rss_articles WHERE user_namespace = 'alice'")
            .fetch_one(&storage.pool)
            .await
            .unwrap();
        assert_eq!(rss_art, 0);
        // 章节：独有书章节删除；共享书章节保留（bob 仍拥有）
        let shared_ch: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM book_chapters WHERE book_url = 'https://s.com/shared'")
            .fetch_one(&storage.pool)
            .await
            .unwrap();
        assert_eq!(shared_ch, 1, "共享书章节应保留");
        let only_ch: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM book_chapters WHERE book_url = 'https://a.com/only'")
            .fetch_one(&storage.pool)
            .await
            .unwrap();
        assert_eq!(only_ch, 0, "独有书章节应删除");
        // bob 数据不受影响
        assert!(storage.find_book("bob", "https://s.com/shared").await.unwrap().is_some());
        // 目录递归删除（含 webdav/opds_files）
        assert!(!alice_dir.exists(), "数据目录应被递归删除");

        // 不存在用户 → 0；非法用户名 → 不 panic、不删目录
        assert_eq!(storage.delete_user("ghost").await.unwrap(), 0);
        assert_eq!(storage.delete_user("../evil").await.unwrap(), 0);
        cleanup(storage, "deluser2").await;
    }

    /// GAP #57：自动备份（auto-YYYYMMDD.zip 写入 + 同日幂等 + 保留最近 7 份）
    #[tokio::test]
    async fn test_auto_backup_and_prune() {
        let storage = test_storage("autobk").await;
        storage
            .upsert_book(
                "default",
                &crate::model::Book {
                    book_url: "https://a.com/b".into(),
                    name: "书".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let legado = storage
            .config
            .storage_dir()
            .join("data")
            .join("default")
            .join("webdav")
            .join("legado");

        // 首次：生成 auto-YYYYMMDD.zip
        assert_eq!(crate::service::schedule::run_auto_backup(&storage).await.unwrap(), 1);
        let today = chrono::Local::now().format("%Y%m%d").to_string();
        let auto_file = legado.join(format!("auto-{today}.zip"));
        assert!(auto_file.exists(), "自动备份文件应生成: {}", auto_file.display());

        // 同日再跑：幂等跳过（不重复生成）
        let files_after_first: Vec<_> = std::fs::read_dir(&legado)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(crate::service::schedule::run_auto_backup(&storage).await.unwrap(), 0);
        let files_after_second: Vec<_> = std::fs::read_dir(&legado)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(files_after_first.len(), files_after_second.len(), "同日不应重复备份");

        // 保留最近 7 份：伪造 12 个历史 auto-*.zip → prune 后剩 7（今天的保留）
        for i in 1..=12 {
            std::fs::write(legado.join(format!("auto-202501{i:02}.zip")), "old").unwrap();
        }
        let removed = storage.prune_auto_backups("default", 7);
        assert_eq!(removed, 6);
        let remaining: Vec<String> = std::fs::read_dir(&legado)
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| n.starts_with("auto-") && n.ends_with(".zip"))
            .collect();
        assert_eq!(remaining.len(), 7);
        assert!(remaining.iter().any(|n| *n == format!("auto-{today}.zip")), "今天的备份应保留");
        assert!(remaining.iter().all(|n| n.as_str() >= "auto-20250107.zip"), "只保留最新的 7 份: {remaining:?}");

        // 手动备份不受影响（backup-*.zip 不参与 auto 清理）
        storage.create_backup_zip("default").await.unwrap();
        let removed = storage.prune_auto_backups("default", 7);
        assert_eq!(removed, 0);
        cleanup(storage, "autobk").await;
    }

    /// F-32：用户管理——列表/权限更新/删除/重置密码
    #[tokio::test]
    async fn test_user_management() {
        let storage = test_storage("usermgmt").await;
        storage
            .insert_user(&User {
                username: "alice".into(),
                password: "p1".into(),
                salt: "s1".into(),
                token: "tok".into(),
                enable_webdav: false,
                enable_book_source: true,
                book_source_limit: 10,
                book_limit: 20,
                ..Default::default()
            })
            .await
            .unwrap();
        storage
            .insert_user(&User {
                username: "bob".into(),
                password: "p2".into(),
                salt: "s2".into(),
                ..Default::default()
            })
            .await
            .unwrap();

        // 列表：含全部用户与启用状态
        let users = storage.list_users().await.unwrap();
        assert_eq!(users.len(), 2);
        let alice = users.iter().find(|u| u.username == "alice").unwrap();
        assert!(!alice.enable_webdav && alice.enable_book_source);
        assert_eq!(alice.book_source_limit, 10);

        // 部分字段更新（None 不覆盖）
        let n = storage
            .update_user_permissions(
                "alice",
                Some(true),
                None,
                Some(false),
                None,
                Some(99),
                None,
            )
            .await
            .unwrap();
        assert_eq!(n, 1);
        let alice = storage.find_user("alice").await.unwrap().unwrap();
        assert!(alice.enable_webdav, "enable_webdav 应更新为 true");
        assert!(!alice.enable_book_source, "enable_book_source 应更新为 false");
        assert_eq!(alice.book_source_limit, 99);
        assert_eq!(alice.book_limit, 20, "未提供的字段应保持原值");
        assert_eq!(alice.enable_local_store, false);
        // 不存在的用户 → 0 行
        assert_eq!(
            storage
                .update_user_permissions("ghost", Some(true), None, None, None, None, None)
                .await
                .unwrap(),
            0
        );

        // 删除
        assert_eq!(storage.delete_user("bob").await.unwrap(), 1);
        assert!(storage.find_user("bob").await.unwrap().is_none());
        assert_eq!(storage.delete_user("ghost").await.unwrap(), 0);

        // 重置密码：新密码可校验、token 清空
        let salt = "newsalt";
        let encrypted = crate::util::md5::gen_encrypted_password("新密码123", salt);
        assert_eq!(storage.reset_user_password("alice", salt, &encrypted).await.unwrap(), 1);
        let alice = storage.find_user("alice").await.unwrap().unwrap();
        assert_eq!(alice.password, encrypted);
        assert_eq!(alice.salt, salt);
        assert!(alice.token.is_empty(), "重置密码后旧 token 应失效");
        assert_eq!(
            storage.reset_user_password("ghost", salt, &encrypted).await.unwrap(),
            0
        );

        cleanup(storage, "usermgmt").await;
    }

    /// F-35：可更新书扫描（仅 can_update=1）+ 更新信息回写（含 None 标题不覆盖 latest_chapter_time）
    #[tokio::test]
    async fn test_updatable_books_and_update_info() {
        let storage = test_storage("shelfupd").await;
        let mut b1 = shelf_book("https://book.com/a", "A");
        b1.can_update = true;
        let mut b2 = shelf_book("https://book.com/b", "B");
        b2.can_update = false;
        storage.upsert_book("default", &b1).await.unwrap();
        storage.upsert_book("default", &b2).await.unwrap();

        let updatable = storage.list_updatable_books().await.unwrap();
        assert_eq!(updatable.len(), 1);
        assert_eq!(updatable[0].book_url, "https://book.com/a");

        let affected = storage
            .update_book_update_info("default", "https://book.com/a", Some("第99章"), 99, 123456)
            .await
            .unwrap();
        assert_eq!(affected, 1);
        let book = storage.find_book("default", "https://book.com/a").await.unwrap().unwrap();
        assert_eq!(book.latest_chapter_title.as_deref(), Some("第99章"));
        assert_eq!(book.total_chapter_num, 99);
        assert_eq!(book.latest_chapter_time, 123456);
        assert_eq!(book.last_check_time, 123456);
        assert_eq!(book.last_check_count, 1);

        // 无最新章节（None）→ 标题/时间保持原值，仅检查计数 +1
        storage
            .update_book_update_info("default", "https://book.com/a", None, 99, 888888)
            .await
            .unwrap();
        let book = storage.find_book("default", "https://book.com/a").await.unwrap().unwrap();
        assert_eq!(book.latest_chapter_title.as_deref(), Some("第99章"));
        assert_eq!(book.latest_chapter_time, 123456);
        assert_eq!(book.last_check_time, 888888);
        assert_eq!(book.last_check_count, 2);
        // 不存在的书 → 0 行
        assert_eq!(
            storage
                .update_book_update_info("default", "https://nope.com", Some("x"), 1, 1)
                .await
                .unwrap(),
            0
        );
        cleanup(storage, "shelfupd").await;
    }

    /// F-39：备份 zip 打包（bookshelf/bookSource 等条目；路径在 webdav/legado 下）
    #[tokio::test]
    async fn test_backup_zip() {
        let storage = test_storage("backup").await;
        storage
            .upsert_book("default", &shelf_book("https://book.com/a", "备份书"))
            .await
            .unwrap();
        storage
            .save_book_source("default", &source("https://s.com", "源A", None))
            .await
            .unwrap();

        let path = storage.create_backup_zip("default").await.unwrap();
        let zip_path = std::path::PathBuf::from(&path);
        assert!(zip_path.exists(), "zip 文件应已生成: {path}");
        let name = zip_path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("backup-") && name.ends_with(".zip"), "文件名应为 backup-*.zip: {name}");
        assert!(
            zip_path.parent().and_then(|p| p.file_name()).map(|n| n == "legado").unwrap_or(false),
            "zip 应在 webdav/legado 下: {path}"
        );

        let file = std::fs::File::open(&zip_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        for expect in ["bookshelf.json", "bookSource.json", "bookmark.json", "bookGroup.json", "rssSources.json"] {
            assert!(names.iter().any(|n| n == expect), "zip 应含 {expect}: {names:?}");
        }
        let mut entry = archive.by_name("bookshelf.json").unwrap();
        let mut content = String::new();
        use std::io::Read;
        entry.read_to_string(&mut content).unwrap();
        assert!(content.contains("备份书"), "bookshelf.json 应含书架书");
        drop(entry);
        let mut entry = archive.by_name("bookSource.json").unwrap();
        let mut content = String::new();
        entry.read_to_string(&mut content).unwrap();
        assert!(content.contains("源A"), "bookSource.json 应含书源");

        cleanup(storage, "backup").await;
    }

    /// 测试用最小备份 zip 构造（条目名 → 内容）
    fn make_backup_zip(entries: &[(&str, &str)]) -> Vec<u8> {
        use std::io::Write;
        let mut buf = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(&mut buf);
        for (name, content) in entries {
            writer
                .start_file(*name, zip::write::FileOptions::default())
                .unwrap();
            writer.write_all(content.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
        drop(writer);
        buf.into_inner()
    }

    /// F-55：备份 zip 恢复（最小备份 zip → 各表断言；幂等 skip / overwrite）
    #[tokio::test]
    async fn test_restore_backup_zip() {
        let storage = test_storage("restore").await;
        let zip = make_backup_zip(&[
            (
                "bookSource.json",
                r#"[{"bookSourceUrl":"https://s1.com","bookSourceName":"源1","bookSourceGroup":"小说"}]"#,
            ),
            (
                "bookshelf.json",
                r#"[{"bookUrl":"https://b1.com","name":"书1","author":"甲","origin":"https://s1.com","group":3}]"#,
            ),
            (
                "bookGroup.json",
                r#"[{"id":3,"name":"我的分组","order":1}]"#,
            ),
            (
                "replaceRule.json",
                r#"[{"id":"r1","name":"规则1","find":"甲","replace":"乙","enabled":true,"order":0}]"#,
            ),
            (
                "txtTocRule.json",
                r#"[{"id":"t1","name":"TXT规则","rule":"^第.*章","serialNumber":1}]"#,
            ),
            (
                "rssSources.json",
                r#"[{"sourceUrl":"https://rss.com/feed","sourceName":"RSS源","header":"UA:1"}]"#,
            ),
            (
                "userConfig.json",
                r#"{"theme":"dark","fontSize":"18"}"#,
            ),
            (
                "httpTTS.json",
                r#"[{"url":"https://tts.com/api","name":"TTS源","type":0}]"#,
            ),
            (
                "bookmark.json",
                r#"[{"bookUrl":"https://b1.com","title":"书签1","chapterIndex":2,"paragraphIndex":10}]"#,
            ),
        ]);

        // 首次恢复：全部 restored
        let report = storage.restore_backup_zip("default", &zip, false).await.unwrap();
        assert_eq!(report.restored.sources, 1);
        assert_eq!(report.restored.books, 1);
        assert_eq!(report.restored.groups, 1);
        assert_eq!(report.restored.rules, 1);
        assert_eq!(report.restored.txt_rules, 1);
        assert_eq!(report.restored.rss, 1);
        assert_eq!(report.restored.config, 2);
        assert_eq!(report.restored.tts, 1);
        assert_eq!(report.restored.bookmarks, 1);
        assert_eq!(report.skipped.sources, 0);

        // 各表断言
        let sources = storage.get_book_sources("default").await.unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].book_source_name, "源1");
        assert_eq!(sources[0].book_source_group.as_deref(), Some("小说"));
        assert_eq!(sources[0].user_namespace, "default");
        let books = storage.list_books("default").await.unwrap();
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].name, "书1");
        assert_eq!(books[0].group, 3, "分组 id 应原样恢复");
        assert_eq!(books[0].user_namespace, "default");
        let groups = storage.list_book_groups("default").await.unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, 3);
        assert_eq!(groups[0].name, "我的分组");
        let rules = storage.get_replace_rules("default").await.unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].find, "甲");
        let txt_rules = storage.get_txt_toc_rules("default").await.unwrap();
        assert_eq!(txt_rules.len(), 1);
        assert_eq!(txt_rules[0].rule, "^第.*章");
        let rss = storage.get_rss_sources("default").await.unwrap();
        assert_eq!(rss.len(), 1);
        assert_eq!(rss[0].source_name, "RSS源");
        assert_eq!(
            rss[0].raw_json.as_deref().map(|r| r.contains("\"header\"")),
            Some(true),
            "RSS raw_json 应保留未知字段"
        );
        assert_eq!(
            storage.get_user_config("default", "theme").await.unwrap().as_deref(),
            Some("dark")
        );
        assert_eq!(
            storage.get_user_config("default", "fontSize").await.unwrap().as_deref(),
            Some("18")
        );
        let tts = storage.get_http_tts_list("default").await.unwrap();
        assert_eq!(tts.len(), 1);
        assert_eq!(tts[0].url, "https://tts.com/api");
        let bookmarks = storage.list_bookmarks("default", "https://b1.com").await.unwrap();
        assert_eq!(bookmarks.len(), 1);
        assert_eq!(bookmarks[0].chapter_index, 2);

        // 幂等：overwrite=false 再恢复 → 全部 skipped
        let report = storage.restore_backup_zip("default", &zip, false).await.unwrap();
        assert_eq!(report.restored.sources, 0);
        assert_eq!(report.skipped.sources, 1);
        assert_eq!(report.skipped.books, 1);
        assert_eq!(report.skipped.groups, 1);
        assert_eq!(report.skipped.rules, 1);
        assert_eq!(report.skipped.txt_rules, 1);
        assert_eq!(report.skipped.rss, 1);
        assert_eq!(report.skipped.config, 2);
        assert_eq!(report.skipped.tts, 1);
        assert_eq!(report.skipped.bookmarks, 1);
        assert_eq!(report.restored.config, 0);

        // overwrite=true → 全部覆盖 restored
        let report = storage.restore_backup_zip("default", &zip, true).await.unwrap();
        assert_eq!(report.restored.sources, 1);
        assert_eq!(report.restored.books, 1);
        assert_eq!(report.restored.groups, 1);
        assert_eq!(report.restored.rules, 1);
        assert_eq!(report.restored.txt_rules, 1);
        assert_eq!(report.restored.rss, 1);
        assert_eq!(report.restored.config, 2);
        assert_eq!(report.restored.tts, 1);
        assert_eq!(report.restored.bookmarks, 1);
        assert_eq!(report.skipped.sources, 0);
        assert_eq!(storage.get_book_sources("default").await.unwrap().len(), 1, "覆盖不应产生重复行");

        // 命名空间隔离：恢复进 alice 命名空间互不影响 default
        // （book_sources 表主键为 book_source_url（全局），跨用户恢复需用不同 URL）
        storage.insert_user(&User {
            username: "alice".into(),
            ..Default::default()
        }).await.unwrap();
        let zip_alice = make_backup_zip(&[
            (
                "bookSource.json",
                r#"[{"bookSourceUrl":"https://s5.com","bookSourceName":"源5"}]"#,
            ),
            ("bookshelf.json", r#"[{"bookUrl":"https://b5.com","name":"书5"}]"#),
        ]);
        let report = storage.restore_backup_zip("alice", &zip_alice, false).await.unwrap();
        assert_eq!(report.restored.sources, 1);
        assert_eq!(report.restored.books, 1);
        assert_eq!(storage.get_book_sources("alice").await.unwrap().len(), 1);
        assert_eq!(storage.get_book_sources("default").await.unwrap().len(), 1, "default 不受影响");
        assert_eq!(storage.list_books("default").await.unwrap().len(), 1, "default 书架不受影响");

        cleanup(storage, "restore").await;
    }

    /// F-55：legacy 布局兼容——config/ 目录 + books.json 命名；非法 zip / 无识别条目报错
    #[tokio::test]
    async fn test_restore_backup_zip_legacy_layout() {
        let storage = test_storage("restorelegacy").await;
        let zip = make_backup_zip(&[
            (
                "config/bookSource.json",
                r#"[{"bookSourceUrl":"https://s2.com","bookSourceName":"源2"}]"#,
            ),
            (
                "config/books.json",
                r#"[{"bookUrl":"https://b2.com","name":"书2"}]"#,
            ),
            (
                "config/bookGroup.json",
                r#"[{"id":7,"name":"legacy分组","order":0}]"#,
            ),
        ]);
        let report = storage.restore_backup_zip("default", &zip, false).await.unwrap();
        assert_eq!(report.restored.sources, 1);
        assert_eq!(report.restored.books, 1);
        assert_eq!(report.restored.groups, 1);
        let books = storage.list_books("default").await.unwrap();
        assert_eq!(books[0].name, "书2");
        let groups = storage.list_book_groups("default").await.unwrap();
        assert_eq!(groups[0].id, 7);

        // 条目同时存在于根与 config/ 时优先根（当前布局覆盖 legacy）
        let zip2 = make_backup_zip(&[
            ("bookSource.json", r#"[{"bookSourceUrl":"https://s3.com","bookSourceName":"根源"}]"#),
            ("config/bookSource.json", r#"[{"bookSourceUrl":"https://s4.com","bookSourceName":"config源"}]"#),
        ]);
        let report = storage.restore_backup_zip("default", &zip2, true).await.unwrap();
        assert_eq!(report.restored.sources, 1);
        assert_eq!(storage.get_book_sources("default").await.unwrap()[0].book_source_url, "https://s3.com");

        // 无效条目（空 url/空名）计入 skipped
        let zip3 = make_backup_zip(&[
            ("bookSource.json", r#"[{"bookSourceName":"没URL"},{"bookSourceUrl":"https://ok.com","bookSourceName":"OK"}]"#),
            ("bookshelf.json", r#"[{"name":"没URL"}]"#),
            ("httpTTS.json", r#"[{"name":"没URL"}]"#),
        ]);
        let report = storage.restore_backup_zip("default", &zip3, false).await.unwrap();
        assert_eq!(report.restored.sources, 1);
        assert_eq!(report.skipped.sources, 1);
        assert_eq!(report.skipped.books, 1);
        assert_eq!(report.skipped.tts, 1);

        // 非 zip 字节 → Err
        assert!(storage.restore_backup_zip("default", b"not a zip file", false).await.is_err());
        // 合法 zip 但无识别条目 → Err
        let empty = make_backup_zip(&[("readme.txt", "hi")]);
        assert!(storage.restore_backup_zip("default", &empty, false).await.is_err());

        cleanup(storage, "restorelegacy").await;
    }

    /// F-35：定时任务主循环（本地书/无书源书跳过；网络书源缺失时静默跳过不报错）
    #[tokio::test]
    async fn test_run_shelf_update_skips() {
        let storage = test_storage("shelfrun").await;
        // 本地书（跳过）
        storage
            .upsert_book("default", &shelf_book("local://abc", "本地书"))
            .await
            .unwrap();
        // 无 tocUrl（跳过）
        let mut b = shelf_book("https://book.com/notoc", "无目录");
        b.toc_url = String::new();
        storage.upsert_book("default", &b).await.unwrap();
        // 无书源（跳过）
        storage
            .upsert_book("default", &shelf_book("https://book.com/nosrc", "无源"))
            .await
            .unwrap();

        // 不应报错、不应更新任何书
        assert_eq!(run_shelf_update(&storage).await.unwrap(), 0);
        let book = storage.find_book("default", "https://book.com/nosrc").await.unwrap().unwrap();
        assert_eq!(book.last_check_count, 0);
        cleanup(storage, "shelfrun").await;
    }

    /// F-28：替换规则 CRUD 往返 + 命名空间隔离 + default 回退
    #[tokio::test]
    async fn test_replace_rules_roundtrip() {
        let storage = test_storage("replrule").await;
        use crate::model::ReplaceRule;
        let rule = |id: &str, name: &str, order: i64| ReplaceRule {
            id: id.into(),
            name: name.into(),
            find: format!("找{name}"),
            replace: format!("替{name}"),
            enabled: true,
            order,
            ..Default::default()
        };

        // 保存两条（order 逆序）→ 按 order_num 排序返回
        storage.save_replace_rule("default", &rule("r1", "一", 2)).await.unwrap();
        storage.save_replace_rule("default", &rule("r2", "二", 1)).await.unwrap();
        let list = storage.get_replace_rules("default").await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "r2", "应按 order_num 排序");
        assert_eq!(list[1].id, "r1");
        assert_eq!(list[0].find, "找二");
        assert_eq!(list[0].user_namespace, "default");

        // 覆盖保存（同 id）
        let mut r = rule("r1", "一v2", 2);
        r.enabled = false;
        storage.save_replace_rule("default", &r).await.unwrap();
        let list = storage.get_replace_rules("default").await.unwrap();
        assert_eq!(list.len(), 2);
        assert!(!list[1].enabled);
        assert_eq!(list[1].name, "一v2");

        // 批量保存（事务）
        storage
            .save_replace_rules("default", &[rule("r3", "三", 3), rule("r4", "四", 4)])
            .await
            .unwrap();
        assert_eq!(storage.get_replace_rules("default").await.unwrap().len(), 4);

        // 删除
        assert_eq!(storage.delete_replace_rule("default", "r3").await.unwrap(), 1);
        assert_eq!(storage.delete_replace_rule("default", "ghost").await.unwrap(), 0);
        assert_eq!(storage.get_replace_rules("default").await.unwrap().len(), 3);

        // 命名空间隔离：alice 无规则时回退 default
        assert_eq!(storage.get_replace_rules("alice").await.unwrap().len(), 3);
        storage.save_replace_rule("alice", &rule("a1", "爱丽丝", 0)).await.unwrap();
        let alice = storage.get_replace_rules("alice").await.unwrap();
        assert_eq!(alice.len(), 1);
        assert_eq!(alice[0].name, "爱丽丝");
        // 删除只影响本命名空间
        assert_eq!(storage.delete_replace_rule("alice", "r1").await.unwrap(), 0);
        assert_eq!(storage.get_replace_rules("default").await.unwrap().len(), 3);

        cleanup(storage, "replrule").await;
    }

    /// F-26：HttpTTS CRUD 往返 + 命名空间隔离 + default 回退
    #[tokio::test]
    async fn test_http_tts_roundtrip() {
        let storage = test_storage("httptts").await;
        use crate::model::HttpTts;
        let tts = |url: &str, name: &str, ty: i64| HttpTts {
            url: url.into(),
            name: name.into(),
            tts_type: ty,
            ..Default::default()
        };

        storage.save_http_tts("default", &tts("https://tts.example.com/a", "引擎甲", 0)).await.unwrap();
        storage.save_http_tts("default", &tts("https://tts.example.com/b", "引擎乙", 1)).await.unwrap();
        let list = storage.get_http_tts_list("default").await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "引擎乙", "应按名称排序");
        assert_eq!(list[0].tts_type, 1);
        assert_eq!(list[1].url, "https://tts.example.com/a");

        // 同 url 覆盖
        storage.save_http_tts("default", &tts("https://tts.example.com/a", "引擎甲v2", 0)).await.unwrap();
        let list = storage.get_http_tts_list("default").await.unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|t| t.name == "引擎甲v2"));

        // 删除
        assert_eq!(storage.delete_http_tts("default", "https://tts.example.com/a").await.unwrap(), 1);
        assert_eq!(storage.get_http_tts_list("default").await.unwrap().len(), 1);

        // 命名空间隔离 + default 回退
        assert_eq!(storage.get_http_tts_list("alice").await.unwrap().len(), 1, "空命名空间回退 default");
        storage.save_http_tts("alice", &tts("https://tts.example.com/x", "爱丽丝引擎", 0)).await.unwrap();
        assert_eq!(storage.get_http_tts_list("alice").await.unwrap().len(), 1);
        assert_eq!(storage.delete_http_tts("alice", "https://tts.example.com/b").await.unwrap(), 0);

        cleanup(storage, "httptts").await;
    }

    /// 自定义 TXT 目录规则：保存/排序/删除/导入默认规则 + 命名空间隔离
    #[tokio::test]
    async fn test_txt_toc_rules_flow() {
        let storage = test_storage("txttoc").await;
        use crate::model::TxtTocRule;
        let rule = |id: &str, name: &str, re: &str, sn: i64| TxtTocRule {
            id: id.into(),
            name: name.into(),
            rule: re.into(),
            enable: true,
            serial_number: sn,
            ..Default::default()
        };

        // 初始无用户规则
        assert!(storage.get_txt_toc_rules("default").await.unwrap().is_empty());

        // 保存（乱序 serialNumber → 按序返回）
        storage.save_txt_toc_rule("default", &rule("t1", "自定义A", r"^第.+章$", 5)).await.unwrap();
        storage.save_txt_toc_rule("default", &rule("t2", "自定义B", r"^楔子$", 1)).await.unwrap();
        let list = storage.get_txt_toc_rules("default").await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "t2", "应按 serial_number 排序");
        assert_eq!(list[1].name, "自定义A");
        assert_eq!(list[1].user_namespace, "default");

        // 覆盖 + 禁用
        let mut r = rule("t1", "自定义Av2", r"^第.+章$", 5);
        r.enable = false;
        storage.save_txt_toc_rule("default", &r).await.unwrap();
        let list = storage.get_txt_toc_rules("default").await.unwrap();
        assert!(!list[1].enable);

        // 删除
        assert_eq!(storage.delete_txt_toc_rule("default", "t2").await.unwrap(), 1);
        assert_eq!(storage.get_txt_toc_rules("default").await.unwrap().len(), 1);

        // 导入默认规则（幂等）
        let count = storage.import_default_txt_toc_rules("default").await.unwrap();
        assert_eq!(count, crate::service::local_book::DEFAULT_TOC_RULES.len());
        let list = storage.get_txt_toc_rules("default").await.unwrap();
        let default_ids = list.iter().filter(|r| r.id.starts_with("default-")).count();
        assert_eq!(default_ids, crate::service::local_book::DEFAULT_TOC_RULES.len());
        assert_eq!(storage.import_default_txt_toc_rules("default").await.unwrap(), count, "重复导入不新增");
        assert_eq!(storage.get_txt_toc_rules("default").await.unwrap().len(), list.len());

        // 命名空间隔离：alice 无规则（不查 default）
        assert!(storage.get_txt_toc_rules("alice").await.unwrap().is_empty());

        cleanup(storage, "txttoc").await;
    }

    /// getSystemInfo 统计：用户数/书数/书源数（全命名空间）
    #[tokio::test]
    async fn test_system_info_counts() {
        let storage = test_storage("sysinfo").await;
        storage
            .insert_user(&User {
                username: "alice".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        storage
            .upsert_book("default", &shelf_book("https://book.com/a", "A"))
            .await
            .unwrap();
        storage
            .upsert_book("alice", &shelf_book("https://book.com/b", "B"))
            .await
            .unwrap();
        storage
            .save_book_source("default", &source("https://s.com", "源A", None))
            .await
            .unwrap();
        storage
            .save_book_source("alice", &source("https://s2.com", "源B", None))
            .await
            .unwrap();

        assert_eq!(storage.count_users().await.unwrap(), 1);
        assert_eq!(storage.count_books().await.unwrap(), 2);
        assert_eq!(storage.count_all_book_sources().await.unwrap(), 2);
        cleanup(storage, "sysinfo").await;
    }

    /// 缓存管理：getCacheInfo 统计（toc_cache 行数 / book_chapters 行数 / sum length 大小）+
    /// clearCache 按 type 清空（toc / chapters / all）
    #[tokio::test]
    async fn test_cache_info_and_clear() {
        let storage = test_storage("cache").await;

        // 空库：全零
        let info = storage.get_cache_info().await.unwrap();
        assert_eq!(info.toc_cache_count, 0);
        assert_eq!(info.chapter_count, 0);
        assert_eq!(info.chapter_size, 0);
        assert_eq!(info.total_size, 0);

        // 写入目录缓存 2 条 + 章节 3 条
        storage.cache_toc("https://book.com/a", "https://book.com/toc", "[{\"title\":\"第一章\"}]").await.unwrap();
        storage.cache_toc("https://book.com/b", "https://book.com/toc2", "[{\"title\":\"第二章\"}]").await.unwrap();
        storage.save_chapters("local://book1", &[
            ("第一章".to_string(), "正文一甲乙丙丁".to_string()),
            ("第二章".to_string(), "正文二戊己庚辛壬癸".to_string()),
            ("第三章".to_string(), "正文三子丑寅卯".to_string()),
        ])
        .await
        .unwrap();

        let info = storage.get_cache_info().await.unwrap();
        assert_eq!(info.toc_cache_count, 2);
        assert_eq!(info.toc_cache_size, 34, "SQLite length() 按字符计，两条各 17 字符");
        assert_eq!(info.chapter_count, 3);
        assert_eq!(info.chapter_size, 23, "7+9+7 字符");
        assert_eq!(info.total_size, info.toc_cache_size + info.chapter_size);

        // 只清 toc
        let (toc_del, chap_del) = storage.clear_cache("toc").await.unwrap();
        assert_eq!(toc_del, 2);
        assert_eq!(chap_del, 0);
        let info = storage.get_cache_info().await.unwrap();
        assert_eq!(info.toc_cache_count, 0);
        assert_eq!(info.chapter_count, 3, "章节缓存不受影响");

        // 只清 chapters
        let (toc_del, chap_del) = storage.clear_cache("chapters").await.unwrap();
        assert_eq!(toc_del, 0);
        assert_eq!(chap_del, 3);
        let info = storage.get_cache_info().await.unwrap();
        assert_eq!(info.chapter_count, 0);
        assert_eq!(info.total_size, 0);

        // all：全清（再写入后验证）
        storage.cache_toc("https://book.com/a", "https://book.com/toc", "[]").await.unwrap();
        storage.save_chapters("local://book1", &[("第四章".to_string(), "正文四".to_string())]).await.unwrap();
        let (toc_del, chap_del) = storage.clear_cache("all").await.unwrap();
        assert_eq!(toc_del, 1);
        assert_eq!(chap_del, 1);
        let info = storage.get_cache_info().await.unwrap();
        assert_eq!(info.toc_cache_count, 0);
        assert_eq!(info.chapter_count, 0);
        assert_eq!(info.total_size, 0);

        // 未知 type：不删任何表
        let (toc_del, chap_del) = storage.clear_cache("unknown").await.unwrap();
        assert_eq!(toc_del, 0);
        assert_eq!(chap_del, 0);

        cleanup(storage, "cache").await;
    }

    /// 单书缓存：book_cache_info（章节数+大小）/ delete_book_cache（仅删该书，不动书架）
    #[tokio::test]
    async fn test_book_cache_info_and_delete() {
        let storage = test_storage("bookcache").await;
        // 书 A：本地书章节 + 书源书正文缓存（md5 键）混合
        storage
            .save_chapters(
                "https://book.com/a",
                &[
                    ("第一章".to_string(), "正文一二三四五".to_string()),
                    ("第二章".to_string(), "正文六七八九十".to_string()),
                ],
            )
            .await
            .unwrap();
        storage
            .cache_chapter_content(
                "https://book.com/a",
                crate::util::md5::chapter_url_hash("https://book.com/c/3"),
                "第三章",
                "正文三",
            )
            .await
            .unwrap();
        storage
            .save_chapters("https://book.com/b", &[("第一章".to_string(), "另一本".to_string())])
            .await
            .unwrap();
        storage
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

        // 信息统计：A 共 3 章（本地 2 + 缓存 1），size = 7+7+3 字符
        let (count, size) = storage.book_cache_info("https://book.com/a").await.unwrap();
        assert_eq!(count, 3);
        assert_eq!(size, 17);
        // 无缓存书 → 0
        let (count, size) = storage.book_cache_info("https://ghost.com").await.unwrap();
        assert_eq!((count, size), (0, 0));

        // 删单书缓存：只删 A 的章节，B 不受影响、书架行保留
        let deleted = storage.delete_book_cache("https://book.com/a").await.unwrap();
        assert_eq!(deleted, 3);
        assert_eq!(storage.count_chapters("https://book.com/a").await.unwrap(), 0);
        assert_eq!(storage.count_chapters("https://book.com/b").await.unwrap(), 1);
        assert!(
            storage
                .find_book("default", "https://book.com/a")
                .await
                .unwrap()
                .is_some(),
            "删除缓存不应影响书架"
        );
        // 再删空书 → 0 行
        assert_eq!(storage.delete_book_cache("https://ghost.com").await.unwrap(), 0);

        cleanup(storage, "bookcache").await;
    }

    /// GAP 96：连接初始化后 WAL 模式 + busy_timeout 生效（幂等：重复 init 不报错且保持 WAL）
    #[tokio::test]
    async fn test_wal_and_busy_timeout() {
        let storage = test_storage("wal").await;
        // journal_mode 返回当前模式字符串（wal 持久化于库文件头）
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&storage.pool)
            .await
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal", "应启用 WAL 模式，实际 {mode}");
        // busy_timeout 毫秒值
        let timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(&storage.pool)
            .await
            .unwrap();
        assert_eq!(timeout, 5000, "busy_timeout 应为 5000ms");
        // WAL 副作用文件存在（reader.db-wal 惰性创建——写一次触发）
        storage
            .save_chapters("local://waltest", &[("第一章".to_string(), "正文".to_string())])
            .await
            .unwrap();
        let db_path = storage.config.storage_dir().join("reader.db");
        assert!(db_path.exists());

        // 幂等：关闭后重新 init 同一目录，仍为 WAL 且 busy_timeout 生效
        let dir = storage.config.work_dir.clone();
        storage.pool.close().await;
        let mut config = AppConfig::from_env();
        config.work_dir = dir.clone();
        let reopened = init(&config).await.unwrap();
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&reopened.pool)
            .await
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal", "重开库应保持 WAL");
        let timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(&reopened.pool)
            .await
            .unwrap();
        assert_eq!(timeout, 5000);
        cleanup(reopened, "wal").await;
    }

    /// GAP 96 回归：WAL 模式下池连接不持有跨语句旧读快照——
    /// 连接 A 建立读快照后，连接 B 写入，A 再次读取必须看到新数据
    #[tokio::test]
    async fn test_wal_no_stale_snapshot_across_connections() {
        let storage = test_storage("snaprepro").await;
        // 连接 A：先读一次（若 WAL 隐式读事务跨语句保持，此处建立旧快照）
        let mut conn_a = storage.pool.acquire().await.unwrap();
        let _: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
            .fetch_one(&mut *conn_a)
            .await
            .unwrap();
        // 连接 B：写入
        let mut conn_b = storage.pool.acquire().await.unwrap();
        sqlx::query("INSERT INTO books (book_url, name, user_namespace) VALUES (?1, ?2, 'default')")
            .bind("https://snap.com/a")
            .bind("快照书")
            .execute(&mut *conn_b)
            .await
            .unwrap();
        drop(conn_b);
        // 连接 A：再次读取——必须能看到写入（快照已刷新）
        let found: Option<(String,)> =
            sqlx::query_as("SELECT name FROM books WHERE book_url = ?1 AND user_namespace = 'default'")
                .bind("https://snap.com/a")
                .fetch_optional(&mut *conn_a)
                .await
                .unwrap();
        assert_eq!(found.map(|r| r.0), Some("快照书".to_string()), "WAL 下旧读快照导致跨连接读不到新数据");
        drop(conn_a);
        cleanup(storage, "snaprepro").await;
    }

    /// 全书搜索：LIKE 匹配 + 命中摘要（前后截取）+ %/_ 转义 + 章节序 + limit
    #[tokio::test]
    async fn test_search_book_content() {
        let storage = test_storage("search").await;
        storage
            .save_chapters(
                "local://book1",
                &[
                    ("第一章".to_string(), "这是第一章的正文，关键词出现了。".to_string()),
                    ("第二章".to_string(), "本章没有匹配内容。".to_string()),
                    ("第三章".to_string(), "在很久很久以前，有一个非常非常长的开头铺垫，它洋洋洒洒写了很多很多字，然后关键词在这里再次出现，后面还有一点内容。".to_string()),
                ],
            )
            .await
            .unwrap();
        storage
            .save_chapters(
                "local://book2",
                &[("第一章".to_string(), "另一本书里的关键词。".to_string())],
            )
            .await
            .unwrap();

        // 命中两章，按章节序返回
        let hits = storage.search_book_content("local://book1", "关键词", 50).await.unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].chapter_index, 0);
        assert_eq!(hits[0].title, "第一章");
        assert!(hits[0].snippet.contains("关键词"));
        assert!(hits[0].snippet.starts_with("这是第一章"));
        assert_eq!(hits[1].chapter_index, 2);
        assert!(hits[1].snippet.contains("关键词"));
        assert!(hits[1].snippet.starts_with("…"), "超长段落应截断补省略号: {}", hits[1].snippet);

        // 其他书不串
        let hits = storage.search_book_content("local://book2", "关键词", 50).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "第一章");

        // 无命中 / 书不存在
        assert!(storage.search_book_content("local://book1", "不存在词", 50).await.unwrap().is_empty());
        assert!(storage.search_book_content("local://ghost", "关键词", 50).await.unwrap().is_empty());

        // 大小写不敏感（ASCII）
        storage
            .save_chapters("local://book3", &[("Ch1".to_string(), "Hello World here".to_string())])
            .await
            .unwrap();
        let hits = storage.search_book_content("local://book3", "world", 50).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.contains("World"));

        // limit 生效
        let hits = storage.search_book_content("local://book1", "关键词", 1).await.unwrap();
        assert_eq!(hits.len(), 1);

        // %/_ 作为字面量转义（不当作 LIKE 通配符）
        storage
            .save_chapters(
                "local://book4",
                &[
                    ("C1".to_string(), "进度5_0%完成。".to_string()),
                    ("C2".to_string(), "完全没有任何特殊符号的一章。".to_string()),
                ],
            )
            .await
            .unwrap();
        let hits = storage.search_book_content("local://book4", "5_0%", 10).await.unwrap();
        assert_eq!(hits.len(), 1, "% 应转义为字面量");
        assert_eq!(hits[0].title, "C1");
        let hits = storage.search_book_content("local://book4", "5_", 10).await.unwrap();
        assert_eq!(hits.len(), 1, "_ 应转义为字面量");
        let hits = storage.search_book_content("local://book4", "%", 10).await.unwrap();
        assert_eq!(hits.len(), 1, "% 转义后只匹配含字面 % 的行（未转义会匹配全部）");
        assert_eq!(hits[0].title, "C1");
        let hits = storage.search_book_content("local://book4", "_", 10).await.unwrap();
        assert_eq!(hits.len(), 1, "_ 转义后只匹配含字面 _ 的行（未转义会匹配全部）");
        assert_eq!(hits[0].title, "C1");
        assert_eq!(storage.count_chapters("local://book4").await.unwrap(), 2);
        assert_eq!(storage.count_chapters("local://ghost").await.unwrap(), 0);

        cleanup(storage, "search").await;
    }

    /// 书源订阅：CRUD 往返 + 命名空间隔离 + default 回退
    #[tokio::test]
    async fn test_source_sub_crud() {
        let storage = test_storage("subs").await;
        let raw = r#"[{"bookSourceUrl":"https://s1.com","bookSourceName":"源1"}]"#;

        // 保存 → 查询往返（raw_json 原文保留）
        storage.save_source_sub("default", "https://sub.com/all.json", "全部书源", raw).await.unwrap();
        let list = storage.get_source_subs("default").await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].url, "https://sub.com/all.json");
        assert_eq!(list[0].name, "全部书源");
        assert!(list[0].enabled);
        assert_eq!(list[0].raw_json.as_deref(), Some(raw));
        assert_eq!(list[0].user_namespace, "default");

        // 覆盖保存（改名）
        storage.save_source_sub("default", "https://sub.com/all.json", "全部书源v2", raw).await.unwrap();
        let list = storage.get_source_subs("default").await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "全部书源v2");

        // 按 URL 查找
        let sub = storage.find_source_sub("default", "https://sub.com/all.json").await.unwrap().unwrap();
        assert_eq!(sub.name, "全部书源v2");
        assert!(storage.find_source_sub("default", "https://sub.com/ghost").await.unwrap().is_none());

        // 命名空间隔离 + default 回退
        assert_eq!(storage.get_source_subs("alice").await.unwrap().len(), 1, "alice 无订阅回退 default");
        assert!(storage.find_source_sub("alice", "https://sub.com/all.json").await.unwrap().is_some());
        storage.save_source_sub("alice", "https://sub.com/a.json", "爱丽丝订阅", raw).await.unwrap();
        let alice = storage.get_source_subs("alice").await.unwrap();
        assert_eq!(alice.len(), 1, "有自有订阅后不再回退 default");
        assert_eq!(alice[0].name, "爱丽丝订阅");

        // 删除：只影响本命名空间；不存在返回 0 行
        assert_eq!(storage.delete_source_sub("alice", "https://sub.com/all.json").await.unwrap(), 0);
        assert_eq!(storage.delete_source_sub("alice", "https://sub.com/a.json").await.unwrap(), 1);
        assert_eq!(storage.get_source_subs("alice").await.unwrap().len(), 1, "回退 default");
        assert_eq!(storage.delete_source_sub("default", "https://sub.com/all.json").await.unwrap(), 1);
        assert!(storage.get_source_subs("default").await.unwrap().is_empty());

        cleanup(storage, "subs").await;
    }

    /// 书源书正文缓存：chapterUrl md5 哈希键写入 → 同键读取；与本地书顺序索引键域不重叠；覆盖写
    #[tokio::test]
    async fn test_chapter_content_cache_roundtrip() {
        let storage = test_storage("chapcache").await;
        let book_url = "https://book.com/a";
        let url1 = "https://book.com/1.html";
        let url2 = "https://book.com/2.html";
        let idx1 = crate::util::md5::chapter_url_hash(url1);
        let idx2 = crate::util::md5::chapter_url_hash(url2);
        assert!(idx1 > 0 && idx2 > 0, "哈希恒为正");
        assert_ne!(idx1, idx2, "不同 chapterUrl 哈希不同");

        // 写入 → 同 chapterUrl 直读
        storage
            .cache_chapter_content(book_url, idx1, "第一章", "第一章正文内容。")
            .await
            .unwrap();
        let got = storage.get_chapter_content(book_url, idx1).await.unwrap();
        assert_eq!(got.as_deref(), Some("第一章正文内容。"));
        assert_eq!(storage.get_chapter_content(book_url, idx2).await.unwrap(), None, "未缓存键应无命中");

        // 覆盖写（同一 chapterUrl 再次缓存更新正文）
        storage
            .cache_chapter_content(book_url, idx1, "第一章", "更新后的正文。")
            .await
            .unwrap();
        assert_eq!(
            storage.get_chapter_content(book_url, idx1).await.unwrap().as_deref(),
            Some("更新后的正文。")
        );

        // 与本地书顺序索引共存：哈希键域（~2^60）不重叠 0..n
        storage
            .save_chapters(book_url, &[("本地1".to_string(), "本地内容1".to_string())])
            .await
            .unwrap();
        assert_eq!(storage.count_chapters(book_url).await.unwrap(), 2, "缓存行 + 本地行共存");
        assert_eq!(
            storage.get_chapter_content(book_url, 0).await.unwrap().as_deref(),
            Some("本地内容1")
        );
        assert_eq!(
            storage.get_chapter_content(book_url, idx1).await.unwrap().as_deref(),
            Some("更新后的正文。")
        );

        // 不同书同 chapterUrl → 按 book_url 隔离
        storage
            .cache_chapter_content("https://book.com/b", idx1, "第一章", "B 书正文。")
            .await
            .unwrap();
        assert_eq!(
            storage.get_chapter_content("https://book.com/a", idx1).await.unwrap().as_deref(),
            Some("更新后的正文。")
        );
        assert_eq!(
            storage.get_chapter_content("https://book.com/b", idx1).await.unwrap().as_deref(),
            Some("B 书正文。")
        );

        cleanup(storage, "chapcache").await;
    }

    /// 分组收尾：带书数列表 / 重命名保留 order / 删除分组组内书置 0 + 命名空间隔离
    #[tokio::test]
    async fn test_book_group_count_rename_delete() {
        let storage = test_storage("grpfin").await;
        let g1 = storage
            .save_book_group("default", &crate::model::BookGroup {
                name: "玄幻".into(),
                order: 1,
                ..Default::default()
            })
            .await
            .unwrap();
        let g2 = storage
            .save_book_group("default", &crate::model::BookGroup {
                name: "言情".into(),
                order: 2,
                ..Default::default()
            })
            .await
            .unwrap();
        // 书：g1 两本、g2 一本、未分组一本（group 0 不计入任何组）
        storage.upsert_book("default", &shelf_book("https://b.com/1", "书1")).await.unwrap();
        storage.upsert_book("default", &shelf_book("https://b.com/2", "书2")).await.unwrap();
        storage.upsert_book("default", &shelf_book("https://b.com/3", "书3")).await.unwrap();
        storage.upsert_book("default", &shelf_book("https://b.com/4", "书4")).await.unwrap();
        storage.update_book_group_id("default", "https://b.com/1", g1.id).await.unwrap();
        storage.update_book_group_id("default", "https://b.com/2", g1.id).await.unwrap();
        storage.update_book_group_id("default", "https://b.com/3", g2.id).await.unwrap();

        // 带书数列表（bookCount + orderNum 别名）
        let list = storage.list_book_groups_with_count("default").await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "玄幻");
        assert_eq!(list[0].book_count, 2);
        assert_eq!(list[0].order, 1);
        assert_eq!(list[0].order_num, 1);
        assert_eq!(list[1].name, "言情");
        assert_eq!(list[1].book_count, 1);

        // 重命名：仅改 name，order/id 保留；不存在返回 0 行
        assert_eq!(storage.rename_book_group("default", g1.id, "玄幻v2").await.unwrap(), 1);
        assert_eq!(storage.rename_book_group("default", 9999, "幽灵").await.unwrap(), 0);
        let list = storage.list_book_groups_with_count("default").await.unwrap();
        assert_eq!(list[0].name, "玄幻v2");
        assert_eq!(list[0].order, 1, "重命名保留 order");
        assert_eq!(list[0].id, g1.id, "重命名保留 id");
        assert_eq!(list[0].book_count, 2, "重命名不影响书数");

        // 删除 g1：组内书置 0，组删除；g2 与书不受影响
        assert_eq!(storage.delete_book_group("default", g1.id).await.unwrap(), 1);
        assert_eq!(storage.delete_book_group("default", g1.id).await.unwrap(), 0, "重复删除 0 行");
        let list = storage.list_book_groups_with_count("default").await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "言情");
        let b1 = storage.find_book("default", "https://b.com/1").await.unwrap().unwrap();
        assert_eq!(b1.group, 0, "组内书应置 0（未分组）");
        let b2 = storage.find_book("default", "https://b.com/2").await.unwrap().unwrap();
        assert_eq!(b2.group, 0);
        let b3 = storage.find_book("default", "https://b.com/3").await.unwrap().unwrap();
        assert_eq!(b3.group, g2.id, "其他组书不受影响");

        // 命名空间隔离：alice 删除不了 default 的分组
        assert_eq!(storage.delete_book_group("alice", g2.id).await.unwrap(), 0);
        assert_eq!(storage.list_book_groups("default").await.unwrap().len(), 1);
        assert_eq!(storage.rename_book_group("alice", g2.id, "越权改名").await.unwrap(), 0);

        cleanup(storage, "grpfin").await;
    }

    // ---------------- 书源登录态 cookie（按用户隔离） ----------------

    #[tokio::test]
    async fn test_cookie_roundtrip_and_namespace_isolation() {
        let storage = test_storage("cookie").await;

        // 初始无 cookie
        assert_eq!(
            storage.get_cookie("default", "https://a.com").await.unwrap(),
            None
        );

        // 写入 → 读回
        storage
            .set_cookie("default", "https://a.com", "sid=abc; token=xyz")
            .await
            .unwrap();
        assert_eq!(
            storage.get_cookie("default", "https://a.com").await.unwrap(),
            Some("sid=abc; token=xyz".to_string())
        );

        // 覆盖写
        storage.set_cookie("default", "https://a.com", "sid=def").await.unwrap();
        assert_eq!(
            storage.get_cookie("default", "https://a.com").await.unwrap(),
            Some("sid=def".to_string())
        );

        // 按用户隔离：alice 读不到 default 的 cookie
        assert_eq!(storage.get_cookie("alice", "https://a.com").await.unwrap(), None);
        storage
            .set_cookie("alice", "https://a.com", "sid=alice")
            .await
            .unwrap();
        assert_eq!(
            storage.get_cookie("default", "https://a.com").await.unwrap(),
            Some("sid=def".to_string())
        );
        assert_eq!(
            storage.get_cookie("alice", "https://a.com").await.unwrap(),
            Some("sid=alice".to_string())
        );

        // 清除
        assert_eq!(storage.clear_cookie("alice", "https://a.com").await.unwrap(), 1);
        assert_eq!(storage.get_cookie("alice", "https://a.com").await.unwrap(), None);
        assert_eq!(storage.clear_cookie("alice", "https://a.com").await.unwrap(), 0);

        cleanup(storage, "cookie").await;
    }

    #[tokio::test]
    async fn test_cookie_by_base_matching() {
        let storage = test_storage("cookiebase").await;
        storage
            .set_cookie("default", "https://a.com", "sid=abc")
            .await
            .unwrap();
        // `##` 备用地址后缀：主地址命中
        storage
            .set_cookie("default", "https://b.com##https://b2.com", "sid=bbb")
            .await
            .unwrap();

        // 请求 URL 的 base 命中书源 source_url base（含端口/路径差异）
        assert_eq!(
            storage.get_cookie_by_base("default", "https://a.com").await.unwrap(),
            Some("sid=abc".to_string())
        );
        assert_eq!(
            storage.get_cookie_by_base("default", "https://a.com/book/1?x=2").await.unwrap(),
            Some("sid=abc".to_string())
        );
        assert_eq!(
            storage.get_cookie_by_base("default", "https://b2.com/path").await.unwrap(),
            Some("sid=bbb".to_string())
        );
        // 不匹配
        assert_eq!(storage.get_cookie_by_base("default", "https://c.com").await.unwrap(), None);
        assert_eq!(storage.get_cookie_by_base("alice", "https://a.com").await.unwrap(), None);
        // 端口不同不命中
        storage
            .set_cookie("default", "https://d.com:8443", "sid=dd")
            .await
            .unwrap();
        assert_eq!(storage.get_cookie_by_base("default", "https://d.com").await.unwrap(), None);
        assert_eq!(
            storage.get_cookie_by_base("default", "https://d.com:8443/x").await.unwrap(),
            Some("sid=dd".to_string())
        );

        cleanup(storage, "cookiebase").await;
    }

    #[tokio::test]
    async fn test_cookie_user_agent_record() {
        let storage = test_storage("cookieua").await;
        storage.set_cookie("default", "https://a.com", "sid=1").await.unwrap();
        storage
            .set_cookie_user_agent("default", "https://a.com", "fs-ua/1.0")
            .await
            .unwrap();
        let (cookie, ua) = storage
            .get_source_session("default", "https://a.com")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cookie, "sid=1");
        assert_eq!(ua, "fs-ua/1.0");
        // set_cookie 覆盖不丢 UA
        storage.set_cookie("default", "https://a.com", "sid=2").await.unwrap();
        let (cookie, ua) = storage
            .get_source_session("default", "https://a.com")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cookie, "sid=2");
        assert_eq!(ua, "fs-ua/1.0");
        cleanup(storage, "cookieua").await;
    }

    #[tokio::test]
    async fn test_delete_book_source_cleans_cookie() {
        let storage = test_storage("cookiedel").await;
        storage
            .set_cookie("default", "https://a.com", "sid=1")
            .await
            .unwrap();
        let s = source("https://a.com", "A源", None);
        storage.save_book_source("default", &s).await.unwrap();

        assert_eq!(storage.delete_book_source("default", "https://a.com").await.unwrap(), 1);
        assert_eq!(storage.get_cookie("default", "https://a.com").await.unwrap(), None);

        // delete_all 清理
        storage
            .set_cookie("default", "https://a.com", "sid=2")
            .await
            .unwrap();
        storage
            .set_cookie("default", "https://b.com", "sid=3")
            .await
            .unwrap();
        storage.save_book_source("default", &source("https://a.com", "A", None)).await.unwrap();
        storage.save_book_source("default", &source("https://b.com", "B", None)).await.unwrap();
        storage.delete_all_book_sources("default").await.unwrap();
        assert_eq!(storage.get_cookie("default", "https://a.com").await.unwrap(), None);
        assert_eq!(storage.get_cookie("default", "https://b.com").await.unwrap(), None);

        cleanup(storage, "cookiedel").await;
    }

    #[test]
    fn test_normalize_base() {
        assert_eq!(normalize_base("https://a.com").as_deref(), Some("https://a.com"));
        assert_eq!(normalize_base("https://a.com/").as_deref(), Some("https://a.com"));
        assert_eq!(normalize_base("https://a.com/book/1?x=2").as_deref(), Some("https://a.com"));
        assert_eq!(normalize_base("https://a.com:8443/x").as_deref(), Some("https://a.com:8443"));
        assert_eq!(normalize_base("http://a.com").as_deref(), Some("http://a.com"));
        assert_eq!(normalize_base("a.com").as_deref(), Some("https://a.com"));
        assert_eq!(normalize_base("").is_none(), true);
    }

    // ---------------- GAP 44：RSS 分组 ----------------

    /// RSS 源 group 往返：保存（含 sourceGroup）→ 列表/查找返回 group；覆盖保存更新 group；
    /// 删除后无残留
    #[tokio::test]
    async fn test_rss_source_group_roundtrip() {
        let storage = test_storage("rssgroup").await;
        let mut s = crate::model::RssSource {
            source_url: "https://r.com/feed.xml".into(),
            source_name: "科技资讯".into(),
            source_group: Some("科技".into()),
            enabled: true,
            ..Default::default()
        };
        storage.save_rss_source("default", &s).await.unwrap();
        let got = storage
            .get_rss_sources("default")
            .await
            .unwrap()
            .into_iter()
            .find(|x| x.source_url == "https://r.com/feed.xml")
            .expect("保存后应能查到");
        assert_eq!(got.source_group.as_deref(), Some("科技"), "group 应落库并读回");
        assert_eq!(got.source_name, "科技资讯");

        // 覆盖保存：改 group
        s.source_group = Some("新闻 综合".into());
        storage.save_rss_source("default", &s).await.unwrap();
        let got = storage
            .find_rss_source("default", "https://r.com/feed.xml")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.source_group.as_deref(), Some("新闻 综合"));

        // 删除
        storage.delete_rss_source("default", "https://r.com/feed.xml").await.unwrap();
        assert!(storage
            .get_rss_sources("default")
            .await
            .unwrap()
            .iter()
            .all(|x| x.source_url != "https://r.com/feed.xml"));

        cleanup(storage, "rssgroup").await;
    }

    // ---------------- GAP 117：删除书清理封面文件 ----------------

    /// 封面残留清理：删除唯一引用书的封面文件被移除；多书共享封面时保留；
    /// 无封面/路径不匹配不报错
    #[tokio::test]
    async fn test_delete_book_cleans_orphan_cover() {
        let storage = test_storage("coverclean").await;
        let cover_dir = storage.config.storage_dir().join("assets/default/covers");
        std::fs::create_dir_all(&cover_dir).unwrap();
        std::fs::write(cover_dir.join("a1.jpg"), b"cover-a").unwrap();
        std::fs::write(cover_dir.join("a2.jpg"), b"cover-b").unwrap();
        let cover_a = "/assets/default/covers/a1.jpg";
        let cover_b = "/assets/default/covers/a2.jpg";

        // 两本书共享 cover_a；一本书独享 cover_b
        for (url, cover) in [
            ("https://b1.com", cover_a),
            ("https://b2.com", cover_a),
            ("https://b3.com", cover_b),
        ] {
            storage
                .upsert_book("default", &crate::model::Book { book_url: url.into(), name: url.into(), cover_url: Some(cover.into()), ..Default::default() })
                .await
                .unwrap();
        }

        // 删 b1（cover_a 仍被 b2 引用 → 文件保留）
        storage.delete_book("default", "https://b1.com").await.unwrap();
        assert!(cover_dir.join("a1.jpg").exists(), "共享封面不应删除");
        // 删 b2 → cover_a 无引用 → 文件清理
        storage.delete_book("default", "https://b2.com").await.unwrap();
        assert!(!cover_dir.join("a1.jpg").exists(), "无引用封面应清理");
        // 删 b3 → cover_b 清理
        storage.delete_book("default", "https://b3.com").await.unwrap();
        assert!(!cover_dir.join("a2.jpg").exists());

        // 批量删除也清理
        std::fs::write(cover_dir.join("a3.jpg"), b"cover-c").unwrap();
        for url in ["https://c1.com", "https://c2.com"] {
            storage
                .upsert_book("default", &crate::model::Book { book_url: url.into(), name: url.into(), cover_url: Some("/assets/default/covers/a3.jpg".into()), ..Default::default() })
                .await
                .unwrap();
        }
        storage.delete_books("default", &["https://c1.com".into(), "https://c2.com".into()]).await.unwrap();
        assert!(!cover_dir.join("a3.jpg").exists(), "批量删除后封面应清理");

        // 封面路径穿越样式（.. / 反斜杠）→ 不删除任何文件
        storage
            .upsert_book("default", &crate::model::Book { book_url: "https://d1.com".into(), name: "d".into(), cover_url: Some("/assets/default/covers/../a1.jpg".into()), ..Default::default() })
            .await
            .unwrap();
        storage.delete_book("default", "https://d1.com").await.unwrap();

        cleanup(storage, "coverclean").await;
    }
}
