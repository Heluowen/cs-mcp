//! 数据源适配器。
//!
//! 降级顺序由各 Provider 的 `priority()` 决定（越小越先）：
//! - `pandascore` 10（主，唯一有实时比分/逐图/直播源）
//! - `liquipedia` 20（辅，无实时比分）
//! - `hltv` 30（默认不加载：Cloudflare 挑战 + ToS 禁止爬取）

pub mod hltv;
pub mod liquipedia;
pub mod pandascore;
