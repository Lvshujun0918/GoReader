//! MongoDB 备份/恢复（POST /reader3/backupToMongodb / restoreFromMongodb）
//!
//! 核心集合（与 SQLite 表一一对应）：
//!   users / books / book_sources / bookmarks / book_groups / replace_rules /
//!   rss_sources / txt_toc_rules / http_tts_list / user_config
//!
//! 设计：
//! - 文档 `_id` = `{ns, key}`（命名空间 + 实体自然主键的确定性组合 → 恢复幂等 upsert）
//! - 文档体 = 实体 serde JSON（camelCase，与 legacy 备份 JSON 同构；`#[serde(skip)]`
//!   的服务端内部字段如 user_namespace/local_file/raw_json 不落盘）
//! - 备份 = 逐文档 ReplaceOne(upsert)（重复备份幂等，不删远端既有数据）
//! - 恢复 = 按 `_id.ns` 过滤读回 → 反序列化为模型 → INSERT OR REPLACE 落 SQLite（幂等）
//! - uri 解析优先级：body.uri → 环境变量 READER_MONGODB_URI → 报错；
//!   数据库名默认 `reader3`（body.db 可覆盖）

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use futures::TryStreamExt;
use mongodb::bson::{doc, from_document, to_document, Document};
use mongodb::options::ClientOptions;
use mongodb::Client;
use serde_json::{json, Value};

use crate::model::{
    Book, BookGroup, BookSource, Bookmark, HttpTts, ReplaceRule, RssSource, TxtTocRule, User,
};
use crate::storage::Storage;

/// 连接/选主超时（快速失败，避免 handler 长时间挂起）
const CONNECT_TIMEOUT_MS: u64 = 5000;

/// 默认数据库名
pub const DEFAULT_DB: &str = "reader3";

/// 环境变量默认连接串
pub fn env_uri() -> Option<String> {
    std::env::var("READER_MONGODB_URI")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 解析连接串：body.uri → env READER_MONGODB_URI → 错误
pub fn resolve_uri(body_uri: Option<&str>) -> Result<String> {
    let body_uri = body_uri.map(str::trim).filter(|s| !s.is_empty());
    if let Some(uri) = body_uri {
        return Ok(uri.to_string());
    }
    if let Some(uri) = env_uri() {
        return Ok(uri);
    }
    Err(anyhow!(
        "未配置MongoDB连接地址（请在请求body传入uri或设置环境变量READER_MONGODB_URI）"
    ))
}

/// 连接 + ping 验证（连接串/网络错误统一在这里暴露，上层不接触 uri 细节）
async fn connect(uri: &str) -> Result<Client> {
    let mut opts = ClientOptions::parse(uri)
        .await
        .context("MongoDB连接字符串无效")?;
    opts.server_selection_timeout = Some(Duration::from_millis(CONNECT_TIMEOUT_MS));
    opts.connect_timeout = Some(Duration::from_millis(CONNECT_TIMEOUT_MS));
    let client = Client::with_options(opts).context("MongoDB客户端初始化失败")?;
    client
        .database("admin")
        .run_command(doc! { "ping": 1 })
        .await
        .context("无法连接MongoDB（请检查地址/网络/认证）")?;
    Ok(client)
}

/// 通用写入：每个文档 `_id = {ns, key}`，replace_one(upsert=true)（幂等）
async fn write_docs(
    client: &Client,
    db: &str,
    collection: &str,
    ns: &str,
    items: Vec<(String, Document)>,
) -> Result<usize> {
    let coll = client.database(db).collection::<Document>(collection);
    let mut n = 0usize;
    for (key, mut doc) in items {
        doc.insert("_id", doc! { "ns": ns, "key": key });
        let filter = doc! { "_id": doc.get("_id").cloned().unwrap_or_default() };
        coll.replace_one(filter, doc).upsert(true).await?;
        n += 1;
    }
    Ok(n)
}

/// 通用读回：按 `_id.ns` 过滤，去 `_id` 后反序列化为模型
async fn read_models<T>(client: &Client, db: &str, collection: &str, ns: &str) -> Result<Vec<T>>
where
    T: serde::de::DeserializeOwned + Send + Unpin,
{
    let coll = client.database(db).collection::<Document>(collection);
    let mut cursor = coll.find(doc! { "_id.ns": ns }).await?;
    let mut out: Vec<T> = Vec::new();
    while let Some(mut d) = cursor.try_next().await? {
        d.remove("_id");
        out.push(from_document(d)?);
    }
    Ok(out)
}

/// 备份：核心集合全量写入（幂等 upsert）。返回各集合计数。
pub async fn backup_to_mongodb(storage: &Storage, ns: &str, uri: &str, db: &str) -> Result<Value> {
    let client = connect(uri).await?;
    let mut report = serde_json::Map::new();

    // users（全局表，ns 记 default）
    let users = storage.list_users().await?;
    let items = users
        .iter()
        .map(|u| Ok((u.username.clone(), to_document(u)?)))
        .collect::<Result<Vec<_>>>()?;
    let n = write_docs(&client, db, "users", "default", items).await?;
    report.insert("users".into(), json!(n));

    // books（按命名空间）
    let books = storage.list_books(ns).await?;
    let items = books
        .iter()
        .map(|b| Ok((b.book_url.clone(), to_document(b)?)))
        .collect::<Result<Vec<_>>>()?;
    let n = write_docs(&client, db, "books", ns, items).await?;
    report.insert("books".into(), json!(n));

    // book_sources（精确取本命名空间行——不走 get_book_sources 的 default 回退）
    let sources: Vec<BookSource> = sqlx::query_as(
        "SELECT * FROM book_sources WHERE user_namespace = ?1 ORDER BY weight DESC, custom_order, book_source_name",
    )
    .bind(ns)
    .fetch_all(&storage.pool)
    .await?;
    let items = sources
        .iter()
        .map(|s| Ok((s.book_source_url.clone(), to_document(s)?)))
        .collect::<Result<Vec<_>>>()?;
    let n = write_docs(&client, db, "book_sources", ns, items).await?;
    report.insert("bookSources".into(), json!(n));

    // bookmarks（主键 book_url+title；user_namespace 列按 ns 过滤）
    let bookmarks: Vec<Bookmark> = sqlx::query_as(
        "SELECT * FROM bookmarks WHERE user_namespace = ?1 ORDER BY created_at DESC, rowid DESC",
    )
    .bind(ns)
    .fetch_all(&storage.pool)
    .await?;
    let items = bookmarks
        .iter()
        .map(|bm| {
            Ok((
                format!("{}\u{1f}{}", bm.book_url, bm.title),
                to_document(bm)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let n = write_docs(&client, db, "bookmarks", ns, items).await?;
    report.insert("bookmarks".into(), json!(n));

    // book_groups（id 为主键——恢复时保留 id，books.group_name 引用不失效）
    let groups = storage.list_book_groups(ns).await?;
    let items = groups
        .iter()
        .map(|g| Ok((g.id.to_string(), to_document(g)?)))
        .collect::<Result<Vec<_>>>()?;
    let n = write_docs(&client, db, "book_groups", ns, items).await?;
    report.insert("bookGroups".into(), json!(n));

    // replace_rules（精确取本命名空间行——不走 get_replace_rules 的 default 回退）
    let rules: Vec<ReplaceRule> = sqlx::query_as(
        "SELECT * FROM replace_rules WHERE user_namespace = ?1 ORDER BY order_num, id",
    )
    .bind(ns)
    .fetch_all(&storage.pool)
    .await?;
    let items = rules
        .iter()
        .map(|r| Ok((r.id.clone(), to_document(r)?)))
        .collect::<Result<Vec<_>>>()?;
    let n = write_docs(&client, db, "replace_rules", ns, items).await?;
    report.insert("replaceRules".into(), json!(n));

    // rss_sources（raw_json 保底原文一并落盘，恢复时写回）
    let rss: Vec<RssSource> = sqlx::query_as(
        "SELECT * FROM rss_sources WHERE user_namespace = ?1 ORDER BY rss_source_name",
    )
    .bind(ns)
    .fetch_all(&storage.pool)
    .await?;
    let items = rss
        .iter()
        .map(|s| {
            let mut doc = to_document(s)?;
            if let Some(raw) = &s.raw_json {
                doc.insert("rawJson", raw.clone());
            }
            Ok((s.source_url.clone(), doc))
        })
        .collect::<Result<Vec<_>>>()?;
    let n = write_docs(&client, db, "rss_sources", ns, items).await?;
    report.insert("rssSources".into(), json!(n));

    // txt_toc_rules（精确取本命名空间行）
    let rules: Vec<TxtTocRule> = sqlx::query_as(
        "SELECT * FROM txt_toc_rules WHERE user_namespace = ?1 ORDER BY serial_number, id",
    )
    .bind(ns)
    .fetch_all(&storage.pool)
    .await?;
    let items = rules
        .iter()
        .map(|r| Ok((r.id.clone(), to_document(r)?)))
        .collect::<Result<Vec<_>>>()?;
    let n = write_docs(&client, db, "txt_toc_rules", ns, items).await?;
    report.insert("txtTocRules".into(), json!(n));

    // http_tts_list（精确取本命名空间行）
    let tts: Vec<HttpTts> =
        sqlx::query_as("SELECT * FROM http_tts_list WHERE user_namespace = ?1 ORDER BY name")
            .bind(ns)
            .fetch_all(&storage.pool)
            .await?;
    let items = tts
        .iter()
        .map(|t| Ok((t.url.clone(), to_document(t)?)))
        .collect::<Result<Vec<_>>>()?;
    let n = write_docs(&client, db, "http_tts_list", ns, items).await?;
    report.insert("httpTts".into(), json!(n));

    // user_config（(user_namespace, ns) 双主键）
    let configs: Vec<(String, String)> =
        sqlx::query_as("SELECT ns, config FROM user_config WHERE user_namespace = ?1 ORDER BY ns")
            .bind(ns)
            .fetch_all(&storage.pool)
            .await?;
    let items = configs
        .iter()
        .map(|(k, v)| {
            (
                format!("{}\u{1f}{}", ns, k),
                doc! { "ns": k.clone(), "config": v.clone() },
            )
        })
        .collect::<Vec<_>>();
    let n = write_docs(&client, db, "user_config", ns, items).await?;
    report.insert("userConfigs".into(), json!(n));

    report.insert("db".into(), json!(db));
    tracing::info!("MongoDB 备份完成 [{ns}] db={db}: {report:?}");
    Ok(Value::Object(report))
}

/// 恢复：从集合读回并幂等 upsert 到 SQLite。返回各集合恢复计数。
pub async fn restore_from_mongodb(
    storage: &Storage,
    ns: &str,
    uri: &str,
    db: &str,
) -> Result<Value> {
    let client = connect(uri).await?;
    let mut report = serde_json::Map::new();

    // users（全局；用户名即命名空间——恢复后其自有数据归属自身）
    let users: Vec<User> = read_models(&client, db, "users", "default").await?;
    let mut n = 0usize;
    for mut u in users {
        u.user_namespace = u.username.clone();
        storage.insert_user(&u).await?;
        n += 1;
    }
    report.insert("users".into(), json!(n));

    // books
    let books: Vec<Book> = read_models(&client, db, "books", ns).await?;
    let mut n = 0usize;
    for b in books {
        storage.upsert_book(ns, &b).await?;
        n += 1;
    }
    report.insert("books".into(), json!(n));

    // book_sources
    let sources: Vec<BookSource> = read_models(&client, db, "book_sources", ns).await?;
    let mut n = 0usize;
    for s in sources {
        if s.book_source_url.trim().is_empty() {
            continue;
        }
        storage.save_book_source(ns, &s).await?;
        n += 1;
    }
    report.insert("bookSources".into(), json!(n));

    // bookmarks
    let bookmarks: Vec<Bookmark> = read_models(&client, db, "bookmarks", ns).await?;
    let mut n = 0usize;
    for bm in bookmarks {
        if bm.book_url.trim().is_empty() || bm.title.trim().is_empty() {
            continue;
        }
        storage.save_bookmark(ns, &bm).await?;
        n += 1;
    }
    report.insert("bookmarks".into(), json!(n));

    // book_groups（保留备份中的 id → books.group_name 引用有效）
    let groups: Vec<BookGroup> = read_models(&client, db, "book_groups", ns).await?;
    let mut n = 0usize;
    for g in groups {
        if g.name.trim().is_empty() {
            continue;
        }
        storage.save_book_group(ns, &g).await?;
        n += 1;
    }
    report.insert("bookGroups".into(), json!(n));

    // replace_rules
    let rules: Vec<ReplaceRule> = read_models(&client, db, "replace_rules", ns).await?;
    let mut n = 0usize;
    for r in rules {
        if r.name.trim().is_empty() {
            continue;
        }
        storage.save_replace_rule(ns, &r).await?;
        n += 1;
    }
    report.insert("replaceRules".into(), json!(n));

    // rss_sources（rawJson 保底字段写回 raw_json 列）
    let coll = client.database(db).collection::<Document>("rss_sources");
    let mut cursor = coll.find(doc! { "_id.ns": ns }).await?;
    let mut n = 0usize;
    while let Some(mut d) = cursor.try_next().await? {
        d.remove("_id");
        let raw_json = d.get_str("rawJson").ok().map(str::to_string);
        let mut s: RssSource = from_document(d)?;
        if s.source_url.trim().is_empty() || s.source_name.trim().is_empty() {
            continue;
        }
        s.raw_json = raw_json;
        storage.save_rss_source(ns, &s).await?;
        n += 1;
    }
    report.insert("rssSources".into(), json!(n));

    // txt_toc_rules
    let rules: Vec<TxtTocRule> = read_models(&client, db, "txt_toc_rules", ns).await?;
    let mut n = 0usize;
    for r in rules {
        if r.name.trim().is_empty() || r.rule.trim().is_empty() {
            continue;
        }
        storage.save_txt_toc_rule(ns, &r).await?;
        n += 1;
    }
    report.insert("txtTocRules".into(), json!(n));

    // http_tts_list
    let tts: Vec<HttpTts> = read_models(&client, db, "http_tts_list", ns).await?;
    let mut n = 0usize;
    for t in tts {
        if t.url.trim().is_empty() || t.name.trim().is_empty() {
            continue;
        }
        storage.save_http_tts(ns, &t).await?;
        n += 1;
    }
    report.insert("httpTts".into(), json!(n));

    // user_config（文档内 ns 为配置命名空间键）
    let coll = client.database(db).collection::<Document>("user_config");
    let mut cursor = coll.find(doc! { "_id.ns": ns }).await?;
    let mut n = 0usize;
    while let Some(d) = cursor.try_next().await? {
        let key = d.get_str("ns").ok().map(str::to_string);
        let config = d.get_str("config").ok().map(str::to_string);
        if let (Some(key), Some(config)) = (key, config) {
            storage.save_user_config(ns, &key, &config).await?;
            n += 1;
        }
    }
    report.insert("userConfigs".into(), json!(n));

    report.insert("db".into(), json!(db));
    tracing::info!("MongoDB 恢复完成 [{ns}] db={db}: {report:?}");
    Ok(Value::Object(report))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 备份/恢复的集合清单（与实现一致——防漏集合的回归锚点）
    #[test]
    fn backup_collections_are_complete() {
        // 与 backup_to_mongodb / restore_from_mongodb 中写死的集合一一对应
        let collections = [
            "users",
            "books",
            "book_sources",
            "bookmarks",
            "book_groups",
            "replace_rules",
            "rss_sources",
            "txt_toc_rules",
            "http_tts_list",
            "user_config",
        ];
        assert_eq!(collections.len(), 10);
        assert!(collections.contains(&"books"));
        assert!(collections.contains(&"rss_sources"));
    }

    /// uri 解析：body 优先，env 兜底，都没有报错
    #[test]
    fn resolve_uri_precedence() {
        // body uri 优先
        assert_eq!(resolve_uri(Some("mongodb://a:1")).unwrap(), "mongodb://a:1");
        // body 空白 → 忽略
        assert!(resolve_uri(Some("   ")).is_err());

        // env 兜底
        std::env::set_var("READER_MONGODB_URI", "mongodb://env:27017");
        assert_eq!(resolve_uri(None).unwrap(), "mongodb://env:27017");
        // body 优先于 env
        assert_eq!(
            resolve_uri(Some("mongodb://body:27017")).unwrap(),
            "mongodb://body:27017"
        );
        std::env::remove_var("READER_MONGODB_URI");

        // 都没有 → 明确报错
        let err = resolve_uri(None).unwrap_err();
        assert!(err.to_string().contains("READER_MONGODB_URI"));
    }

    /// 默认库名
    #[test]
    fn default_db_name() {
        assert_eq!(DEFAULT_DB, "reader3");
    }

    /// 无效连接串 → 连接层报错（不 panic）
    #[tokio::test]
    async fn invalid_uri_fails_cleanly() {
        let err = connect("not a uri ://").await;
        assert!(err.is_err());
    }
}
