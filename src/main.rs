//! `cs-mcp` 入口：初始化日志 → 载入配置 → 选择 transport → 启动。
//!
//! 关键约束：**stdio 模式下 stdout 属于协议通道**，任何日志/调试输出都必须走 stderr，
//! 否则会污染 JSON-RPC 流导致客户端立即断开。

mod aggregator;
mod cache;
mod config;
mod error;
mod models;
mod providers;
mod ratelimit;
mod server;

use anyhow::Result;
use rmcp::ServiceExt;

use crate::config::Config;
use crate::server::CsServer;

/// 初始化 tracing（强制输出到 stderr）。
fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
}

fn print_help() {
    // 帮助信息也走 stderr，避免 stdio 模式下污染 stdout。
    eprintln!(
        "cs-mcp {}\n\
         \n\
         用法:\n\
         \x20 cs-mcp                        # 默认 stdio（供 MCP 客户端拉起）\n\
         \x20 cs-mcp --transport http --bind 0.0.0.0:8080\n\
         \n\
         环境变量:\n\
         \x20 PANDASCORE_TOKEN     PandaScore token（缺失则实时比分不可用）\n\
         \x20 ENABLE_HLTV_SCRAPE   0|1，默认 0（HLTV 有 CF 挑战且 ToS 禁爬）\n\
         \x20 HTTP_TIMEOUT_SECS     上游超时，默认 5\n\
         \x20 CACHE_TTL_SCHEDULE   赛程缓存 TTL，默认 120\n\
         \x20 CACHE_TTL_LIVE       实时缓存 TTL，默认 30\n\
         \x20 CACHE_TTL_RANK       排名缓存 TTL，默认 3600\n\
         \x20 HTTP_PROXY            出站代理，如 http://127.0.0.1:7890\n\
         \x20 RUST_LOG              tracing 过滤，如 info / debug",
        server::SERVER_VERSION
    );
}

/// 解析命令行：`--transport <stdio|http>`、`--bind <addr>`、`--help`。
fn parse_cli(default_transport: &str, default_bind: &str) -> (String, String) {
    let mut transport = default_transport.to_string();
    let mut bind = default_bind.to_string();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--transport" => {
                if let Some(v) = args.next() {
                    transport = v;
                }
            }
            "--bind" => {
                if let Some(v) = args.next() {
                    bind = v;
                }
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => {
                if !other.starts_with('-') {
                    tracing::warn!(arg = %other, "忽略未知参数");
                }
            }
        }
    }

    (transport, bind)
}

async fn serve_stdio(cfg: Config) -> Result<()> {
    let server = CsServer::new(cfg)?;
    tracing::info!(version = server::SERVER_VERSION, "cs-mcp 启动（stdio）");

    let service = server.serve(rmcp::transport::stdio()).await?;
    let reason = service.waiting().await;
    tracing::info!("cs-mcp stdio 会话结束");
    reason?;
    Ok(())
}

async fn serve_http(cfg: Config, bind: &str) -> Result<()> {
    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService,
    };

    let server = CsServer::new(cfg)?;
    let handle = server.clone();

    // 注：这里刻意只用 default()，不依赖 with_json_response 等可选构造器，
    // 以降低 rmcp 小版本间的 API 漂移风险。
    let config = StreamableHttpServerConfig::default();
    let service = StreamableHttpService::new(
        move || Ok(handle.clone()),
        LocalSessionManager::default().into(),
        config,
    );

    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(addr = %bind, endpoint = "/mcp", "cs-mcp 启动（http）");

    axum::serve(listener, router).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let cfg = Config::from_env()?;
    let (transport, bind) = parse_cli(&cfg.mcp_transport, &cfg.mcp_bind);

    tracing::debug!(
        transport = %transport,
        bind = %bind,
        pandascore_ready = cfg.has_pandascore(),
        "配置加载完成"
    );

    match transport.as_str() {
        "http" => serve_http(cfg, &bind).await,
        "stdio" => serve_stdio(cfg).await,
        other => {
            // 不 panic：打印可读错误并按 stdio 兜底。
            tracing::error!(transport = %other, "未知 transport，回退到 stdio");
            serve_stdio(cfg).await
        }
    }
}
