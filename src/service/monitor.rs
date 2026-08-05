//! 服务监控：真实内存/CPU（sysinfo）、请求计数（内存原子计数器——总数/今日/按接口 Top）、
//! 在线会话（storage 有效 token 数）、书源检测结果（最近一次 getInvalidBookSources 等）。
//!
//! - 内存/CPU：`sysinfo` crate 跨平台读取（Windows 上为真实值，修复旧 getSystemInfo 全 0M）
//! - 请求计数：stats 中间件每请求调用 [`record_request`]；今日计数按本地日期滚动清零
//! - 书源成功率：检测类接口（getInvalidBookSources / disableInvalidBookSources）执行后
//!   记录最近一次结果；从未检测时返回 `successRate: null` + 说明

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use once_cell::sync::Lazy;
use serde_json::{json, Value};
use sysinfo::{Pid, System};

/// CPU 短采样间隔（ms）——两次采样差值计算使用率
const CPU_SAMPLE_MS: u64 = 200;
/// 按接口计数表上限（超出不再记录新路径，防内存无限增长）
const ENDPOINT_MAP_CAP: usize = 300;
/// Top 接口默认条数
pub const TOP_ENDPOINTS_DEFAULT: usize = 10;

/// 内存快照（MB）
#[derive(Debug, Clone, Copy)]
pub struct MemSample {
    /// 物理内存总量
    pub total_mb: u64,
    /// 可用物理内存
    pub available_mb: u64,
    /// 已用物理内存
    pub used_mb: u64,
    /// 本进程内存
    pub process_mb: u64,
}

impl MemSample {
    /// 已用占比 0..=100
    pub fn percent(&self) -> f64 {
        if self.total_mb == 0 {
            0.0
        } else {
            self.used_mb as f64 / self.total_mb as f64 * 100.0
        }
    }
}

/// 书源检测快照（最近一次）
#[derive(Debug, Clone)]
pub struct BookSourceCheck {
    pub total: u64,
    pub ok: u64,
    pub failed: u64,
    pub checked_at_ms: i64,
    pub namespace: String,
}

/// 请求计数（可独立实例化——单元测试避免共享全局状态）
#[derive(Debug, Default)]
pub struct RequestCounters {
    total: AtomicU64,
    today: AtomicU64,
    today_date: Mutex<String>,
    endpoints: Mutex<HashMap<String, u64>>,
}

impl RequestCounters {
    /// 记录一次请求（path = 请求路径，含 method 无关——按接口路径聚合）
    pub fn record(&self, path: &str) {
        self.total.fetch_add(1, Ordering::Relaxed);
        // 本地日期滚动：跨天清零今日计数
        let date = today_date_str();
        let mut guard = self.today_date.lock().unwrap();
        if *guard != date {
            *guard = date;
            self.today.store(0, Ordering::Relaxed);
        }
        self.today.fetch_add(1, Ordering::Relaxed);
        drop(guard);
        let mut eps = self.endpoints.lock().unwrap();
        if let Some(c) = eps.get_mut(path) {
            *c += 1;
        } else if eps.len() < ENDPOINT_MAP_CAP {
            eps.insert(path.to_string(), 1);
        }
    }

    /// 请求量快照：总数 / 今日 / 按接口 Top（次数降序，同次数按路径升序）
    pub fn snapshot(&self, limit: usize) -> (u64, u64, Vec<(String, u64)>) {
        let total = self.total.load(Ordering::Relaxed);
        let today = self.today.load(Ordering::Relaxed);
        let eps = self.endpoints.lock().unwrap();
        let mut v: Vec<(String, u64)> = eps.iter().map(|(k, c)| (k.clone(), *c)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v.truncate(limit);
        (total, today, v)
    }

    /// 今日日期（测试钩子：模拟跨天滚动）
    #[cfg(test)]
    fn set_today_date(&self, date: &str) {
        *self.today_date.lock().unwrap() = date.to_string();
    }
}

/// 全局请求计数器（stats 中间件写入，getSystemInfo/getServerStats 读取）
pub static REQUESTS: Lazy<RequestCounters> = Lazy::new(RequestCounters::default);

/// 书源检测结果记录器（可独立实例化——单元测试避免共享全局状态）
#[derive(Debug, Default)]
pub struct BookSourceRecorder {
    inner: Mutex<Option<BookSourceCheck>>,
}

impl BookSourceRecorder {
    /// 记录最近一次检测结果
    pub fn record(&self, namespace: &str, total: u64, failed: u64) {
        *self.inner.lock().unwrap() = Some(BookSourceCheck {
            total,
            ok: total.saturating_sub(failed),
            failed,
            checked_at_ms: now_ms(),
            namespace: namespace.to_string(),
        });
    }

    /// 成功率快照（从未检测 → successRate: null + 说明）
    pub fn snapshot(&self) -> Value {
        let guard = self.inner.lock().unwrap();
        match guard.as_ref() {
            Some(c) => {
                let rate = if c.total > 0 {
                    ((c.ok as f64 / c.total as f64) * 1000.0).round() / 1000.0
                } else {
                    0.0
                };
                json!({
                    "total": c.total,
                    "ok": c.ok,
                    "failed": c.failed,
                    "successRate": rate,
                    "checkedAt": c.checked_at_ms,
                    "namespace": c.namespace,
                    "note": "",
                })
            }
            None => json!({
                "total": 0,
                "ok": 0,
                "failed": 0,
                "successRate": Value::Null,
                "checkedAt": Value::Null,
                "namespace": "",
                "note": "尚未执行过书源检测（可在书源管理页执行检测）",
            }),
        }
    }
}

/// 全局最近一次书源检测结果
pub static BOOK_SOURCE_CHECK: Lazy<BookSourceRecorder> = Lazy::new(BookSourceRecorder::default);

/// 进程启动时刻（uptime 基准）
static STARTED_AT_MS: Lazy<i64> = Lazy::new(now_ms);

/// 中间件入口：记录一次请求
pub fn record_request(path: &str) {
    REQUESTS.record(path);
}

/// 记录最近一次书源检测结果（getInvalidBookSources / disableInvalidBookSources 调用）
pub fn record_book_source_check(namespace: &str, total: u64, failed: u64) {
    BOOK_SOURCE_CHECK.record(namespace, total, failed);
}

/// 书源成功率快照（从未检测 → successRate: null + 说明）
pub fn book_source_snapshot() -> Value {
    BOOK_SOURCE_CHECK.snapshot()
}

/// 真实内存采样（sysinfo；Windows 下为 GlobalMemoryStatusEx 实际值）
pub fn sample_memory() -> MemSample {
    let mut sys = System::new();
    sys.refresh_memory();
    let total_mb = bytes_to_mb(sys.total_memory());
    let available_mb = bytes_to_mb(sys.available_memory());
    let used_mb = bytes_to_mb(sys.used_memory());
    // 本进程内存（进程列表采样——监控接口 10s 一次，开销可接受）
    sys.refresh_processes();
    let process_mb = sys
        .process(Pid::from_u32(std::process::id()))
        .map(|p| bytes_to_mb(p.memory()))
        .unwrap_or(0);
    MemSample {
        total_mb,
        available_mb,
        used_mb,
        process_mb,
    }
}

/// CPU 短采样：两次 refresh_cpu_usage（间隔 CPU_SAMPLE_MS）取全局使用率；
/// 返回 (使用率 0..=100, 逻辑核心数)
pub async fn sample_cpu() -> (f32, usize) {
    let mut sys = System::new();
    sys.refresh_cpu_usage(); // 首次调用建立基线
    tokio::time::sleep(Duration::from_millis(CPU_SAMPLE_MS)).await;
    sys.refresh_cpu_usage();
    let cores = sys.cpus().len();
    let usage = if cores == 0 {
        0.0
    } else {
        sys.global_cpu_info().cpu_usage()
    };
    (usage, cores)
}

/// 进程运行时长（秒）
pub fn uptime_seconds() -> i64 {
    (now_ms() - *STARTED_AT_MS) / 1000
}

/// 监控聚合（getSystemInfo / getServerStats 共用）
pub struct ServerStatsAggregate {
    pub memory: MemSample,
    pub cpu_percent: f32,
    pub cpu_cores: usize,
    pub total_requests: u64,
    pub today_requests: u64,
    pub top_endpoints: Vec<(String, u64)>,
    pub online_sessions: i64,
    pub book_source: Value,
    pub uptime_seconds: i64,
    pub timestamp_ms: i64,
}

/// 采集一次完整聚合（内存 + CPU 短采样 ~200ms + 会话/计数/书源快照）
pub async fn collect(storage: &crate::storage::Storage) -> ServerStatsAggregate {
    let memory = sample_memory();
    let (cpu_percent, cpu_cores) = sample_cpu().await;
    let online_sessions = storage.count_active_tokens().await.unwrap_or(0);
    let (total_requests, today_requests, top_endpoints) = REQUESTS.snapshot(TOP_ENDPOINTS_DEFAULT);
    ServerStatsAggregate {
        memory,
        cpu_percent,
        cpu_cores,
        total_requests,
        today_requests,
        top_endpoints,
        online_sessions,
        book_source: book_source_snapshot(),
        uptime_seconds: uptime_seconds(),
        timestamp_ms: now_ms(),
    }
}

impl ServerStatsAggregate {
    /// JSON 结构（camelCase，与前端 ServerStats 类型对应）
    pub fn to_json(&self) -> Value {
        json!({
            "timestamp": self.timestamp_ms,
            "uptimeSeconds": self.uptime_seconds,
            "memory": {
                "totalMb": self.memory.total_mb,
                "availableMb": self.memory.available_mb,
                "usedMb": self.memory.used_mb,
                "processMb": self.memory.process_mb,
                "percent": (self.memory.percent() * 10.0).round() / 10.0,
            },
            "cpu": { "percent": self.cpu_percent, "cores": self.cpu_cores },
            "requests": {
                "total": self.total_requests,
                "today": self.today_requests,
                "topEndpoints": self
                    .top_endpoints
                    .iter()
                    .map(|(path, count)| json!({ "path": path, "count": count }))
                    .collect::<Vec<_>>(),
            },
            "online": { "sessions": self.online_sessions },
            "bookSource": self.book_source,
        })
    }
}

fn bytes_to_mb(bytes: u64) -> u64 {
    bytes / (1024 * 1024)
}

fn today_date_str() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_counters_basic_and_top() {
        let c = RequestCounters::default();
        c.record("/reader3/getBookshelf");
        c.record("/reader3/getBookshelf");
        c.record("/reader3/getBookSources");
        c.record("/assets/proxy");
        let (total, today, top) = c.snapshot(10);
        assert_eq!(total, 4);
        assert_eq!(today, 4);
        assert_eq!(top.len(), 3);
        assert_eq!(top[0], ("/reader3/getBookshelf".to_string(), 2));
        assert_eq!(top[1].1, 1);
        // limit 截断
        let (_, _, top1) = c.snapshot(1);
        assert_eq!(top1.len(), 1);
        assert_eq!(top1[0].0, "/reader3/getBookshelf");
    }

    #[test]
    fn test_request_counters_date_rollover() {
        let c = RequestCounters::default();
        c.set_today_date("2026-01-01");
        c.record("/a");
        c.record("/a");
        assert_eq!(c.snapshot(10).1, 2);
        // 跨天：今日计数清零后重新累计
        c.set_today_date("2026-01-02");
        c.record("/b");
        let (total, today, _) = c.snapshot(10);
        assert_eq!(total, 3, "总数跨天不清零");
        assert_eq!(today, 1, "今日计数跨天清零");
    }

    #[test]
    fn test_request_counters_map_cap() {
        let c = RequestCounters::default();
        for i in 0..(ENDPOINT_MAP_CAP + 50) {
            c.record(&format!("/p{i}"));
        }
        assert_eq!(c.snapshot(10_000).2.len(), ENDPOINT_MAP_CAP, "超限不再记录新路径");
    }

    #[test]
    fn test_sample_memory_real_values() {
        let m = sample_memory();
        assert!(m.total_mb > 0, "物理内存应 > 0（Windows 真实读取）：{m:?}");
        assert!(m.used_mb > 0, "已用内存应 > 0");
        assert!(m.process_mb > 0, "本进程内存应 > 0");
        assert!(m.percent() > 0.0 && m.percent() <= 100.0);
    }

    #[tokio::test]
    async fn test_sample_cpu_real_values() {
        let (usage, cores) = sample_cpu().await;
        assert!(cores >= 1, "CPU 核心数应 >= 1");
        assert!(
            (0.0..=100.0).contains(&usage),
            "CPU 使用率应在 0..=100：{usage}"
        );
    }

    #[test]
    fn test_book_source_recorder_exact() {
        // 独立实例——精确断言（全局实例会被其他测试/接口并发写入）
        let r = BookSourceRecorder::default();
        let v = r.snapshot();
        assert!(v["successRate"].is_null(), "未检测 → successRate null");
        assert!(v["note"].as_str().unwrap().contains("尚未执行过书源检测"));
        assert_eq!(v["total"], 0);

        r.record("default", 10, 3);
        let v = r.snapshot();
        assert_eq!(v["total"], 10);
        assert_eq!(v["ok"], 7);
        assert_eq!(v["failed"], 3);
        assert_eq!(v["successRate"], 0.7, "7/10 成功率 0.7");
        assert!(v["checkedAt"].as_i64().unwrap() > 0);
        assert_eq!(v["namespace"], "default");
        assert_eq!(v["note"], "");

        // 覆盖：失败数 > 总数 → ok 饱和为 0
        r.record("alice", 5, 9);
        let v = r.snapshot();
        assert_eq!(v["ok"], 0);
        assert_eq!(v["successRate"], 0.0);
    }

    #[test]
    fn test_uptime_non_negative() {
        assert!(uptime_seconds() >= 0);
    }
}
