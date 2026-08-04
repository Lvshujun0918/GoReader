//! 启动时数据库备份（GAP 148：迁移/升级前自动快照）
//!
//! - 时机：storage 初始化（打开连接/建表/迁移）之前，main.rs 调用；
//! - 行为：`storage/reader.db` → `storage/reader.db.bak-{YYYYMMDDHHMMSS}`；
//! - 保留：最近 `KEEP_BACKUPS`（5）份，更旧的删除；
//! - 禁用：env `READER_DB_BACKUP=0`；
//! - WAL 一致性：若存在 `reader.db-wal`（上次未正常 checkpoint），先用临时连接执行
//!   `PRAGMA wal_checkpoint(TRUNCATE)` 再复制，保证备份为一致快照（尽力而为，失败仅告警）。

use std::path::{Path, PathBuf};

use anyhow::Result;
use sqlx::ConnectOptions as _; // SqliteConnectOptions::connect

/// 保留的备份份数（超出删最旧）
pub const KEEP_BACKUPS: usize = 5;

/// 执行启动备份。返回生成的备份路径（禁用/无库文件 → None）
pub async fn backup_reader_db(storage_dir: &Path) -> Result<Option<PathBuf>> {
    // env READER_DB_BACKUP=0 禁用
    if std::env::var("READER_DB_BACKUP")
        .map(|v| v.trim() == "0")
        .unwrap_or(false)
    {
        tracing::info!("READER_DB_BACKUP=0——跳过启动数据库备份");
        return Ok(None);
    }
    let db_path = storage_dir.join("reader.db");
    if !db_path.exists() {
        return Ok(None); // 首次启动无库，无需备份
    }

    // WAL 残留时先 checkpoint（保证副本一致；失败不阻塞备份流程）
    if storage_dir.join("reader.db-wal").exists() {
        let opts = sqlx::sqlite::SqliteConnectOptions::new().filename(&db_path);
        match opts.connect().await {
            Ok(mut conn) => {
                if let Err(e) = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
                    .execute(&mut conn)
                    .await
                {
                    tracing::warn!("备份前 WAL checkpoint 失败（副本可能缺失未落盘事务）: {e}");
                }
                // drop(conn)：关闭连接（sqlx 0.7 无显式 close）
            }
            Err(e) => {
                tracing::warn!("备份前打开数据库失败（跳过 checkpoint）: {e}");
            }
        }
    }

    let ts = chrono::Local::now().format("%Y%m%d%H%M%S%3f");
    let backup_path = storage_dir.join(format!("reader.db.bak-{ts}"));
    std::fs::copy(&db_path, &backup_path)?;

    let pruned = prune_backups(storage_dir, KEEP_BACKUPS);
    tracing::info!(
        "数据库已备份 → {}（保留最近 {KEEP_BACKUPS} 份，清理 {pruned} 份旧备份）",
        backup_path.display()
    );
    Ok(Some(backup_path))
}

/// 清理旧备份：仅保留最近 `keep` 份 `reader.db.bak-*`（文件名时间戳定长，字典序 = 时间序）。
/// 返回删除数量。
pub fn prune_backups(storage_dir: &Path, keep: usize) -> usize {
    let Ok(entries) = std::fs::read_dir(storage_dir) else {
        return 0;
    };
    let mut backups: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().starts_with("reader.db.bak-"))
                .unwrap_or(false)
        })
        .collect();
    backups.sort();
    let overflow = backups.len().saturating_sub(keep.max(1));
    let mut deleted = 0;
    for old in backups.into_iter().take(overflow) {
        if std::fs::remove_file(&old).is_ok() {
            deleted += 1;
        }
    }
    deleted
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造临时 storage 目录（带测试数据库文件）——每测试独立子目录（并行测试不串扰）
    fn temp_storage(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "reader-db-backup-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn test_backup_creates_copy_and_prunes() {
        let dir = temp_storage("main");
        std::fs::write(dir.join("reader.db"), b"db-bytes").unwrap();
        // 预置 3 份旧备份
        for ts in ["20240101000000", "20240102000000", "20240103000000"] {
            std::fs::write(dir.join(format!("reader.db.bak-{ts}")), b"old").unwrap();
        }
        let backup = backup_reader_db(&dir).await.expect("备份应成功").expect("应生成备份");
        let name = backup.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("reader.db.bak-"), "命名: {name}");
        assert_eq!(std::fs::read(&backup).unwrap(), b"db-bytes", "内容一致");
        // 保留最近 5 份：3 旧 + 1 新 = 4 份，无删除
        let count = std::fs::read_dir(&dir)
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with("reader.db.bak-"))
            .count();
        assert_eq!(count, 4);
        // 再备份 2 次 → 共 6 份 → 删最旧 1 份，剩 5（毫秒级时间戳避免同秒覆盖）
        for _ in 0..2 {
            std::fs::write(dir.join("reader.db"), b"db-bytes").unwrap();
            backup_reader_db(&dir).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("reader.db.bak-"))
            .collect();
        names.sort();
        assert_eq!(names.len(), KEEP_BACKUPS, "保留最近 5 份: {names:?}");
        assert!(
            !names.iter().any(|n| n.contains("20240101000000")),
            "最旧备份应被清理: {names:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_backup_disabled_and_missing_db() {
        let dir = temp_storage("disabled");
        // 无库文件 → None
        assert!(backup_reader_db(&dir).await.unwrap().is_none());
        // READER_DB_BACKUP=0 → None
        std::fs::write(dir.join("reader.db"), b"x").unwrap();
        std::env::set_var("READER_DB_BACKUP", "0");
        assert!(backup_reader_db(&dir).await.unwrap().is_none());
        std::env::remove_var("READER_DB_BACKUP");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_prune_backups_keep_count() {
        let dir = temp_storage("prune");
        for ts in ["20240101000000", "20240102000000", "20240103000000", "20240104000000"] {
            std::fs::write(dir.join(format!("reader.db.bak-{ts}")), b"old").unwrap();
        }
        // 无关文件不受影响
        std::fs::write(dir.join("reader.db"), b"db").unwrap();
        assert_eq!(prune_backups(&dir, 2), 2, "删 2 留 2");
        let remaining: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("reader.db.bak-"))
            .collect();
        assert_eq!(remaining.len(), 2);
        assert!(remaining.iter().any(|n| n.contains("20240104000000")));
        assert!(std::fs::read_dir(&dir).unwrap().count() >= 3, "reader.db 不受影响");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
