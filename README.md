# cs-mcp — CS2 赛事 MCP Server（Rust）

把 CS2 赛事数据暴露为 7 个 MCP tool，供任意 MCP 客户端（Claude Desktop / Cursor / AstrBot）直调。

**核心原则：只「取数 + 结构化」，不做文案渲染。** 自然语言由调用方模型组织。

```
cs-mcp  →  PandaScore（主）→ Liquipedia（辅）→ 5EPlay（可选）  →  结构化 JSON
```

---

## 1. 当前进度

| 阶段 | 内容 | 状态 |
|---|---|---|
| S1 | 工程骨架、8 个 tool 全量注册、stdio 打通、`cs_ping` 可用 | ✅ **已完成并验证**（见 §7.1） |
| S2 | PandaScore 真实接入（5 个数据 tool） | ⏳ 待做（需 token） |
| S3 | 降级 / 缓存 / 限流 / 错误码全链路 | ⏳ 待做 |
| S4 | Liquipedia + `cs_world_ranking` + `cs_today_schedule` | ⏳ 待做 |
| S5–S7 | HTTP transport、联调、硬化验收 | ⏳ 待做 |

> S1 阶段：`cs_ping` 返回真实自检信息；其余 7 个 tool **可调、schema 正确**，但因 Provider 尚未实现，返回结构化 `NO_DATA`（这是正确行为，不是故障）。

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
| `cs_match_detail` | `match_id` | 单场详情：逐图比分、地图、直播源 |

调用原则（建议写进系统提示）：

- 实时比分只有 PandaScore 提供；返回 `NO_DATA` 时**如实告知用户**，禁止用其它工具的历史数据替代，禁止编造比分。
- 时间一律 `+08:00`；未开赛的比赛不给比分。
- `meta.sources_used` 是真实来源，`meta.degraded=true` 表示发生了降级，`meta.cached=true` 表示命中缓存——回复时应如实标注来源。

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
| `HTTP_TIMEOUT_SECS` | `5` | 上游超时 |
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

### 7.4 rmcp 升级时的检查点（历史经验，供未来参考）

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

---

## 9. 已知限制

1. **未编译验证**（见 §7）。本仓库当前状态是「可审查的完整骨架」，第一次 `cargo build` 仍需按 §7 清单微调。
2. **无实时比分兜底**：没有 PandaScore token 时，`cs_live_matches` 恒为 `NO_DATA`。这是设计选择，不是 bug。
3. **HLTV 不可用**：Cloudflare 挑战 + ToS 禁爬。需 HLTV 原始数据只能走 TLS 指纹伪装 sidecar 或代理，不在本 crate 范围。
4. **Liquipedia 排名口径滞后**：`Ranking.stale=true`，可能滞后 1–7 天。
5. **中文队名**：`name_zh` 来自内置静态对照表，缺省为 `null`，**不做机器翻译**。
6. **缓存/s 限流在 S3 才接入聚合链**：S1 只有工具函数骨架与单元测试。
7. **HTTP transport 未验证**：S1 已写代码，但按计划 S5 才联调。
8. **rmcp 迭代快**：升级小版本前先跑 `cargo build` + Inspector，重点看 transport 与宏参数。
9. **HTTP 响应的 `Content-Type` 不带 `charset=utf-8`**：实测响应体字节是正确的 UTF-8，但头部只有 `text/event-stream`。标准 MCP 客户端默认按 UTF-8 解析 JSON，通常无碍；若遇到中文描述乱码的客户端，在 S5 加一层 tower 中间件改写成 `text/event-stream; charset=utf-8` 即可。
10. **`TZ` 等环境变量若被定义为空**（`TZ=`）已由 `Config::normalize()` 兜底，不会再产生空时区；但**新增配置项时请同步扩充 `normalize()`**。

---

## 10. 目录结构

```
cs-mcp/
├── Cargo.toml
├── .env.example
├── README.md
└── src/
    ├── main.rs          # 启动、tracing（stderr）、transport 选择
    ├── server.rs        # CsServer + 7 个 tool + 参数校验 + #[tool_handler]
    ├── aggregator.rs    # Provider trait + 降级链
    ├── models.rs        # Match / Team / Tournament / ApiSuccess / ApiFailure / Meta
    ├── error.rs         # ErrorCode / ApiError / ProviderError（thiserror）
    ├── config.rs        # envy + dotenvy 配置
    ├── cache.rs         # moka TTL 缓存（S3 启用）
    ├── ratelimit.rs     # governor 按 host 限流（S2/S3 启用）
    └── providers/
        ├── mod.rs
        ├── pandascore.rs   # 主源（S2）
        ├── liquipedia.rs   # 辅源（S4）
        └── hltv.rs         # 默认禁用
```

---

## 11. 质量门禁

- 生产路径不得出现 `unwrap()` / `expect()` / `panic!`（`[lints.clippy]` 已设为 warn，`cargo clippy -- -D warnings` 会把它们变成失败）。
- `release` profile **不得**设置 `panic = "abort"`（会让任一 panic 杀掉进程，违背「不崩」铁律）。
- 所有失败返回结构化错误 JSON，进程不退出。
