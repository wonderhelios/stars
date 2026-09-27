mod error;
mod okx;
mod paper;
mod research;
mod signal;
mod state;
mod types;
mod web;

use rust_decimal::Decimal;
use state::AppState;
use std::str::FromStr;
use std::sync::Arc;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let args: Vec<String> = std::env::args().collect();
    let db_path_owned = std::env::var("OKX_QUANT_DB")
        .unwrap_or_else(|_| "/var/lib/okx-quant/paper.sqlite".to_string());
    let db_path = db_path_owned.as_str();
    // 确保目录存在
    if let Some(parent) = std::path::Path::new(db_path).parent() {
        let _ = std::fs::create_dir_all(parent);
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
                    "run" => paper::run_daemon(db_path, threshold).await,
                    "scan" => paper::run_scan_once(db_path, threshold).await,
                    "report" => {
                        let db = paper::PaperDb::open(db_path)?;
                        paper::report(&db).await
                    }
                    other => {
                        eprintln!("未知子命令: paper {}", other);
                        eprintln!("用法: paper [run|scan|report]");
                        Ok(())
                    }
                };
            }
            _ => {}
        }
    }

    // 默认 serve 模式
    let state = AppState::new();
    let paper_db = Arc::new(paper::PaperDb::open(db_path)?);

    // 后台同时跑纸上交易的扫描与跟踪
    let threshold = Decimal::from_str(signal::FUNDING_THRESHOLD_STR)?;
    let paper_for_daemon = paper_db.clone();
    tokio::spawn(async move {
        // 扫描任务
        let client = Arc::new(okx::RestClient::new());
        let mut scan_ticker = tokio::time::interval(std::time::Duration::from_secs(300));
        let mut track_ticker = tokio::time::interval(std::time::Duration::from_secs(60));

        loop {
            tokio::select! {
                _ = scan_ticker.tick() => {
                    if let Err(e) = paper::scan_once(&client, &paper_for_daemon, threshold).await {
                        tracing::error!("scan_once: {}", e);
                    }
                }
                _ = track_ticker.tick() => {
                    if let Err(e) = paper::update_open_signals(&client, &paper_for_daemon).await {
                        tracing::error!("update_open_signals: {}", e);
                    }
                }
            }
        }
    });

    // 实时行情 WS
    let ws_state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = okx::ws::run_forever(ws_state).await {
            tracing::error!("ws task exited: {}", e);
        }
    });

    // HTTP 服务
    let web_state = web::api::WebState {
        app: state.clone(),
        paper: paper_db,
    };
    let app = web::api::router(web_state);
    let addr = "0.0.0.0:3000";
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("Web server listening on http://{}", addr);
    info!("纸上交易自动运行中，数据库={}", db_path);

    axum::serve(listener, app).await?;
    Ok(())
}
