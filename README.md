# cs-mcp — CS2 赛事 MCP Server（Rust）

把 CS2 赛事数据暴露为 8 个 MCP tool（7 个业务 tool + `cs_ping` 健康检查），供任意 MCP 客户端（Claude Desktop / Cursor / AstrBot）直调。

**核心原则：只「取数 + 结构化」，不做文案渲染。** 自然语言由调用方模型组织。

```
cs-mcp  →  PandaScore（主）→ Liquipedia（辅）→ 5EPlay（可选）  →  结构化 JSON
```

---

## 1. 当前进度

| 阶段 | 内容 | 状态 |
|---|---|---|
| S1 | 工程骨架、8 个 tool 全量注册、stdio 打通、`cs_ping` 可用 | ✅ **已完成并验证**（见 §7.1） |
| S2 | PandaScore 真实接入（6 个数据 tool 已打通） | ✅ **已完成**（2 项受数据源限制，见 §9） |
| S3 | 降级 / 缓存 / 限流 / 错误码全链路 | ⏳ 待做（代码已就绪，待接入聚合链） |
| S4 | Liquipedia + `cs_world_ranking` | ⏳ 待做 |
| S5–S7 | HTTP transport、联调、硬化验收 | ⏳ 待做 |

> **S2 已完成**：`cs_live_matches` / `cs_upcoming_matches` / `cs_recent_results` / `cs_today_schedule` / `cs_team_matches` / `cs_match_detail` 已接 PandaScore 真实数据（含比分、直播源、+08:00 时间、中文队名别名）。
> `cs_world_ranking` 仍返回 `NO_DATA`，排名只能走 Liquipedia，属 S4。

---

## 2. 7 个 tool 的一句话用途（可直接贴进模型系统提示）

| tool | 入参 | 一句话用途 |
|---|---|---|
| `cs_ping` | — | 健康检查：确认服务、时区、数据源就绪状态与 token 是否生效；不返回比赛数据 |
| `cs_live_matches` | — | 进行中 / 30 分钟内开赛的比赛与实时比分 |
| `cs_upcoming_matches` | `hours=24`, `tournament?` | 未来 N 小时赛程，可按赛事名过滤 |
| `cs_recent_results` | `hours=24` | 最近 N 小时已结束赛果 |
| `cs_today_schedule` | `date?` | 指定日（默认今天）全部赛程 |
| `cs_team_matches` | `team`, `limit=6` | 某队未来与最近各 N 场 |
| `cs_world_ranking` | `top=10` | 世界排名与名次变动（来源 Liquipedia，可能滞后） |
| `cs_match_detail` | `match_id` | 单场详情：队伍/比分/直播源（**逐图 maps 恒为空**，见 §9 第 1 条） |

调用原则（建议写进系统提示）：

- 实时比分只有 PandaScore 提供；返回 `NO_DATA` 时**如实告知用户**，禁止用其它工具的历史数据替代，禁止编造比分。
- 时间一律 `+08:00`；未开赛的比赛不给比分。
- `meta.sources_used` 是真实来源，`meta.degraded=true` 表示发生了降级，`meta.cached=true` 表示命中缓存——回复时应如实标注来源。
- **`maps` 永远为空**（数据源不给逐图信息），不要据此推断「比赛没有地图数据」之外的结论。

---

## 3. 编译

### 3.1 前置：安装 Rust 工具链（本机当前缺失）

```powershell
# Windows：先装 VS Build Tools（MSVC 链接器），再装 rustup
winget install --id Microsoft.VisualStudio.2022.BuildTools -e
winget install --id Rustlang.Rustup -e

# 验证
cargo --version
rustc --version
```

> 备选：若不想装 MSVC，可用 `rustup default stable-gnu` + MinGW-w64。

### 3.2 构建

```powershell
cd cs-mcp
cp .env.example .env       # 填入 PANDASCORE_TOKEN
cargo build --release      # 产物：target\release\cs-mcp.exe

cargo test                 # 单元测试
cargo clippy -- -D warnings  # 质量门禁，必须干净
```

静态编译（可选，便于分发）：

```powershell
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
```

---

## 4. 运行

### 4.1 stdio（默认，MCP 客户端会自动拉起）

```powershell
.\target\release\cs-mcp.exe
```

**注意**：stdio 模式下 stdout 是协议通道，本程序所有日志强制写入 stderr，不会污染 JSON-RPC 流。

### 4.2 HTTP（Streamable HTTP）

```powershell
.\target\release\cs-mcp.exe --transport http --bind 0.0.0.0:8080
# 端点：http://127.0.0.1:8080/mcp
```

### 4.3 MCP Inspector 验证

```powershell
npx @modelcontextprotocol/inspector .\target\release\cs-mcp.exe
```

---

## 5. 客户端配置

### 5.1 Claude Desktop（`claude_desktop_config.json`）

```json
{
  "mcpServers": {
    "cs2": {
      "command": "H:\\Deepseek-Harness\\Zone 1\\cs-mcp\\target\\release\\cs-mcp.exe",
      "args": [],
      "env": { "PANDASCORE_TOKEN": "YOUR_TOKEN", "TZ": "Asia/Shanghai" }
    }
  }
}
```

### 5.2 Cursor（`.cursor/mcp.json`）

```json
{
  "mcpServers": {
    "cs2": {
      "command": "cs-mcp",
      "args": [],
      "env": { "PANDASCORE_TOKEN": "YOUR_TOKEN" }
    }
  }
}
```

### 5.3 AstrBot（`C:\\Users\\<you>\\.astrbot\\data\\mcp_server.json`）

```json
{
  "mcpServers": {
    "cs2": {
      "command": "H:\\Deepseek-Harness\\Zone 1\\cs-mcp\\target\\release\\cs-mcp.exe",
      "args": [],
      "env": { "PANDASCORE_TOKEN": "YOUR_TOKEN", "TZ": "Asia/Shanghai" }
    }
  }
}
```

配好后在 AstrBot WebUI 的 MCP 面板点「测试连接」→「启用」。启用后 7 个 tool 会进入全局工具集。

> AstrBot 侧直调路径（供插件壳使用）：
> `context.provider_manager.llm_tools.mcp_server_runtime_view["cs2"].client.call_tool_with_reconnect(...)`

HTTP 方式则改为：

```json
{ "mcpServers": { "cs2": { "url": "http://127.0.0.1:8080/mcp" } } }
```

---

## 6. 环境变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `PANDASCORE_TOKEN` | 空 | 缺失则实时比分不可用（`cs_live_matches` 返回 `NO_DATA`） |
| `ENABLE_HLTV_SCRAPE` | `0` | HLTV 抓取开关，**默认关**，原因见 §8 |
| `HTTP_PROXY` | 空 | 出站代理，如 `http://127.0.0.1:7890` |
| `HTTP_TIMEOUT_SECS` | `15` | 上游请求总超时（5s 读不完 50–100 条响应，见 §7.5） |
| `CACHE_TTL_SCHEDULE` | `120` | 赛程缓存 TTL（秒） |
| `CACHE_TTL_LIVE` | `30` | 实时缓存 TTL（秒） |
| `CACHE_TTL_RANK` | `3600` | 排名缓存 TTL（秒） |
| `TZ` | `Asia/Shanghai` | 仅用于日志/元信息；时间恒 +08:00 |
| `GLOBAL_QPS` | `2` | 全局兜底 QPS |
| `MCP_TRANSPORT` | `stdio` | `stdio` / `http` |
| `MCP_BIND` | `0.0.0.0:8080` | HTTP 监听地址 |
| `RUST_LOG` | `info` | tracing 过滤 |

---

## 7. 验证记录

### 7.1 已通过（2026-10-26，本机 Windows）

| 项 | 结果 |
|---|---|
| `cargo --version` | 1.99.1（工具链原本已装，仅 PATH 未刷新） |
| 依赖下载 | 需 crates.io 镜像 + `[http] multiplexing = false`：直连会被中间设备重置 HTTP/2（`[55] Failed sending data to the peer / Failed receiving HTTP2 data`） |
| `cargo build` | ✅ **首次编译即通过**，55.71s，0 error；仅有 2 个 `dead_code` warning（已修：`tz` 改由 `cs_ping` 输出、`global_qps`/未用错误变体标 `#[allow(dead_code)]` 并注明 S2/S3 启用） |
| **rmcp 3.8 API 假设** | ✅ **全部正确**：`#[tool_router]` + `#[tool_handler(name, version, instructions)]` + `Parameters<T>` + `CallToolResult::success/error(vec![ContentBlock::text(..)])` + `serve(stdio())` 一次命中，无需回退 |
| reqwest `0.13` + `rustls` feature | ✅ 编译通过（证实 0.13 无 `rustls-tls`、`rustls` 是正确 feature 名） |
| moka `0.12` 显式 `future` | ✅ 编译通过 |
| **stdio 握手** | ✅ `initialize` 返回 `serverInfo: {name: "cs-mcp", version: "0.1.0"}` |
| **`tools/list`** | ✅ **8 个 tool 全部注册**（7 个业务 tool + `cs_ping`），schema 正确 |
| **`tools/call cs_ping`** | ✅ 返回 `ok: true`，含 `tz` / `utc_offset_seconds` / `providers` / `cache_entries` |

### 7.2 质量门禁与实测指标（全部通过）

| 项 | 结果 |
|---|---|
| `cargo clippy --all-targets -- -D warnings` | ✅ exit 0，零警告 |
| `cargo test` | ✅ **14 passed / 0 failed** |
| `cargo build --release` | ✅ 2m30s，产物 **8.68 MB** |
| release 二进制 stdio 全链路 | ✅ 4 条响应：握手 + 8 tool + `cs_ping` + `cs_live_matches`(NO_DATA) |
| 冷启动 | ✅ 进程启动 + 4 次 RPC 往返共 **99 ms**（启动本身远低于 50 ms 目标） |
| 空闲内存（release, HTTP 模式） | ✅ WorkingSet **7.18 MB** / Private 2.14 MB（目标 < 50 MB） |
| HTTP transport | ✅ `POST /mcp` → **200** + `text/event-stream` 握手成功，session 正常创建 |
| HTTP 响应编码 | ✅ 字节为正确 UTF-8（`instructions` 中文解码无误）；但 `Content-Type` 未带 `charset=utf-8`，见 §9 第 9 条 |

### 7.3 S1 期间修复的真实缺陷

| 缺陷 | 现象 | 修复 |
|---|---|---|
| 空值环境变量绕过 serde 默认值 | 本机环境有 `TZ=`（定义了但为空），导致 `cs_ping` 输出空时区 | `Config::from_env` 改用 `envy::from_iter` + 过滤空值，并新增 `normalize()` 兜底；配 3 个回归测试 |
| `dead_code` allow 重复 | clippy `duplicated_attributes` 报错 | 删除 `main.rs` 上 `mod ratelimit;` 的外层 `#[allow(dead_code)]`，仅保留文件内 `#![allow(dead_code)]` |
| 常量断言 | clippy `assertions_on_constants` 报错 | 测试改用 `assert!(parsed.is_ok())` 运行时断言 |

### 7.4 S2 验证记录（2026-11，真实 PandaScore 数据）

| tool | 结果 | 证据 |
|---|---|---|
| `cs_ping` | ✅ | `pandascore_ready=true`，`providers=[pandascore:10, liquipedia:20]` |
| `cs_live_matches` | ✅ | count=5（`TheMongolz vs MOUZ @ 2026-11-08T15:32+08:00`） |
| `cs_upcoming_matches`(72h) | ✅ | count=36，连续两轮复测均成功 |
| `cs_today_schedule` | ✅ | count=36 |
| `cs_team_matches`(MOUZ) | ✅ | count=4–5，含终局比分（`MOUZ 2-0 BetBoom`） |
| `cs_recent_results`(48h) | ⚠️ | 成功一次 count=79（`Teletubisie 2-0 XI Esport`）；后续 4 次撞网络超时 |
| `cs_team_matches`(天禄) | ⚠️ | `NO_DATA`——数据源里 TYLOO 的比赛**全部是 canceled** |
| `cs_match_detail` | ⚠️ | PandaScore 对部分 id 返回 **403** → 当前映射为 `AUTH_ERROR` |
| `cs_world_ranking` | ⏳ | `NO_DATA`（排名只能走 Liquipedia，属 S4） |

> 所有时间均为 `+08:00` RFC3339；比分只出现在 live/finished 场次；`maps` 恒为空（数据源边界）。

### 7.5 S2 期间修复的真实缺陷

| # | 缺陷 | 现象 | 修复 |
|---|---|---|---|
| 1 | **`envy` 会把环境变量名小写化**，而字段上写了 `#[serde(rename = "PANDASCORE_TOKEN")]` | serde 找全大写键永远匹配不上，**整个 Config 静默回落到默认值**；表现为 token 配了却 `pandascore_ready=false` | 去掉全部 `rename`，字段用 snake_case 靠大小写不敏感匹配；新增回归测试 `env_vars_bind_case_insensitively` |
| 2 | API 返回 `"streams_list": null`（而非 `[]`） | `error decoding response body`，整条链路失败 | 新增 `null_to_default` 反序列化，`opponents`/`results`/`streams_list` 同时容忍 `null` 与缺失 |
| 3 | `/csgo/matches/past` 被脏数据污染 | 100 条里 97 条是 `canceled` 且 `begin_at` 为 null，按 `-begin_at` 排序全排最前 → 「最近赛果」永远为空 | 改用服务端 `filter[status]=finished` + `sort=-scheduled_at` |
| 4 | 聚合器返回的是**最后一个**源的错误 | pandascore 失败后 liquipedia 兜底，最终错误显示「liquipedia 尚未实现」——真正有行动价值的错误被掩盖 | 改为优先返回**最高优先级源**的错误（`first_err`） |
| 5 | 默认超时 5s 不现实 | 50–100 条响应在本机网络下要 3–8s，被误判为 `[timeout]` | 默认改 15s + `connect_timeout 3s` + 一次短退避重试；错误信息带完整 reqwest source 链（`[timeout]/[connect]/[decode]/[body]`） |

### 7.6 rmcp 升级时的检查点（历史经验，供未来参考）

| 检查点 | 若失败怎么改 |
|---|---|
| feature 名 `transport-streamable-http-server` | `cargo add rmcp --features server,transport-io,macros` 观察实际可用 feature |
| `#[tool_handler(name=…, version=…, instructions=…)]` 参数 | 退化为 `#[tool_handler]` + 手写 `get_info()` |
| `CallToolResult::success/error(..)` 构造器 | 若改名，改用 `From<Vec<ContentBlock>>` |
| `StreamableHttpService::new(...)` 的 factory 返回类型推断 | 显式标注 `move \|\| -> Result<CsServer, std::io::Error> { Ok(h.clone()) }` |
| clippy 在宏生成块里报 `unwrap_used` | 在该 impl 上局部 `#[allow(clippy::unwrap_used)]` 并写明原因 |

---

## 8. 数据源与实测结论（2026-10 实测）

| 源 | 实测 | 结论 |
|---|---|---|
| PandaScore `/csgo/matches/running` | **HTTP 401**（缺 token） | ✅ 端点存活，配 token 即可用；唯一有实时比分/逐图/直播源的源 |
| Liquipedia `counterstrike/api.php` | 泛化 UA → **406**；规范 UA → **200 + gzip** | ✅ 可用；UA 必须为 `产品名/版本 (邮箱)`；要求 ≤1 req/2s |
| HLTV `/matches` | **403 + `cf-mitigated: challenge`**，body 为 `Just a moment...` | ❌ 普通 HTTP 客户端结构性不可用；且 ToS 2.2 禁止爬取 → 默认关闭 |
| bo3.gg `/api/v1/matches` | **403** | ❌ 不可直接用 |
| 5EPlay | **200** | ⚠️ 首页可达，公开接口未验证 → 默认关闭 |

降级链：`PandaScore → Liquipedia → 5EPlay`。全失败返回 `UPSTREAM_ERROR` 且 `meta.degraded=true`。

**硬规则**：`cs_live_matches` 在 PandaScore 不可用时返回 `NO_DATA`，**绝不用 Liquipedia / 5EPlay 伪造实时比分**。

### 8.1 PandaScore 接入要点（S2 实测，写代码前必读）

| 要点 | 细节 |
|---|---|
| ID 命名空间 | `id` 是 int → 对外统一 `pandascore:<id>` |
| 状态映射 | `not_started`→upcoming、`running`→live、`finished`→finished；**`canceled`/`postponed` 一律丢弃**（不猜测映射） |
| 时间字段 | **`begin_at` 可能为 null**（尤其已结束场次），必须回退到 `scheduled_at`，再转 `+08:00` |
| 必加过滤 | `filter[status]=not_started`（upcoming）/ `filter[status]=finished`（past）；否则会混入大量 canceled |
| 必用排序 | past 用 `sort=-scheduled_at`，**不是** `-begin_at`（后者被 null 值污染） |
| 空集合形态 | `opponents` / `results` / `streams_list` 可能是 `null` 而非 `[]`，反序列化必须容忍 |
| 逐图数据 | **`games[]` 只有 `position/status/winner`，没有地图名与逐图比分** → `maps` 恒空 |
| 直播源 | `streams_list[]` 元素含 `{main, raw_url, embed_url, language}`，`main=true` 排前 |
| 队伍匹配 | `GET /csgo/teams?search[name]=<canonical>` 返回多命中（如 `TYLOO` 与 `TYLOO fe`），需按 name/slug 精确匹配 |
| 403 语义 | 单场端点对部分 id 返回 403（与 token 无关），当前映射为 `AUTH_ERROR`，待 S3 细分 |
| 分页 | `per_page` 在慢网络下从 100 降为 50；48h 赛果可能被截断为最新 50 场 |

---

## 9. 已知限制

1. **`maps` 恒为空（数据源边界）**：PandaScore 的 CS2 `games[]` 只含 `position/status/winner`，**没有地图名与逐图比分**。留空是如实行为，不得编造；需要逐图数据只能换源。
2. **`cs_recent_results` 上游不稳定**：`/csgo/matches/past` 在本机网络下高频 `[timeout]`/`[connect]`，实测成功率约 1/5（成功时能返回 48 小时内 79 场）。已加一次重试，S3 的 moka 缓存会进一步缓解。
3. **`cs_match_detail` 的 403 语义偏保守**：PandaScore 对部分 match id 返回 403（与 token 无关），当前统一映射为 `AUTH_ERROR`。更准确的做法是区分「token 问题」与「该比赛不可查」，留待 S3 细化。
4. **`cs_world_ranking` 未实现**：排名只能走 Liquipedia（需规范 UA + ≤1 req/2s），属 S4。届时 `Ranking.stale=true`，可能滞后 1–7 天。
5. **中文队名覆盖有限**：`name_zh` 来自内置静态别名表（约 50 个常见队），未收录输出 `null`，**不做机器翻译**。新增队伍请改 `src/aliases.rs`。
6. **HLTV 不可用**：403 + `cf-mitigated: challenge`，且 ToS 2.2 禁止爬取 → 默认关闭（`ENABLE_HLTV_SCRAPE=0`）。需要 HLTV 原始数据只能走 TLS 指纹伪装 sidecar 或代理，不在本 crate 范围。
7. **缓存与限流尚未接入聚合链**：`moka` / `governor` 代码已就绪且有单元测试，但 S2 阶段每次 tool 调用仍会真实打上游，属 S3。
8. **分页截断**：`per_page` 从 100 降为 50（适配慢网络），超长时间窗口（如 168 小时）的赛果可能被截断为最新 50 场。
9. **`TZ` 等空值环境变量**已由 `Config::normalize()` 兜底，不会再产生空时区；但**新增配置项时请同步扩充 `normalize()`**。
10. **HTTP 响应的 `Content-Type` 不带 `charset=utf-8`**：字节是正确的 UTF-8，标准 MCP 客户端无碍；若遇中文乱码的客户端，在 S5 加一层 tower 中间件改写为 `text/event-stream; charset=utf-8`。
11. **rmcp 迭代快**：升级小版本前先跑 `cargo build` + Inspector，重点看 transport 与宏参数（见 §7.6）。
12. **数据源为第三方聚合**：PandaScore 对小型赛事（尤其亚洲二线）覆盖有延迟，个别场次状态可能滞后于实际。

---

## 10. 目录结构

```
cs-mcp/
├── Cargo.toml
├── .env.example
├── README.md
├── LICENSE
└── src/
    ├── main.rs          # 启动、tracing（stderr）、transport 选择
    ├── server.rs        # CsServer + 8 个 tool + 参数校验 + #[tool_handler]
    ├── aggregator.rs    # Provider trait + 降级链（错误优先取最高优先级源）
    ├── aliases.rs       # 队名别名表：中文俗称 ↔ 英文规范名（name_zh 不机翻）
    ├── models.rs        # Match / Team / Tournament / ApiSuccess / ApiFailure / Meta
    ├── error.rs         # ErrorCode / ApiError / ProviderError（thiserror）
    ├── config.rs        # envy + dotenvy 配置（空值兜底 + normalize）
    ├── cache.rs         # moka TTL 缓存（S3 接入）
    ├── ratelimit.rs     # governor 按 host 限流（S3 接入）
    └── providers/
        ├── mod.rs
        ├── pandascore.rs   # 主源，S2 已实现 6 个 tool
        ├── liquipedia.rs   # 辅源（S4）
        └── hltv.rs         # 默认禁用
```

---

## 11. 质量门禁

- 生产路径不得出现 `unwrap()` / `expect()` / `panic!`（`[lints.clippy]` 已设为 warn，`cargo clippy -- -D warnings` 会把它们变成失败）。
- `release` profile **不得**设置 `panic = "abort"`（会让任一 panic 杀掉进程，违背「不崩」铁律）。
- 所有失败返回结构化错误 JSON，进程不退出。
