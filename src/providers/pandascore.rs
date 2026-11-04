//! PandaScore 数据源（**主源**）。
//!
//! 实测结论（2026-10）：
//! - `GET https://api.pandascore.co/csgo/matches/running` 返回 **401**（缺 token），
//!   说明端点存活、网络可达，**不是被封锁**。
//! - 它是目前唯一同时提供「实时比分 + 逐图 + 直播源」的来源。
//!
//! S2 实现要点：
//! - 端点：`/csgo/matches/running`、`/csgo/matches/upcoming`、`/csgo/matches/past`
//! - 鉴权：`Authorization: Bearer <PANDASCORE_TOKEN>`
//! - 限流：≤1 QPS（见 `ratelimit::Limiters::pandascore`）
//! - `match_id` 命名空间：`pandascore:<id>`
//! - 无 token 时不构造本 Provider，`cs_live_matches` 直接返回 `NO_DATA`
//!   （**绝不**用其它源伪造实时比分）

use async_trait::async_trait;
use reqwest::Client;

use crate::aggregator::Provider;
use crate::config::Config;

pub const NAME: &str = "pandascore";
pub const PRIORITY: u8 = 10;
pub const BASE_URL: &str = "https://api.pandascore.co";

pub struct PandaScoreProvider {
    /// S2 使用。
    #[allow(dead_code)]
    client: Client,
    /// S2 使用。
    #[allow(dead_code)]
    token: Option<String>,
}

impl PandaScoreProvider {
    /// 仅在配置了非空 token 时返回 `Some`。
    pub fn new(cfg: &Config) -> Option<Self> {
        let token = cfg
            .pandascore_token
            .clone()
            .filter(|t| !t.trim().is_empty())?;

        let mut builder = Client::builder().timeout(cfg.http_timeout());
        if let Some(proxy) = cfg.http_proxy.as_deref() {
            if let Ok(p) = reqwest::Proxy::all(proxy) {
                builder = builder.proxy(p);
            }
        }
        let client = builder.build().ok()?;
        Some(Self {
            client,
            token: Some(token),
        })
    }

    /// 端基地址（S2 使用）。
    #[allow(dead_code)]
    pub fn base_url(&self) -> &'static str {
        BASE_URL
    }
}

#[async_trait]
impl Provider for PandaScoreProvider {
    fn name(&self) -> &'static str {
        NAME
    }

    fn priority(&self) -> u8 {
        PRIORITY
    }
}
