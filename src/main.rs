mod binance;
mod error;
mod hyperliquid;
mod okx;
mod paper;
mod paper_store;
mod research;
mod signal;
mod state;
mod types;
mod web;

use rust_decimal::Decimal;
use state::AppState;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

/// 单次扫描的整体超时，超过就放弃本轮
const SCAN_TIMEOUT: Duration = Duration::from_secs(120);
/// 单次跟踪的超时
const TRACK_TIMEOUT: Duration = Duration::from_secs(60);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let args: Vec<String> = std::env::args().collect();
    let okx_db_path = std::env::var("OKX_QUANT_DB")
        .unwrap_or_else(|_| "/var/lib/okx-quant/paper.sqlite".to_string());
    let binance_db_path = std::env::var("OKX_QUANT_BINANCE_DB")
        .unwrap_or_else(|_| "/var/lib/okx-quant/binance.sqlite".to_string());
    let hl_db_path = std::env::var("OKX_QUANT_HL_DB")
        .unwrap_or_else(|_| "/var/lib/okx-quant/hyperliquid.sqlite".to_string());

    for path in [&okx_db_path, &binance_db_path, &hl_db_path] {
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent)?;
        }
    }

    if args.len() > 1 {
        match args[1].as_str() {
            "research" => {
                if args.len() > 2 && args[2] == "scan" {
                    let top_n: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(200);
                    return research::run_scan(top_n).await;
                }
                return research::run(&args[2..]).await;
            }
            "paper" => {
                let threshold = Decimal::from_str(signal::FUNDING_THRESHOLD_STR)?;
                let sub = args.get(2).map(|s| s.as_str()).unwrap_or("run");
                return match sub {
                    "run" => paper::run_daemon(&okx_db_path, threshold).await,
                    "scan" => paper::run_scan_once(&okx_db_path, threshold).await,
                    "report" => {
                        let db = paper::PaperDb::open(&okx_db_path)?;
                        paper::report(&db).await
                    }
                    other => {
                        eprintln!("未知子命令: paper {}", other);
                        Ok(())
                    }
                };
            }
            "binance" => {
                let threshold = Decimal::from_str(signal::FUNDING_THRESHOLD_STR)?;
                let sub = args.get(2).map(|s| s.as_str()).unwrap_or("run");
                return match sub {
                    "run" => binance::paper::run_daemon(&binance_db_path, threshold).await,
                    other => {
                        eprintln!("未知子命令: binance {}", other);
                        Ok(())
                    }
                };
            }
            "hyperliquid" => {
                let threshold = Decimal::from_str(signal::FUNDING_THRESHOLD_STR)?;
                let sub = args.get(2).map(|s| s.as_str()).unwrap_or("run");
                return match sub {
                    "run" => hyperliquid::paper::run_daemon(&hl_db_path, threshold).await,
                    other => {
                        eprintln!("未知子命令: hyperliquid {}", other);
                        Ok(())
                    }
                };
            }
            _ => {}
        }
    }

    // ===== 默认 serve 模式 =====
    let state = AppState::new();
    let okx_paper_db = Arc::new(paper::PaperDb::open(&okx_db_path)?);
    let binance_paper_db = Arc::new(binance::paper::BinancePaperDb::open(&binance_db_path)?);
    let hl_paper_db = Arc::new(hyperliquid::paper::HyperliquidPaperDb::open(&hl_db_path)?);

    let threshold = Decimal::from_str(signal::FUNDING_THRESHOLD_STR)?;

    // ===== OKX（t=0s 启动）=====
    {
        let db = okx_paper_db.clone();
        let th = threshold;
        tokio::spawn(async move {
            let client = Arc::new(okx::RestClient::new());
            let mut scan_ticker = tokio::time::interval(Duration::from_secs(300));
            let mut track_ticker = tokio::time::interval(Duration::from_secs(60));
            // 首次立即触发
            scan_ticker.tick().await;
            track_ticker.tick().await;
            loop {
                tokio::select! {
                    _ = scan_ticker.tick() => {
                        match timeout(SCAN_TIMEOUT, paper::scan_once(&client, &db, th)).await {
                            Ok(Ok(n)) => info!("OKX 扫描: {} 触发", n),
                            Ok(Err(e)) => error!("OKX scan: {}", e),
                            Err(_) => error!("OKX scan timeout"),
                        }
                    }
                    _ = track_ticker.tick() => {
                        match timeout(TRACK_TIMEOUT, paper::update_open_signals(&client, &db)).await {
                            Ok(Ok(_)) => {}
                            Ok(Err(e)) => error!("OKX track: {}", e),
                            Err(_) => error!("OKX track timeout"),
                        }
                    }
                }
            }
        });
    }

    // ===== 币安（t=100s 启动）=====
    {
        let db = binance_paper_db.clone();
        let th = threshold;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(100)).await;
            let client = Arc::new(binance::BinanceRestClient::new());
            let mut scan_ticker = tokio::time::interval(Duration::from_secs(300));
            let mut track_ticker = tokio::time::interval(Duration::from_secs(60));
            loop {
                tokio::select! {
                    _ = scan_ticker.tick() => {
                        match timeout(SCAN_TIMEOUT, binance::paper::scan_once(&client, &db, th)).await {
                            Ok(Ok(n)) => info!("BN 扫描: {} 触发", n),
                            Ok(Err(e)) => error!("BN scan: {}", e),
                            Err(_) => error!("BN scan timeout"),
                        }
                    }
                    _ = track_ticker.tick() => {
                        match timeout(TRACK_TIMEOUT, binance::paper::update_open_signals(&client, &db)).await {
                            Ok(Ok(_)) => {}
                            Ok(Err(e)) => error!("BN track: {}", e),
                            Err(_) => error!("BN track timeout"),
                        }
                    }
                }
            }
        });
    }

    // ===== Hyperliquid（t=200s 启动）=====
    {
        let db = hl_paper_db.clone();
        let th = threshold;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(200)).await;
            let client = Arc::new(hyperliquid::HyperliquidRestClient::new());
            let mut scan_ticker = tokio::time::interval(Duration::from_secs(300));
            let mut track_ticker = tokio::time::interval(Duration::from_secs(60));
            loop {
                tokio::select! {
                    _ = scan_ticker.tick() => {
                        match timeout(SCAN_TIMEOUT, hyperliquid::paper::scan_once(&client, &db, th)).await {
                            Ok(Ok(n)) => info!("HL 扫描: {} 触发", n),
                            Ok(Err(e)) => error!("HL scan: {}", e),
                            Err(_) => error!("HL scan timeout"),
                        }
                    }
                    _ = track_ticker.tick() => {
                        match timeout(TRACK_TIMEOUT, hyperliquid::paper::update_open_signals(&client, &db)).await {
                            Ok(Ok(_)) => {}
                            Ok(Err(e)) => error!("HL track: {}", e),
                            Err(_) => error!("HL track timeout"),
                        }
                    }
                }
            }
        });
    }

    // ===== OKX WebSocket 实时行情 =====
    let ws_state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = okx::ws::run_forever(ws_state).await {
            error!("ws task exited: {}", e);
        }
    });

    // ===== HTTP 服务 =====
    let web_state = web::api::WebState {
        app: state.clone(),
        okx_paper: okx_paper_db,
        binance_paper: binance_paper_db,
        hl_paper: hl_paper_db,
    };
    let app = web::api::router(web_state);
    let addr = "0.0.0.0:3000";
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("Web server listening on http://{}", addr);
    info!("OKX DB: {}", okx_db_path);
    info!("Binance DB: {}", binance_db_path);
    info!("Hyperliquid DB: {}", hl_db_path);

    axum::serve(listener, app).await?;
    Ok(())
}
