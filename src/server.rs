//! MCP server 主体：`CsServer` 定义与 7 个 tool 的路由。
//!
//! 铁律 1：tool 只返回**结构化 JSON**（放在 `CallToolResult` 的 text 块里），
//! 不做文案渲染；由调用方模型组织自然语言。
//! 铁律 2：每个 tool 的 `description` 必须写清「何时该调用」。
//! 铁律 3：任何失败都映射成结构化错误 JSON，绝不 panic。

use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, FixedOffset};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};
use serde::Deserialize;

use crate::aggregator::{AggregateResult, Aggregator, Provider};
use crate::cache::CacheSet;
use crate::config::Config;
use crate::error::ApiError;
// 注：本文件只用 ApiError 构造错误，ErrorCode 由 error 模块内部使用。
use crate::models::{ApiFailure, ApiSuccess, Match, Meta, Ranking};
use crate::providers::{hltv, liquipedia, pandascore};

pub const SERVER_NAME: &str = "cs-mcp";
pub const SERVER_VERSION: &str = "0.1.0";

/// `hours` 上限：7 天。
const MAX_HOURS: u32 = 168;
const MAX_TOP: u32 = 100;
const MAX_LIMIT: u32 = 50;

struct ServerInner {
    cfg: Config,
    tz: FixedOffset,
    agg: Aggregator,
    cache: CacheSet,
}

/// MCP 服务句柄。`Clone` 是 rmcp 宏的要求。
#[derive(Clone)]
pub struct CsServer {
    inner: Arc<ServerInner>,
}

impl CsServer {
    pub fn new(cfg: Config) -> anyhow::Result<Self> {
        let tz = Config::cn_offset()?;
        let cache = CacheSet::new(cfg.ttl_schedule(), cfg.ttl_live(), cfg.ttl_rank());

        let mut providers: Vec<Box<dyn Provider>> = Vec::new();

        // 主源：无 token 时不构造（实时类 tool 因此返回 NO_DATA，绝不伪造）。
        if let Some(p) = pandascore::PandaScoreProvider::new(&cfg) {
            providers.push(Box::new(p));
        }
        // 辅源：赛程/赛事/排名（无实时比分）。
        if let Some(p) = liquipedia::LiquipediaProvider::new(&cfg) {
            providers.push(Box::new(p));
        }
        // 兜底：默认关闭（Cloudflare 挑战 + ToS 禁爬）。
        if cfg.enable_hltv_scrape {
            if let Some(p) = hltv::HltvProvider::new(&cfg) {
                providers.push(Box::new(p));
            }
        }

        Ok(Self {
            inner: Arc::new(ServerInner {
                cfg,
                tz,
                agg: Aggregator::new(providers),
                cache,
            }),
        })
    }

    fn now(&self) -> DateTime<FixedOffset> {
        Config::now(self.inner.tz)
    }
}

// ---------------------------------------------------------------- 结果构造

/// 把任意可序列化载荷塞进 text 块。
fn to_text(payload: &impl serde::Serialize, is_error: bool) -> Result<CallToolResult, McpError> {
    match serde_json::to_string(payload) {
        Ok(json) => {
            let blocks = vec![ContentBlock::text(json)];
            Ok(if is_error {
                CallToolResult::error(blocks)
            } else {
                CallToolResult::success(blocks)
            })
        }
        Err(e) => {
            // 序列化失败也必须是结构化错误，且不得 panic。
            let fallback = format!(
                "{{\"ok\":false,\"error\":{{\"code\":\"UPSTREAM_ERROR\",\"message\":\"结果序列化失败: {e}\"}}}}"
            );
            Ok(CallToolResult::error(vec![ContentBlock::text(fallback)]))
        }
    }
}

fn err_body(err: ApiError) -> Result<CallToolResult, McpError> {
    to_text(&ApiFailure::new(err), true)
}

fn ok_body<T: serde::Serialize>(
    items: Vec<T>,
    meta: Meta,
) -> Result<CallToolResult, McpError> {
    to_text(&ApiSuccess::new(items, meta), false)
}

fn finish_matches(
    result: Result<AggregateResult<Match>, ApiError>,
    started: Instant,
) -> Result<CallToolResult, McpError> {
    match result {
        Ok(r) => {
            let meta = Meta::new(r.items.len(), r.sources_used, r.degraded)
                .with_timing(false, started.elapsed().as_millis() as u64);
            ok_body(r.items, meta)
        }
        Err(e) => err_body(e),
    }
}

fn finish_rankings(
    result: Result<AggregateResult<Ranking>, ApiError>,
    started: Instant,
) -> Result<CallToolResult, McpError> {
    match result {
        Ok(r) => {
            let meta = Meta::new(r.items.len(), r.sources_used, r.degraded)
                .with_timing(false, started.elapsed().as_millis() as u64);
            ok_body(r.items, meta)
        }
        Err(e) => err_body(e),
    }
}

// ---------------------------------------------------------------- 入参

fn default_hours() -> u32 {
    24
}
fn default_limit() -> u32 {
    6
}
fn default_top() -> u32 {
    10
}

/// `cs_upcoming_matches` 入参。
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpcomingParams {
    /// 未来多少小时内的赛程，默认 24，上限 168（7 天）。
    #[serde(default = "default_hours")]
    pub hours: u32,
    /// 可选：按赛事名过滤，例如 IEM、BLAST、Major。
    #[serde(default)]
    pub tournament: Option<String>,
}

/// `cs_recent_results` 入参。
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RecentParams {
    /// 回溯多少小时内的赛果，默认 24，上限 168。
    #[serde(default = "default_hours")]
    pub hours: u32,
}

/// `cs_today_schedule` 入参。
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TodayParams {
    /// 日期，格式 YYYY-MM-DD，缺省为服务器当天（+08:00）。
    #[serde(default)]
    pub date: Option<String>,
}

/// `cs_team_matches` 入参。
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TeamParams {
    /// 战队名，支持英文名或常见中文别名（天禄 / 小蜜蜂 / 绿龙 …）。
    pub team: String,
    /// 未来与最近各取多少场，默认 6，上限 50。
    #[serde(default = "default_limit")]
    pub limit: u32,
}

/// `cs_world_ranking` 入参。
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RankingParams {
    /// 取前多少名，默认 10，上限 100。
    #[serde(default = "default_top")]
    pub top: u32,
}

/// `cs_match_detail` 入参。
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DetailParams {
    /// 比赛 ID，形如 `pandascore:12345` 或 `liquipedia:PageName`。
    pub match_id: String,
}

// ---------------------------------------------------------------- tools

#[tool_router]
impl CsServer {
    /// 健康检查。何时调用：排查「为什么查不到比赛」时先调它；或客户端首次连接后验证服务可用。
    #[tool(
        description = "健康检查与自检：返回服务版本、当前时间（+08:00）、各数据源就绪状态、缓存条目数。何时调用：①排查『为什么查不到比赛』时先调它确认数据源是否就绪；②确认 PandaScore token 是否生效；③客户端首次连接后验证服务正常。不返回任何比赛数据。"
    )]
    async fn cs_ping(&self) -> Result<CallToolResult, McpError> {
        let now = self.now();
        let providers: Vec<serde_json::Value> = self
            .inner
            .agg
            .describe()
            .iter()
            .map(|(name, priority)| {
                serde_json::json!({ "name": name, "priority": priority })
            })
            .collect();

        let payload = serde_json::json!({
            "ok": true,
            "data": [{
                "server": SERVER_NAME,
                "version": SERVER_VERSION,
                "now": now.to_rfc3339(),
                "utc_offset_seconds": self.inner.tz.local_minus_utc(),
                "tz": self.inner.cfg.tz,
                "pandascore_ready": self.inner.cfg.has_pandascore(),
                "hltv_scrape_enabled": self.inner.cfg.enable_hltv_scrape,
                "providers": providers,
                "cache_entries": {
                    "schedule": self.inner.cache.schedule.entry_count(),
                    "live": self.inner.cache.live.entry_count(),
                    "rank": self.inner.cache.rank.entry_count(),
                }
            }],
            "meta": Meta::new(1, vec![], false),
        });

        to_text(&payload, false)
    }

    /// 进行中 + 30 分钟内开赛的比赛（含实时比分）。
    #[tool(
        description = "获取正在进行或 30 分钟内开赛的比赛，含实时比分与逐图比分。何时调用：用户问『现在有什么比赛』『正在打什么』『现在几比几』『马上开赛的是哪场』。重要：实时比分只有 PandaScore 提供；若返回 NO_DATA，说明未配置 token 或上游不可用，此时必须如实告知用户，禁止用其它工具的历史数据替代，也禁止编造比分。"
    )]
    async fn cs_live_matches(&self) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        let result = self.inner.agg.live().await;
        finish_matches(result, started)
    }

    /// 未来 N 小时赛程。
    #[tool(
        description = "获取未来 N 小时内的赛程（默认 24 小时，上限 168 小时）。何时调用：用户问『接下来有什么比赛』『明天有什么比赛』『本周赛程』『IEM 什么时候打』。参数 tournament 可按赛事名过滤。返回结果含开赛时间（+08:00）、赛事、Bo 制；未开赛的比赛不给比分。"
    )]
    async fn cs_upcoming_matches(
        &self,
        Parameters(params): Parameters<UpcomingParams>,
    ) -> Result<CallToolResult, McpError> {
        if params.hours == 0 || params.hours > MAX_HOURS {
            let hours = params.hours;
            return err_body(ApiError::bad_input(format!(
                "hours 必须在 1..={MAX_HOURS} 之间，收到 {hours}"
            )));
        }
        let started = Instant::now();
        let tournament = params.tournament.as_deref();
        let result = self.inner.agg.upcoming(params.hours, tournament).await;
        finish_matches(result, started)
    }

    /// 最近 N 小时赛果。
    #[tool(
        description = "获取最近 N 小时内已结束的比赛结果（默认 24 小时，上限 168 小时）。何时调用：用户问『昨晚谁赢了』『昨天结果如何』『最近打完的比赛怎么样了』。已结束的比赛必须给出终局比分（以及有则给出的逐图比分）。"
    )]
    async fn cs_recent_results(
        &self,
        Parameters(params): Parameters<RecentParams>,
    ) -> Result<CallToolResult, McpError> {
        if params.hours == 0 || params.hours > MAX_HOURS {
            let hours = params.hours;
            return err_body(ApiError::bad_input(format!(
                "hours 必须在 1..={MAX_HOURS} 之间，收到 {hours}"
            )));
        }
        let started = Instant::now();
        let result = self.inner.agg.recent(params.hours).await;
        finish_matches(result, started)
    }

    /// 指定日期全赛程。
    #[tool(
        description = "获取指定日期（缺省为服务器当天）的全部赛程。何时调用：用户问『今天的比赛』『X 月 X 日有什么比赛』。日期格式 YYYY-MM-DD；缺省按 +08:00 的当天计算。想查『今天』时优先用本工具，而不是 cs_upcoming_matches。"
    )]
    async fn cs_today_schedule(
        &self,
        Parameters(params): Parameters<TodayParams>,
    ) -> Result<CallToolResult, McpError> {
        let date = params.date.as_deref();
        if let Some(d) = date {
            if d.trim().is_empty() {
                return err_body(ApiError::bad_input("date 不能为空字符串"));
            }
        }
        let started = Instant::now();
        let result = self.inner.agg.today(date).await;
        finish_matches(result, started)
    }

    /// 指定战队的未来与最近比赛。
    #[tool(
        description = "获取指定战队未来与最近的比赛（各 limit 场，默认 6，上限 50）。何时调用：用户问『NAVI 今天打吗』『G2 最近战绩』『某某队下一场打谁』。team 支持英文名与常见中文别名（天禄→TYLOO、小蜜蜂→Vitality、绿龙→Team Spirit、李逵→Liquid）。"
    )]
    async fn cs_team_matches(
        &self,
        Parameters(params): Parameters<TeamParams>,
    ) -> Result<CallToolResult, McpError> {
        let team = params.team.trim();
        if team.is_empty() {
            return err_body(ApiError::bad_input("team 不能为空"));
        }
        if params.limit == 0 || params.limit > MAX_LIMIT {
            let limit = params.limit;
            return err_body(ApiError::bad_input(format!(
                "limit 必须在 1..={MAX_LIMIT} 之间，收到 {limit}"
            )));
        }
        let started = Instant::now();
        let result = self.inner.agg.team_matches(team, params.limit).await;
        finish_matches(result, started)
    }

    /// 世界排名。
    #[tool(
        description = "获取 CS2 世界排名（含名次变动），取前 top 名（默认 10，上限 100）。何时调用：用户问『世界排名』『现在谁是第一』『VRS 排名』『某某队排第几』。数据源为 Liquipedia，meta 中会带 stale=true，回复时必须如实说明排名可能滞后 1 到 7 天。"
    )]
    async fn cs_world_ranking(
        &self,
        Parameters(params): Parameters<RankingParams>,
    ) -> Result<CallToolResult, McpError> {
        if params.top == 0 || params.top > MAX_TOP {
            let top = params.top;
            return err_body(ApiError::bad_input(format!(
                "top 必须在 1..={MAX_TOP} 之间，收到 {top}"
            )));
        }
        let started = Instant::now();
        let result = self.inner.agg.ranking(params.top).await;
        finish_rankings(result, started)
    }

    /// 单场详情。
    #[tool(
        description = "获取单场比赛详情（逐图比分、地图、直播源）。何时调用：用户追问某一场具体比赛，或先用 cs_live_matches / cs_recent_results 拿到 id 后深挖。match_id 形如 pandascore:12345 或 liquipedia:PageName，必须原样传入，不要自己拼接或猜造 ID。"
    )]
    async fn cs_match_detail(
        &self,
        Parameters(params): Parameters<DetailParams>,
    ) -> Result<CallToolResult, McpError> {
        let id = params.match_id.trim();
        if id.is_empty() {
            return err_body(ApiError::bad_input("match_id 不能为空"));
        }
        let started = Instant::now();
        let result = self.inner.agg.match_detail(id).await;
        finish_matches(result, started)
    }
}

#[tool_handler(
    name = "cs-mcp",
    version = "0.1.0",
    instructions = "CS2 赛事数据服务。只返回结构化 JSON，不做文案渲染：由你（调用方）组织自然语言。原则：①只能使用工具返回的事实，没有的信息一律回答『暂无数据』，禁止推测或用历史交锋替代；②时间均为 +08:00；③未开赛的比赛不得给比分；④实时比分只有 PandaScore 提供，未配置时返回 NO_DATA，此时如实告知用户，禁止编造；⑤返回值的 meta.degraded=true 表示发生了降级、meta.cached=true 表示命中缓存，sources_used 是真实来源，回复时要如实标注来源。"
)]
impl ServerHandler for CsServer {}
