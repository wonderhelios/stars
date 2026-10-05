mod exchange;
mod hl;
mod live;
mod momentum;
mod paper;
mod portfolio;
mod probe;
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
    let probe_path = std::env::var("STARS_PROBE")
        .unwrap_or_else(|_| "/var/lib/stars/probe.json".to_string());

    // ===== trade subcommand (live execution) =====
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("trade") {
        return run_trade(&args, &db_path).await;
    }

    let store = Arc::new(Store::open(std::path::Path::new(&db_path))?);
    let client = HlClient::new();
    let paper = Arc::new(Mutex::new(PaperState::load(std::path::Path::new(&paper_path))));
    let live = Arc::new(Mutex::new(live::LiveState::load(std::path::Path::new(&live_path))));
    let probe = Arc::new(Mutex::new(probe::ProbeState::load(std::path::Path::new(&probe_path))));
    let meta = Arc::new(Mutex::new(MetaCache::default()));
    let refresh = Arc::new(Mutex::new(RefreshStatus::default()));

    let state = AppState {
        store: store.clone(),
        paper: paper.clone(),
        paper_path: Arc::new(std::path::PathBuf::from(&paper_path)),
        live: live.clone(),
        live_path: Arc::new(std::path::PathBuf::from(&live_path)),
        probe: probe.clone(),
        probe_path: Arc::new(std::path::PathBuf::from(&probe_path)),
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .pool_max_idle_per_host(8)
            .build()?,
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

    // ===== monthly-hourly backfill + probe loop =====
    {
        let store = store.clone();
        let meta = meta.clone();
        let refresh = refresh.clone();
        let probe = probe.clone();
        let probe_path = probe_path.clone();
        // 回填独立运行，不阻塞探测器
        {
            let store = store.clone();
            let client = HlClient::new();
            let meta = meta.clone();
            let refresh = refresh.clone();
            tokio::spawn(async move {
                // 启动后先等 3 分钟，避免用户刚部署就操作时和回填抢配额
                tokio::time::sleep(Duration::from_secs(180)).await;
                if let Err(e) = backfill_hourly(&store, &client, &meta, &refresh).await {
                    error!("小时线回填失败: {e}");
                }
                loop {
                    tokio::time::sleep(Duration::from_secs(1800)).await;
                    if let Err(e) = backfill_hourly(&store, &client, &meta, &refresh).await {
                        error!("小时线增量更新失败: {e}");
                    }
                }
            });
        }
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(120)).await;
                // 每 4 小时推进一轮探测器
                let now = now_ms();
                let due = {
                    let p = probe.lock().await;
                    let step = p.config.rebal_h.max(1) as i64 * 3_600_000;
                    match p.last_run_at {
                        None => true,
                        Some(t) => now - t >= step,
                    }
                };
                if !due {
                    continue;
                }
                match run_probe_once(&store, &probe).await {
                    Ok(msg) => info!("探测器: {msg}"),
                    Err(e) => {
                        error!("探测器失败: {e}");
                        let mut p = probe.lock().await;
                        p.last_error = Some(format!("{e}"));
                    }
                }
                let p = probe.lock().await;
                let _ = p.save(std::path::Path::new(&probe_path));
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
    let delisted_set: std::collections::HashSet<String> = delisted.iter().cloned().collect();
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
        // Skip the full re-download so a restart does not re-fetch hundreds of
        // coins and trip the exchange rate limit. Active coins only need recent
        // data; a delisted coin's history never changes once it is cached.
        let cached = store.coin_latest_ts(coin).ok().flatten();
        let fresh = match cached {
            Some(_) if delisted_set.contains(coin) => true,
            Some(t) => t >= now - 2 * 86_400_000,
            None => false,
        };
        if fresh {
            continue;
        }
        match client.daily_candles(coin, CANDLE_START_MS, now).await {
            Ok(candles) => {
                if let Err(e) = store.upsert_candles(coin, &candles) {
                    error!("cache {coin}: {e}");
                }
            }
            Err(e) => error!("candles {coin}: {e}"),
        }
        // Pace the backfill: unthrottled it fires hundreds of requests at once.
        tokio::time::sleep(Duration::from_millis(80)).await;
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
        tokio::time::sleep(Duration::from_millis(80)).await;
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
    let outcome = trader::execute(&exec, &plan, &cfg, &markets, true).await?;
    for line in &outcome.prelim {
        println!("  注意: {line}");
    }
    for line in &outcome.orders {
        if !line.is_empty() {
            println!("  {line}");
        }
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



/// 回填小时线（探测器用）。只做流动性宇宙，已新鲜的跳过。
async fn backfill_hourly(
    store: &Arc<Store>,
    client: &HlClient,
    meta: &Arc<Mutex<MetaCache>>,
    refresh: &Arc<Mutex<RefreshStatus>>,
) -> anyhow::Result<()> {
    // 等主回填把宇宙准备好
    for _ in 0..60 {
        if !meta.lock().await.liquid.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    let liquid = meta.lock().await.liquid.clone();
    if liquid.is_empty() {
        anyhow::bail!("流动性宇宙为空，跳过小时线回填");
    }
    let now = now_ms();
    let from = now - 210 * 86_400_000; // 210 天：留足 30 天量能窗口后仍有 ~180 天可用于组合分析
    {
        let mut r = refresh.lock().await;
        r.phase = "hourly".into();
        r.coins_total = liquid.len();
        r.coins_done = 0;
    }
    let total = liquid.len();
    for (i, coin) in liquid.iter().enumerate() {
        {
            let mut r = refresh.lock().await;
            r.current = coin.clone();
            r.coins_done = i;
        }
        let have = store.hourly_latest_ts(coin).ok().flatten();
        let first = store.hourly_first_ts(coin).ok().flatten();
        let fresh = have.map(|t| t >= now - 2 * 3_600_000).unwrap_or(false);
        // 还要检查历史是否够长：只判断「最新是否新鲜」会跳过更早的补拉
        let need_older = first.map(|t| t > from + 3_600_000).unwrap_or(true);
        if fresh && !need_older {
            continue;
        }
        if need_older {
            let end = first.unwrap_or(now);
            match client.hourly_candles(coin, from, end).await {
                Ok(cs) => {
                    let _ = store.upsert_hourly(coin, &cs);
                }
                Err(e) => error!("小时线补历史 {coin}: {e}"),
            }
            tokio::time::sleep(Duration::from_millis(80)).await;
        }
        if !fresh {
            let start = have.map(|t| t + 1).unwrap_or(now - 3 * 86_400_000);
            match client.hourly_candles(coin, start, now).await {
                Ok(cs) => {
                    let _ = store.upsert_hourly(coin, &cs);
                }
                Err(e) => error!("小时线 {coin}: {e}"),
            }
            tokio::time::sleep(Duration::from_millis(80)).await;
        }
    }
    {
        let mut r = refresh.lock().await;
        r.phase = "idle".into();
        r.coins_done = total;
        r.current = String::new();
    }
    info!("小时线回填完成：{total} 币");
    Ok(())
}

/// 推进一轮探测器：先评估上一轮（窗口已结束），再开启新一轮。
async fn run_probe_once(
    store: &Arc<Store>,
    probe: &Arc<Mutex<probe::ProbeState>>,
) -> anyhow::Result<String> {
    let panel = store.hourly_panels()?;
    if panel.len() < 8 {
        anyhow::bail!("小时线不足（{} 币）", panel.len());
    }
    let mut p = probe.lock().await;
    if !p.config.enabled {
        return Ok("已停用".into());
    }
    let cfg = p.config.clone();
    // 1) 评估尚未评估且窗口已结束的轮次
    let now = now_ms();
    let mut done = 0;
    for r in p.rounds.iter_mut() {
        if !r.evaluated && now >= r.ts + cfg.rebal_h as i64 * 3_600_000 {
            probe::evaluate(r, &panel, &cfg);
            if r.evaluated {
                done += 1;
            }
        }
    }
    // 2) 开新轮
    let (longs, shorts, mids) = probe::targets(&panel, &cfg);
    let started = !longs.is_empty();
    if started {
        p.rounds.push(probe::ProbeRound {
            ts: now,
            longs,
            shorts,
            mids,
            ..Default::default()
        });
        if p.rounds.len() > 500 {
            let drop = p.rounds.len() - 500;
            p.rounds.drain(0..drop);
        }
    }
    p.last_run_at = Some(now);
    p.last_error = None;
    Ok(format!("评估 {done} 轮 · 新轮 {}", if started { "已开" } else { "跳过" }))
}
