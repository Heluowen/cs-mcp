//! 降级链 + TTL 缓存聚合层。
//!
//! 职责：
//! 1. 按 `priority` 升序依次尝试 Provider，命中即返回（降级链）；
//! 2. 对**最终结果**做 TTL 缓存（`meta.cached` 是真实信号）；
//! 3. 错误优先返回**最高优先级源**的错误，而不是最后一个源的错误。
//!
//! 铁律 4：数据源可降级，字段统一，上层不感知来源。

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::cache::{CacheSet, TtlCache};
use crate::error::{ApiError, ErrorCode, ProviderError};
use crate::models::{Match, Ranking};

/// 数据源统一接口。
///
/// 所有方法都有默认实现（返回 `NotImplemented`），因此新增 Provider 时
/// 只需覆盖 `name` / `priority` 与真正支持的方法。
#[async_trait]
pub trait Provider: Send + Sync {
    /// 源标识（同时写入 `Match.source`）。
    fn name(&self) -> &'static str;

    /// 越小越优先。
    fn priority(&self) -> u8;

    async fn live(&self) -> Result<Vec<Match>, ProviderError> {
        Err(ProviderError::NotImplemented(format!(
            "{} 尚未实现 live",
            self.name()
        )))
    }

    async fn upcoming(
        &self,
        _hours: u32,
        _tournament: Option<&str>,
    ) -> Result<Vec<Match>, ProviderError> {
        Err(ProviderError::NotImplemented(format!(
            "{} 尚未实现 upcoming",
            self.name()
        )))
    }

    async fn recent(&self, _hours: u32) -> Result<Vec<Match>, ProviderError> {
        Err(ProviderError::NotImplemented(format!(
            "{} 尚未实现 recent",
            self.name()
        )))
    }

    async fn today(&self, _date: Option<&str>) -> Result<Vec<Match>, ProviderError> {
        Err(ProviderError::NotImplemented(format!(
            "{} 尚未实现 today",
            self.name()
        )))
    }

    async fn team_matches(
        &self,
        _team: &str,
        _limit: u32,
    ) -> Result<Vec<Match>, ProviderError> {
        Err(ProviderError::NotImplemented(format!(
            "{} 尚未实现 team_matches",
            self.name()
        )))
    }

    async fn match_detail(&self, _match_id: &str) -> Result<Vec<Match>, ProviderError> {
        Err(ProviderError::NotImplemented(format!(
            "{} 尚未实现 match_detail",
            self.name()
        )))
    }

    async fn ranking(&self, _top: u32) -> Result<Vec<Ranking>, ProviderError> {
        Err(ProviderError::NotImplemented(format!(
            "{} 尚未实现 ranking",
            self.name()
        )))
    }
}

/// 聚合结果。
///
/// 实现 `Serialize`/`Deserialize` 是为了**整体写入缓存**：
/// 只缓存 `items` 会丢掉 `sources_used` / `degraded`，导致缓存命中时
/// 上层读到的来源与降级状态是错的。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateResult<T> {
    pub items: Vec<T>,
    pub sources_used: Vec<String>,
    /// 是否发生过降级（前面的源失败/返回空）。
    pub degraded: bool,
    /// 是否命中缓存。命中时不打上游。
    #[serde(default)]
    pub cached: bool,
}

/// 依次尝试所有 Provider 的辅助宏。
///
/// 错误选择策略：全部失败时优先返回**最高优先级源**的错误（`first_err`），
/// 而不是最后一个源的错误。踩过的坑：pandascore 失败后由 liquipedia 兜底，
/// 而 liquipedia 尚未实现，最终错误变成「liquipedia 尚未实现 team_matches」，
/// 完全掩盖了真正有行动价值的 pandascore 错误。
///
/// 注意：宏内部用 `return` 提前返回，调用方必须包在 `async { }` 块里，
/// 否则会跳过缓存写入。
macro_rules! try_providers {
    ($self:expr, $method:ident $(, $arg:expr)* $(,)?) => {{
        let mut sources_used: Vec<String> = Vec::new();
        let mut degraded = false;
        let mut first_err: Option<ApiError> = None;
        let mut last_err: Option<ApiError> = None;

        for provider in $self.providers.iter() {
            match provider.$method($($arg),*).await {
                Ok(items) if !items.is_empty() => {
                    sources_used.push(provider.name().to_string());
                    return Ok(AggregateResult {
                        items,
                        sources_used,
                        degraded,
                        cached: false,
                    });
                }
                Ok(_) => {
                    degraded = true;
                    let e = ApiError::no_data(format!("{} 返回空结果", provider.name()));
                    if first_err.is_none() {
                        first_err = Some(e.clone());
                    }
                    last_err = Some(e);
                }
                Err(err) => {
                    degraded = true;
                    let e = err.to_api_error();
                    if first_err.is_none() {
                        first_err = Some(e.clone());
                    }
                    last_err = Some(e);
                }
            }
        }

        Err(first_err.or(last_err).unwrap_or_else(|| {
            ApiError::new(ErrorCode::NoData, "没有可用的数据源")
        }))
    }};
}

/// 读缓存；命中时把 `cached` 置为 `true`。
async fn read_cache<T>(cache: &TtlCache, key: &str) -> Option<AggregateResult<T>>
where
    T: for<'de> Deserialize<'de>,
{
    let raw = cache.get(key).await?;
    let mut hit: AggregateResult<T> = serde_json::from_str(&raw).ok()?;
    hit.cached = true;
    Some(hit)
}

/// 写缓存；序列化失败只是放弃缓存，不影响主流程。
async fn write_cache<T: Serialize>(cache: &TtlCache, key: &str, res: &AggregateResult<T>) {
    if let Ok(raw) = serde_json::to_string(res) {
        cache.put(key, raw).await;
    }
}

/// 降级链聚合器（带缓存）。
pub struct Aggregator {
    providers: Vec<Box<dyn Provider>>,
    caches: Arc<CacheSet>,
}

impl Aggregator {
    /// 按 `priority` 升序排列后构造。
    pub fn new(mut providers: Vec<Box<dyn Provider>>, caches: Arc<CacheSet>) -> Self {
        providers.sort_by_key(|p| p.priority());
        Self { providers, caches }
    }

    /// 是否没有任何 Provider（自检与测试用）。
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// 各源的 `(name, priority)`，用于 `cs_ping` 自检。
    pub fn describe(&self) -> Vec<(String, u8)> {
        self.providers
            .iter()
            .map(|p| (p.name().to_string(), p.priority()))
            .collect()
    }

    // ---- 实时类：短 TTL（默认 30s）----

    pub async fn live(&self) -> Result<AggregateResult<Match>, ApiError> {
        let key = "live".to_string();
        if let Some(hit) = read_cache(&self.caches.live, &key).await {
            return Ok(hit);
        }
        let res = async { try_providers!(self, live) }.await?;
        write_cache(&self.caches.live, &key, &res).await;
        Ok(res)
    }

    // ---- 赛程/赛果类：中 TTL（默认 120s）----

    pub async fn upcoming(
        &self,
        hours: u32,
        tournament: Option<&str>,
    ) -> Result<AggregateResult<Match>, ApiError> {
        let key = format!("upcoming|{hours}|{}", tournament.unwrap_or(""));
        if let Some(hit) = read_cache(&self.caches.schedule, &key).await {
            return Ok(hit);
        }
        let res = async { try_providers!(self, upcoming, hours, tournament) }.await?;
        write_cache(&self.caches.schedule, &key, &res).await;
        Ok(res)
    }

    pub async fn recent(&self, hours: u32) -> Result<AggregateResult<Match>, ApiError> {
        let key = format!("recent|{hours}");
        if let Some(hit) = read_cache(&self.caches.schedule, &key).await {
            return Ok(hit);
        }
        let res = async { try_providers!(self, recent, hours) }.await?;
        write_cache(&self.caches.schedule, &key, &res).await;
        Ok(res)
    }

    pub async fn today(&self, date: Option<&str>) -> Result<AggregateResult<Match>, ApiError> {
        let key = format!("today|{}", date.unwrap_or(""));
        if let Some(hit) = read_cache(&self.caches.schedule, &key).await {
            return Ok(hit);
        }
        let res = async { try_providers!(self, today, date) }.await?;
        write_cache(&self.caches.schedule, &key, &res).await;
        Ok(res)
    }

    pub async fn team_matches(
        &self,
        team: &str,
        limit: u32,
    ) -> Result<AggregateResult<Match>, ApiError> {
        let key = format!("team|{team}|{limit}");
        if let Some(hit) = read_cache(&self.caches.schedule, &key).await {
            return Ok(hit);
        }
        let res = async { try_providers!(self, team_matches, team, limit) }.await?;
        write_cache(&self.caches.schedule, &key, &res).await;
        Ok(res)
    }

    pub async fn match_detail(
        &self,
        match_id: &str,
    ) -> Result<AggregateResult<Match>, ApiError> {
        let key = format!("detail|{match_id}");
        if let Some(hit) = read_cache(&self.caches.schedule, &key).await {
            return Ok(hit);
        }
        let res = async { try_providers!(self, match_detail, match_id) }.await?;
        write_cache(&self.caches.schedule, &key, &res).await;
        Ok(res)
    }

    // ---- 排名类：长 TTL（默认 3600s）----

    pub async fn ranking(&self, top: u32) -> Result<AggregateResult<Ranking>, ApiError> {
        let key = format!("ranking|{top}");
        if let Some(hit) = read_cache(&self.caches.rank, &key).await {
            return Ok(hit);
        }
        let res = async { try_providers!(self, ranking, top) }.await?;
        write_cache(&self.caches.rank, &key, &res).await;
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn test_caches() -> Arc<CacheSet> {
        Arc::new(CacheSet::new(
            Duration::from_secs(120),
            Duration::from_secs(30),
            Duration::from_secs(3600),
        ))
    }

    /// 计数 Provider：用于验证缓存是否真的避免了重复调用。
    struct CountingProvider {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Provider for CountingProvider {
        fn name(&self) -> &'static str {
            "counting"
        }
        fn priority(&self) -> u8 {
            1
        }
        async fn recent(&self, _hours: u32) -> Result<Vec<Match>, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(ProviderError::NoData("空".to_string()))
        }
    }

    struct EmptyProvider;

    #[async_trait]
    impl Provider for EmptyProvider {
        fn name(&self) -> &'static str {
            "empty"
        }
        fn priority(&self) -> u8 {
            1
        }
    }

    #[tokio::test]
    async fn empty_chain_returns_no_data() {
        let agg = Aggregator::new(vec![Box::new(EmptyProvider)], test_caches());
        let err = agg.live().await.err();
        assert_eq!(err.map(|e| e.code), Some(ErrorCode::NoData));
    }

    #[tokio::test]
    async fn no_providers_returns_no_data() {
        let agg = Aggregator::new(vec![], test_caches());
        assert!(agg.is_empty());
        assert!(agg.live().await.is_err());
    }

    #[tokio::test]
    async fn failure_is_not_cached_so_retry_reaches_provider() {
        // 失败结果不入缓存：否则一次网络抖动会被缓存 120 秒，用户要等两分钟。
        let calls = Arc::new(AtomicUsize::new(0));
        let agg = Aggregator::new(
            vec![Box::new(CountingProvider {
                calls: calls.clone(),
            })],
            test_caches(),
        );
        assert!(agg.recent(24).await.is_err());
        assert!(agg.recent(24).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 2, "失败不应命中缓存");
    }

    /// 造一场最小可用的 Match（缓存往返需要真实可序列化的数据）。
    fn sample_match() -> Option<Match> {
        let offset = crate::config::Config::cn_offset().ok()?;
        let now = crate::config::Config::now(offset);
        Some(Match {
            id: "pandascore:1".to_string(),
            status: crate::models::Status::Finished,
            starts_at: now,
            tournament: crate::models::Tournament {
                name: "Test Cup".to_string(),
                tier: Some("S".to_string()),
                region: None,
            },
            bo: Some(3),
            teams: vec![
                crate::models::Team {
                    name: "A".to_string(),
                    name_zh: None,
                    score: Some(2),
                },
                crate::models::Team {
                    name: "B".to_string(),
                    name_zh: None,
                    score: Some(1),
                },
            ],
            maps: Vec::new(),
            streams: Vec::new(),
            source: "test".to_string(),
            fetched_at: now,
        })
    }

    /// 计数 Provider：成功返回一场比赛，用于验证**命中缓存后不再调用 Provider**。
    struct FixedProvider {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Provider for FixedProvider {
        fn name(&self) -> &'static str {
            "fixed"
        }
        fn priority(&self) -> u8 {
            1
        }
        async fn recent(&self, _hours: u32) -> Result<Vec<Match>, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match sample_match() {
                Some(m) => Ok(vec![m]),
                None => Err(ProviderError::NoData("构造样本失败".to_string())),
            }
        }
    }

    #[tokio::test]
    async fn identical_query_hits_cache_and_skips_provider() {
        let calls = Arc::new(AtomicUsize::new(0));
        let agg = Aggregator::new(
            vec![Box::new(FixedProvider {
                calls: calls.clone(),
            })],
            test_caches(),
        );

        let first = agg.recent(24).await;
        assert!(first.is_ok(), "首次调用应成功");
        let Ok(first) = first else { return };
        assert!(!first.cached, "首次不应是缓存命中");
        assert_eq!(first.items.len(), 1);

        let second = agg.recent(24).await;
        assert!(second.is_ok(), "第二次调用应成功");
        let Ok(second) = second else { return };
        assert!(second.cached, "相同参数第二次必须命中缓存");
        assert_eq!(second.items.len(), 1, "缓存内容应完整往返");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "命中缓存后不得再次调用 Provider"
        );
    }

    #[tokio::test]
    async fn different_args_do_not_share_cache() {
        let calls = Arc::new(AtomicUsize::new(0));
        let agg = Aggregator::new(
            vec![Box::new(FixedProvider {
                calls: calls.clone(),
            })],
            test_caches(),
        );
        let _ = agg.recent(24).await;
        let _ = agg.recent(48).await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "不同参数必须各自打 Provider"
        );
    }
}
