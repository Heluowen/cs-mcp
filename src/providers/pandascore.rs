//! PandaScore 数据源（**主源**）。
//!
//! 实测（2026-11，token 有效，端点均 200）：
//! - `GET /csgo/matches/running|upcoming|past`、`GET /csgo/matches/{id}`、`GET /csgo/teams?search[name]=`
//! - `id` 是 **int** → 对外统一 `pandascore:<id>` 前缀式 ID
//! - `status`：`not_started` / `running` / `finished`（另有 canceled 等，一律丢弃不猜测）
//! - `begin_at`：RFC3339 UTC（`2026-11-07T14:00:00Z`），统一转 +08:00
//! - `streams_list`：**数组**，元素 `{main, raw_url, embed_url, language}`，可直接映射
//! - **`games[]` 不含地图名与逐图比分**（只有 `position/status/winner`）
//!   → `Match.maps` 恒为空。这是数据源能力边界，**不得编造**。
//!
//! 设计取舍：只打「列表端点 + per_page」，时间窗口与赛事过滤在本地完成，
//! 避免依赖 `filter[...]` / `range[...]` 的语法细节（版本间易变）。

use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
use reqwest::Client;
use serde::{Deserialize, Deserializer};

use crate::aggregator::Provider;
use crate::config::Config;
use crate::error::ProviderError;
use crate::models::{Match, Status, StreamLink, Team, Tournament};

pub const NAME: &str = "pandascore";
pub const PRIORITY: u8 = 10;
pub const BASE_URL: &str = "https://api.pandascore.co";

/// 把 `null` 或缺失都当作空集合。
///
/// 踩过的坑：PandaScore 对 `streams_list` / `results` / `opponents` 可能返回 `null`
/// 而非 `[]`，若字段声明为 `Vec<T>`，serde 会直接报
/// `error decoding response body`，整条链路失败。
fn null_to_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

// ------------------------------------------------------------------ DTO

#[derive(Debug, Deserialize)]
struct PsTeamRef {
    id: i64,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    acronym: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PsOpponent {
    opponent: Option<PsTeamRef>,
}

#[derive(Debug, Deserialize)]
struct PsResult {
    team_id: Option<i64>,
    score: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct PsStream {
    /// 主流源标记，用于排序（PandaScore 会同时给出多个镜像）。
    #[serde(default)]
    main: bool,
    #[serde(default)]
    raw_url: Option<String>,
    #[serde(default)]
    embed_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PsTournament {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    tier: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PsSerie {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PsLeague {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PsMatch {
    id: i64,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    begin_at: Option<String>,
    #[serde(default)]
    scheduled_at: Option<String>,
    #[serde(default)]
    number_of_games: Option<u32>,
    #[serde(default, deserialize_with = "null_to_default")]
    opponents: Vec<PsOpponent>,
    #[serde(default, deserialize_with = "null_to_default")]
    results: Vec<PsResult>,
    #[serde(default, deserialize_with = "null_to_default")]
    streams_list: Vec<PsStream>,
    #[serde(default)]
    tournament: Option<PsTournament>,
    #[serde(default)]
    serie: Option<PsSerie>,
    #[serde(default)]
    league: Option<PsLeague>,
}

#[derive(Debug, Deserialize)]
struct PsTeamHit {
    id: i64,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    slug: Option<String>,
}

// ------------------------------------------------------------------ 类型

/// PandaScore 数据源。
pub struct PandaScoreProvider {
    client: Client,
    token: Option<String>,
    /// 展示时区，恒为 +08:00。
    tz: FixedOffset,
    /// 按 host 限流：PandaScore 免费档约 1 QPS。
    limiter: Option<crate::ratelimit::HostLimiter>,
}

// ------------------------------------------------------------------ impl

impl PandaScoreProvider {
    /// 仅在配置了非空 token 时返回 `Some`（无 token 时不构造，实时类 tool 返回 `NO_DATA`）。
    pub fn new(cfg: &Config) -> Option<Self> {
        let token = cfg
            .pandascore_token
            .clone()
            .filter(|t| !t.trim().is_empty())?;

        let mut builder = Client::builder()
            .timeout(cfg.http_timeout())
            // 连接阶段快速失败，避免整体超时被建连耗时吃掉。
            .connect_timeout(std::time::Duration::from_secs(3));
        if let Some(proxy) = cfg.http_proxy.as_deref() {
            if let Ok(p) = reqwest::Proxy::all(proxy) {
                builder = builder.proxy(p);
            }
        }
        let client = builder.build().ok()?;
        let tz = Config::cn_offset().ok()?;

        Some(Self {
            client,
            token: Some(token),
            tz,
            // 1 QPS、允许 3 次突发（一次 tool 调用可能连打 2–3 个端点）。
            limiter: crate::ratelimit::HostLimiter::per_second(1, 3),
        })
    }

    /// 软限流：拿不到令牌时最多等待 2 秒（4 × 500ms）后再放行。
    ///
    /// 硬拒绝会让用户请求无谓失败，而缓存已经在绝大多数情况下避免重复打上游；
    /// 这里的目标是「限制突发」而不是「限死」。
    async fn throttle(&self) {
        let Some(limiter) = self.limiter.as_ref() else {
            return;
        };
        if limiter.try_acquire() {
            return;
        }
        for _ in 0..4 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if limiter.try_acquire() {
                return;
            }
        }
        tracing::debug!("PandaScore 限流令牌等待超时，放行本次请求");
    }

    fn token(&self) -> Result<&str, ProviderError> {
        self.token.as_deref().ok_or_else(|| {
            ProviderError::Auth("未配置 PANDASCORE_TOKEN，实时赛事数据不可用".to_string())
        })
    }

    /// 统一处理 HTTP 状态码与网络错误，映射为领域错误。
    fn check_status(status: reqwest::StatusCode, path: &str) -> Result<(), ProviderError> {
        if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            return Err(ProviderError::Auth(format!(
                "PandaScore 拒绝鉴权（HTTP {status}），请检查 token 是否有效或已过期"
            )));
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ProviderError::RateLimited {
                retry_after: None,
                detail: format!("PandaScore 限流（HTTP 429，{path}）"),
            });
        }
        if status.is_server_error() {
            return Err(ProviderError::Upstream(format!(
                "PandaScore 上游故障（HTTP {status}，{path}）"
            )));
        }
        if !status.is_success() {
            return Err(ProviderError::Upstream(format!(
                "PandaScore 返回异常状态（HTTP {status}，{path}）"
            )));
        }
        Ok(())
    }

    async fn fetch_list(&self, path: &str) -> Result<Vec<PsMatch>, ProviderError> {
        // 这个网络环境下大响应体偶发读取中断（表现为
        // `error decoding response body`），因此对可重试错误做一次短退避重试。
        let mut last_err: Option<ProviderError> = None;
        for attempt in 0..2 {
            match self.fetch_list_once(path).await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    let retryable = matches!(e, ProviderError::Upstream(_));
                    last_err = Some(e);
                    if !retryable || attempt == 1 {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                }
            }
        }
        Err(last_err.unwrap_or_else(|| ProviderError::Upstream(format!("请求 {path} 失败"))))
    }

    async fn fetch_list_once(&self, path: &str) -> Result<Vec<PsMatch>, ProviderError> {
        self.throttle().await;
        let url = format!("{BASE_URL}{path}");
        let resp = self
            .client
            .get(&url)
            .bearer_auth(self.token()?)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(format!("请求 {path} 失败: {}", describe(&e))))?;

        Self::check_status(resp.status(), path)?;

        resp.json::<Vec<PsMatch>>()
            .await
            .map_err(|e| ProviderError::Upstream(format!("解析 {path} 响应失败: {}", describe(&e))))
    }

    /// 单场端点返回的是**对象**而非数组。
    async fn fetch_single(&self, path: &str) -> Result<Option<PsMatch>, ProviderError> {
        self.throttle().await;
        let url = format!("{BASE_URL}{path}");
        let resp = self
            .client
            .get(&url)
            .bearer_auth(self.token()?)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(format!("请求 {path} 失败: {e}")))?;

        // 403 在此端点通常表示「该比赛不在当前数据范围内」（与 token 无关），
        // 按「查不到」处理，避免误报 AUTH_ERROR。
        if resp.status() == reqwest::StatusCode::NOT_FOUND
            || resp.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Ok(None);
        }
        Self::check_status(resp.status(), path)?;

        resp.json::<PsMatch>()
            .await
            .map(Some)
            .map_err(|e| ProviderError::Upstream(format!("解析 {path} 响应失败: {e}")))
    }

    /// DTO → 统一契约。无法映射的条目（canceled/postponed/队伍不全）返回 `None`。
    fn to_match(&self, m: &PsMatch) -> Option<Match> {
        let status = match m.status.as_deref() {
            Some("not_started") => Status::Upcoming,
            Some("running") => Status::Live,
            Some("finished") => Status::Finished,
            _ => return None,
        };

        let begin_raw = m.begin_at.as_deref().or(m.scheduled_at.as_deref())?;
        let begin = DateTime::parse_from_rfc3339(begin_raw).ok()?;
        let starts_at = begin.with_timezone(&self.tz);

        let mut teams: Vec<Team> = Vec::new();
        for opp in &m.opponents {
            let Some(t) = opp.opponent.as_ref() else {
                continue;
            };
            let name = t
                .name
                .clone()
                .or_else(|| t.acronym.clone())
                .unwrap_or_else(|| format!("team:{}", t.id));
            let score = m
                .results
                .iter()
                .find(|r| r.team_id == Some(t.id))
                .and_then(|r| r.score)
                .and_then(|s| u32::try_from(s).ok());
            teams.push(Team {
                name_zh: crate::aliases::zh_name(&name).map(str::to_string),
                name,
                score,
            });
        }

        if teams.len() < 2 {
            return None;
        }

        let tournament_name = m
            .tournament
            .as_ref()
            .and_then(|t| t.name.clone())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| m.serie.as_ref().and_then(|s| s.name.clone()))
            .or_else(|| m.league.as_ref().and_then(|l| l.name.clone()))
            .unwrap_or_else(|| "未知赛事".to_string());

        let tier = m
            .tournament
            .as_ref()
            .and_then(|t| t.tier.clone())
            .map(|t| t.trim().to_uppercase())
            .filter(|t| !t.is_empty());

        let mut streams: Vec<StreamLink> = m
            .streams_list
            .iter()
            .filter_map(|s| {
                let url = s.raw_url.clone().or_else(|| s.embed_url.clone())?;
                Some(StreamLink {
                    platform: platform_of(&url),
                    url,
                })
            })
            .collect();
        // 主流源排前面（PandaScore 给的顺序不稳定）。
        {
            let mains: Vec<bool> = m.streams_list.iter().map(|s| s.main).collect();
            if mains.len() == streams.len() {
                let mut paired: Vec<(bool, StreamLink)> = mains.into_iter().zip(streams).collect();
                paired.sort_by_key(|(is_main, _)| !*is_main);
                streams = paired.into_iter().map(|(_, s)| s).collect();
            }
        }

        let fetched_at = Utc::now().with_timezone(&self.tz);

        Some(Match {
            id: format!("pandascore:{}", m.id),
            status,
            starts_at,
            tournament: Tournament {
                name: tournament_name,
                tier,
                // PandaScore 的比赛载荷里没有赛事地区字段，留空而不是猜。
                region: None,
            },
            bo: m.number_of_games,
            teams,
            // 数据源不给逐图信息。
            maps: Vec::new(),
            streams,
            source: NAME.to_string(),
            fetched_at,
        })
    }

    fn map_all(&self, raw: &[PsMatch]) -> Vec<Match> {
        raw.iter().filter_map(|m| self.to_match(m)).collect()
    }

    async fn resolve_team_id(&self, input: &str) -> Result<i64, ProviderError> {
        let canonical = crate::aliases::canonical_name(input).unwrap_or(input).to_string();
        let path = format!("/csgo/teams?search[name]={}&per_page=5", urlencode(&canonical));

        self.throttle().await;
        let url = format!("{BASE_URL}{path}");
        let resp = self
            .client
            .get(&url)
            .bearer_auth(self.token()?)
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(format!("查询战队失败: {e}")))?;
        Self::check_status(resp.status(), "/csgo/teams")?;

        let hits: Vec<PsTeamHit> = resp
            .json()
            .await
            .map_err(|e| ProviderError::Upstream(format!("解析战队响应失败: {e}")))?;

        let wanted = canonical.to_lowercase();
        let hit = hits
            .iter()
            .find(|t| {
                t.name.as_deref().map(|n| n.to_lowercase() == wanted).unwrap_or(false)
                    || t.slug
                        .as_deref()
                        .map(|s| s.to_lowercase() == wanted)
                        .unwrap_or(false)
            })
            .or_else(|| hits.first())
            .ok_or_else(|| ProviderError::NoData(format!("找不到战队: {input}")))?;

        Ok(hit.id)
    }
}

/// 把 reqwest 错误连同底层 source 链一起打印，并标注类别。
///
/// 只看 `Display` 会把「serde 解析失败」和「响应体读取中断」混为一谈
/// （两者都显示 `error decoding response body`），排障时必须看到根因。
fn describe(e: &reqwest::Error) -> String {
    let mut out = e.to_string();

    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        out.push_str(" <- ");
        out.push_str(&s.to_string());
        src = s.source();
    }

    if e.is_timeout() {
        out.push_str(" [timeout]");
    }
    if e.is_connect() {
        out.push_str(" [connect]");
    }
    if e.is_body() {
        out.push_str(" [body]");
    }
    if e.is_decode() {
        out.push_str(" [decode]");
    }
    out
}

/// 从直播 URL 推断平台名。
fn platform_of(url: &str) -> String {
    let lower = url.to_lowercase();
    if lower.contains("twitch.tv") {
        "twitch".to_string()
    } else if lower.contains("youtube.com") || lower.contains("youtu.be") {
        "youtube".to_string()
    } else if lower.contains("bilibili.com") {
        "bilibili".to_string()
    } else if lower.contains("huya.com") {
        "huya".to_string()
    } else if lower.contains("douyu.com") {
        "douyu".to_string()
    } else {
        "web".to_string()
    }
}

/// 极简 URL 编码（查询参数用）。
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[async_trait]
impl Provider for PandaScoreProvider {
    fn name(&self) -> &'static str {
        NAME
    }

    fn priority(&self) -> u8 {
        PRIORITY
    }

    async fn live(&self) -> Result<Vec<Match>, ProviderError> {
        let running = self.fetch_list("/csgo/matches/running?per_page=50").await?;
        let mut out = self.map_all(&running);

        let upcoming = self
            .fetch_list("/csgo/matches/upcoming?filter[status]=not_started&per_page=50")
            .await
            .unwrap_or_default();
        let now = Utc::now();
        let horizon = now + chrono::Duration::minutes(30);
        for m in upcoming.iter() {
            let Some(begin_raw) = m.begin_at.as_deref() else {
                continue;
            };
            let Ok(begin) = DateTime::parse_from_rfc3339(begin_raw) else {
                continue;
            };
            if begin >= now && begin <= horizon {
                if let Some(mapped) = self.to_match(m) {
                    out.push(mapped);
                }
            }
        }

        if out.is_empty() {
            return Err(ProviderError::NoData(
                "当前没有正在进行或 30 分钟内开赛的比赛".to_string(),
            ));
        }
        Ok(out)
    }

    async fn upcoming(
        &self,
        hours: u32,
        tournament: Option<&str>,
    ) -> Result<Vec<Match>, ProviderError> {
        let raw = self
            .fetch_list("/csgo/matches/upcoming?filter[status]=not_started&per_page=50")
            .await?;
        let now = Utc::now();
        let horizon = now + chrono::Duration::hours(i64::from(hours));
        let needle = tournament.map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty());

        let mut filtered: Vec<Match> = self
            .map_all(&raw)
            .into_iter()
            .filter(|m| m.status == Status::Upcoming)
            .filter(|m| {
                let ts = m.starts_at.with_timezone(&Utc);
                ts >= now && ts <= horizon
            })
            .filter(|m| match &needle {
                Some(n) => m.tournament.name.to_lowercase().contains(n),
                None => true,
            })
            .collect();

        filtered.sort_by_key(|m| m.starts_at);

        if filtered.is_empty() {
            return Err(ProviderError::NoData(format!(
                "未来 {hours} 小时内没有符合条件的赛程"
            )));
        }
        Ok(filtered)
    }

    async fn recent(&self, hours: u32) -> Result<Vec<Match>, ProviderError> {
        // 关键：必须服务端过滤 status=finished 并按 -scheduled_at 排序。
        // `/past` 原始列表里大量是 `canceled` 且 `begin_at` 为 null 的脏数据，
        // 按 `-begin_at` 排序会把它们排在最前，导致「最近赛果」永远是空。
        let raw = self
            .fetch_list("/csgo/matches/past?filter[status]=finished&per_page=50&sort=-scheduled_at")
            .await?;
        let now = Utc::now();
        let from = now - chrono::Duration::hours(i64::from(hours));

        let mut filtered: Vec<Match> = self
            .map_all(&raw)
            .into_iter()
            .filter(|m| m.status == Status::Finished)
            .filter(|m| {
                // 近似：用开赛时间判断窗口（载荷里 end_at 不稳定，避免误伤）。
                let ts = m.starts_at.with_timezone(&Utc);
                ts >= from && ts <= now
            })
            .collect();

        filtered.sort_by_key(|m| std::cmp::Reverse(m.starts_at));

        if filtered.is_empty() {
            return Err(ProviderError::NoData(format!(
                "最近 {hours} 小时内没有已结束的比赛"
            )));
        }
        Ok(filtered)
    }

    async fn today(&self, date: Option<&str>) -> Result<Vec<Match>, ProviderError> {
        let target = match date {
            Some(d) => NaiveDate::parse_from_str(d.trim(), "%Y-%m-%d")
                .map_err(|_| ProviderError::NoData(format!("日期格式应为 YYYY-MM-DD，收到 {d}")))?,
            None => Utc::now().with_timezone(&self.tz).date_naive(),
        };

        let mut out: Vec<Match> = Vec::new();

        let up = self
            .fetch_list("/csgo/matches/upcoming?filter[status]=not_started&per_page=50")
            .await?;
        out.extend(
            self.map_all(&up)
                .into_iter()
                .filter(|m| m.starts_at.date_naive() == target),
        );

        let past = self
            .fetch_list("/csgo/matches/past?filter[status]=finished&per_page=50&sort=-scheduled_at")
            .await
            .unwrap_or_default();
        out.extend(
            self.map_all(&past)
                .into_iter()
                .filter(|m| m.status == Status::Finished && m.starts_at.date_naive() == target),
        );

        out.sort_by_key(|m| m.starts_at);

        if out.is_empty() {
            return Err(ProviderError::NoData(format!("{target} 没有 CS2 赛程")));
        }
        Ok(out)
    }

    async fn team_matches(&self, team: &str, limit: u32) -> Result<Vec<Match>, ProviderError> {
        let team_id = self.resolve_team_id(team).await?;
        let lim = limit as usize;

        let up_raw = self
            .fetch_list(&format!(
                "/csgo/matches/upcoming?filter[opponent_id]={team_id}&filter[status]=not_started&per_page={limit}"
            ))
            .await?;
        let mut futures: Vec<Match> = self.map_all(&up_raw);
        futures.sort_by_key(|m| m.starts_at);
        futures.truncate(lim);

        let past_raw = self
            .fetch_list(&format!(
                "/csgo/matches/past?filter[opponent_id]={team_id}&filter[status]=finished&per_page={limit}&sort=-scheduled_at"
            ))
            .await?;
        let mut pasts: Vec<Match> = self.map_all(&past_raw);
        pasts.sort_by_key(|m| std::cmp::Reverse(m.starts_at));
        pasts.truncate(lim);

        let mut merged = futures;
        merged.extend(pasts);

        if merged.is_empty() {
            return Err(ProviderError::NoData(format!("{team} 没有可查的比赛")));
        }
        Ok(merged)
    }

    async fn match_detail(&self, match_id: &str) -> Result<Vec<Match>, ProviderError> {
        let trimmed = match_id.trim();
        let raw_id = trimmed.strip_prefix("pandascore:").unwrap_or(trimmed);

        if raw_id.is_empty() || !raw_id.chars().all(|c| c.is_ascii_digit()) {
            return Err(ProviderError::NoData(format!(
                "match_id 需形如 pandascore:<数字>，收到 {match_id}"
            )));
        }

        // 官方比赛 ID 可能不在 csgo 列表端点覆盖范围内；单场端点直接返回对象。
        let single = self.fetch_single(&format!("/csgo/matches/{raw_id}")).await?;

        match single.and_then(|m| self.to_match(&m)) {
            Some(m) => Ok(vec![m]),
            None => Err(ProviderError::NoData(format!(
                "找不到比赛 {match_id}（或该比赛状态不受支持）"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        Config {
            pandascore_token: Some("dummy".to_string()),
            enable_hltv_scrape: false,
            tz: "Asia/Shanghai".to_string(),
            http_proxy: None,
            http_timeout_secs: 5,
            cache_ttl_schedule: 120,
            cache_ttl_live: 30,
            cache_ttl_rank: 3600,
            mcp_transport: "stdio".to_string(),
            mcp_bind: "0.0.0.0:8080".to_string(),
            global_qps: 2,
        }
    }

    #[test]
    fn platform_detection() {
        assert_eq!(platform_of("https://www.twitch.tv/esl_csgo"), "twitch");
        assert_eq!(platform_of("https://www.youtube.com/@x/live"), "youtube");
        assert_eq!(platform_of("https://live.bilibili.com/123"), "bilibili");
        assert_eq!(platform_of("https://unknown.example/x"), "web");
    }

    #[test]
    fn urlencode_handles_space_and_unicode() {
        assert_eq!(urlencode("Team Spirit"), "Team%20Spirit");
        assert_eq!(urlencode("NAVI"), "NAVI");
        assert!(urlencode("天禄").starts_with('%'));
    }

    #[test]
    fn dto_maps_to_contract() {
        let json = r#"{
            "id": 1795385,
            "status": "not_started",
            "begin_at": "2026-11-07T14:00:00Z",
            "number_of_games": 3,
            "opponents": [
                {"opponent": {"id": 1, "name": "LPH Gaming", "acronym": "LPH"}},
                {"opponent": {"id": 2, "name": "aimclub", "acronym": "AIMC"}}
            ],
            "results": [],
            "streams_list": [
                {"main": false, "raw_url": "https://www.twitch.tv/mirror"},
                {"main": true, "raw_url": "https://www.youtube.com/@x/live"}
            ],
            "tournament": {"name": "Group D", "tier": "c"},
            "serie": {"name": "Europe Cup #11"},
            "league": {"name": "ESL Challenger League"}
        }"#;

        let parsed: Result<PsMatch, _> = serde_json::from_str(json);
        assert!(parsed.is_ok(), "反序列化失败");
        let Ok(ps) = parsed else { return };

        let provider = match PandaScoreProvider::new(&test_config()) {
            Some(p) => p,
            None => return,
        };

        let mapped = provider.to_match(&ps);
        assert!(mapped.is_some(), "应能映射为 Match");
        let Some(m) = mapped else { return };

        assert_eq!(m.id, "pandascore:1795385");
        assert_eq!(m.status, Status::Upcoming);
        assert_eq!(m.bo, Some(3));
        assert_eq!(m.teams.len(), 2);
        assert_eq!(m.teams[0].name, "LPH Gaming");
        assert!(m.teams[0].score.is_none(), "未开赛不应有比分");
        assert_eq!(m.tournament.name, "Group D");
        assert_eq!(m.tournament.tier.as_deref(), Some("C"));
        assert_eq!(m.tournament.region, None, "无地区字段时不得猜测");
        assert_eq!(m.streams.len(), 2);
        assert_eq!(m.streams[0].platform, "youtube", "main 流源应排第一");
        assert!(m.maps.is_empty(), "数据源无逐图信息，必须留空");
        assert_eq!(m.source, "pandascore");
        // 14:00Z → 22:00 +08:00
        assert_eq!(
            m.starts_at.format("%Y-%m-%dT%H:%M").to_string(),
            "2026-11-07T22:00"
        );
    }

    #[test]
    fn canceled_status_is_dropped() {
        let provider = match PandaScoreProvider::new(&test_config()) {
            Some(p) => p,
            None => return,
        };
        let ps = PsMatch {
            id: 1,
            status: Some("canceled".to_string()),
            begin_at: Some("2026-11-07T14:00:00Z".to_string()),
            scheduled_at: None,
            number_of_games: Some(3),
            opponents: Vec::new(),
            results: Vec::new(),
            streams_list: Vec::new(),
            tournament: None,
            serie: None,
            league: None,
        };
        assert!(provider.to_match(&ps).is_none());
    }
}
