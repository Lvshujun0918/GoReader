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
            local_epub TEXT,
            local_pdf TEXT,
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

    // 幂等补列（兼容旧库：缺列则 ALTER TABLE 补上）
    let columns = [
        ("users", &["token_map", "raw_json"][..]),
        (
            "books",
            &[
                "toc_url", "custom_tag", "custom_intro", "latest_chapter_title", "latest_chapter_time",
                "last_check_time", "last_check_count", "total_chapter_num", "word_count",
                "order_num", "origin_order", "use_replace_rule", "variable", "read_config",
                "is_in_shelf", "cbz", "display_cover", "display_intro", "local_epub", "local_pdf", "pdf", "split_long_chapter", "info_html", "toc_html", "raw_json",
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
}
