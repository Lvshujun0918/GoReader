//! 图片代理磁盘缓存（/assets/proxy 回源加速）
//!
//! - 磁盘缓存：`storage/cache/images/{md5(url)前16}.{ext}`——命中直接读盘，避免每次回源；
//!   命中方（router）下发长 Cache-Control（public, max-age=31536000, immutable）
//! - 容量上限：env `READER_IMAGE_CACHE_MB`（默认 512MB，0 = 禁用磁盘缓存），
//!   超限按 LRU（最近最少使用，进程内单调时钟序）清理
//! - 并发去重：同 URL 同时请求共享一次回源（内存 in-flight map + 每 key 信号量）
//! - 安全：缓存键 = md5(url) 前 16 位十六进制（不落原始 URL）；扩展名白名单——
//!   仅白名单 Content-Type 落盘（文件名扩展名从白名单映射），非白名单原样透传

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::util::md5::md5_encode;

/// 磁盘缓存目录（相对 storage 根）：storage/cache/images
const CACHE_SUBDIR: &str = "cache/images";

/// 默认容量（MB，env READER_IMAGE_CACHE_MB 覆盖）
const DEFAULT_CACHE_MB: u64 = 512;

/// 内容类型 → 扩展名白名单（仅白名单内的图片类型才落盘缓存）
fn ext_for_content_type(content_type: &str) -> Option<&'static str> {
    let ct = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    match ct.as_str() {
        "image/jpeg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/avif" => Some("avif"),
        "image/bmp" => Some("bmp"),
        "image/x-icon" | "image/vnd.microsoft.icon" => Some("ico"),
        _ => None,
    }
}

/// 扩展名 → Content-Type（读盘命中时还原响应头）
fn content_type_for_ext(ext: &str) -> &'static str {
    match ext {
        "jpg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

/// 缓存键：md5(url) 前 16 位十六进制（URL 哈希——不落原始 URL）
fn cache_key(url: &str) -> String {
    let hex = md5_encode(url);
    hex[..16].to_string()
}

/// 索引条目
struct CacheEntry {
    /// 文件字节数
    size: u64,
    /// 最近使用时钟（全局单调递增，越大越新；LRU 清理取最小者）
    last_used: u64,
    /// 扩展名（白名单内）
    ext: String,
}

/// 缓存索引（内存态；构造时从磁盘目录种子化；文件读写/清理在锁内串行，
/// 避免 LRU 清理与读取竞态）
struct CacheState {
    entries: HashMap<String, CacheEntry>,
    total_bytes: u64,
}

/// 临时文件序号（并发写盘文件名不冲突）
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// 图片代理磁盘缓存
pub struct ImageCache {
    /// 缓存目录（storage/cache/images）
    dir: PathBuf,
    /// 容量上限（字节；0 = 禁用磁盘缓存，仅保留并发去重）
    max_bytes: u64,
    /// LRU 时钟（全局单调递增）
    clock: AtomicU64,
    /// 索引 + 容量统计（std Mutex：临界区短，无跨 await 持有）
    state: Mutex<CacheState>,
    /// 并发去重：key → in-flight 信号量（首个请求持有，其余等待其完成后再查盘）
    inflight: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

/// 取图结果：(字节, Content-Type, 上游状态码, 是否磁盘命中)
pub type FetchOutcome = (Vec<u8>, Option<String>, u16, bool);

impl ImageCache {
    /// 从应用配置构建：目录 = {storage_dir}/cache/images，容量 = env READER_IMAGE_CACHE_MB
    /// （默认 512MB；0 = 禁用磁盘缓存）
    pub fn new(config: &crate::AppConfig) -> Arc<Self> {
        let mb = std::env::var("READER_IMAGE_CACHE_MB")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_CACHE_MB);
        Self::with_capacity(
            config.storage_dir().join(CACHE_SUBDIR),
            mb.saturating_mul(1024 * 1024),
        )
    }

    /// 指定目录与容量（测试/精确控制用；max_bytes = 0 → 禁用磁盘缓存）
    pub fn with_capacity(dir: PathBuf, max_bytes: u64) -> Arc<Self> {
        let cache = Arc::new(Self {
            dir,
            max_bytes,
            clock: AtomicU64::new(0),
            state: Mutex::new(CacheState {
                entries: HashMap::new(),
                total_bytes: 0,
            }),
            inflight: Mutex::new(HashMap::new()),
        });
        cache.seed_from_disk();
        cache
    }

    /// 缓存目录（测试断言用）
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 容量上限（字节）
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    /// 当前缓存总字节数（测试断言用）
    pub fn total_bytes(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .total_bytes
    }

    /// 取图片：磁盘命中直接返回；未命中回源（同 URL 并发去重）并写入缓存。
    ///
    /// 返回 (字节, Content-Type, 上游状态码, 是否磁盘命中)；仅 HTTP 200 且白名单
    /// 图片类型会落盘；缓存 I/O 失败不影响响应（回源结果照常返回）。
    pub async fn get_or_fetch(
        &self,
        ns: &str,
        url: &str,
        referer: Option<&str>,
        timeout_secs: u64,
        max_bytes: u64,
    ) -> anyhow::Result<FetchOutcome> {
        let key = cache_key(url);
        if self.max_bytes > 0 {
            if let Some((bytes, ct)) = self.read_disk(&key) {
                return Ok((bytes, Some(ct.to_string()), 200, true));
            }
        }
        // 并发去重：同 key 同时请求共享一次回源（等待者醒来后重新查盘，不再各自回源）
        let gate = {
            let mut map = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
            map.entry(key.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _guard = gate.lock().await;
        // 等待期间可能已被并发请求写入 → 再次查盘
        if self.max_bytes > 0 {
            if let Some((bytes, ct)) = self.read_disk(&key) {
                return Ok((bytes, Some(ct.to_string()), 200, true));
            }
        }
        let result =
            crate::service::crawler::fetch_image(ns, url, referer, timeout_secs, max_bytes).await;
        if self.max_bytes > 0 {
            if let Ok((bytes, content_type, status)) = &result {
                if *status == 200 {
                    self.write_disk(&key, bytes, content_type.as_deref());
                }
            }
        }
        // 先释放信号量再移除条目：等待者此刻已被唤醒（重新查盘命中）；移除后新请求开新条目
        drop(_guard);
        self.inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&key);
        result.map(|(bytes, ct, status)| (bytes, ct, status, false))
    }

    /// 读盘命中：索引查扩展名 → 读文件 → 刷新 LRU 时钟。
    /// 文件读取在索引锁内完成（与 LRU 清理互斥，避免删读竞态）。
    fn read_disk(&self, key: &str) -> Option<(Vec<u8>, &'static str)> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let ext = state.entries.get(key).map(|e| e.ext.clone())?;
        let path = self.dir.join(format!("{key}.{ext}"));
        let bytes = std::fs::read(&path).ok()?;
        // 命中：刷新 LRU 时钟
        let now = self.clock.fetch_add(1, Ordering::Relaxed);
        if let Some(e) = state.entries.get_mut(key) {
            e.last_used = now;
        }
        Some((bytes, content_type_for_ext(&ext)))
    }

    /// 写盘：白名单 Content-Type 才落盘；临时文件 + rename 原子写（读方永远见完整文件）；
    /// 随后按容量 LRU 清理。
    fn write_disk(&self, key: &str, bytes: &[u8], content_type: Option<&str>) {
        let Some(ext) = content_type.and_then(ext_for_content_type) else {
            return; // 非白名单类型：不缓存（原样透传）
        };
        // 单图超过容量：缓存不下，直接不落盘（避免写后立即被 LRU 清掉的空转）
        if (bytes.len() as u64) > self.max_bytes {
            return;
        }
        if std::fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        let path = self.dir.join(format!("{key}.{ext}"));
        let tmp = self.dir.join(format!(
            ".{key}.{ext}.tmp{}-{}",
            std::process::id(),
            TMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        if std::fs::write(&tmp, bytes).is_err() {
            return;
        }
        if std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = self.clock.fetch_add(1, Ordering::Relaxed);
        if let Some(old) = state.entries.insert(
            key.to_string(),
            CacheEntry {
                size: bytes.len() as u64,
                last_used: now,
                ext: ext.to_string(),
            },
        ) {
            state.total_bytes = state.total_bytes.saturating_sub(old.size);
        }
        state.total_bytes = state.total_bytes.saturating_add(bytes.len() as u64);
        drop(state);
        self.evict_lru();
    }

    /// 容量超限 → LRU 清理（最近最少使用：last_used 最小者先删），直到总字节 ≤ 上限
    fn evict_lru(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while state.total_bytes > self.max_bytes {
            let victim = state
                .entries
                .iter()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, _)| k.clone());
            let Some(victim) = victim else { break };
            let entry = state
                .entries
                .remove(&victim)
                .expect("victim 刚取自 entries，必然存在");
            state.total_bytes = state.total_bytes.saturating_sub(entry.size);
            let path = self.dir.join(format!("{victim}.{}", entry.ext));
            if std::fs::remove_file(&path).is_err() {
                tracing::debug!("图片缓存 LRU 删除失败（可能已被外部清理）: {path:?}");
            }
            tracing::debug!("图片缓存 LRU 清理: {victim}.{}", entry.ext);
        }
    }

    /// 启动种子化：扫描目录，将存量缓存文件纳入索引（key 形如 16 位 hex + 扩展名白名单），
    /// 并立即按容量 LRU 清理（重启后磁盘可能已超限）。
    fn seed_from_disk(&self) {
        if self.max_bytes == 0 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return; // 目录尚不存在（首次启动）
        };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some((key, ext)) = name.rsplit_once('.') else {
                continue;
            };
            if key.len() != 16 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
                continue; // 非缓存命名（如 .tmp 临时文件）
            }
            // 扩展名必须在白名单（content_type_for_ext → ext_for_content_type 往返一致）
            if ext_for_content_type(content_type_for_ext(ext)).is_none() {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let now = self.clock.fetch_add(1, Ordering::Relaxed);
            if let Some(old) = state.entries.insert(
                key.to_string(),
                CacheEntry {
                    size: meta.len(),
                    last_used: now,
                    ext: ext.to_string(),
                },
            ) {
                state.total_bytes = state.total_bytes.saturating_sub(old.size);
            }
            state.total_bytes = state.total_bytes.saturating_add(meta.len());
        }
        drop(state);
        self.evict_lru();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// mock 图片服务器：每收到一个请求计数 +1；按路径返回不同内容；delay_ms > 0 时
    /// 延迟响应（并发去重测试用，保证请求重叠）。
    async fn mock_server(
        delay_ms: u64,
    ) -> (std::net::SocketAddr, Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::AtomicUsize;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let count_for_task = count.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let count = count_for_task.clone();
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 4096];
                    let _ = sock.read(&mut buf).await;
                    let req = String::from_utf8_lossy(&buf);
                    let path = req
                        .lines()
                        .next()
                        .unwrap_or("")
                        .split(' ')
                        .nth(1)
                        .unwrap_or("/")
                        .to_string();
                    if delay_ms > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    }
                    count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let (ct, body): (&str, Vec<u8>) = match path.as_str() {
                        "/text.txt" => ("text/plain", b"not-an-image".to_vec()),
                        "/small.png" => ("image/png", vec![0xCC; 10_000]),
                        "/medium.png" => ("image/png", vec![0xBB; 20_000]),
                        "/big.png" => ("image/png", vec![0xAA; 30_000]),
                        "/error.png" => ("image/png", Vec::new()),
                        _ => ("image/png", vec![0x89, b'P', b'N', b'G', 9, 9, 9]),
                    };
                    let status = if path == "/error.png" {
                        "404 Not Found"
                    } else {
                        "200 OK"
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let mut resp = head.into_bytes();
                    resp.extend_from_slice(&body);
                    let _ = sock.write_all(&resp).await;
                });
            }
        });
        (addr, count)
    }

    /// 独立临时缓存目录（避免污染真实 storage）
    async fn temp_cache(tag: &str, max_bytes: u64) -> (Arc<ImageCache>, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "reader-image-cache-test-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        (ImageCache::with_capacity(dir.clone(), max_bytes), dir)
    }

    /// 首次拉取：回源 1 次并写盘 storage/cache/images/{md5前16}.png；二次请求磁盘命中、
    /// 不再回源（Content-Type 由扩展名还原）。
    #[tokio::test]
    async fn test_first_fetch_writes_cache_and_second_hits() {
        let (cache, dir) = temp_cache("first", 1024 * 1024).await;
        let (addr, count) = mock_server(0).await;
        let url = format!("http://{addr}/cover.png");
        let expected: Vec<u8> = vec![0x89, b'P', b'N', b'G', 9, 9, 9];

        let (bytes, ct, status, from_cache) = cache
            .get_or_fetch("default", &url, None, 10, 5 * 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(status, 200);
        assert!(!from_cache, "首次应为回源");
        assert_eq!(ct.as_deref(), Some("image/png"));
        assert_eq!(bytes, expected);

        // 缓存文件落盘：{md5(url)前16}.png
        let key = cache_key(&url);
        assert_eq!(key.len(), 16);
        let path = cache.dir().join(format!("{key}.png"));
        assert!(path.exists(), "首次拉取应写缓存文件: {path:?}");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            expected,
            "缓存内容 = 上游字节"
        );

        // 二次请求：磁盘命中，不再回源
        let (bytes2, ct2, status2, from_cache2) = cache
            .get_or_fetch("default", &url, None, 10, 5 * 1024 * 1024)
            .await
            .unwrap();
        assert!(from_cache2, "二次应为磁盘命中");
        assert_eq!(status2, 200);
        assert_eq!(ct2.as_deref(), Some("image/png"));
        assert_eq!(bytes2, expected);
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "二次请求不应回源"
        );
        assert_eq!(cache.total_bytes(), expected.len() as u64);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 容量超限 → LRU 清理：最久未使用先删；读命中刷新 recency 后保留。
    ///
    /// 容量 45KB：small(10K)+medium(20K) 写入后 30K；big(30K) 写入 → 60K > 45K →
    /// 清 small → 50K > 45K → 清 medium → 余 big(30K)。
    /// 重新拉取 small（miss 回源）→ 40K；再拉取 medium → 60K → 清最久未用的 big → 30K。
    #[tokio::test]
    async fn test_capacity_lru_eviction() {
        let (cache, dir) = temp_cache("lru", 45_000).await;
        let (addr, count) = mock_server(0).await;
        let small = format!("http://{addr}/small.png");
        let medium = format!("http://{addr}/medium.png");
        let big = format!("http://{addr}/big.png");
        let file_of = |u: &str| cache.dir().join(format!("{}.png", cache_key(u)));

        for url in [&small, &medium, &big] {
            let (_, _, status, _) = cache
                .get_or_fetch("default", url, None, 10, 5 * 1024 * 1024)
                .await
                .unwrap();
            assert_eq!(status, 200);
        }
        // big 写入触发清理：small、medium 被 LRU 清掉，big 保留
        assert!(!file_of(&small).exists(), "最久未使用 small 应被清理");
        assert!(!file_of(&medium).exists(), "次久 medium 应被清理");
        assert!(file_of(&big).exists(), "最新 big 应保留");
        assert!(cache.total_bytes() <= 45_000, "总字节应 ≤ 容量上限");

        // 重新拉取 small（miss → 回源写盘）→ 40K，不触发清理
        let (_, _, _, from_cache) = cache
            .get_or_fetch("default", &small, None, 10, 5 * 1024 * 1024)
            .await
            .unwrap();
        assert!(!from_cache);
        assert!(file_of(&small).exists());
        assert!(file_of(&big).exists());

        // 再拉取 medium（miss → 回源写盘）→ 60K → 清最久未用 big（small 刚被用过）
        cache
            .get_or_fetch("default", &medium, None, 10, 5 * 1024 * 1024)
            .await
            .unwrap();
        assert!(!file_of(&big).exists(), "big 最久未用应被清理");
        assert!(file_of(&small).exists(), "small 刚被命中应保留");
        assert!(file_of(&medium).exists());
        assert!(cache.total_bytes() <= 45_000);
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            5,
            "small/medium/big + 重拉 small/medium 共 5 次回源"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 并发去重：同 URL 同时请求（慢上游）仅回源一次，5 个请求全部拿到相同字节
    #[tokio::test]
    async fn test_concurrent_same_url_single_fetch() {
        let (cache, dir) = temp_cache("dedup", 1024 * 1024).await;
        let (addr, count) = mock_server(300).await; // 慢上游：确保请求重叠
        let url = format!("http://{addr}/cover.png");
        let expected: Vec<u8> = vec![0x89, b'P', b'N', b'G', 9, 9, 9];

        let mut handles = Vec::new();
        for _ in 0..5 {
            let cache = cache.clone();
            let url = url.clone();
            handles.push(tokio::spawn(async move {
                cache
                    .get_or_fetch("default", &url, None, 10, 5 * 1024 * 1024)
                    .await
                    .unwrap()
            }));
        }
        let results = futures::future::join_all(handles).await;
        for r in &results {
            let (bytes, ct, status, _) = r.as_ref().unwrap();
            assert_eq!(status, &200);
            assert_eq!(ct.as_ref().unwrap(), "image/png");
            assert_eq!(bytes, &expected);
        }
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "同 URL 并发请求只应回源一次"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 安全：非白名单 Content-Type 与 非 200 响应不落盘（原样透传，后续仍回源）
    #[tokio::test]
    async fn test_non_whitelist_and_error_not_cached() {
        let (cache, dir) = temp_cache("whitelist", 1024 * 1024).await;
        let (addr, count) = mock_server(0).await;

        // text/plain → 白名单外，不缓存
        let url = format!("http://{addr}/text.txt");
        let (bytes, ct, status, from_cache) = cache
            .get_or_fetch("default", &url, None, 10, 5 * 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(status, 200);
        assert!(!from_cache);
        assert_eq!(ct.as_deref(), Some("text/plain"));
        assert_eq!(bytes, b"not-an-image");
        assert!(
            !cache
                .dir()
                .join(format!("{}.txt", cache_key(&url)))
                .exists(),
            "白名单外类型不应落盘"
        );
        // 再次请求仍回源（未缓存）
        cache
            .get_or_fetch("default", &url, None, 10, 5 * 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2);

        // 404 → 不缓存
        let url = format!("http://{addr}/error.png");
        let (_, _, status, _) = cache
            .get_or_fetch("default", &url, None, 10, 5 * 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(status, 404);
        assert!(
            !cache
                .dir()
                .join(format!("{}.png", cache_key(&url)))
                .exists(),
            "非 200 不应落盘"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 安全：缓存键 = md5(url) 前 16 位十六进制——原始 URL 不出现在文件名
    #[test]
    fn test_cache_key_is_md5_prefix() {
        let key = cache_key("https://example.com/cover.png");
        assert_eq!(key.len(), 16);
        assert!(key.bytes().all(|b| b.is_ascii_hexdigit()));
        // 同 URL 稳定；不同 URL 不同
        assert_eq!(key, cache_key("https://example.com/cover.png"));
        assert_ne!(key, cache_key("https://example.com/cover2.png"));
        // 确为 md5 前 16 位
        let full = md5_encode("https://example.com/cover.png");
        assert_eq!(key, &full[..16]);
    }
}
