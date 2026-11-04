//! HLTV 数据源（**默认禁用**）。
//!
//! 实测结论（2026-10）：`GET https://www.hltv.org/matches` 返回
//! **HTTP 403 + 响应头 `cf-mitigated: challenge`**，响应体是 Cloudflare 的
//! `Just a moment...` 挑战页。这意味着：
//!
//! 1. 任何普通 HTTP 客户端（`reqwest` / `reqwest + scraper`）**结构性拿不到数据**，
//!    不是换 UA 或加重试能解决的；
//! 2. 重试只会推高 IP 风险分，最终变成 1015/1020 封禁；
//! 3. HLTV ToS（clause 2.2）**禁止网页抓取/数据挖掘**。
//!
//! 因此本 Provider 默认不构造（`ENABLE_HLTV_SCRAPE=0`）。若开启，也只做
//! 「尽力而为」的一次性尝试，失败即降级，绝不重试、绝不解析挑战页。
//!
//! 若未来必须拿到 HLTV 原始数据，正确路径是 TLS 指纹伪装 sidecar 或代理，
//! 而不是在本 crate 里硬撸。

use async_trait::async_trait;
use reqwest::Client;

use crate::aggregator::Provider;
use crate::config::Config;

pub const NAME: &str = "hltv";
pub const PRIORITY: u8 = 30;
pub const MATCHES_URL: &str = "https://www.hltv.org/matches";

pub struct HltvProvider {
    /// S6 使用（可选抓取）。
    #[allow(dead_code)]
    client: Client,
}

impl HltvProvider {
    pub fn new(cfg: &Config) -> Option<Self> {
        let mut builder = Client::builder().timeout(cfg.http_timeout());
        if let Some(proxy) = cfg.http_proxy.as_deref() {
            if let Ok(p) = reqwest::Proxy::all(proxy) {
                builder = builder.proxy(p);
            }
        }
        let client = builder.build().ok()?;
        Some(Self { client })
    }

    /// 抓取目标地址（S6 使用）。
    #[allow(dead_code)]
    pub fn matches_url(&self) -> &'static str {
        MATCHES_URL
    }
}

#[async_trait]
impl Provider for HltvProvider {
    fn name(&self) -> &'static str {
        NAME
    }

    fn priority(&self) -> u8 {
        PRIORITY
    }
}
