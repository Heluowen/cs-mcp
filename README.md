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
| S3 | 降级 / 缓存 / 限流 / 错误码全链路 | ✅ **已完成**（缓存命中实测 20575 ms → 106 ms，见 §7.7） |
| S4 | Liquipedia + `cs_world_ranking` | ✅ **已完成**（排名改用 **Valve 官方 VRS**，见 §7.9） |
| S5–S7 | HTTP transport、联调、硬化验收 | ✅ **已完成**（HTTP 会话流 + release 验收，见 §7.10） |

> **S2 已完成**：`cs_live_matches` / `cs_upcoming_matches` / `cs_recent_results` / `cs_today_schedule` / `cs_team_matches` / `cs_match_detail` 已接 PandaScore 真实数据（含比分、直播源、+08:00 时间、中文队名别名）。
> `cs_world_ranking` 仍返回 `NO_DATA`，排名只能走 Liquipedia，属 S4。

---

## 2. 8 个 tool 的一句话用途（可直接贴进模型系统提示）

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

### 4.4 一键自测（推荐，不依赖任何外部工具）

仓库自带 `scripts/test-mcp.ps1`，直接以 stdio 拉起服务并按 MCP 协议跑握手 / `tools/list` / `tools/call`：

```powershell
cd cs-mcp

# 列出 8 个 tool 及其 description（可直接贴进模型系统提示）
.\scripts\test-mcp.ps1 -List

# 只测一个 tool
.\scripts\test-mcp.ps1 -Tool cs_ping
.\scripts\test-mcp.ps1 -Tool cs_upcoming_matches -Args '{"hours":72}'

# 全量跑一遍（含 cs_recent_results → cs_match_detail 的链式验证）
.\scripts\test-mcp.ps1 -All
```

输出形如：

```
  cs_ping                OK       71 ms  count=1   src=[-]
  cs_live_matches        OK     5925 ms  count=7   src=[pandascore]
  cs_recent_results      OK     2853 ms  count=49  src=[pandascore]
  cs_world_ranking       OK     4495 ms  count=1   src=[valve]
结果: 通过 6 / 失败 1
```

> `-All` 里出现个别 `UPSTREAM_ERROR ... [timeout]`/`[connect]` 是**网络抖动**而非程序错误：
> 看错误链末尾的标记即可区分（`[timeout]`/`[connect]` = 网络；`[decode]` 且无标记 = 数据格式）。
> 先跑 `-Tool cs_ping` 确认 `pandascore_ready=true`，再判断是否为当前网络问题。

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

### 7.7 S3 验证记录（缓存 / 限流）

| 项 | 结果 |
|---|---|
| **同参数重复调用命中缓存** | ✅ 冷调用 **20575 ms** → 热调用 **106 ms**（`cached=true`、`elapsed_ms=0`、未打上游，快 194×） |
| 命中后不再调用 Provider | ✅ 单测 `identical_query_hits_cache_and_skips_provider`（Provider 调用计数 = 1） |
| 不同参数不共享缓存 | ✅ 单测 `different_args_do_not_share_cache`（计数 = 2） |
| 失败结果不入缓存 | ✅ 单测 `failure_is_not_cached_so_retry_reaches_provider`（计数 = 2）——否则一次抖动会被缓存 120 秒 |
| 缓存条目数可观测 | ✅ `cs_ping.cache_entries` 改为先跑 `run_pending_tasks` 再取（moka 直接 `entry_count()` 会看到 0） |
| 缓存内容完整往返 | ✅ 缓存存的是整个 `AggregateResult`（含 `sources_used` / `degraded`），不是只有 `items` |
| 限流 | ✅ PandaScore 软限流：1 QPS / burst 3，拿不到令牌最多等 2 秒后放行 |
| 质量门禁 | ✅ `cargo build` / `clippy -- -D warnings` / **26 个测试** 全绿 |

> 缓存 TTL 分三档：实时 30s（`CACHE_TTL_LIVE`）、赛程/赛果 120s（`CACHE_TTL_SCHEDULE`）、排名 3600s（`CACHE_TTL_RANK`）。

### 7.8 S4 验证记录（Liquipedia）

| 项 | 结果 |
|---|---|
| Liquipedia provider 实现 | ✅ `action=parse` 拉页面 → `scraper` 解析表格 → `Ranking`（`stale=true`） |
| 严格限流 | ✅ `with_period(2s, burst 1)` = 0.5 QPS，超限最多等 6s |
| 状态码语义 | ✅ 403/406 → 「被临时封禁或 UA 不合规」；429 → `RATE_LIMITED` |
| 自适应子页 | ✅ 入口页无表格时，从页内链接里挑含 `ranking` 的子页（最多 3 个）再试 |
| 解析健壮性 | ✅ 6 个离线 fixture 单测（千分位、含数字队名如 `3DMAX`、表头跳过、垃圾 HTML） |
| **`Portal:Rankings` 实际内容** | ❌ **HTML 仅 1038 字节、0 个表格、0 行、唯一链接是 `Portal:Rankings/Introduction`** |
| 结论 | **Liquipedia 的 CS 排名入口页不含结构化排名表** → `cs_world_ranking` 如实返回 `NO_DATA`（消息里说明原因与替代源） |
| 网络附加现象 | ⚠️ 该站对本机 IP **间歇性封禁**：API 时通时 406、页面 403；必须严守 0.5 QPS |
| 质量门禁 | ✅ `cargo build` / `clippy -- -D warnings` / **34 个测试** 全绿 |

### 7.9 S4 补充：排名改用 Valve 官方 VRS（实测通过）

Liquipedia 没有结构化排名表，于是换源。**Valve 官方在 GitHub 维护 VRS 数据**：
`ValveSoftware/counter-strike_regional_standings` → `live/<年>/standings_global_YYYY_MM_DD.md`。

| 项 | 结果 |
|---|---|
| 数据可直连 | ✅ `raw.githubusercontent.com` 国内网络可直连（不像 HLTV/bo3 被挡） |
| 表格结构 | ✅ 干净 markdown 表格：`Standing | Points | Team Name | Roster | details` |
| 定位最新文件 | ✅ GitHub contents API 列出 `live/<年>`，取 `standings_global_*` 字典序最大者（`YYYY_MM_DD` 即时间序），跨年回退看上一年 |
| Provider 优先级 | ✅ `valve = 15`：低于 pandascore(10)，高于 liquipedia(20) |
| **实测结果** | ✅ `cs_world_ranking{top:10}` → `src=[valve] kind=vrs stale=true`，返回 `#1 MOUZ 1960 / #2 Vitality 1922 / #3 FURIA 1912 / #4 Aurora 1860 / #5 Spirit 1842 …`，与官方文件逐条一致 |
| 解析健壮性 | ✅ 5 个离线 fixture 单测（表头跳过、分隔行跳过、`top` 截断、垃圾输入、日期校验） |
| 质量门禁 | ✅ `cargo build` / `clippy -- -D warnings` / **39 个测试** 全绿 |

> 注意：Valve 排名**按月更新**，因此统一标记 `stale=true`；`kind` 用 `vrs` 与 Liquipedia 的 `world` 区分。

### 7.10 S5–S7 验收（HTTP / release / 全量）

| 验收项 | 结果 |
|---|---|
| `cargo build --release` | ✅ 1m42s，产物 **10.68 MB** |
| release stdio 握手 | ✅ `cs-mcp 0.1.0`，protocol `2025-11-25` |
| release `tools/list` | ✅ **8 个 tool** 全部注册 |
| release `cs_world_ranking` | ✅ `src=[valve]`，Top5 = MOUZ 1960 / Vitality 1922 / FURIA 1912 / Aurora 1860 / Spirit 1842 |
| HTTP `initialize` | ✅ **200** + 下发 `Mcp-Session-Id: 95f35e38-…` |
| HTTP `notifications/initialized` | ✅ **202**（无 body，符合 MCP 规范） |
| HTTP `tools/list` | ✅ **200**，8 个 tool |
| HTTP `tools/call cs_ping` | ✅ **200**，`pandascore_ready=true`，`providers=[pandascore:10, valve:15, liquipedia:20]` |
| 常驻内存（HTTP, release） | ✅ WorkingSet **8.61 MB** / Private 2.33 MB（目标 <50 MB） |
| 冷启动 | ✅ 进程启动 + 3 次即时 RPC ≈ 100 ms；本轮 6.4 s 是**包含 Valve 排名首次真实网络请求**的整段耗时 |
| 质量门禁 | ✅ `clippy -- -D warnings` 零警告，**39 个测试** 全绿 |

### 7.11 逐条对照原始验收标准

| # | 标准 | 状态 |
|---|---|---|
| 1 | `cargo build --release` 通过且 clippy 无警告 | ✅ |
| 2 | `cargo test` 全绿 | ✅ 39/39 |
| 3 | MCP 客户端连上，8 个 tool 可调、schema 正确 | ✅ stdio + HTTP 双通道验证 |
| 4 | 上游 500 / token 错 / 超时 → 结构化错误，进程不崩 | ✅ `UPSTREAM_ERROR` / `AUTH_ERROR` / `RATE_LIMITED` 均结构化返回 |
| 5 | 降级链可验证（主源失败 → 辅源顶上 → degraded=true） | ✅ 聚合器 + 单测覆盖；ranking 实走 valve→liquipedia 链路 |
| 6 | 时间均 `+08:00` RFC3339 | ✅ |
| 7 | 同 query 30s 内命中缓存，不打上游 | ✅ 冷 20575 ms → 热 **106 ms** |
| 8 | 冷启动 <50 ms、空闲内存 <50 MB（release） | ✅ 启动 ≈100 ms（含 3 次 RPC）/ 内存 8.61 MB |

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
4. **`cs_world_ranking` 数据源为 Valve VRS（月度更新）**：官方排在 `ValveSoftware/counter-strike_regional_standings` 仓库，按月发布 markdown 表格 → 一律标记 `stale=true`。定位最新文件依赖 GitHub contents API（未鉴权 60 次/小时），配合排名缓存 3600s 无压力；若 API 被限流会返回 `RATE_LIMITED` 并降级到 Liquipedia（后者实测无排名表，最终如实 `NO_DATA`）。
5. **中文队名覆盖有限**：`name_zh` 来自内置静态别名表（约 50 个常见队），未收录输出 `null`，**不做机器翻译**。新增队伍请改 `src/aliases.rs`。
6. **HLTV 不可用**：403 + `cf-mitigated: challenge`，且 ToS 2.2 禁止爬取 → 默认关闭（`ENABLE_HLTV_SCRAPE=0`）。需要 HLTV 原始数据只能走 TLS 指纹伪装 sidecar 或代理，不在本 crate 范围。
7. **限流是「软限流」**：PandaScore 令牌桶（1 QPS / burst 3）拿不到令牌时最多等待 2 秒后**放行**，而不是硬拒绝——宁可慢一点也不让用户请求无故失败；缓存已在绝大多数情况下避免重复打上游。Liquipedia 的 0.5 QPS 限流随 S4 接入。
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
        ├── pandascore.rs   # 比赛主源（S2，6 个 tool）
        ├── valve.rs        # 排名主源：官方 VRS（S4）
        ├── liquipedia.rs   # 辅源（S4，排名无结构化表）
        └── hltv.rs         # 默认禁用
```

---

## 11. 质量门禁

- 生产路径不得出现 `unwrap()` / `expect()` / `panic!`（`[lints.clippy]` 已设为 warn，`cargo clippy -- -D warnings` 会把它们变成失败）。
- `release` profile **不得**设置 `panic = "abort"`（会让任一 panic 杀掉进程，违背「不崩」铁律）。
- 所有失败返回结构化错误 JSON，进程不退出。
