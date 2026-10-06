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
        exec_gate: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        store: store.clone(),
        paper: paper.clone(),
        paper_path: Arc::new(std::path::PathBuf::from(&paper_path)),
        live: live.clone(),
        live_path: Arc::new(std::path::PathBuf::from(&live_path)),
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
                // 换仓节奏必须按「UTC 日」判断，不能按「距上次运行多久」。
                //
                // 之前用 last_run_at 算间隔，而手动调仓也会更新它 —— 结果一次手动
                // 调仓就把自动调仓往后推了整整一个周期（实测把 10-06 00:05 那次
                // 推到了 10-08，组合两天没人管）。按 UTC 日的模数判断则与手动操作
                // 完全无关。
                let n = snapshot.config.rebalance_days.max(1) as i64;
                if n > 1 && (now_ms() / DAY) % n != 0 {
                    info!("实盘自动调仓跳过：按 {n} 天节奏，今天不是调仓日");
                    continue;
                }
                // 与手动调仓共用同一道执行闸门。否则自动调仓和手动点击会交错，
                // 两边都在对方写入前读到同一个账户，于是发出两倍的「开仓」单。
                let _gate = match st.exec_gate.try_lock() {
                    Ok(g) => g,
                    Err(_) => {
                        info!("实盘自动调仓跳过：已有订单操作在执行中");
                        continue;
                    }
                };
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
                            pnl: 0.0,
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

    // ===== 持仓量快照采集（只记录，绝不下单）=====
    //
    // 官方 API 不提供 OI 历史，只能拿当前快照。第三方有历史但要付费，且免费档
    // 只给 30 天 —— 不足以回测。所以这里每天记一次，攒够几个月后才有真正
    // 的样本外数据来判断「持仓量拥挤度」是不是有效因子。
    {
        let store = store.clone();
        let http = state.http.clone();
        tokio::spawn(async move {
            loop {
                match snapshot_open_interest(&store, &http).await {
                    Ok(n) => info!("持仓量快照：写入 {n} 个币"),
                    Err(e) => error!("持仓量快照失败: {e}"),
                }
                // 6 小时跑一次，按 UTC 日对齐，同一天重复写入会覆盖
                tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
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
        // 与网页端同一区间：仓位数过大时 k 会超过半宇宙，重叠区被判成多头，
        // 组合会变成净多头。
        cfg.target_positions = v.parse::<usize>().context("--positions")?.clamp(1, 30);
    }
    if let Some(v) = value("--leverage") {
        cfg.leverage = v.parse().context("--leverage")?;
    }
    if let Some(v) = value("--lookback") {
        cfg.lookback = v.parse().context("--lookback")?;
    }
    if let Some(v) = value("--top") {
        cfg.top_frac = v.parse::<f64>().context("--top")?.clamp(0.02, 0.5);
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

    let equity_hint: f64 = std::env::var("HL_ACCOUNT_ADDRESS")
        .ok()
        .and_then(|_| None)
        .unwrap_or_else(|| {
            std::env::args()
                .collect::<Vec<_>>()
                .windows(2)
                .find(|w| w[0] == "--capital")
                .and_then(|w| w[1].parse().ok())
                .unwrap_or(500.0)
        });
    let (weights, liquid) = trader::target_weights(&panel, &cfg, equity_hint, i64::MAX);
    anyhow::ensure!(!weights.is_empty(), "没有选出候选（流动性过滤后为空）");
    println!(
        "流动宇宙 {} 币 · 多头腿 {} · 空头腿 {}",
        liquid.len(),
        weights.iter().filter(|(_, w)| *w > 0.0).count(),
        weights.iter().filter(|(_, w)| *w < 0.0).count()
    );

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
    let mut coins: Vec<String> = weights.iter().map(|(c, _)| c.clone()).collect();
    coins.extend(acct.positions.keys().cloned());
    coins.sort();
    coins.dedup();
    let mids = trader::fetch_mids(&exec, &coins).await;

    let plan = trader::build_plan(&weights, &acct, &markets, &mids, &cfg, None);
    // 调仓前撤掉自己的旧止盈单（与网页端同一流程）；失败即中止。
    if live {
        match live::cancel_our_take_profits(&exec).await {
            Ok(n) if n > 0 => println!("撤销旧止盈单 {n} 个"),
            Ok(_) => {}
            Err(e) => return Err(e.context("撤销旧止盈单失败，已在调仓前中止")),
        }
    }
    println!(
        "\n目标：每腿 {} 仓 · 每仓 ${:.2} · 目标总名义 ${:.0} · 杠杆 {}x · 保证金缓冲 {:.0}%",
        weights.iter().filter(|(_, w)| *w > 0.0).count(),
        plan.per_coin,
        plan.gross_notional,
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
    // 调仓后按「本次调仓的参考价」挂止盈单，与网页端同一流程。
    // 止盈幅度取持久化的实盘配置，保证 CLI 与网页端一致
    let live_path = std::env::var("STARS_LIVE").unwrap_or_else(|_| "live.json".into());
    let tp_pct = crate::live::LiveState::load(std::path::Path::new(&live_path))
        .config
        .take_profit_pct;
    let tp_ref: std::collections::HashMap<String, f64> = plan
        .long_leg
        .iter()
        .chain(plan.short_leg.iter())
        .filter_map(|c| mids.get(c).map(|m| (c.clone(), *m)))
        .collect();
    match live::place_take_profits(&exec, &markets, tp_pct, &tp_ref).await {
        Ok(log) => {
            for l in log {
                println!("  {l}");
            }
        }
        Err(e) => println!("  止盈单挂单失败: {e}"),
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



/// 抓一次全市场的当前持仓量快照并落库。只看公开行情，不涉及账户、不下任何单。
async fn snapshot_open_interest(
    store: &Arc<Store>,
    http: &reqwest::Client,
) -> anyhow::Result<usize> {
    let v: serde_json::Value = http
        .post("https://api.hyperliquid.xyz/info")
        .json(&serde_json::json!({"type": "metaAndAssetCtxs"}))
        .send()
        .await?
        .json()
        .await?;
    let universe = v[0]["universe"].as_array().cloned().unwrap_or_default();
    let ctxs = v[1].as_array().cloned().unwrap_or_default();
    let mut rows: Vec<(String, f64, f64)> = Vec::new();
    for (u, c) in universe.iter().zip(ctxs.iter()) {
        if u["isDelisted"].as_bool().unwrap_or(false) {
            continue;
        }
        let Some(coin) = u["name"].as_str() else { continue };
        if coin.contains(':') {
            continue; // HIP-3 命名空间不在交易范围内
        }
        let oi = c["openInterest"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        let px = c["markPx"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        if oi <= 0.0 || px <= 0.0 {
            continue;
        }
        rows.push((coin.to_string(), oi * px, px)); // 统一存 USD 名义
    }
    anyhow::ensure!(!rows.is_empty(), "快照为空，可能是接口异常");
    store.upsert_oi_snapshot(now_ms(), &rows)
}
