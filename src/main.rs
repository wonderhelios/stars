mod exchange;
mod hl;
mod live;
mod momentum;
mod paper;
mod store;
mod trader;
mod web;

use anyhow::Context;
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
    let live_path = std::env::var("STARS_LIVE")
        .unwrap_or_else(|_| "/var/lib/stars/live.json".to_string());

    // ===== trade subcommand (live execution) =====
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("trade") {
        return run_trade(&args, &db_path).await;
    }

    let store = Arc::new(Store::open(std::path::Path::new(&db_path))?);
    let client = HlClient::new();
    let paper = Arc::new(Mutex::new(PaperState::load(std::path::Path::new(&paper_path))));
    let live = Arc::new(Mutex::new(live::LiveState::load(std::path::Path::new(&live_path))));
    let meta = Arc::new(Mutex::new(MetaCache::default()));
    let refresh = Arc::new(Mutex::new(RefreshStatus::default()));

    let state = AppState {
        store: store.clone(),
        paper: paper.clone(),
        paper_path: Arc::new(std::path::PathBuf::from(&paper_path)),
        live: live.clone(),
        live_path: Arc::new(std::path::PathBuf::from(&live_path)),
        meta: meta.clone(),
        refresh: refresh.clone(),
    };

    // ===== daily live rebalance task =====
    {
        let st = state.clone();
        tokio::spawn(async move {
            const DAY: i64 = 86_400_000;
            loop {
                let now = now_ms();
                // Next 00:05 UTC — just after the daily candle closes.
                let mut next = now / DAY * DAY + 5 * 60 * 1000;
                if next <= now {
                    next += DAY;
                }
                let wait = (next - now).max(0) as u64;
                info!("实盘定时器：{} 分钟后检查（UTC 00:05 触发）", wait / 60_000);
                tokio::time::sleep(Duration::from_millis(wait)).await;

                let snapshot = st.live.lock().await.clone();
                if !(snapshot.config.auto_run && snapshot.config.armed) {
                    continue;
                }
                info!("实盘自动调仓开始");
                let markets = {
                    let meta = st.meta.lock().await;
                    live::markets_from_meta(&meta.universe)
                };
                match live::run(&st.store, &snapshot, &markets, true).await {
                    Ok((result, records)) => {
                        let mut guard = st.live.lock().await;
                        guard.last_run_at = Some(now_ms());
                        guard.last_plan = result.plan_lines.clone();
                        guard.last_live = true;
                        guard.records.extend(records);
                        guard.history.push(live::EquityPoint {
                            ts: now_ms(),
                            equity: result.equity,
                        });
                        let _ = guard.save(&st.live_path);
                        info!("实盘自动调仓完成：{:?}", result.executed);
                    }
                    Err(e) => error!("实盘自动调仓失败：{e}"),
                }
            }
        });
    }

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

/// `stars trade [--live] [--positions N] [--leverage X] [--slippage 0.005]`
///
/// Defaults to a dry run that prints exactly what would be sent. Live trading
/// requires `--live` plus HL_ACCOUNT_ADDRESS and HL_API_WALLET_KEY_FILE.
async fn run_trade(args: &[String], db_path: &str) -> anyhow::Result<()> {
    let flag = |name: &str| args.iter().position(|a| a == name);
    let value = |name: &str| -> Option<String> {
        flag(name).and_then(|i| args.get(i + 1)).cloned()
    };
    let live = flag("--live").is_some();

    let mut cfg = trader::TradeConfig::default();
    if let Some(v) = value("--positions") {
        cfg.target_positions = v.parse().context("--positions")?;
    }
    if let Some(v) = value("--leverage") {
        cfg.leverage = v.parse().context("--leverage")?;
    }
    if let Some(v) = value("--lookback") {
        cfg.lookback = v.parse().context("--lookback")?;
    }
    if let Some(v) = value("--top") {
        cfg.top_frac = v.parse().context("--top")?;
    }
    if let Some(v) = value("--minvol") {
        cfg.min_vol_usd = v.parse().context("--minvol")?;
    }
    if let Some(v) = value("--slippage") {
        cfg.slippage = v.parse().context("--slippage")?;
    }
    if let Some(v) = value("--buffer") {
        cfg.margin_buffer = v.parse().context("--buffer")?;
    }

    let store = Store::open(std::path::Path::new(db_path))?;
    let panel = trader::load_panel(&store)?;
    anyhow::ensure!(
        panel.len() >= 20,
        "本地 K 线缓存不足（{} 币），先启动一次服务完成回填",
        panel.len()
    );

    let (long, short, liquid) = trader::ranking(&panel, &cfg);
    anyhow::ensure!(!long.is_empty(), "没有选出候选（流动性过滤后为空）");
    println!("流动宇宙 {} 币 · 多头腿 {} · 空头腿 {}", liquid.len(), long.len(), short.len());

    // Account + markets (signing client only when live).
    let addr_env = std::env::var("HL_ACCOUNT_ADDRESS").ok();
    let exec = match (&live, &addr_env) {
        (true, Some(addr)) => {
            let key = std::env::var("HL_API_WALLET_KEY_FILE")
                .context("live 交易需要 HL_API_WALLET_KEY_FILE")?;
            exchange::Exec::signer(addr, std::path::Path::new(&key)).await?
        }
        // Dry run still reads the real account (read-only) so the plan matches.
        (false, Some(addr)) => exchange::Exec::reader_for(Some(addr)).await?,
        (false, None) => exchange::Exec::reader().await?,
        (true, None) => anyhow::bail!("live 交易需要 HL_ACCOUNT_ADDRESS"),
    };
    let markets = exec.markets().await?;

    let acct = if addr_env.is_some() {
        let a = exec.account().await?;
        println!("账户净值 ${:.2} · 现有持仓 {} 个", a.equity, a.positions.len());
        if !a.positions.is_empty() {
            println!("现有仓位：");
            let mut list: Vec<_> = a.positions.iter().collect();
            list.sort_by(|x, y| x.0.cmp(y.0));
            for (coin, pos) in list {
                println!("  {coin}: {:.6}", pos.size);
            }
            if live {
                anyhow::ensure!(
                    flag("--close-existing").is_some(),
                    "账户已有仓位。本程序会把不在目标名单里的仓位全部平掉（reduce-only）。\
                     确认要接管请加 --close-existing；若是别的机器人开的仓，先停掉它并手动清空。"
                );
            } else {
                println!("（dry-run 只展示，不会平掉这些仓位）");
            }
        }
        a
    } else {
        // Dry run without an account: assume the configured capital.
        let equity: f64 = value("--capital")
            .map(|v| v.parse().unwrap_or(2000.0))
            .unwrap_or(2000.0);
        println!("[DRY-RUN] 未读取账户，按 --capital ${equity:.0} 计算（真实运行时用账户净值）");
        exchange::Acct {
            equity,
            positions: Default::default(),
        }
    };

    // Mids for every coin we might touch.
    let mut coins: Vec<String> = long.iter().chain(short.iter()).cloned().collect();
    coins.extend(acct.positions.keys().cloned());
    coins.sort();
    coins.dedup();
    let mids = trader::fetch_mids(&exec, &coins).await;

    let plan = trader::build_plan(&long, &short, &acct, &markets, &mids, &cfg, None);
    println!(
        "\n目标：每腿 {} 仓 · 每仓 ${:.2} · 目标总名义 ${:.0} · 杠杆 {}x · 保证金缓冲 {:.0}%",
        cfg.target_positions.min(long.len().max(1)),
        plan.per_coin,
        plan.per_coin * 2.0 * cfg.target_positions.min(long.len().max(1)) as f64,
        cfg.leverage,
        cfg.margin_buffer * 100.0
    );
    println!("多头腿: {}", plan.long_leg.join(" "));
    println!("空头腿: {}", plan.short_leg.join(" "));
    println!("账户净值 ${:.2}", plan.equity);
    if plan.orders.is_empty() {
        println!("无需调仓（已在目标状态）。");
    } else {
        println!("\n计划下单 {} 笔：", plan.orders.len());
        for o in &plan.orders {
            println!(
                "  {} {} {:.6} @≈{:.6} (${:.2}) · {}",
                if o.buy { "买入" } else { "卖出" },
                o.coin,
                o.size,
                o.mid,
                o.notional,
                o.reason
            );
        }
    }
    for n in &plan.notes {
        println!("  注意: {n}");
    }

    if !live {
        println!("\n[DRY-RUN] 未发送任何订单。确认无误后加 --live 执行。");
        return Ok(());
    }

    println!("\n发送订单…");
    for line in trader::execute(&exec, &plan, &cfg, &markets, true).await? {
        println!("  {line}");
    }
    if let Ok(a) = exec.account().await {
        println!(
            "\n执行后：净值 ${:.2} · 持仓 {} 个",
            a.equity,
            a.positions.len()
        );
    }
    Ok(())
}

