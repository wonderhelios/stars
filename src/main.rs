mod cli;
mod exchange;
mod hlcarry;
mod health;
mod hl;
mod limitoco;
mod live;
mod scheduler;
mod store;
mod trader;
mod web;

use crate::hl::HlClient;
use crate::store::Store;
use crate::web::{AppState, MetaCache, RefreshStatus};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let db_path = std::env::var("STARS_DB")
        .unwrap_or_else(|_| "/var/lib/stars/candles.sqlite".to_string());
    let auth_token = std::env::var("STARS_TOKEN").ok().filter(|v| !v.trim().is_empty());
    if auth_token.is_none() {
        warn!("未设置 STARS_TOKEN：HTTP 接口【没有任何鉴权】✗ 公网可访问时请务必设置");
    }
    let live_path = std::env::var("STARS_LIVE")
        .unwrap_or_else(|_| "/var/lib/stars/live.json".to_string());
    // HL 单账户 carry 影子（现货 + 空永续，赚资金费）。默认预算是影子规模，不涉真钱。
    let hlcarry_path = std::env::var("STARS_HLCARRY")
        .unwrap_or_else(|_| "/var/lib/stars/hlcarry.json".to_string());
    let intraday_path = std::env::var("STARS_INTRADAY")
        .unwrap_or_else(|_| "/var/lib/stars/intraday_shadow.json".to_string());
    let tradfi_path = std::env::var("STARS_TRADFI")
        .unwrap_or_else(|_| "/var/lib/stars/tradfi_shadow.json".to_string());
    let fastswing_path = std::env::var("STARS_FASTSWING")
        .unwrap_or_else(|_| "/var/lib/stars/fastswing_shadow.json".to_string());
    let limitoco_path = std::env::var("STARS_LIMITOCO")
        .unwrap_or_else(|_| "/var/lib/stars/limitoco_shadow.json".to_string());

    // ===== trade subcommand (live execution) =====
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("trade") {
        return cli::run_trade(&args, &db_path).await;
    }

    let store = Arc::new(Store::open(std::path::Path::new(&db_path))?);
    let client = HlClient::new();
    let hl_live = live::LiveState::load(std::path::Path::new(&live_path));
    let live = Arc::new(Mutex::new(hl_live));
    let hlcarry = Arc::new(Mutex::new(crate::hlcarry::HlCarryState::load(std::path::Path::new(&hlcarry_path))));
    let limitoco = Arc::new(Mutex::new(crate::limitoco::LimitOcoState::load(std::path::Path::new(&limitoco_path))));
    let meta = Arc::new(Mutex::new(MetaCache::default()));
    let refresh = Arc::new(Mutex::new(RefreshStatus::default()));

    let state = AppState {
        exec_gate: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        run_gate: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        store: store.clone(),
        auth_token: auth_token.clone(),
        live: live.clone(),
        live_path: Arc::new(std::path::PathBuf::from(&live_path)),
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .pool_max_idle_per_host(8)
            .build()?,
        meta: meta.clone(),
        refresh: refresh.clone(),
        hlcarry: hlcarry.clone(),
        hlcarry_path: Arc::new(std::path::PathBuf::from(&hlcarry_path)),
        intraday_path: Arc::new(std::path::PathBuf::from(&intraday_path)),
        tradfi_path: Arc::new(std::path::PathBuf::from(&tradfi_path)),
        fastswing_path: Arc::new(std::path::PathBuf::from(&fastswing_path)),
        limitoco: limitoco.clone(),
        limitoco_path: Arc::new(std::path::PathBuf::from(&limitoco_path)),
        limitoco_gate: std::sync::Arc::new(tokio::sync::Mutex::new(())),
    };

    scheduler::spawn_all(
        &state,
        store.clone(),
        client,
        meta.clone(),
        refresh.clone(),
    );

    // ===== HTTP =====
    let app = web::router(state);
    let addr = std::env::var("STARS_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("stars listening on http://{addr}");
    info!("db: {db_path}");
    axum::serve(listener, app).await?;
    Ok(())
}

