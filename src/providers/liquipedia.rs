//! Liquipedia 数据源（**辅助源**）。
//!
//! 实测（2026-10 ~ 11）：
//! - 泛化 User-Agent → **HTTP 406**（被拒）
//! - 规范 UA（`产品名/版本 (联系方式邮箱)`）→ **HTTP 200 + gzip**
//! - 要求 **≤1 req / 2s**（0.5 QPS）；超频会被临时封禁，此后 **API 全 406、页面 403**
//!   —— 2026-11 实测就踩到了这个封禁。
//!
//! 能力边界：**没有实时比分**。本文件目前只实现 `ranking()`（世界排名），
//! 它是目前唯一可用的排名来源；结果标记 `Ranking.stale = true`（口径滞后 1–7 天）。
//!
//! 排名页解析采取**宽容策略**：不依赖具体 CSS 类名，只按「表格行 → 单元格」
//! 提取「靠前的纯数字=名次 / 含字母=队名 / 靠后的纯数字=积分」，
//! 页面改版时劣化为「提取不到 → NO_DATA」而不是给出错数据。

use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use scraper::{Html, Selector};
use serde::Deserialize;

use crate::aggregator::Provider;
use crate::config::Config;
use crate::error::ProviderError;
use crate::models::{Ranking, RankingEntry};

pub const NAME: &str = "liquipedia";
pub const PRIORITY: u8 = 20;
pub const API_BASE: &str = "https://liquipedia.net/counterstrike/api.php";

/// Liquipedia 要求可识别的 UA，缺省或过于泛化会返回 406。
pub const USER_AGENT: &str =
    "CS2EsportsMCP/0.1 (https://github.com/Heluowen/cs-mcp; heluowen0831@gmail.com)";

/// 排名页（`Portal:Rankings` 为 CS 项目部的排名入口页）。
const RANKING_PAGE: &str = "Portal:Rankings";

/// Liquipedia 硬性要求：两次请求间隔 ≥2 秒。
const MIN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

pub struct LiquipediaProvider {
    client: Client,
    limiter: Option<crate::ratelimit::HostLimiter>,
}

// ------------------------------------------------------------------ DTO

#[derive(Debug, Deserialize)]
struct ParseResponse {
    parse: ParseBody,
}

#[derive(Debug, Deserialize)]
struct ParseBody {
    #[serde(default)]
    text: String,
}

// ------------------------------------------------------------------ impl

impl LiquipediaProvider {
    pub fn new(cfg: &Config) -> Option<Self> {
        let mut builder = Client::builder()
            .timeout(cfg.http_timeout())
            .connect_timeout(std::time::Duration::from_secs(3))
            .user_agent(USER_AGENT);
        if let Some(proxy) = cfg.cs_mcp_http_proxy.as_deref() {
            if let Ok(p) = reqwest::Proxy::all(proxy) {
                builder = builder.proxy(p);
            }
        }
        let client = builder.build().ok()?;

        Some(Self {
            client,
            // 0.5 QPS：每 2 秒 1 次，突发 1。超频会被 Liquipedia 封 IP。
            limiter: crate::ratelimit::HostLimiter::with_period(MIN_INTERVAL, 1),
        })
    }

    /// 严格节流：Liquipedia 的封禁代价很高（API 全 406、页面 403），
    /// 因此这里**最多等 6 秒**；拿不到令牌仍然放行，只记警告。
    async fn throttle(&self) {
        let Some(limiter) = self.limiter.as_ref() else {
            return;
        };
        if limiter.try_acquire() {
            return;
        }
        for _ in 0..12 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if limiter.try_acquire() {
                return;
            }
        }
        tracing::warn!("Liquipedia 节流等待超时，放行本次请求（可能触发封禁）");
    }

    /// 拉取某页面的解析后 HTML（`action=parse&prop=text`）。
    async fn fetch_parse_html(&self, page: &str) -> Result<String, ProviderError> {
        self.throttle().await;

        let url = Self::api_url(&format!(
            "action=parse&page={}&prop=text&format=json&formatversion=2",
            percent_encode(page)
        ));

        let resp = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(format!("请求 Liquipedia 失败: {e}")))?;

        if let Some(err) = Self::map_status(resp.status()) {
            return Err(err);
        }

        let body: ParseResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::Upstream(format!("解析 Liquipedia 响应失败: {e}")))?;

        Ok(body.parse.text)
    }

    /// 排名：先试入口页，失败则跟随页内「含 ranking 字样的子页面」再试。
    ///
    /// 为什么要自适应：`Portal:Rankings` 是入口页，真正的表格可能在其子页；
    /// 而 Liquipedia 页面结构会变，硬编码子页名会在改版后直接失效。
    async fn ranking_impl(&self, top: u32) -> Result<Vec<Ranking>, ProviderError> {
        let portal_html = self.fetch_parse_html(RANKING_PAGE).await?;

        let links = extract_ranking_links(&portal_html, RANKING_PAGE);
        tracing::debug!(
            html_len = portal_html.len(),
            tables = portal_html.matches("<table").count(),
            rows = portal_html.matches("<tr").count(),
            links = ?links,
            "Liquipedia 入口页已拉取"
        );

        let mut entries = parse_ranking_table(&portal_html, top);
        if !entries.is_empty() {
            return self.wrap_ranking(entries);
        }

        for page in links.into_iter().take(3) {
            tracing::debug!("Liquipedia 入口页无表格，尝试子页面: {page}");
            let html = self.fetch_parse_html(&page).await?;
            entries = parse_ranking_table(&html, top);
            if !entries.is_empty() {
                return self.wrap_ranking(entries);
            }
        }

        Err(ProviderError::NoData(
            "Liquipedia 的 Portal:Rankings 不含结构化排名表（实测仅约 1KB 入口页、0 个表格），\
             当前没有可用的 CS2 世界排名数据源：官方排名在 HLTV（Cloudflare 拦截）与 Valve VRS"
                .to_string(),
        ))
    }

    fn wrap_ranking(&self, entries: Vec<RankingEntry>) -> Result<Vec<Ranking>, ProviderError> {
        let tz = Config::cn_offset()
            .map_err(|e| ProviderError::Upstream(format!("时区初始化失败: {e}")))?;

        Ok(vec![Ranking {
            kind: "world".to_string(),
            entries,
            source: NAME.to_string(),
            fetched_at: Utc::now().with_timezone(&tz),
            // Liquipedia 排名口径滞后，必须如实标记。
            stale: true,
        }])
    }

    fn api_url(params: &str) -> String {
        format!("{API_BASE}?{params}")
    }

    /// 把 HTTP 状态映射为领域错误，重点是区分「被限流/被封」与「上游故障」。
    fn map_status(status: reqwest::StatusCode) -> Option<ProviderError> {
        match status.as_u16() {
            403 | 406 => Some(ProviderError::Upstream(
                "Liquipedia 拒绝访问（HTTP 403/406）：通常是请求过频被临时封禁，\
                 或 User-Agent 不符合要求（需形如 `产品名/版本 (联系方式)`）"
                    .to_string(),
            )),
            429 => Some(ProviderError::RateLimited {
                retry_after: Some(60),
                detail: "Liquipedia 触发限流（HTTP 429）".to_string(),
            }),
            s if s >= 500 => Some(ProviderError::Upstream(format!(
                "Liquipedia 上游故障（HTTP {s}）"
            ))),
            s if s >= 400 => Some(ProviderError::Upstream(format!(
                "Liquipedia 返回异常状态（HTTP {s}）"
            ))),
            _ => None,
        }
    }
}

/// 从排名页 HTML 中提取排名条目。
fn parse_ranking_table(html: &str, top: u32) -> Vec<RankingEntry> {
    let doc = Html::parse_document(html);
    let Ok(row_sel) = Selector::parse("table tr") else {
        return Vec::new();
    };
    let Ok(cell_sel) = Selector::parse("td, th") else {
        return Vec::new();
    };

    let mut out: Vec<RankingEntry> = Vec::new();
    let mut seen_ranks: Vec<u32> = Vec::new();

    for row in doc.select(&row_sel) {
        let cells: Vec<String> = row
            .select(&cell_sel)
            .map(|c| normalize_ws(&c.text().collect::<String>()))
            .collect();

        if cells.len() < 2 {
            continue;
        }

        let mut rank: Option<u32> = None;
        let mut team: Option<String> = None;
        let mut points: Option<i64> = None;

        for (idx, cell) in cells.iter().enumerate() {
            if cell.is_empty() {
                continue;
            }
            if rank.is_none() && idx <= 1 {
                if let Some(n) = parse_rank(cell) {
                    rank = Some(n);
                    continue;
                }
            }
            if team.is_none() && cell.chars().any(|c| c.is_alphabetic()) {
                team = Some(cell.clone());
                continue;
            }
            if let Some(p) = parse_points(cell) {
                points = Some(p);
            }
        }

        let (Some(rank), Some(team)) = (rank, team) else {
            continue;
        };
        if rank == 0 || seen_ranks.contains(&rank) {
            continue;
        }

        seen_ranks.push(rank);
        out.push(RankingEntry {
            rank,
            team,
            points,
            change: None,
        });
    }

    out.sort_by_key(|e| e.rank);
    out.truncate(top as usize);
    out
}

/// 从入口页 HTML 里挑出「看起来是排名子页」的项目内链接。
///
/// 只保留 `href="/counterstrike/..."` 且链接文本或 href 含 `ranking`
/// （不区分大小写）的页面，排除入口页自身，去重后按出现顺序返回。
fn extract_ranking_links(html: &str, self_page: &str) -> Vec<String> {
    let doc = Html::parse_document(html);
    let Ok(a_sel) = Selector::parse("a[href]") else {
        return Vec::new();
    };

    let mut out: Vec<String> = Vec::new();
    let self_norm = self_page.to_lowercase();

    for a in doc.select(&a_sel) {
        let Some(href) = a.value().attr("href") else {
            continue;
        };
        let Some(rest) = href.strip_prefix("/counterstrike/") else {
            continue;
        };
        // 去掉锚点与查询串，并还原常见转义。
        let page = rest
            .split(['#', '?'])
            .next()
            .unwrap_or(rest)
            .replace("%3A", ":")
            .replace("%20", " ");

        if page.is_empty() || page.to_lowercase() == self_norm {
            continue;
        }

        let text = normalize_ws(&a.text().collect::<String>());
        let looks_ranking = page.to_lowercase().contains("ranking")
            || text.to_lowercase().contains("ranking")
            || text.contains("排名");
        if !looks_ranking {
            continue;
        }
        // 只接受像页面名的（含命名空间冒号或以大写字母开头），排除 Special: 等特殊页。
        if page.to_lowercase().starts_with("special:")
            || page.to_lowercase().starts_with("file:")
            || page.to_lowercase().starts_with("template:")
        {
            continue;
        }
        if !out.contains(&page) {
            out.push(page);
        }
    }

    out
}

fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 名次：1–3 位纯数字（允许尾随 `.` / `#` / `)`）。
fn parse_rank(cell: &str) -> Option<u32> {
    let t = cell.trim().trim_end_matches(['.', '#', ')']).trim();
    if t.is_empty() || t.len() > 3 || !t.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    t.parse::<u32>().ok()
}

/// 积分：允许千分位逗号，要求 ≥2 位，避免把个位数的名次误当积分。
fn parse_points(cell: &str) -> Option<i64> {
    let cleaned: String = cell
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == ',')
        .collect::<String>()
        .replace(',', "");
    if cleaned.len() < 2 || !cleaned.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    cleaned.parse::<i64>().ok()
}

/// 极简百分号编码（用于 page 参数里的冒号）。
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[async_trait]
impl Provider for LiquipediaProvider {
    fn name(&self) -> &'static str {
        NAME
    }

    fn priority(&self) -> u8 {
        PRIORITY
    }

    async fn ranking(&self, top: u32) -> Result<Vec<Ranking>, ProviderError> {
        self.ranking_impl(top).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 模仿 Liquipedia `table2` 结构的排名表。
    const FIXTURE: &str = r#"
<div class="mw-parser-output">
<table class="table2 table2--striped">
  <thead>
    <tr class="table2__row table2__row--header">
      <th>#</th><th>Team</th><th>Points</th><th>Change</th>
    </tr>
  </thead>
  <tbody>
    <tr class="table2__row">
      <td>1</td>
      <td><span class="team-template-text"><a title="Team Vitality">Team Vitality</a></span></td>
      <td>1,024</td><td>—</td>
    </tr>
    <tr class="table2__row">
      <td>2</td>
      <td><a title="Natus Vincere">Natus Vincere</a></td>
      <td>998</td><td>+1</td>
    </tr>
    <tr class="table2__row">
      <td>3</td>
      <td><a title="MOUZ">MOUZ</a></td>
      <td>877</td><td>-1</td>
    </tr>
    <tr class="table2__row">
      <td>4</td>
      <td><a title="G2 Esports">G2 Esports</a></td>
      <td>702</td><td>0</td>
    </tr>
    <tr class="table2__row">
      <td>5</td>
      <td><a title="3DMAX">3DMAX</a></td>
      <td>650</td><td>+2</td>
    </tr>
  </tbody>
</table>
</div>
"#;

    #[test]
    fn parses_ranking_table() {
        let entries = parse_ranking_table(FIXTURE, 10);
        assert_eq!(entries.len(), 5, "应解析出 5 条");

        assert_eq!(entries[0].rank, 1);
        assert_eq!(entries[0].team, "Team Vitality");
        assert_eq!(entries[0].points, Some(1024), "千分位逗号应被去掉");

        assert_eq!(entries[1].team, "Natus Vincere");
        assert_eq!(entries[2].team, "MOUZ");
        // 队名含数字的必须完整保留，不能被误判为积分
        assert_eq!(entries[4].team, "3DMAX");
        assert_eq!(entries[4].points, Some(650));
    }

    #[test]
    fn respects_top_limit() {
        let entries = parse_ranking_table(FIXTURE, 2);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].rank, 2);
    }

    #[test]
    fn header_row_is_skipped() {
        let entries = parse_ranking_table(FIXTURE, 10);
        assert!(
            entries.iter().all(|e| e.team != "Team" && e.team != "#"),
            "表头不应被当成数据行"
        );
    }

    #[test]
    fn garbage_html_yields_empty() {
        assert!(
            parse_ranking_table("<html><body><p>nothing here</p></body></html>", 10).is_empty()
        );
        assert!(parse_ranking_table("", 10).is_empty());
    }

    #[test]
    fn parse_rank_rejects_long_numbers() {
        assert_eq!(parse_rank("12"), Some(12));
        assert_eq!(parse_rank("12."), Some(12));
        assert_eq!(parse_rank("#3"), None);
        assert_eq!(parse_rank("1987"), None, "四位数字视为积分而非名次");
    }

    #[test]
    fn percent_encode_handles_colon() {
        assert_eq!(percent_encode("Portal:Rankings"), "Portal%3ARankings");
    }

    #[test]
    fn extracts_ranking_subpage_links() {
        let html = r#"
            <div>
              <a href="/counterstrike/Portal:Rankings/World_Ranking">World Ranking</a>
              <a href="/counterstrike/Special:RecentChanges">Recent Changes</a>
              <a href="/counterstrike/Portal:Rankings#top">Back</a>
              <a href="/counterstrike/Portal:Rankings/Europe">欧洲排名</a>
              <a href="/counterstrike/Intel_Extreme_Masters">IEM</a>
            </div>
        "#;
        let links = extract_ranking_links(html, "Portal:Rankings");
        assert_eq!(links, vec![
            "Portal:Rankings/World_Ranking".to_string(),
            "Portal:Rankings/Europe".to_string(),
        ]);
    }

    #[test]
    fn extract_ranking_links_dedups_and_ignores_self() {
        let html = r#"
            <a href="/counterstrike/Portal:Rankings">Portal:Rankings</a>
            <a href="/counterstrike/Portal:Rankings/World">Ranking A</a>
            <a href="/counterstrike/Portal:Rankings/World">Ranking A again</a>
        "#;
        let links = extract_ranking_links(html, "Portal:Rankings");
        assert_eq!(links, vec!["Portal:Rankings/World".to_string()]);
    }
}
