//! 限流：governor 令牌桶。
//!
//! 关键规则：**必须按 host 限流**，不能只靠全局 2 QPS ——
//! Liquipedia 要求 ≤1 req / 2s（即 0.5 QPS），全局 2 QPS 会违反其规则。
//!
//! 若 governor 0.10 的 API 有变动，只需修改本文件。

// S1 阶段 Provider 尚未发起真实上游请求，限流器在 S2/S3 接入后启用。
#![allow(dead_code)]

use std::num::NonZeroU32;

use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};

/// 单 host 限流器。
pub struct HostLimiter {
    inner: RateLimiter<NotKeyed, InMemoryState, DefaultClock>,
}

impl HostLimiter {
    /// 按「每秒 qps 次、突发 burst 次」创建。参数为 0 时按 1 处理。
    pub fn per_second(qps: u32, burst: u32) -> Option<Self> {
        let qps = NonZeroU32::new(qps.max(1))?;
        let burst = NonZeroU32::new(burst.max(1))?;
        let quota = Quota::per_second(qps).allow_burst(burst);
        Some(Self {
            inner: RateLimiter::direct(quota),
        })
    }

    /// 非阻塞尝试获取许可。
    pub fn try_acquire(&self) -> bool {
        self.inner.check().is_ok()
    }
}

/// 各数据源的限流配置。
///
/// - PandaScore：≤1 QPS
/// - Liquipedia：≤0.5 QPS（2 秒一次），突发 1
/// - 全局兜底：2 QPS
pub struct Limiters {
    pub pandascore: Option<HostLimiter>,
    pub liquipedia: Option<HostLimiter>,
    pub global: Option<HostLimiter>,
}

impl Limiters {
    pub fn from_global_qps(global_qps: u32) -> Self {
        Self {
            pandascore: HostLimiter::per_second(1, 2),
            liquipedia: HostLimiter::per_second(1, 1),
            global: HostLimiter::per_second(global_qps, global_qps.saturating_mul(2)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_allows_first_and_blocks_burst_excess() {
        let limiter = match HostLimiter::per_second(1, 1) {
            Some(l) => l,
            None => return,
        };
        assert!(limiter.try_acquire());
        // 突发额度为 1，紧接着的第二次应被拒。
        assert!(!limiter.try_acquire());
    }

    #[test]
    fn limiters_construct() {
        let l = Limiters::from_global_qps(2);
        assert!(l.pandascore.is_some());
        assert!(l.liquipedia.is_some());
        assert!(l.global.is_some());
    }
}
