//! Valve VRS（Valve Regional Standings）数据源 —— **排名主源**。
//!
//! 官方数据在 GitHub 仓库 [`ValveSoftware/counter-strike_regional_standings`]：
//! - `live/<年>/standings_global_YYYY_MM_DD.md`（全球排名，markdown 表格）
//! - 另有 `standings_europe_*` / `standings_americas_*` / `standings_asia_*`
//!
//! 表格结构（实测 2026-11-02）：
//! ```text
//! | Standing | Points | Team Name | Roster | ... |
//! | :- | -: | :- | :- | :- |
//! | 1        |   1960 | MOUZ      | PR, Spinx, ... | [details](..) |
//! ```
//!
//! 关键实测结论：
//! - `raw.githubusercontent.com` 在国内网络**可直连**（不像 HLTV / bo3 被挡）
//! - 文件名带日期，需要先列目录再取最新一份；目录列表走 GitHub API（未鉴权 60 次/小时，
//!   配合排名缓存 3600s 完全够用）
//! - 排名按月更新，因此必须标记 `stale = true`
//!
//! 优先级设为 15：**低于** pandascore（10，但它不支持 ranking 会被跳过）、
//! **高于** liquipedia（20，实测其排名页无结构化表格）。

use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;

use crate::aggregator::Provider;
use crate::config::Config;
use crate::error::ProviderError;
use crate::models::{Ranking, RankingEntry};

pub const NAME: &str = "valve";
pub const PRIORITY: u8 = 15;
const API_CONTENTS: &str = "https://api.github.com/repos/ValveSoftware/counter-strike_regional_standings/contents/live";
const RAW_BASE: &str =
    "https://raw.githubusercontent.com/ValveSoftware/counter-strike_regional_standings/main";

/// GitHub 要求非空 UA。
const USER_AGENT: &str = "CS2EsportsMCP/0.1 (https://github.com/Heluowen/cs-mcp)";

pub struct ValveVrsProvider {
    client: Client,
}

// ------------------------------------------------------------------ DTO

#[derive(Debug, Deserialize)]
struct ContentEntry {
    name: String,
    #[serde(default)]
    download_url: Option<String>,
}

// ------------------------------------------------------------------ impl

impl ValveVrsProvider {
    pub fn new(cfg: &Config) -> Option<Self> {
        let mut builder = Client::builder()
            .timeout(cfg.http_timeout())
            .connect_timeout(std::time::Duration::from_secs(3))
            .user_agent(USER_AGENT);
        if let Some(proxy) = cfg.http_proxy.as_deref() {
            if let Ok(p) = reqwest::Proxy::all(proxy) {
                builder = builder.proxy(p);
            }
        }
        let client = builder.build().ok()?;
        Some(Self { client })
    }

    /// 列出指定年份目录下的文件。
    async fn list_year(&self, year: i32) -> Result<Vec<ContentEntry>, ProviderError> {
        let url = format!("{API_CONTENTS}/{year}");
        let resp = self
            .client
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(format!("列出 Valve 排名目录失败: {e}")))?;

        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        {
            return Err(ProviderError::RateLimited {
                retry_after: Some(300),
                detail: "GitHub API 限流（未鉴权 60 次/小时），请稍后重试".to_string(),
            });
        }
        if !status.is_success() {
            return Err(ProviderError::Upstream(format!(
                "列出 Valve 排名目录返回 HTTP {status}"
            )));
        }

        resp.json::<Vec<ContentEntry>>()
            .await
            .map_err(|e| ProviderError::Upstream(format!("解析目录列表失败: {e}")))
    }

    /// 找到最新的全球 standings 文件（按 `YYYY_MM_DD` 字典序即时间序）。
    async fn latest_global(&self) -> Result<(String, String), ProviderError> {
        // 以 +08:00 的当前年份为准，同时回退看上一年（1 月初可能还没今年数据）。
        let tz = Config::cn_offset()
            .map_err(|e| ProviderError::Upstream(format!("时区初始化失败: {e}")))?;
        let now = Utc::now().with_timezone(&tz);
        let year = chrono::Datelike::year(&now);

        for y in [year, year - 1] {
            let entries = self.list_year(y).await?;
            let mut best: Option<(String, String)> = None;
            for e in entries {
                let name = &e.name;
                let Some(stem) = name
                    .strip_prefix("standings_global_")
                    .and_then(|s| s.strip_suffix(".md"))
                else {
                    continue;
                };
                if !is_date_stem(stem) {
                    continue;
                }
                let url = match &e.download_url {
                    Some(u) if !u.is_empty() => u.clone(),
                    _ => format!("{RAW_BASE}/live/{y}/{name}"),
                };
                // 字典序比较即可：YYYY_MM_DD 定长。
                match &best {
                    Some((best_stem, _)) if best_stem.as_str() >= stem => {}
                    _ => best = Some((stem.to_string(), url)),
                }
            }
            if let Some((stem, url)) = best {
                return Ok((stem, url));
            }
        }

        Err(ProviderError::NoData(
            "Valve 排名仓库中没有找到 standings_global_*.md".to_string(),
        ))
    }

    async fn fetch_markdown(&self, url: &str) -> Result<String, ProviderError> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(format!("下载 Valve 排名文件失败: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(ProviderError::Upstream(format!(
                "下载 Valve 排名文件返回 HTTP {status}"
            )));
        }

        resp.text()
            .await
            .map_err(|e| ProviderError::Upstream(format!("读取 Valve 排名文件失败: {e}")))
    }
}

/// `stem` 形如 `2026_11_02`。
fn is_date_stem(stem: &str) -> bool {
    let parts: Vec<&str> = stem.split('_').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// 解析 Valve 的 markdown 排名表。
///
/// 列序固定：`Standing | Points | Team Name | Roster | details`。
/// 表头行（`Standing` 非数字）与分隔行（含 `:-`）会被自动跳过。
fn parse_markdown_table(md: &str, top: u32) -> Vec<RankingEntry> {
    let mut out: Vec<RankingEntry> = Vec::new();

    for line in md.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = trimmed
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        if cells.len() < 3 {
            continue;
        }
        // 分隔行：| :- | -: | :- |
        if cells.iter().any(|c| c.contains(":-") || c.contains("-:")) {
            continue;
        }

        let Ok(rank) = cells[0].parse::<u32>() else {
            continue;
        };
        let Ok(points) = cells[1].parse::<i64>() else {
            continue;
        };
        let team = cells[2];
        if team.is_empty() || team.eq_ignore_ascii_case("team name") {
            continue;
        }

        out.push(RankingEntry {
            rank,
            team: team.to_string(),
            points: Some(points),
            change: None,
        });

        if out.len() >= top as usize {
            break;
        }
    }

    out
}

#[async_trait]
impl Provider for ValveVrsProvider {
    fn name(&self) -> &'static str {
        NAME
    }

    fn priority(&self) -> u8 {
        PRIORITY
    }

    async fn ranking(&self, top: u32) -> Result<Vec<Ranking>, ProviderError> {
        let (stem, url) = self.latest_global().await?;
        let md = self.fetch_markdown(&url).await?;

        let entries = parse_markdown_table(&md, top);
        if entries.is_empty() {
            return Err(ProviderError::NoData(format!(
                "Valve 排名文件 {stem} 中未解析出条目"
            )));
        }

        let tz = Config::cn_offset()
            .map_err(|e| ProviderError::Upstream(format!("时区初始化失败: {e}")))?;

        Ok(vec![Ranking {
            kind: "vrs".to_string(),
            entries,
            source: NAME.to_string(),
            fetched_at: Utc::now().with_timezone(&tz),
            // Valve 排名按月更新，必须如实标记可能滞后。
            stale: true,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"### Standings as of 2026_11_02<br />
<br />

| Standing | Points | Team Name            | Roster                                |                                                                      |
| :- | -: | :- | :- | :- |
| 1        |   1960 | MOUZ                 | PR, Spinx, torzsi, xelex, xertioN     | [details](details/2026_11_02/0001--mouz.md)      |
| 2        |   1922 | Vitality             | flameZ, jL, mezii, ropz, ZywOo        | [details](details/2026_11_02/0002--vitality.md)  |
| 3        |   1912 | FURIA                | FalleN, KSCERATO, molodoy, YEKINDAR   | [details](details/2026_11_02/0003--furia.md)     |
| 11       |   1616 | G2                   | HeavyGod, huNter-, MATYS, NertZ       | [details](details/2026_11_02/0011--g2.md)        |
| 13       |   1589 | Natus Vincere        | Aleksib, b1t, iM, makazze, w0nderful  | [details](details/2026_11_02/0013--navi.md)      |
"#;

    #[test]
    fn parses_valve_markdown_table() {
        let entries = parse_markdown_table(FIXTURE, 10);
        assert_eq!(entries.len(), 5);
        assert_eq!(entries[0].rank, 1);
        assert_eq!(entries[0].team, "MOUZ");
        assert_eq!(entries[0].points, Some(1960));
        assert_eq!(entries[4].team, "Natus Vincere");
        assert_eq!(entries[4].points, Some(1589));
    }

    #[test]
    fn header_and_separator_are_skipped() {
        let entries = parse_markdown_table(FIXTURE, 10);
        assert!(entries.iter().all(|e| e.team != "Team Name" && e.rank >= 1));
    }

    #[test]
    fn respects_top_limit() {
        let entries = parse_markdown_table(FIXTURE, 2);
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn garbage_yields_empty() {
        assert!(parse_markdown_table("no table here", 10).is_empty());
        assert!(parse_markdown_table("", 10).is_empty());
        assert!(parse_markdown_table("| Standing | Points | Team Name |\n| :- | -: | :- |", 10).is_empty());
    }

    #[test]
    fn date_stem_validation() {
        assert!(is_date_stem("2026_11_02"));
        assert!(!is_date_stem("2026_11"));
        assert!(!is_date_stem("2026_11_02_extra"));
        assert!(!is_date_stem(""));
    }
}
