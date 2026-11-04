//! Liquipedia 数据源（**辅助源**）。
//!
//! 实测结论（2026-10）：
//! - 泛化 User-Agent → **HTTP 406**（被拒）
//! - 规范 UA（`产品名/版本 (联系方式邮箱)`）→ **HTTP 200 + gzip**
//! - 要求 ≤1 req / 2s（0.5 QPS），见 `ratelimit::Limiters::liquipedia`
//!
//! 能力边界：**没有实时比分**。用于赛程、赛事信息、阵容、以及**排名**（唯一可用排名源）。
//! `match_id` 命名空间：`liquipedia:<PageName>`。
//! 排名结果必须标记 `Ranking.stale = true`（口径滞后 1–7 天）。

use async_trait::async_trait;
use reqwest::Client;

use crate::aggregator::Provider;
use crate::config::Config;

pub const NAME: &str = "liquipedia";
pub const PRIORITY: u8 = 20;
pub const API_BASE: &str = "https://liquipedia.net/counterstrike/api.php";

/// Liquipedia 要求可识别的 UA，缺省会返回 406。
pub const USER_AGENT: &str =
    "CS2EsportsMCP/0.1 (https://github.com/example/cs-mcp; admin@example.com)";

pub struct LiquipediaProvider {
    /// S2 使用。
    #[allow(dead_code)]
    client: Client,
}

impl LiquipediaProvider {
    pub fn new(cfg: &Config) -> Option<Self> {
        let mut builder = Client::builder()
            .timeout(cfg.http_timeout())
            .user_agent(USER_AGENT);
        if let Some(proxy) = cfg.http_proxy.as_deref() {
            if let Ok(p) = reqwest::Proxy::all(proxy) {
                builder = builder.proxy(p);
            }
        }
        let client = builder.build().ok()?;
        Some(Self { client })
    }

    /// 接口基地址（S4 使用）。
    #[allow(dead_code)]
    pub fn api_base(&self) -> &'static str {
        API_BASE
    }
}

#[async_trait]
impl Provider for LiquipediaProvider {
    fn name(&self) -> &'static str {
        NAME
    }

    fn priority(&self) -> u8 {
        PRIORITY
    }
}
