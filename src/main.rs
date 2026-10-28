mod combine;
mod exchange;
mod hlcarry;
mod health;
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
use tracing::{error, info, warn};

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
    let auth_token = std::env::var("STARS_TOKEN").ok().filter(|v| !v.trim().is_empty());
    if auth_token.is_none() {
        warn!("未设置 STARS_TOKEN：HTTP 接口【没有任何鉴权】✗ 公网可访问时请务必设置");
    }
    let paper2_path = std::env::var("STARS_PAPER2")
        .unwrap_or_else(|_| "/var/lib/stars/paper2.json".to_string());
    let paper3_path = std::env::var("STARS_PAPER3")
        .unwrap_or_else(|_| "/var/lib/stars/paper3.json".to_string());
    let live_path = std::env::var("STARS_LIVE")
        .unwrap_or_else(|_| "/var/lib/stars/live.json".to_string());
    // HL 单账户 carry 影子（现货 + 空永续，赚资金费）。默认预算是影子规模，不涉真钱。
    let hlcarry_path = std::env::var("STARS_HLCARRY")
        .unwrap_or_else(|_| "/var/lib/stars/hlcarry.json".to_string());

    // ===== trade subcommand (live execution) =====
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("trade") {
        return run_trade(&args, &db_path).await;
    }

    let store = Arc::new(Store::open(std::path::Path::new(&db_path))?);
    let client = HlClient::new();
    let paper = Arc::new(Mutex::new(PaperState::load(std::path::Path::new(&paper_path))));
    let base_cfg = paper.lock().await.config.clone().unwrap_or_default();
    let mut paper2 = PaperState::load(std::path::Path::new(&paper2_path));
    if paper2.config.is_none() {
        // 影子组合：动量 + 距高点 + 上涨占比 三因子等权（动量权重置 1 使量能为 0）。
        let base = base_cfg.clone();
        paper2.config = Some(crate::paper::PaperConfig {
            momentum_weight: 1.0,
            dd_weight: 1.0,
            upfrac_weight: 1.0,
            capital: base.capital,
            // 与实盘同宇宙（$10M ✗）✗ 否则和实盘/回测都不可比 ✗
            min_vol_usd: 10_000_000.0,
            // 影子组合的默认【不回放】✗ 从启动当天开始记 ✗
            // 回放 90 天是 in-sample 预览 ✗ 不是样本外验证 ✗
            replay_days: 1,
            ..base
        });
        paper2.equity = base.capital;
        paper2.running = true;
    }
    let paper2 = Arc::new(Mutex::new(paper2));
    let mut paper3 = PaperState::load(std::path::Path::new(&paper3_path));
    if paper3.config.is_none() {
        paper3.config = Some(crate::paper::PaperConfig {
            vol_target: 0.02,
            momentum_weight: 1.0,
            dd_weight: 0.0,
            upfrac_weight: 0.0,
            capital: base_cfg.capital,
            min_vol_usd: 10_000_000.0,
            replay_days: 1,
            ..base_cfg.clone()
        });
        paper3.equity = base_cfg.capital;
        paper3.running = true;
    }
    let paper3 = Arc::new(Mutex::new(paper3));
    let combine_path = std::env::var("STARS_COMBINE")
        .unwrap_or_else(|_| "/var/lib/stars/combine.json".to_string());
    let combine = Arc::new(Mutex::new(crate::combine::CombineState::load_or_init(
        std::path::Path::new(&combine_path),
        5000.0,
    )));
    let hl_live = live::LiveState::load(std::path::Path::new(&live_path));
    let live = Arc::new(Mutex::new(hl_live));
    let hlcarry = Arc::new(Mutex::new(crate::hlcarry::HlCarryState::load(std::path::Path::new(&hlcarry_path))));
    let meta = Arc::new(Mutex::new(MetaCache::default()));
    let refresh = Arc::new(Mutex::new(RefreshStatus::default()));

    let state = AppState {
        exec_gate: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        run_gate: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        store: store.clone(),
        paper: paper.clone(),
        paper_path: Arc::new(std::path::PathBuf::from(&paper_path)),
        auth_token: auth_token.clone(),
        paper2: paper2.clone(),
        paper2_path: Arc::new(std::path::PathBuf::from(&paper2_path)),
        paper3: paper3.clone(),
        paper3_path: Arc::new(std::path::PathBuf::from(&paper3_path)),
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
        combine: combine.clone(),
        combine_path: Arc::new(std::path::PathBuf::from(&combine_path)),
    };

    // ===== HL 单账户 carry 影子：每 5 分钟推进一步 =====
    //
    // 只读公开行情 + 模拟持仓，不发任何真实订单。
    // 尚未初始化时按资金分配里的 carry 金额自动开仓；影子账户一旦初始化，
    // 就保留页面设置的预算与空仓状态，不再每轮用实盘预算覆盖或重置历史。
    {
        let st = state.clone();
        tokio::spawn(async move {
            loop {
                match crate::hlcarry::fetch_market(&st.http).await {
                    Ok(market) => {
                        let now = crate::live::now_ms_pub();
                        let budget = {
                            st.live.lock().await.capital_alloc.carry_usd
                        };
                        {
                            let mut hc = st.hlcarry.lock().await;
                            let previous = hc.clone();
                            if hc.start_equity <= 0.0 {
                                // 尚未设置影子预算时，才从实盘资金分配读取初始金额。
                                if budget > 0.0 {
                                    hc.open(budget, &market, now);
                                }
                            } else if !hc.legs.is_empty() {
                                hc.step(&market, now);
                                // 先计提资金费与盯市（equity 依赖它们），再评估换仓。
                                hc.maybe_rebalance(&market, now);
                            }
                            // 状态锁持有到落盘完成，避免慢的旧快照覆盖刚提交的新预算。
                            if let Err(e) = hc.save(&st.hlcarry_path) {
                                *hc = previous;
                                warn!("HL carry 影子保存失败: {e}");
                            }
                        }
                    }
                    Err(e) => warn!("HL carry 影子行情失败: {e}"),
                }
                tokio::time::sleep(Duration::from_secs(300)).await;
            }
        });
    }

    // ===== 净值曲线记录器（Hyperliquid）=====
    //
    // 曲线以前只在页面 handler 里记点：不看页面就不记，断档几小时；并发请求还会
    // 写出重复时间戳。改成后台定时器，与页面无关。record_equity 内部按小时去重，
    // 所以每 5 分钟跑一次、实际一小时只落一个点。
    {
        let st = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(300)).await;
                if st.live.lock().await.config.account.is_empty() {
                    continue;
                }
                if let Err(e) =
                    live::record_equity_now(&st.live, &st.live_path).await
                {
                    warn!("记录净值点失败: {e}");
                }
            }
        });
    }

    // ===== 净敞口保护：每 5 分钟检查一次 =====
    //
    // 市场中性组合在【止盈成交 / 手动平仓 / 强平】之后会剩下单边敞口，
    // 在下一次日调仓前一直带着方向性风险。这里只做减仓（reduce_only），
    // 把偏多/偏空的那条腿按名义比例减掉，让净敞口回到 ≈0，不新增任何开仓。
    {
        let st = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(300)).await;
                let cfg = st.live.lock().await.config.clone();
                if !(cfg.armed && cfg.auto_run && cfg.can_sign()) {
                    continue;
                }
                // 与日调仓/手动调仓共用执行闸门，避免两边同时下单。
                let _gate = match st.exec_gate.try_lock() {
                    Ok(g) => g,
                    Err(_) => continue,
                };
                let exec = match crate::exchange::Exec::signer_config(&cfg).await {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let acct = match exec.account().await {
                    Ok(a) => a,
                    Err(e) => {
                        warn!("净敞口检查读账户失败: {e}");
                        continue;
                    }
                };
                if acct.positions.is_empty() {
                    continue;
                }
                let coins: Vec<String> = acct.positions.keys().cloned().collect();
                let mids = crate::trader::fetch_mids(&exec, &coins).await;
                let markets = crate::live::markets_from_meta(&st.meta.lock().await.universe);
                if markets.is_empty() {
                    continue;
                }
                let orders = crate::trader::neutralize_orders(&acct, &markets, &mids, 0.02, 10.0);
                for o in orders {
                    match exec
                        .ioc(&o.coin, o.buy, true, o.size, o.mid, cfg.slippage, o.sz_decimals)
                        .await
                    {
                        Ok(_) => {
                            info!(
                                "净敞口中性化：{} {} {:.6}（{}）",
                                if o.buy { "买" } else { "卖" },
                                o.coin,
                                o.size,
                                o.reason
                            );
                            // 把中性化订单也写进记录 ✗ 否则对账时它们只会显示成
                            // "交易所补录 · 意图未确认"✗ 用户看不懂这几笔是怎么来的 ✓
                            let mut g = st.live.lock().await;
                            g.records.push(crate::live::LiveRecord {
                                ts: crate::live::now_ms_pub(),
                                coin: o.coin.clone(),
                                side: if o.buy { "买".into() } else { "卖".into() },
                                action: "净敞口中性化".into(),
                                size: o.size,
                                price: o.mid,
                                notional: o.size * o.mid,
                                result: format!("按中间价 {:.6} 发送", o.mid),
                                live: true,
                                pnl: None,
                                tid: None,
                                entry_px: acct.positions.get(&o.coin).map(|p| p.entry_px),
                                reduce_only: true,
                            });
                            let _ = g.save(&st.live_path);
                        }
                        Err(e) => warn!("中性化下单失败 {}: {e}", o.coin),
                    }
                }
            }
        });
    }

    // ===== daily live rebalance task =====
    {
        let st = state.clone();
        let client = client.clone();
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
                // 每日调仓。曾经改成「每 2 日」以省一半换手，但回测显示那个
                // 结果完全取决于 2 天周期的相位（相位 A Sharpe 2.07 / 相位 B 1.42，
                // 只差错开一天），不是真实边际，已回退。
                // 与手动调仓共用同一道执行闸门。否则自动调仓和手动点击会交错，
                // 两边都在对方写入前读到同一个账户，于是发出两倍的「开仓」单。
                let _gate = match st.exec_gate.try_lock() {
                    Ok(g) => g,
                    Err(_) => {
                        info!("实盘自动调仓跳过：已有订单操作在执行中");
                        continue;
                    }
                };
                // 调仓前**强制刷新一次日线**。
                //
                // 审计指出：30 分钟一次的刷新可能在 23:30 跑过，而 00:05 的调仓
                // 早于下一次刷新 —— 缓存里昨天那根只更新到 23:30，但它的 open
                // timestamp 已满足「已收盘」，于是被 last_closed_index 当成完整
                // 日线使用，量冲击/动量/波动都用的是残缺数据。
                info!("调仓前强制刷新日线…");
                if let Err(e) = refresh_liquid(&st.store, &client, &st.meta).await {
                    warn!("调仓前刷新失败：{e} —— 为避免用残缺日线，本次跳过调仓");
                    continue;
                }
                // 刷新后再确认最新一根已收盘日线确实是「昨天」，而不是更早
                // （否则说明刷新没拿到新数据，信号会是旧的）。
                {
                    let panel = match trader::load_panel(&st.store) {
                        Ok(p) => p,
                        Err(e) => {
                            warn!("调仓前读面板失败：{e}，跳过");
                            continue;
                        }
                    };
                    let fp = trader::FactorPanel::build(&panel);
                    let i = fp.last_closed_index(now_ms());
                    let last = fp.ts.get(i).copied().unwrap_or(0);
                    let expect = now_ms() / 86_400_000 * 86_400_000 - 86_400_000;
                    if last < expect {
                        warn!(
                            "最新已收盘日线是 {}，早于预期的 {} —— 数据不新鲜，跳过本次调仓",
                            last, expect
                        );
                        continue;
                    }
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
                        for (coin, until) in result.stopped.iter() {
                            guard.stop_cooldowns.insert(coin.clone(), *until);
                        }
                        guard.last_plan = result.plan_lines.clone();
                        guard.last_live = true;
                        guard.records.extend(records);
                        guard.tp_oids.extend(result.tp_oids.clone());
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
                // 睡到下一个 UTC 00:05 ✗ 让样本的日内时刻固定 ✗ 重启也不漂移
                // 中途每 30 分钟醒一次补跑 ✗ 保证当天至少有样本（幂等覆盖 ✗ 无副作用）
                loop {
                    let now = now_ms();
                    let day = now / 86_400_000 * 86_400_000;
                    let next_target = day + 86_400_000 + 5 * 60_000; // 次日 00:05 UTC
                    let wait = ((next_target - now).max(30 * 60_000)).min(6 * 3600 * 1000);
                    tokio::time::sleep(Duration::from_millis(wait as u64)).await;
                    if now_ms() >= next_target - 60_000 {
                        break;
                    }
                }
            }
        });
    }

    // ===== 1h 数据采集（供组合策略的区间位置因子用 ✗ 只拉流动性最好的 40 个币）=====
    {
        let store = store.clone();
        let client = client.clone();
        let meta = meta.clone();
        tokio::spawn(async move {
            loop {
                // 启动时 meta 可能还没刷新 ✗ liquid 是空的 ✗ 先刷一次 ✓
                let mut coins: Vec<String> = meta.lock().await.liquid.iter().take(40).cloned().collect();
                if coins.is_empty() {
                    if let Err(e) = refresh_meta(&client, &meta).await {
                        warn!("1h 采集前刷新 meta 失败: {e}");
                    }
                    coins = meta.lock().await.liquid.iter().take(40).cloned().collect();
                }
                if coins.is_empty() {
                    // 还是拿不到 ✗ 一分钟后再试 ✗ 不要睡 6 小时 ✓
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    continue;
                }
                let now = now_ms();
                let from = now - 200 * 86_400_000;
                for coin in &coins {
                    match client.candles(coin, "1h", from, now, 400 * 3_600_000).await {
                        Ok(cs) => {
                            let _ = store.upsert_candles_1h(coin, &cs);
                        }
                        Err(e) => warn!("1h {coin}: {e}"),
                    }
                    tokio::time::sleep(Duration::from_millis(120)).await;
                }
                let n = store.all_panels_1h().map(|p| p.len()).unwrap_or(0);
                info!("1h 采集：{n} 个币");
                tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
            }
        });
    }

    // ===== 组合策略纸面步进（每天 ✗ 用 store 的日线 + 1h 面板）=====
    {
        let store = store.clone();
        let combine = combine.clone();
        tokio::spawn(async move {
            loop {
                if let (Ok(panel_1d), Ok(panel_1h)) = (store.all_panels(), store.all_panels_1h()) {
                    let d: Vec<crate::momentum::PanelEntry> = panel_1d
                        .into_iter()
                        .map(|(coin, candles)| crate::momentum::PanelEntry { coin, candles })
                        .collect();
                    let h: Vec<crate::momentum::PanelEntry> = panel_1h
                        .into_iter()
                        .map(|(coin, candles)| crate::momentum::PanelEntry { coin, candles })
                        .collect();
                    let mut st = combine.lock().await;
                    if st.running {
                        let before = st.history.len();
                        crate::combine::step(&mut st, &d, &h);
                        // 还没有历史（刚部署）时 ✗ 至少把当前仓位算出来给页面看 ✓
                        if st.history.len() < 2 {
                            crate::combine::preview(&mut st, &d, &h);
                        }
                        if st.history.len() != before {
                            st.save(combine_path.as_ref());
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(600)).await;
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
        let paper2 = paper2.clone();
        let paper2_path = paper2_path.clone();
        tokio::spawn(async move {
            refresh_loop(
                store, client, meta, refresh, paper, paper_path, paper2, paper2_path, paper3,
                paper3_path,
            )
            .await;
        });
    }

    // ===== HTTP =====
    let app = web::router(state);
    let addr = std::env::var("STARS_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
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
    paper2: Arc<Mutex<PaperState>>,
    paper2_path: String,
    paper3: Arc<Mutex<PaperState>>,
    paper3_path: String,
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
        drop(p);
        let mut p2 = paper2.lock().await;
        if p2.running {
            crate::paper::step(&mut p2, &panel, &max_lev);
            let _ = p2.save(std::path::Path::new(&paper2_path));
        }
        drop(p2);
        let mut p3 = paper3.lock().await;
        if p3.running {
            crate::paper::step(&mut p3, &panel, &max_lev);
            let _ = p3.save(std::path::Path::new(&paper3_path));
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
        Ok((log, _oids)) => {
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
/// 额外记两样：
///   · 每个币的当前资金费率（funding ✗ 元数据里就有 ✗ 零额外请求）——注意是【采样时刻的小时费率】✗ 不是日费率
///   · 30 个流动性最好的币的盘口汇总（点差 + 前 5 档深度 ✗ 30 次 l2Book 请求）
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
    if universe.len() != ctxs.len() {
        warn!("metaAndAssetCtxs 长度不一致: universe {} vs ctx {}", universe.len(), ctxs.len());
    }
    let mut rows: Vec<(String, f64, f64, f64)> = Vec::new();
    let mut ranked: Vec<(f64, String)> = Vec::new(); // (OI 美元 ✗ 币名) 用来挑盘口样本
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
        let fund = c["funding"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        if oi <= 0.0 || px <= 0.0 {
            continue;
        }
        let oi_usd = oi * px;
        rows.push((coin.to_string(), oi_usd, px, fund));
        ranked.push((oi_usd, coin.to_string()));
    }
    anyhow::ensure!(!rows.is_empty(), "快照为空，可能是接口异常");
    let snap_ts = now_ms(); // 同一轮里 OI / depth / prune 都用这个时刻 ✗ 避免跨午夜错位
    let n = store.upsert_oi_snapshot(snap_ts, &rows)?;

    // 盘口汇总：只取 OI 最大的 30 个币 ✗ 一天一次 ✗ 不存原始盘口 ✓
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut depth_rows: Vec<(String, f64, f64, f64, f64)> = Vec::new();
    for (_, coin) in ranked.iter().take(30) {
        let book: serde_json::Value = match http
            .post("https://api.hyperliquid.xyz/info")
            .json(&serde_json::json!({"type": "l2Book", "coin": coin}))
            .send()
            .await
        {
            Ok(r) => match r.json().await {
                Ok(v) => v,
                Err(_) => continue,
            },
            Err(_) => continue,
        };
        let Some(levels) = book.get("levels").and_then(|x| x.as_array()) else {
            continue;
        };
        let Some(bids) = levels.get(0).and_then(|x| x.as_array()) else {
            continue;
        };
        let Some(asks) = levels.get(1).and_then(|x| x.as_array()) else {
            continue;
        };
        let px = |o: &serde_json::Value| o["px"].as_str().and_then(|s| s.parse::<f64>().ok());
        let sz = |o: &serde_json::Value| o["sz"].as_str().and_then(|s| s.parse::<f64>().ok());
        let (Some(best_bid), Some(best_ask)) = (
            bids.first().and_then(|o| px(o)),
            asks.first().and_then(|o| px(o)),
        ) else {
            continue;
        };
        let bid5: f64 = bids.iter().take(5).filter_map(|o| sz(o).zip(px(o)).map(|(s, p)| s * p)).sum();
        let ask5: f64 = asks.iter().take(5).filter_map(|o| sz(o).zip(px(o)).map(|(s, p)| s * p)).sum();
        if !(best_bid > 0.0 && best_ask > 0.0 && best_ask > best_bid) {
            continue;
        }
        let mid = (best_bid + best_ask) / 2.0;
        let spread = (best_ask - best_bid) / mid * 10_000.0;
        depth_rows.push((coin.clone(), spread, bid5, ask5, mid));
    }
    // ③ 盘口样本不足要告警 ✗ 不能无声缺一天 ✓
    if depth_rows.len() < 30 {
        warn!("盘口汇总只拿到 {}/30 个币", depth_rows.len());
    }
    // ④ 两处写入独立 ✗ OI 失败不连坐 depth ✓
    if let Err(e) = store.upsert_depth(snap_ts, &depth_rows) {
        warn!("盘口汇总写库失败: {e}");
    }
    // 磁盘保险：只留最近 400 天 ✗ 一年约 5MB 的量级 ✗ 永远不会撑爆 ✓（失败只记日志 ✗ 不连坐本轮结果）
    if let Err(e) = store.prune_snapshots(snap_ts, 400) {
        warn!("快照清理失败: {e}");
    }
    Ok(n)
}
