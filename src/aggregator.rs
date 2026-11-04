//! 降级链：按 `priority` 升序依次尝试 Provider，第一个返回非空结果即采用。
//!
//! 铁律 4：数据源可降级，字段统一，上层不感知来源。
//! 全失败返回 `UPSTREAM_ERROR` / `NO_DATA`，并把 `degraded=true` 透出。

use async_trait::async_trait;

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
pub struct AggregateResult<T> {
    pub items: Vec<T>,
    pub sources_used: Vec<String>,
    /// 是否发生过降级（前面的源失败/返回空）。
    pub degraded: bool,
}

/// 依次尝试所有 Provider 的辅助宏。
macro_rules! try_providers {
    ($self:expr, $method:ident $(, $arg:expr)* $(,)?) => {{
        let mut sources_used: Vec<String> = Vec::new();
        let mut degraded = false;
        let mut last_err: Option<ApiError> = None;

        for provider in $self.providers.iter() {
            match provider.$method($($arg),*).await {
                Ok(items) if !items.is_empty() => {
                    sources_used.push(provider.name().to_string());
                    return Ok(AggregateResult {
                        items,
                        sources_used,
                        degraded,
                    });
                }
                Ok(_) => {
                    degraded = true;
                    last_err = Some(ApiError::no_data(format!(
                        "{} 返回空结果",
                        provider.name()
                    )));
                }
                Err(err) => {
                    degraded = true;
                    last_err = Some(err.to_api_error());
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            ApiError::new(ErrorCode::NoData, "没有可用的数据源")
        }))
    }};
}

/// 降级链聚合器。
pub struct Aggregator {
    providers: Vec<Box<dyn Provider>>,
}

impl Aggregator {
    /// 按 `priority` 升序排列后构造。
    pub fn new(mut providers: Vec<Box<dyn Provider>>) -> Self {
        providers.sort_by_key(|p| p.priority());
        Self { providers }
    }

    /// 是否没有任何 Provider（S1 阶段用于自检与测试）。
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

    pub async fn live(&self) -> Result<AggregateResult<Match>, ApiError> {
        try_providers!(self, live)
    }

    pub async fn upcoming(
        &self,
        hours: u32,
        tournament: Option<&str>,
    ) -> Result<AggregateResult<Match>, ApiError> {
        try_providers!(self, upcoming, hours, tournament)
    }

    pub async fn recent(&self, hours: u32) -> Result<AggregateResult<Match>, ApiError> {
        try_providers!(self, recent, hours)
    }

    pub async fn today(&self, date: Option<&str>) -> Result<AggregateResult<Match>, ApiError> {
        try_providers!(self, today, date)
    }

    pub async fn team_matches(
        &self,
        team: &str,
        limit: u32,
    ) -> Result<AggregateResult<Match>, ApiError> {
        try_providers!(self, team_matches, team, limit)
    }

    pub async fn match_detail(
        &self,
        match_id: &str,
    ) -> Result<AggregateResult<Match>, ApiError> {
        try_providers!(self, match_detail, match_id)
    }

    pub async fn ranking(&self, top: u32) -> Result<AggregateResult<Ranking>, ApiError> {
        try_providers!(self, ranking, top)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let agg = Aggregator::new(vec![Box::new(EmptyProvider)]);
        let err = agg.live().await.err();
        assert_eq!(err.map(|e| e.code), Some(ErrorCode::NoData));
    }

    #[tokio::test]
    async fn no_providers_returns_no_data() {
        let agg = Aggregator::new(vec![]);
        assert!(agg.is_empty());
        assert!(agg.live().await.is_err());
    }
}
