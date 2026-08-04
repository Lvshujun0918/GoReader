//! 登录限流（GAP 61）：内存计数（用户名 + IP）
//!
//! - 失败 5 次 → 锁定 5 分钟（返回「尝试过多请稍后」）；
//! - 成功登录 → 计数清零；
//! - 纯内存（进程内有效；多实例部署需外部限流，见报告说明）。

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// 失败次数阈值（达到即锁定）
pub const MAX_LOGIN_FAILS: u32 = 5;
/// 锁定时间（5 分钟）
pub const LOCK_DURATION: Duration = Duration::from_secs(300);
/// 内存表上限（防滥用增长；超限时清理过期条目）
const MAX_ENTRIES: usize = 8192;

struct Entry {
    fails: u32,
    /// 最近一次失败时间（过期清理用）
    last_fail: Instant,
    /// 锁定截止（None = 未锁定）
    locked_until: Option<Instant>,
}

static FAILS: LazyLock<Mutex<HashMap<String, Entry>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock() -> std::sync::MutexGuard<'static, HashMap<String, Entry>> {
    FAILS.lock().unwrap_or_else(|e| e.into_inner())
}

/// 登录前检查：锁定中 → Err("尝试过多请稍后")
pub fn check_allowed(username: &str, ip: &str) -> Result<(), String> {
    let mut map = lock();
    prune(&mut map);
    let Some(entry) = map.get_mut(&key(username, ip)) else {
        return Ok(());
    };
    if let Some(until) = entry.locked_until {
        if Instant::now() < until {
            return Err("尝试过多请稍后".to_string());
        }
        // 锁定期已过：清除锁定（保留计数，重新累计）
        entry.locked_until = None;
    }
    Ok(())
}

/// 记录一次登录失败（达到阈值 → 锁定）
pub fn record_failure(username: &str, ip: &str) {
    let mut map = lock();
    prune(&mut map);
    let entry = map.entry(key(username, ip)).or_insert(Entry {
        fails: 0,
        last_fail: Instant::now(),
        locked_until: None,
    });
    entry.fails += 1;
    entry.last_fail = Instant::now();
    if entry.fails >= MAX_LOGIN_FAILS {
        entry.locked_until = Some(Instant::now() + LOCK_DURATION);
        tracing::warn!(
            "登录失败 {} 次（{username}@{ip}）——锁定 {} 秒",
            entry.fails,
            LOCK_DURATION.as_secs()
        );
    }
}

/// 登录成功：清除计数与锁定
pub fn reset(username: &str, ip: &str) {
    let mut map = lock();
    map.remove(&key(username, ip));
}

fn key(username: &str, ip: &str) -> String {
    format!("{username}|{ip}")
}

/// 过期清理：非锁定且距上次失败超过锁定窗口的条目移除；表超限时全量清理过期
fn prune(map: &mut HashMap<String, Entry>) {
    if map.len() < MAX_ENTRIES {
        map.retain(|_, e| {
            e.locked_until.map(|u| Instant::now() < u).unwrap_or(false)
                || e.last_fail.elapsed() < LOCK_DURATION
        });
    } else {
        map.retain(|_, e| e.locked_until.map(|u| Instant::now() < u).unwrap_or(false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lock_after_5_failures() {
        // 前 4 次失败：仍允许
        for _ in 0..4 {
            assert!(check_allowed("alice", "1.2.3.4").is_ok());
            record_failure("alice", "1.2.3.4");
        }
        // 第 5 次失败 → 锁定
        record_failure("alice", "1.2.3.4");
        let err = check_allowed("alice", "1.2.3.4").unwrap_err();
        assert_eq!(err, "尝试过多请稍后");
        // 其他用户/IP 不受影响
        assert!(check_allowed("alice", "5.6.7.8").is_ok());
        assert!(check_allowed("bob", "1.2.3.4").is_ok());
        // 锁定期内持续拒绝
        assert!(check_allowed("alice", "1.2.3.4").is_err());
    }

    #[test]
    fn test_success_resets_counter() {
        for _ in 0..4 {
            record_failure("carol", "9.9.9.9");
        }
        reset("carol", "9.9.9.9");
        assert!(check_allowed("carol", "9.9.9.9").is_ok());
        // 重置后重新累计：再失败 4 次仍不锁（需满 5 次）
        for _ in 0..4 {
            record_failure("carol", "9.9.9.9");
        }
        assert!(check_allowed("carol", "9.9.9.9").is_ok());
        record_failure("carol", "9.9.9.9");
        assert!(check_allowed("carol", "9.9.9.9").is_err());
    }

    #[test]
    fn test_lock_expires() {
        record_failure("dave", "1.1.1.1");
        record_failure("dave", "1.1.1.1");
        record_failure("dave", "1.1.1.1");
        record_failure("dave", "1.1.1.1");
        record_failure("dave", "1.1.1.1");
        assert!(check_allowed("dave", "1.1.1.1").is_err());
        // 手动把锁定截止拨到过去（模拟 5 分钟流逝）→ 解锁
        {
            let mut map = lock();
            let e = map.get_mut("dave|1.1.1.1").unwrap();
            e.locked_until = Some(Instant::now() - Duration::from_secs(1));
        }
        assert!(check_allowed("dave", "1.1.1.1").is_ok(), "锁定过期应恢复");
    }
}
