//! 数据源适配器。
//!
//! 降级顺序由各 Provider 的 `priority()` 决定（越小越先）：
//! - `pandascore` 10（比赛主源，唯一有实时比分/直播源；不支持 ranking）
//! - `valve` 15（**排名主源**：官方 VRS 全球排名，raw.githubusercontent 可直连）
//! - `liquipedia` 20（辅源：无实时比分；其排名页实测无结构化表格）
//! - `hltv` 30（默认不加载：Cloudflare 挑战 + ToS 禁止爬取）
//!
//! 各源只实现自己擅长的接口，其余走 `Provider` 默认的 `NotImplemented`；
//! 聚合器会跳过 `NotImplemented`，因此优先级只需表达「同类能力」的相对顺序。

pub mod hltv;
pub mod liquipedia;
pub mod pandascore;
pub mod valve;
