//! 定时任务（GAP #57 自动备份 / GAP #101 订阅源与 RSS 自动刷新）
//!
//! - `spawn_schedule_jobs`：书架更新循环（每 10 分钟）顺带刷新订阅源（每 6 小时）
//!   与 RSS 源（每 30 分钟，节流）
//! - `spawn_auto_backup_job`：每天 READER_AUTO_BACKUP_HOUR（默认 03:00）自动备份
//!   各命名空间到 webdav/legado/auto-YYYYMMDD.zip，保留最近 7 份
//! - `refresh_source_sub_core`：订阅抓取核心（saveSourceSub/refreshSourceSub 与定时任务共用）

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{anyhow, Result};
use chrono::Timelike;

use crate::storage::Storage;

/// 订阅源刷新周期（6 小时）
const SUB_REFRESH_INTERVAL: Duration = Duration::from_secs(6 * 3600);
/// RSS 源刷新周期（30 分钟）
const RSS_REFRESH_INTERVAL: Duration = Duration::from_secs(30 * 60);
/// 自动备份保留份数（最近 7 份）
const AUTO_BACKUP_KEEP: usize = 7;
/// 订阅抓取超时（秒）
const SUB_FETCH_TIMEOUT_SECS: u64 = 15;

/// 启动定时任务（书架更新 + 订阅/RSS 自动刷新；在 lib.rs serve 时调用一次）
pub fn spawn_schedule_jobs(storage: Storage) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10 * 60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_sub = std::time::Instant::now();
        let mut last_rss = std::time::Instant::now();
        loop {
            interval.tick().await;
            // ① 书架更新（原有 F-35：can_update=1 的书回写最新章节/总数）
            match crate::storage::run_shelf_update(&storage).await {
                Ok(n) => tracing::info!("书架更新检查完成：更新 {n} 本"),
                Err(e) => tracing::warn!("书架更新检查失败: {e:#}"),
            }
            // ② 订阅源自动刷新（每 6 小时，节流）
            if last_sub.elapsed() >= SUB_REFRESH_INTERVAL {
                match run_source_sub_refresh(&storage).await {
                    Ok(n) => tracing::info!("订阅源自动刷新完成：{n} 个订阅"),
                    Err(e) => tracing::warn!("订阅源自动刷新失败: {e:#}"),
                }
                last_sub = std::time::Instant::now();
            }
            // ③ RSS 源自动刷新（每 30 分钟，节流）
            if last_rss.elapsed() >= RSS_REFRESH_INTERVAL {
                match run_rss_refresh(&storage).await {
                    Ok(n) => tracing::info!("RSS 源自动刷新完成：{n} 个源"),
                    Err(e) => tracing::warn!("RSS 源自动刷新失败: {e:#}"),
                }
                last_rss = std::time::Instant::now();
            }
        }
    });
}

/// 启动自动备份定时任务（GAP #57）
///
/// 每天 READER_AUTO_BACKUP_HOUR（0-23，默认 3 = 03:00）触发一次（同日不重复）；
/// 环境变量为空 → 默认 3；非法值 → 告警并回退默认。
pub fn spawn_auto_backup_job(storage: Storage) {
    let raw = std::env::var("READER_AUTO_BACKUP_HOUR").unwrap_or_default();
    let hour: i64 = if raw.trim().is_empty() {
        3
    } else {
        match raw.trim().parse::<i64>() {
            Ok(h) if (0..=23).contains(&h) => h,
            _ => {
                tracing::warn!("READER_AUTO_BACKUP_HOUR 非法（{raw}），使用默认 3");
                3
            }
        }
    };
    tracing::info!("自动备份定时任务：每天 {hour:02}:00 执行");
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10 * 60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_date: Option<chrono::NaiveDate> = None;
        loop {
            interval.tick().await;
            let now = chrono::Local::now();
            if now.hour() as i64 != hour {
                continue;
            }
            if last_date == Some(now.date_naive()) {
                continue; // 当天已备份
            }
            match run_auto_backup(&storage).await {
                Ok(n) => {
                    tracing::info!("自动备份完成：{} 个命名空间", n);
                    last_date = Some(now.date_naive());
                }
                Err(e) => tracing::warn!("自动备份失败: {e:#}"),
            }
        }
    });
}

/// 自动备份（GAP #57）：各命名空间 → webdav/legado/auto-YYYYMMDD.zip（同日跳过）→
/// 保留最近 7 份（删除更旧的 auto-*.zip）。返回执行备份的命名空间数。
pub async fn run_auto_backup(storage: &Storage) -> Result<usize> {
    let nss = storage.schedule_namespaces().await;
    let today = chrono::Local::now().format("%Y%m%d").to_string();
    let mut done = 0usize;
    for ns in nss {
        let legado = storage
            .config
            .storage_dir()
            .join("data")
            .join(&ns)
            .join("webdav")
            .join("legado");
        // 同日已备份 → 跳过（重启/多轮 tick 幂等）
        if legado.join(format!("auto-{today}.zip")).exists() {
            continue;
        }
        match storage
            .write_backup_zip(&ns, &format!("auto-{today}"))
            .await
        {
            Ok(path) => {
                tracing::info!("自动备份 [{ns}]: {}", path);
                done += 1;
            }
            Err(e) => tracing::warn!("自动备份 [{ns}] 失败: {e:#}"),
        }
        // 保留最近 7 份（单命名空间失败不影响清理）
        storage.prune_auto_backups(&ns, AUTO_BACKUP_KEEP);
    }
    Ok(done)
}

/// 订阅源定时刷新（GAP #101）：各命名空间启用的订阅重新拉取并覆盖书源。
/// 返回刷新成功的订阅数（单订阅失败仅告警，不影响其余）。
pub async fn run_source_sub_refresh(storage: &Storage) -> Result<usize> {
    let nss = storage.schedule_namespaces().await;
    let mut refreshed = 0usize;
    for ns in nss {
        let Ok(subs) = storage.get_source_subs(&ns).await else {
            continue;
        };
        for sub in subs {
            if !sub.enabled {
                continue;
            }
            match refresh_source_sub_core(storage, &ns, &sub.url, &sub.name).await {
                Ok(n) => {
                    tracing::info!("订阅自动刷新 [{ns}] {}：{n} 个书源", sub.name);
                    refreshed += 1;
                }
                Err(e) => tracing::warn!("订阅自动刷新跳过 [{ns}] {}: {e:#}", sub.name),
            }
        }
    }
    Ok(refreshed)
}

/// RSS 源定时刷新（GAP #101）：各命名空间启用的 RSS 源抓取文章入库（第 1 页）。
/// 返回刷新成功的源数（单源失败仅告警）。
pub async fn run_rss_refresh(storage: &Storage) -> Result<usize> {
    let nss = storage.schedule_namespaces().await;
    let mut refreshed = 0usize;
    for ns in nss {
        let Ok(sources) = storage.get_rss_sources(&ns).await else {
            continue;
        };
        for source in sources {
            if !source.enabled {
                continue;
            }
            match crate::service::rss::fetch_articles(&source, 1).await {
                Ok(articles) => {
                    if let Err(e) = storage.save_rss_articles(&ns, &articles).await {
                        tracing::warn!("RSS 自动刷新入库失败 [{ns}] {}: {e:#}", source.source_name);
                    } else {
                        refreshed += 1;
                    }
                }
                Err(e) => tracing::warn!("RSS 自动刷新跳过 [{ns}] {}: {e:#}", source.source_name),
            }
        }
    }
    Ok(refreshed)
}

/// 订阅抓取核心（saveSourceSub / refreshSourceSub / 定时刷新共用）
///
/// 抓取订阅 URL → 校验书源数组 → 订阅入库（raw_json 存原文）→ 批量导入书源
/// （已存在覆盖；书源数上限整批拒绝）。错误信息即对外文案：
/// "远程书源链接错误" / "书源数据格式错误" / "超过书源数上限" / "保存失败"。
pub async fn refresh_source_sub_core(
    storage: &Storage,
    ns: &str,
    url: &str,
    name: &str,
) -> Result<usize> {
    let headers_map: HashMap<String, String> = HashMap::new();
    let resp = crate::service::crawler::fetch(
        url,
        &headers_map,
        SUB_FETCH_TIMEOUT_SECS,
        "GET",
        None,
        None,
    )
    .await
    .map_err(|_| anyhow!("远程书源链接错误"))?;
    // 校验：必须是书源数组（每项含非空 bookSourceUrl）
    let json: serde_json::Value =
        serde_json::from_str(&resp.body).map_err(|_| anyhow!("书源数据格式错误"))?;
    let sources: Vec<crate::model::BookSource> =
        serde_json::from_value(json).map_err(|_| anyhow!("书源数据格式错误"))?;
    if sources.is_empty() || sources.iter().any(|s| s.book_source_url.trim().is_empty()) {
        return Err(anyhow!("书源数据格式错误"));
    }
    // F-7 书源数上限（已存在覆盖不计名额，超限整批拒绝）
    if let Some(limit) = storage.book_source_limit_for(ns).await.ok().flatten() {
        if limit > 0 {
            let mut new_count = 0i64;
            for s in &sources {
                let exists = storage
                    .find_book_source(ns, &s.book_source_url)
                    .await
                    .ok()
                    .flatten()
                    .is_some();
                if !exists {
                    new_count += 1;
                }
            }
            match storage.count_book_sources(ns).await {
                Ok(count) if count + new_count > limit => {
                    return Err(anyhow!("超过书源数上限"));
                }
                Ok(_) => {}
                Err(_) => return Err(anyhow!("系统错误")),
            }
        }
    }
    storage
        .save_source_sub(ns, url, name, &resp.body)
        .await
        .map_err(|_| anyhow!("保存失败"))?;
    storage
        .save_book_sources(ns, &sources)
        .await
        .map_err(|_| anyhow!("保存失败"))?;
    Ok(sources.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RssSource;
    use crate::storage::Storage;

    async fn test_storage(tag: &str) -> (Storage, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("reader-schedule-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut config = crate::AppConfig::from_env();
        config.work_dir = dir.to_string_lossy().into_owned();
        let storage = crate::storage::init(&config).await.unwrap();
        (storage, dir)
    }

    async fn cleanup(storage: Storage, dir: std::path::PathBuf) {
        storage.pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 微型 HTTP 服务器：按请求路径应答（routes: path → body）；返回 base URL
    async fn mock_server(routes: Vec<(&'static str, &'static str)>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let routes: HashMap<String, String> = routes
                .into_iter()
                .map(|(p, b)| (p.to_string(), b.to_string()))
                .collect();
            for _ in 0..10 {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                let body = routes.get(&path).cloned().unwrap_or_default();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        format!("http://{addr}")
    }

    const SAMPLE_RSS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel><title>频道</title><link>https://e.com</link>
<item><title>文章1</title><link>https://e.com/1</link><guid>https://e.com/1</guid></item>
</channel></rss>"#;

    /// GAP #101：订阅源定时刷新——重新拉取订阅并导入/覆盖书源
    #[tokio::test]
    async fn test_run_source_sub_refresh() {
        let (storage, dir) = test_storage("subrefresh").await;
        let base = mock_server(vec![(
            "/sub",
            r#"[{"bookSourceUrl":"https://x.com","bookSourceName":"X源"}]"#,
        )])
        .await;
        let sub_url = format!("{base}/sub");
        storage
            .save_source_sub("default", &sub_url, "订阅", "[]")
            .await
            .unwrap();
        // 禁用的订阅不刷新
        storage
            .save_source_sub("default", &format!("{base}/off"), "停用", "[]")
            .await
            .unwrap();
        sqlx::query("UPDATE source_subs SET enabled = 0 WHERE url = ?1")
            .bind(format!("{base}/off"))
            .execute(&storage.pool)
            .await
            .unwrap();

        assert_eq!(run_source_sub_refresh(&storage).await.unwrap(), 1);
        let sources = storage.get_book_sources("default").await.unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].book_source_url, "https://x.com");
        assert_eq!(sources[0].book_source_name, "X源");
        // 订阅 raw_json 已覆盖
        let sub = storage
            .find_source_sub("default", &sub_url)
            .await
            .unwrap()
            .unwrap();
        assert!(sub
            .raw_json
            .as_deref()
            .unwrap_or("")
            .contains("bookSourceUrl"));
        cleanup(storage, dir).await;
    }

    /// GAP #101：订阅抓取核心校验（非书源数组 → 拒绝）
    #[tokio::test]
    async fn test_refresh_source_sub_core_rejects_bad_data() {
        // P1 SSRF：订阅抓取走 crawler::fetch（入口公网校验）——mock 绑定 127.0.0.1，
        // 持放行守卫（仅测试代码可设置）
        let _ssrf = crate::service::crawler::ssrf_allow_private_guard(true);
        let (storage, dir) = test_storage("subbad").await;
        let base = mock_server(vec![("/bad", "<html>不是json</html>")]).await;
        let err = refresh_source_sub_core(&storage, "default", &format!("{base}/bad"), "订阅")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("书源数据格式错误"), "{err}");
        // 连接失败 → 远程书源链接错误
        let err = refresh_source_sub_core(&storage, "default", "http://127.0.0.1:1/x", "订阅")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("远程书源链接错误"), "{err}");
        cleanup(storage, dir).await;
    }

    /// GAP #101：RSS 源定时刷新——抓取文章入库
    #[tokio::test]
    async fn test_run_rss_refresh() {
        // P1 SSRF：RSS 抓取走 crawler::fetch——mock 绑定 127.0.0.1，持放行守卫
        let _ssrf = crate::service::crawler::ssrf_allow_private_guard(true);
        let (storage, dir) = test_storage("rssrefresh").await;
        let base = mock_server(vec![("/feed.xml", SAMPLE_RSS)]).await;
        storage
            .save_rss_source(
                "default",
                &RssSource {
                    source_url: format!("{base}/feed.xml"),
                    source_name: "RSS".into(),
                    enabled: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        // 禁用的源不刷新
        storage
            .save_rss_source(
                "default",
                &RssSource {
                    source_url: format!("{base}/off.xml"),
                    source_name: "停用".into(),
                    enabled: false,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(run_rss_refresh(&storage).await.unwrap(), 1);
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rss_articles")
            .fetch_one(&storage.pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
        let art = storage
            .get_rss_article("default", "https://e.com/1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(art.title, "文章1");
        cleanup(storage, dir).await;
    }

    /// GAP #57：命名空间集合（非 secure → default）
    #[tokio::test]
    async fn test_schedule_namespaces_non_secure() {
        let (mut storage, dir) = test_storage("ns").await;
        assert_eq!(storage.schedule_namespaces().await, vec!["default"]);
        // secure：用户列表 + 残留 default 目录
        storage
            .insert_user(&crate::model::User {
                username: "alice".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        storage.config.secure = true;
        let nss = storage.schedule_namespaces().await;
        assert!(nss.contains(&"alice".to_string()));
        cleanup(storage, dir).await;
    }
}
