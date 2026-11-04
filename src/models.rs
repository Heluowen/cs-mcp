//! 统一数据契约。
//!
//! 铁律 4：数据源可降级，字段统一，上层不感知来源。
//! 因此所有 Provider 必须产出本模块的类型；`source` 字段记录真实来源，便于上层如实标注。

use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;

/// 比赛状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Upcoming,
    Live,
    Finished,
}

/// 赛事信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tournament {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
}

/// 参赛队伍。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub name: String,
    /// 中文名。缺省为 `null`，**禁止猜译**。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_zh: Option<String>,
    /// 当前比分。未开赛为 `null`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<u32>,
}

/// 单图比分。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapResult {
    pub name: String,
    /// `[team1, team2]`
    pub score: [u32; 2],
}

/// 直播源。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamLink {
    pub platform: String,
    pub url: String,
}

/// 一场比赛（统一契约核心类型）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Match {
    /// 前缀式 ID：`pandascore:<id>` / `liquipedia:<PageName>`。
    pub id: String,
    pub status: Status,
    /// RFC3339，固定 +08:00。
    pub starts_at: DateTime<FixedOffset>,
    pub tournament: Tournament,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bo: Option<u32>,
    pub teams: Vec<Team>,
    pub maps: Vec<MapResult>,
    pub streams: Vec<StreamLink>,
    /// 真实来源域名，上层必须如实标注。
    pub source: String,
    /// 抓取时间，RFC3339，+08:00。
    pub fetched_at: DateTime<FixedOffset>,
}

/// 排名条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankingEntry {
    pub rank: u32,
    pub team: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub points: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<i32>,
}

/// 一份排名表。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ranking {
    /// `world` / `vrs`
    pub kind: String,
    pub entries: Vec<RankingEntry>,
    pub source: String,
    pub fetched_at: DateTime<FixedOffset>,
    /// 数据源已知滞后（Liquipedia 排名口径）。
    pub stale: bool,
}

/// 响应元信息。
///
/// `cached` 与 `elapsed_ms` 是「30s 内命中缓存」验收的唯一可观测证据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    pub count: usize,
    pub sources_used: Vec<String>,
    pub degraded: bool,
    pub cached: bool,
    pub elapsed_ms: u64,
}

impl Meta {
    pub fn new(count: usize, sources_used: Vec<String>, degraded: bool) -> Self {
        Self {
            count,
            sources_used,
            degraded,
            cached: false,
            elapsed_ms: 0,
        }
    }

    pub fn with_timing(mut self, cached: bool, elapsed_ms: u64) -> Self {
        self.cached = cached;
        self.elapsed_ms = elapsed_ms;
        self
    }
}

/// 成功响应：`{"ok":true,"data":[...],"meta":{...}}`
#[derive(Debug, Clone, Serialize)]
pub struct ApiSuccess<T> {
    pub ok: bool,
    pub data: Vec<T>,
    pub meta: Meta,
}

impl<T> ApiSuccess<T> {
    pub fn new(data: Vec<T>, meta: Meta) -> Self {
        Self {
            ok: true,
            data,
            meta,
        }
    }
}

/// 失败响应：`{"ok":false,"error":{"code":"...","message":"..."}}`
#[derive(Debug, Clone, Serialize)]
pub struct ApiFailure {
    pub ok: bool,
    pub error: ApiError,
}

impl ApiFailure {
    pub fn new(error: ApiError) -> Self {
        Self { ok: false, error }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_serializes_lowercase() {
        let s = serde_json::to_string(&Status::Live).unwrap_or_default();
        assert_eq!(s, "\"live\"");
    }

    #[test]
    fn success_payload_shape() {
        let meta = Meta::new(0, vec![], false);
        let body = ApiSuccess::<Match>::new(vec![], meta);
        let json = serde_json::to_string(&body).unwrap_or_default();
        assert!(json.contains("\"ok\":true"));
        assert!(json.contains("\"cached\":false"));
        assert!(json.contains("\"elapsed_ms\":0"));
    }
}
