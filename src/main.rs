mod hl;
mod momentum;
mod paper;
mod store;
mod web;

use crate::hl::{CoinMeta, HlClient, MarketCtx};
use crate::paper::PaperState;
use crate::store::Store;
use crate::web::{AppState, MetaCache, RefreshStatus};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{error, info};

/// Start of daily-candle backfill (Hyperliquid perp launch era).
const CANDLE_START_MS: i64 = 1688000000000; // 2023-06-29
/// Coins with current 24h volume above this are "liquid" (prioritized + paper universe).
const LIQUID_VOL_USD: f64 = 1_000_000.0;

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
    let paper_path = std::env::var("STARS_PAPER")
        .unwrap_or_else(|_| "/var/lib/stars/paper.json".to_string());

    let store = Arc::new(Store::open(std::path::Path::new(&db_path))?);
    let client = HlClient::new();
    let paper = Arc::new(Mutex::new(PaperState::load(std::path::Path::new(&paper_path))));
    let meta = Arc::new(Mutex::new(MetaCache::default()));
    let refresh = Arc::new(Mutex::new(RefreshStatus::default()));

    let state = AppState {
        store: store.clone(),
        paper: paper.clone(),
        paper_path: Arc::new(std::path::PathBuf::from(&paper_path)),
        meta: meta.clone(),
        refresh: refresh.clone(),
    };

    // ===== background refresh task =====
    {
        let store = store.clone();
        let client = client.clone();
        let meta = meta.clone();
        let refresh = refresh.clone();
        let paper = paper.clone();
        let paper_path = paper_path.clone();
        tokio::spawn(async move {
            refresh_loop(store, client, meta, refresh, paper, paper_path).await;
        });
    }

    // ===== HTTP =====
    let app = web::router(state);
    let addr = "0.0.0.0:3000";
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("stars listening on http://{addr}");
    info!("db: {db_path}, paper: {paper_path}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn refresh_loop(
    store: Arc<Store>,
    client: HlClient,
    meta: Arc<Mutex<MetaCache>>,
    refresh: Arc<Mutex<RefreshStatus>>,
    paper: Arc<Mutex<PaperState>>,
    paper_path: String,
) {
    // Initial full backfill.
    if let Err(e) = backfill(&store, &client, &meta, &refresh).await {
        error!("initial backfill failed: {e}");
    }

    // Periodic refresh: re-fetch contexts + latest candles for liquid coins,
    // and step the paper trade when new daily data lands.
    let mut ticker = tokio::time::interval(Duration::from_secs(1800));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        if let Err(e) = refresh_liquid(&store, &client, &meta).await {
            error!("refresh failed: {e}");
        }
        // advance paper trade on fresh data
        let panel = store
            .all_panels()
            .unwrap_or_default()
            .into_iter()
            .map(|(coin, candles)| crate::momentum::PanelEntry { coin, candles })
            .collect::<Vec<_>>();
        let max_lev: std::collections::HashMap<String, u32> = {
            let m = meta.lock().await;
            m.universe
                .iter()
                .map(|c| (c.name.clone(), c.max_leverage))
                .collect()
        };
        let mut p = paper.lock().await;
        if p.running {
            crate::paper::step(&mut p, &panel, &max_lev);
            let _ = p.save(std::path::Path::new(&paper_path));
        }
    }
}

/// One-shot full backfill: liquid coins first, then delisted, then the rest.
async fn backfill(
    store: &Arc<Store>,
    client: &HlClient,
    meta: &Arc<Mutex<MetaCache>>,
    refresh: &Arc<Mutex<RefreshStatus>>,
) -> anyhow::Result<()> {
    let (universe, ctxs) = refresh_meta(client, meta).await?;

    // Order: liquid, delisted, then remaining active.
    let mut liquid: Vec<String> = Vec::new();
    let mut delisted: Vec<String> = Vec::new();
    let mut rest: Vec<String> = Vec::new();
    for u in &universe {
        let vol = ctxs
            .iter()
            .find(|c| c.coin == u.name)
            .map(|c| c.day_ntl_vlm)
            .unwrap_or(0.0);
        if u.is_delisted {
            delisted.push(u.name.clone());
        } else if vol >= LIQUID_VOL_USD {
            liquid.push(u.name.clone());
        } else {
            rest.push(u.name.clone());
        }
    }
    let mut ordered = Vec::new();
    ordered.extend(liquid);
    ordered.extend(delisted);
    ordered.extend(rest);

    let total = ordered.len();
    let now = now_ms();
    {
        let mut r = refresh.lock().await;
        r.phase = "backfill".into();
        r.coins_total = total;
        r.coins_done = 0;
    }

    for (i, coin) in ordered.iter().enumerate() {
        {
            let mut r = refresh.lock().await;
            r.current = coin.clone();
            r.coins_done = i;
        }
        match client.daily_candles(coin, CANDLE_START_MS, now).await {
            Ok(candles) => {
                if let Err(e) = store.upsert_candles(coin, &candles) {
                    error!("cache {coin}: {e}");
                }
            }
            Err(e) => error!("candles {coin}: {e}"),
        }
    }
    {
        let mut r = refresh.lock().await;
        r.phase = "idle".into();
        r.coins_done = total;
        r.current = String::new();
    }
    Ok(())
}

async fn refresh_liquid(
    store: &Arc<Store>,
    client: &HlClient,
    meta: &Arc<Mutex<MetaCache>>,
) -> anyhow::Result<()> {
    let (_universe, _ctxs) = refresh_meta(client, meta).await?;
    let liquid = meta.lock().await.liquid.clone();
    let now = now_ms();
    let from = now - 7 * 86_400_000;
    for coin in &liquid {
        match client.daily_candles(coin, from, now).await {
            Ok(candles) => {
                let _ = store.upsert_candles(coin, &candles);
            }
            Err(e) => error!("refresh {coin}: {e}"),
        }
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    Ok(())
}

async fn refresh_meta(
    client: &HlClient,
    meta: &Arc<Mutex<MetaCache>>,
) -> anyhow::Result<(Vec<CoinMeta>, Vec<MarketCtx>)> {
    let universe = client.universe().await?;
    let ctxs = client.market_ctxs().await?;
    let (fee_taker, fee_maker) = client.fee_schedule().await.unwrap_or((0.00045, 0.00015));
    let liquid: Vec<String> = ctxs
        .iter()
        .filter(|c| !c.is_delisted && c.day_ntl_vlm >= LIQUID_VOL_USD)
        .map(|c| c.coin.clone())
        .collect();
    let mut m = meta.lock().await;
    m.universe = universe.clone();
    m.ctxs = ctxs.clone();
    m.liquid = liquid;
    m.refreshed_at = now_ms();
    m.fee_taker = fee_taker;
    m.fee_maker = fee_maker;
    Ok((universe, ctxs))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
