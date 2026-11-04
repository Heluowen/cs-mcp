//! 缓存：moka TTL 缓存封装。
//!
//! 三组独立缓存（赛程 / 实时 / 排名），key 为「查询参数拼串」。
//! 命中的判定证据通过 `Meta.cached` + `Meta.elapsed_ms` 暴露给调用方。

// S1 阶段缓存只有 cs_ping 读取条目数，读写接口在 S3 接入聚合链后启用。
#![allow(dead_code)]

use std::time::Duration;

use moka::future::Cache;

/// 单个 TTL 缓存。
#[derive(Clone)]
pub struct TtlCache {
    inner: Cache<String, String>,
}

impl TtlCache {
    pub fn new(ttl: Duration, max_capacity: u64) -> Self {
        Self {
            inner: Cache::builder()
                .max_capacity(max_capacity)
                .time_to_live(ttl)
                .build(),
        }
    }

    pub async fn get(&self, key: &str) -> Option<String> {
        self.inner.get(key).await
    }

    pub async fn put(&self, key: &str, value: String) {
        self.inner.insert(key.to_string(), value).await;
    }

    pub fn entry_count(&self) -> u64 {
        self.inner.entry_count()
    }
}

/// 缓存的 key 构造：稳定、可读、便于排查。
pub fn cache_key(params: &[(&str, &str)]) -> String {
    let mut parts: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    parts.sort();
    parts.join("&")
}

/// 三组缓存集合。
pub struct CacheSet {
    pub schedule: TtlCache,
    pub live: TtlCache,
    pub rank: TtlCache,
}

impl CacheSet {
    pub fn new(schedule_ttl: Duration, live_ttl: Duration, rank_ttl: Duration) -> Self {
        Self {
            schedule: TtlCache::new(schedule_ttl, 512),
            live: TtlCache::new(live_ttl, 128),
            rank: TtlCache::new(rank_ttl, 64),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_is_order_independent() {
        let a = cache_key(&[("hours", "24"), ("tournament", "IEM")]);
        let b = cache_key(&[("tournament", "IEM"), ("hours", "24")]);
        assert_eq!(a, b);
        assert!(a.contains("hours=24"));
    }

    #[tokio::test]
    async fn ttl_cache_roundtrip() {
        let cache = TtlCache::new(Duration::from_secs(60), 8);
        cache.put("k", "v".to_string()).await;
        assert_eq!(cache.get("k").await.as_deref(), Some("v"));
        assert!(cache.get("missing").await.is_none());
    }
}
