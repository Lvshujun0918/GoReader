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

    // 幂等补列（兼容旧库：缺列则 ALTER TABLE 补上）
    let columns = [
        ("users", &["token_map", "raw_json"][..]),
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

    tracing::info!("storage initialized at {}", db_path.display());

    // JSON → SQLite 迁移（幂等：users 表非空跳过）
    let storage = Storage {
        pool,
        config: config.clone(),
    };
    if let Err(e) = crate::storage::migrate::migrate_if_needed(&storage).await {
        tracing::error!("JSON→SQLite 迁移失败（服务继续启动，数据仍保留在 JSON）：{e}");
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
    pub async fn delete_book_source(&self, ns: &str, url: &str) -> Result<u64> {
        let r = sqlx::query(
            "DELETE FROM book_sources WHERE user_namespace = ?1 AND book_source_url = ?2",
        )
        .bind(ns)
        .bind(url)
        .execute(&self.pool)
        .await?;
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

    /// 清空命名空间全部书源
    pub async fn delete_all_book_sources(&self, ns: &str) -> Result<u64> {
        let r = sqlx::query("DELETE FROM book_sources WHERE user_namespace = ?1")
            .bind(ns)
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
    pub async fn delete_book(&self, ns: &str, book_url: &str) -> Result<u64> {
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
        Ok(r.rows_affected())
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
             user_namespace, created_at, raw_json)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                    ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27,
                    ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37, ?38, ?39, ?40,
                    ?41, ?42, ?43, ?44, ?45, ?46, ?47)"#,
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
}

/// 幂等补列：列不存在则 ALTER TABLE ADD COLUMN（旧库升级用）
async fn ensure_column(pool: &SqlitePool, table: &str, column: &str) -> anyhow::Result<()> {
    let row: (i64,) = sqlx::query_as(&format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = '{column}'"))
        .fetch_one(pool)
        .await?;
    if row.0 == 0 {
        let sql = format!("ALTER TABLE {table} ADD COLUMN {column} TEXT");
        sqlx::query(&sql).execute(pool).await?;
        tracing::info!("ALTER TABLE {table} ADD COLUMN {column}");
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
             rule_toc, rule_content, search_rule, explore_rule, book_info_rule, toc_rule,
             content_rule, key, tag, logger, variable, user_namespace, raw_json)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29,
                ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37, ?38)
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
}
