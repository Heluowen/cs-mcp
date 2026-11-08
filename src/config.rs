//! 配置：环境变量 + .env，字段名显式映射到 `SCREAMING_SNAKE_CASE` 变量名。

use std::time::Duration;

use anyhow::{anyhow, Context};
use chrono::{DateTime, FixedOffset, Utc};
use serde::Deserialize;

/// 中国标准时间偏移（秒）。仅允许 +08:00。
const CN_OFFSET_SECS: i32 = 8 * 3600;

fn default_tz() -> String {
    "Asia/Shanghai".to_string()
}
/// 上游请求总超时（秒）。
///
/// 实测：`/csgo/matches/upcoming?per_page=100` 的响应体在这种网络下要 3–8 秒才能读完，
/// 5 秒会把大分页误判为 timeout（表现为 `error decoding response body [timeout]`），
/// 因此默认给到 15 秒；单连接建立仍由 `connect_timeout` 3 秒快速失败。
fn default_http_timeout() -> u64 {
    15
}
fn default_ttl_schedule() -> u64 {
    120
}
fn default_ttl_live() -> u64 {
    30
}
fn default_ttl_rank() -> u64 {
    3600
}
fn default_transport() -> String {
    "stdio".to_string()
}
fn default_bind() -> String {
    "0.0.0.0:8080".to_string()
}
fn default_global_qps() -> u32 {
    2
}

/// 运行期配置。
///
/// 所有字段都有默认值，因此缺少环境变量不会导致启动失败。
///
/// **重要（踩过的坑）**：这里**不能**用 `#[serde(rename = "SCREAMING_SNAKE")]`。
/// `envy` 会把环境变量名 `to_lowercase()` 后再匹配（见 envy 源码 `Deserializer::new`），
/// 而 `rename` 会让 serde 去找全大写键 —— 永远匹配不上，导致**整个 Config 静默吃默认值**。
/// 因此字段名用 snake_case，靠 envy 的大小写不敏感匹配对应 `PANDASCORE_TOKEN` 这类变量。
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// PandaScore API token。缺失时 `cs_live_matches` 等实时类工具返回 `NO_DATA`。
    #[serde(default)]
    pub pandascore_token: Option<String>,

    /// 是否启用 HLTV 抓取。默认关闭（HLTV 有 Cloudflare 挑战且 ToS 禁止爬取）。
    #[serde(default)]
    pub enable_hltv_scrape: bool,

    /// 展示时区名（仅用于日志/元信息；时间恒为 +08:00）。
    #[serde(default = "default_tz")]
    pub tz: String,

    /// 出站 HTTP 代理，例如 `http://127.0.0.1:7890`。
    #[serde(default)]
    pub http_proxy: Option<String>,

    /// 单次上游请求超时（秒）。
    #[serde(default = "default_http_timeout")]
    pub http_timeout_secs: u64,

    /// 赛程类缓存 TTL（秒）。
    #[serde(default = "default_ttl_schedule")]
    pub cache_ttl_schedule: u64,

    /// 实时比分缓存 TTL（秒）。
    #[serde(default = "default_ttl_live")]
    pub cache_ttl_live: u64,

    /// 排名缓存 TTL（秒）。
    #[serde(default = "default_ttl_rank")]
    pub cache_ttl_rank: u64,

    /// 传输方式：`stdio`（默认）或 `http`。
    #[serde(default = "default_transport")]
    pub mcp_transport: String,

    /// HTTP 模式监听地址。
    #[serde(default = "default_bind")]
    pub mcp_bind: String,

    /// 全局兜底 QPS 上限（各 host 另有独立更严格限制）。
    ///
    /// S3 接入聚合链时由 `ratelimit::Limiters` 读取；S1 阶段仅做配置留存。
    #[serde(default = "default_global_qps")]
    #[allow(dead_code)]
    pub global_qps: u32,
}

impl Config {
    /// 从环境变量（含 `.env`）加载。
    ///
    /// 注意：很多环境（Git Bash / CI / 部分 Windows 终端）会**定义空值**变量，
    /// 例如 `TZ=`。serde 的 `#[serde(default)]` 只在字段**缺失**时生效，
    /// 空串会原样进来，因此这里统一做一次归一化。
    pub fn from_env() -> anyhow::Result<Self> {
        // .env 缺失是正常情况，不作为错误。
        dotenvy::dotenv().ok();

        // 忽略空值变量：`TZ=` 这类「定义了但为空」的变量会绕过 `#[serde(default)]`，
        // 因此先把空值整体丢弃，让默认值生效。
        let vars = std::env::vars().filter(|(_, v)| !v.trim().is_empty());
        let mut cfg: Self = envy::from_iter(vars).context("读取环境变量配置失败")?;

        cfg.normalize();
        Ok(cfg)
    }

    /// 归一化：空字符串 / 纯空白一律回退默认值。
    fn normalize(&mut self) {
        if self.tz.trim().is_empty() {
            self.tz = default_tz();
        }
        if self.mcp_transport.trim().is_empty() {
            self.mcp_transport = default_transport();
        }
        if self.mcp_bind.trim().is_empty() {
            self.mcp_bind = default_bind();
        }
        // token / proxy 的空串等价于「未配置」。
        if self
            .pandascore_token
            .as_deref()
            .map(|s| s.trim().is_empty())
            .unwrap_or(false)
        {
            self.pandascore_token = None;
        }
        if self
            .http_proxy
            .as_deref()
            .map(|s| s.trim().is_empty())
            .unwrap_or(false)
        {
            self.http_proxy = None;
        }
    }

    /// 固定的 +08:00 偏移。8 小时在 `FixedOffset` 合法范围内，理论上不会失败。
    pub fn cn_offset() -> anyhow::Result<FixedOffset> {
        FixedOffset::east_opt(CN_OFFSET_SECS)
            .ok_or_else(|| anyhow!("非法时区偏移: {CN_OFFSET_SECS}s"))
    }

    /// 当前时间（+08:00）。
    pub fn now(offset: FixedOffset) -> DateTime<FixedOffset> {
        Utc::now().with_timezone(&offset)
    }

    pub fn http_timeout(&self) -> Duration {
        Duration::from_secs(self.http_timeout_secs)
    }

    pub fn ttl_schedule(&self) -> Duration {
        Duration::from_secs(self.cache_ttl_schedule)
    }

    pub fn ttl_live(&self) -> Duration {
        Duration::from_secs(self.cache_ttl_live)
    }

    pub fn ttl_rank(&self) -> Duration {
        Duration::from_secs(self.cache_ttl_rank)
    }

    /// 是否具备 PandaScore 访问能力（实时比分的前提）。
    pub fn has_pandascore(&self) -> bool {
        self.pandascore_token
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归用例：环境里 `TZ=`（定义了但为空）不能让 tz 变成空串。
    ///
    /// 这是本机实测踩到的真实 bug：serde 的 `default` 只在字段缺失时生效，
    /// 空串会原样进来，导致 `cs_ping` 输出空时区。
    #[test]
    fn empty_values_fall_back_to_defaults() {
        let json = r#"{"tz":"","mcp_transport":"","mcp_bind":"","pandascore_token":"   ","http_proxy":""}"#;
        let parsed: Result<Config, _> = serde_json::from_str(json);
        assert!(parsed.is_ok(), "配置反序列化失败");
        let mut cfg = match parsed {
            Ok(c) => c,
            Err(_) => return,
        };

        cfg.normalize();

        assert_eq!(cfg.tz, "Asia/Shanghai");
        assert_eq!(cfg.mcp_transport, "stdio");
        assert_eq!(cfg.mcp_bind, "0.0.0.0:8080");
        assert!(cfg.pandascore_token.is_none());
        assert!(cfg.http_proxy.is_none());
    }

    /// 非空配置必须原样保留，不被归一化误伤。
    #[test]
    fn non_empty_values_are_preserved() {
        let json = r#"{"tz":"UTC","pandascore_token":"tok","cache_ttl_live":15}"#;
        let parsed: Result<Config, _> = serde_json::from_str(json);
        assert!(parsed.is_ok(), "配置反序列化失败");
        let mut cfg = match parsed {
            Ok(c) => c,
            Err(_) => return,
        };

        cfg.normalize();

        assert_eq!(cfg.tz, "UTC");
        assert_eq!(cfg.pandascore_token.as_deref(), Some("tok"));
        assert_eq!(cfg.cache_ttl_live, 15);
        assert!(cfg.has_pandascore());
    }

    #[test]
    fn cn_offset_is_eight_hours() {
        let offset = Config::cn_offset();
        assert!(offset.is_ok());
        if let Ok(o) = offset {
            assert_eq!(o.local_minus_utc(), 28_800);
        }
    }

    /// 回归用例：环境变量必须能被 envy 绑定上。
    ///
    /// 踩过的真实 bug：字段上写了 `#[serde(rename = "PANDASCORE_TOKEN")]`，
    /// 而 envy 会把环境变量名 lowercase 后匹配，导致 serde 找全大写键失败、
    /// **整个配置静默回落到默认值**（表现为 `cs_ping` 里 `pandascore_ready=false`）。
    #[test]
    fn env_vars_bind_case_insensitively() {
        let vars: Vec<(String, String)> = vec![
            ("PANDASCORE_TOKEN".to_string(), "abc123".to_string()),
            ("HTTP_TIMEOUT_SECS".to_string(), "9".to_string()),
            ("TZ".to_string(), "Asia/Shanghai".to_string()),
        ];

        let parsed: Result<Config, _> = envy::from_iter(vars);
        assert!(parsed.is_ok(), "envy 解析失败");
        let Ok(cfg) = parsed else { return };

        assert_eq!(cfg.pandascore_token.as_deref(), Some("abc123"));
        assert_eq!(cfg.http_timeout_secs, 9);
        assert_eq!(cfg.tz, "Asia/Shanghai");
        assert!(cfg.has_pandascore(), "读到 token 后应判定具备 PandaScore 能力");
    }
}
