//! Live trading on top of the same signal the paper engine uses.
//!
//! This is the only place in the project that can send real orders. It reuses
//! `trader::ranking` (identical to the paper engine) and `exchange::Exec`, so the
//! live book cannot drift from what was validated.

use crate::exchange::{order_price, round_size, Exec, MarketInfo};
use crate::hl::maintenance_margin_rate;
use crate::trader::{self, TradeConfig};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Build the market map from the cached universe so no meta request is needed.
pub fn markets_from_meta(universe: &[crate::hl::CoinMeta]) -> HashMap<String, MarketInfo> {
    universe
        .iter()
        .filter(|c| !c.is_delisted)
        .map(|c| {
            (
                c.name.clone(),
                MarketInfo {
                    sz_decimals: c.sz_decimals,
                    max_leverage: c.max_leverage,
                },
            )
        })
        .collect()
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LiveConfig {
    pub account: String,
    pub key_path: String,
    pub target_positions: usize,
    pub leverage: f64,
    pub margin_buffer: f64,
    pub slippage: f64,
    pub lookback: usize,
    pub top_frac: f64,
    pub min_vol_usd: f64,
    /// 止盈幅度（相对调仓时的中间价）。0 = 不挂止盈单。
    #[serde(default = "default_tp")]
    pub take_profit_pct: f64,
    /// allow sending real orders; off until the user arms it
    pub armed: bool,
    /// run one rebalance per day automatically
    pub auto_run: bool,
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self {
            account: String::new(),
            key_path: String::new(),
            target_positions: 8,
            leverage: 3.0,
            margin_buffer: 0.90,
            slippage: 0.005,
            lookback: 14,
            top_frac: 0.2,
            min_vol_usd: 5_000_000.0,
            take_profit_pct: 0.10,
            armed: false,
            auto_run: false,
        }
    }
}

fn default_tp() -> f64 {
    0.10
}

impl LiveConfig {
    pub fn trade_config(&self) -> TradeConfig {
        TradeConfig {
            lookback: self.lookback,
            top_frac: self.top_frac,
            min_vol_usd: self.min_vol_usd,
            leverage: self.leverage,
            target_positions: self.target_positions,
            slippage: self.slippage,
            margin_buffer: self.margin_buffer,
            ..Default::default()
        }
    }

    pub fn can_sign(&self) -> bool {
        !self.account.is_empty() && !self.key_path.is_empty()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct EquityPoint {
    pub ts: i64,
    pub equity: f64,
}

/// One order we sent (or planned) and what the exchange said about it.
#[derive(Clone, Serialize, Deserialize)]
pub struct LiveRecord {
    pub ts: i64,
    pub coin: String,
    pub side: String,
    pub action: String,
    pub size: f64,
    pub price: f64,
    pub notional: f64,
    pub result: String,
    pub live: bool,
    /// Position entry price when this order closes/reduces, so the UI can show
    /// the realised P&L of the close. None for opening orders.
    #[serde(default)]
    pub entry_px: Option<f64>,
    /// True when the order shrinks/closes a position (so the UI can label the
    /// row with the *position* direction rather than the order direction).
    #[serde(default)]
    pub reduce_only: bool,
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct LiveState {
    pub config: LiveConfig,
    pub history: Vec<EquityPoint>,
    pub records: Vec<LiveRecord>,
    pub last_run_at: Option<i64>,
    pub last_plan: Vec<String>,
    /// 上次调仓时各币的中间价。止盈幅度必须相对它计算，而不是相对当前价 ——
    /// 否则手动刷新会把止盈线随行情搬走（100 建仓、现价 80 的多头会被挂到 88）。
    #[serde(default)]
    pub tp_ref: HashMap<String, f64>,
    pub last_live: bool,
}

impl LiveState {
    pub fn load(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

#[derive(Serialize)]
pub struct LivePosition {
    pub coin: String,
    pub side: String,
    pub size: f64,
    pub entry_px: f64,
    pub mark_px: f64,
    pub notional: f64,
    pub unrealized: f64,
    pub liq_px: Option<f64>,
    pub dist_pct: Option<f64>,
    /// true = 全仓，false = 逐仓
    pub is_cross: bool,
    pub leverage: u32,
}

#[derive(Serialize)]
pub struct LiveSnapshot {
    pub configured: bool,
    pub error: Option<String>,
    pub account: String,
    pub equity: f64,
    pub positions: Vec<LivePosition>,
    pub gross_notional: f64,
    pub net_notional: f64,
    pub margin_used: f64,
    pub maintenance_margin: f64,
    pub liq_buffer_pct: f64,
    pub nearest_liq_pct: Option<f64>,
    /// 仍是逐仓的仓位数（Hyperliquid 不允许持仓时切换模式）
    pub isolated_count: usize,
    /// 盘口上挂着的止盈单
    pub tp_orders: Vec<crate::exchange::OpenOrder>,
    pub config: LiveConfig,
    pub history: Vec<EquityPoint>,
    pub records: Vec<LiveRecord>,
    pub last_run_at: Option<i64>,
    pub last_plan: Vec<String>,
    /// 上次调仓时各币的中间价。止盈幅度必须相对它计算，而不是相对当前价 ——
    /// 否则手动刷新会把止盈线随行情搬走（100 建仓、现价 80 的多头会被挂到 88）。
    #[serde(default)]
    pub tp_ref: HashMap<String, f64>,
    pub last_live: bool,
}

/// Read-only view of the live account: positions, exposure and liquidation risk.
pub async fn snapshot(
    state: &LiveState,
    markets: &HashMap<String, MarketInfo>,
    http: Option<reqwest::Client>,
) -> LiveSnapshot {
    let cfg = state.config.clone();
    let mut snap = LiveSnapshot {
        configured: cfg.can_sign(),
        error: None,
        account: cfg.account.clone(),
        equity: 0.0,
        positions: Vec::new(),
        gross_notional: 0.0,
        net_notional: 0.0,
        margin_used: 0.0,
        maintenance_margin: 0.0,
        liq_buffer_pct: 0.0,
        nearest_liq_pct: None,
        isolated_count: 0,
        tp_orders: Vec::new(),
        tp_ref: HashMap::new(),
        config: cfg.clone(),
        history: state.history.clone(),
        records: state.records.clone(),
        last_run_at: state.last_run_at,
        last_plan: state.last_plan.clone(),
        last_live: state.last_live,
    };
    if cfg.account.is_empty() {
        snap.error = Some("未配置账户地址".into());
        return snap;
    }
    let exec_res = match http {
        Some(h) => Exec::reader_shared(h, Some(&cfg.account)).await,
        None => Exec::reader_for(Some(&cfg.account)).await,
    };
    let exec = match exec_res {
        Ok(e) => e,
        Err(e) => {
            snap.error = Some(format!("{e}"));
            return snap;
        }
    };
    let acct = match exec.account().await {
        Ok(a) => a,
        Err(e) => {
            snap.error = Some(format!("读取账户失败: {e}"));
            return snap;
        }
    };
    snap.equity = acct.equity;
    let coins: Vec<String> = acct.positions.keys().cloned().collect();
    let mids = trader::fetch_mids(&exec, &coins).await;

    for (coin, pos) in acct.positions.iter() {
        let mark = mids.get(coin).copied().unwrap_or(pos.entry_px);
        let notional = (pos.size * mark).abs();
        let unrealized = pos.size * (mark - pos.entry_px);
        let dist = pos.liq_px.and_then(|liq| {
            if mark > 0.0 && liq > 0.0 {
                Some(if pos.size > 0.0 {
                    (mark - liq) / mark * 100.0
                } else {
                    (liq - mark) / mark * 100.0
                })
            } else {
                None
            }
        });
        snap.gross_notional += notional;
        snap.net_notional += pos.size * mark;
        if let Some(m) = markets.get(coin.as_str()) {
            snap.maintenance_margin += notional * maintenance_margin_rate(m.max_leverage);
        }
        if let Some(d) = dist {
            snap.nearest_liq_pct = Some(snap.nearest_liq_pct.map_or(d, |x: f64| x.min(d)));
        }
        snap.positions.push(LivePosition {
            coin: coin.clone(),
            side: if pos.size > 0.0 { "long".into() } else { "short".into() },
            size: pos.size,
            entry_px: pos.entry_px,
            mark_px: mark,
            notional,
            unrealized,
            liq_px: pos.liq_px,
            dist_pct: dist,
            is_cross: pos.is_cross,
            leverage: pos.leverage,
        });
    }
    snap.positions.sort_by(|a, b| a.coin.cmp(&b.coin));
    snap.isolated_count = snap.positions.iter().filter(|p| !p.is_cross).count();
    // 不按 reduceOnly 过滤：该字段在 openOrders 里不一定存在，过滤会导致
    // 表格永远是空的。程序只挂只减仓单，所以全部展示即可。
    if let Ok(orders) = exec.open_order_details().await {
        snap.tp_orders = orders;
    }
    snap.margin_used = if cfg.leverage > 0.0 {
        snap.gross_notional / cfg.leverage
    } else {
        0.0
    };
    snap.liq_buffer_pct = if snap.equity > 0.0 {
        ((snap.equity - snap.maintenance_margin) / snap.equity * 100.0).max(0.0)
    } else {
        0.0
    };
    snap
}

#[derive(Serialize)]
pub struct RunResult {
    pub equity: f64,
    pub long_leg: Vec<String>,
    pub short_leg: Vec<String>,
    pub per_coin: f64,
    pub plan_lines: Vec<String>,
    /// 本次调仓使用的参考价，供上层持久化（止盈幅度相对它计算）。
    #[serde(default)]
    pub tp_ref: Option<HashMap<String, f64>>,
    pub executed: Vec<String>,
    pub live: bool,
}

/// Compute the target book and, when `live` is true and the config is armed,
/// send the orders. Ranking is identical to the paper engine.
pub async fn run(
    store: &crate::store::Store,
    state: &LiveState,
    markets: &HashMap<String, MarketInfo>,
    live: bool,
) -> Result<(RunResult, Vec<LiveRecord>)> {
    let cfg = state.config.clone();
    anyhow::ensure!(!cfg.account.is_empty(), "未配置账户地址");
    if live {
        // A dry run is read-only, so only real trading needs the signing key.
        anyhow::ensure!(!cfg.key_path.is_empty(), "未配置 API 钱包密钥路径");
        anyhow::ensure!(cfg.armed, "实盘未启用（需先打开「启用实盘」开关）");
    }

    let exec = if live {
        Exec::signer(&cfg.account, std::path::Path::new(&cfg.key_path)).await?
    } else {
        Exec::reader_for(Some(&cfg.account)).await?
    };
    let tc = cfg.trade_config();

    let panel = trader::load_panel(store)?;
    anyhow::ensure!(panel.len() >= 20, "K 线缓存不足（{} 币）", panel.len());
    let acct0 = exec.account().await?;
    let (weights, liquid) = trader::target_weights(&panel, &tc, acct0.equity, crate::live::now_ms_pub() as i64);
    anyhow::ensure!(!weights.is_empty(), "流动性过滤后没有候选");

    let acct = exec.account().await?;
    let mut coins: Vec<String> = weights.iter().map(|(c, _)| c.clone()).collect();
    coins.extend(acct.positions.keys().cloned());
    coins.sort();
    coins.dedup();
    let mids = trader::fetch_mids(&exec, &coins).await;

    let plan = trader::build_plan(&weights, &acct, markets, &mids, &tc, None);

    let mut plan_lines = vec![format!(
        "流动宇宙 {} 币 · 多头腿 {} · 空头腿 {} · 账户净值 ${:.2} · 每仓 ${:.2} · 杠杆 {:.0}x · 缓冲 {:.0}%",
        liquid.len(),
        weights.iter().filter(|(_, w)| *w > 0.0).count(),
        weights.iter().filter(|(_, w)| *w < 0.0).count(),
        plan.equity,
        plan.per_coin,
        cfg.leverage,
        cfg.margin_buffer * 100.0
    )];
    plan_lines.push(format!("多头腿: {}", plan.long_leg.join(" ")));
    plan_lines.push(format!("空头腿: {}", plan.short_leg.join(" ")));
    for o in &plan.orders {
        plan_lines.push(format!(
            "{} {} {} {:.6} @≈{:.6} (${:.2}) · {}",
            if o.buy { "买入" } else { "卖出" },
            o.coin,
            if o.reduce_only { "只减仓" } else { "开仓" },
            o.size,
            o.mid,
            o.notional,
            o.reason
        ));
    }
    for n in &plan.notes {
        plan_lines.push(format!("注意: {n}"));
    }
    if !plan.held.is_empty() {
        plan_lines.push(format!(
            "注意: {} 个已有仓位无法切换保证金模式（Hyperliquid 不允许持仓时切换），\
             如需全部转为全仓请用「转为全仓」逐币处理",
            plan.held.len()
        ));
    }
    if plan.orders.is_empty() {
        plan_lines.push("无需调仓（已在目标状态）".into());
    }

    let now = now_ms_pub();
    let mut records = Vec::new();
    for o in &plan.orders {
        // 减仓/平仓的单子带上原持仓的入场价，前端据此算这笔的已实现盈亏
        let entry_px = if o.reduce_only {
            acct.positions.get(&o.coin).map(|p| p.entry_px)
        } else {
            None
        };
        records.push(LiveRecord {
            ts: now,
            coin: o.coin.clone(),
            side: if o.buy { "买".into() } else { "卖".into() },
            action: o.reason.clone(),
            size: o.size,
            price: o.mid,
            notional: o.notional,
            result: if live { "已发送".into() } else { "计划".into() },
            live,
            entry_px,
            reduce_only: o.reduce_only,
        });
    }

    // 调仓前先撤掉自己的旧止盈单。必须在 execute 之前：否则昨天的止盈单会在
    // 调仓过程中成交，把计划要减的仓位提前平掉，之后的减仓单被拒、开仓单照发。
    //
    // 失败必须中止，不能只记录：撤不掉的旧单会在新单之外继续存在，同一个仓位上
    // 出现两张只减仓单（交易所只按仓位校验，不会去重）。
    if live {
        match cancel_our_take_profits(&exec).await {
            Ok(n) if n > 0 => plan_lines.push(format!("撤销旧止盈单 {n} 个")),
            Ok(_) => {}
            Err(e) => {
                return Err(e.context("撤销旧止盈单失败，已在调仓前中止"));
            }
        }
    }

    let outcome = trader::execute(&exec, &plan, &tc, markets, live).await?;
    // 前置日志（设杠杆失败等）单独放在最前面，绝不与订单结果混用下标
    for line in &outcome.prelim {
        plan_lines.push(format!("注意: {line}"));
    }
    // 逐条按同一下标对应，保证失败信息挂在正确的币上
    for (i, line) in outcome.orders.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        if let Some(r) = records.get_mut(i) {
            r.result = line.clone();
        }
    }
    let executed = outcome.orders;
    let mut tp_ref_out: HashMap<String, f64> = HashMap::new();
    if live {
        // 基准价用本次调仓的目标币中间价，而不是挂单时的实时价 ——
        // 后者会让手动「刷新止盈单」把止盈线随行情一起搬走。
        let tp_ref: HashMap<String, f64> = plan
            .long_leg
            .iter()
            .chain(plan.short_leg.iter())
            .filter_map(|c| mids.get(c).map(|m| (c.clone(), *m)))
            .collect();
        tp_ref_out = tp_ref.clone();
        match place_take_profits(&exec, markets, cfg.take_profit_pct, &tp_ref).await {
            Ok(log) => plan_lines.extend(log),
            Err(e) => plan_lines.push(format!("止盈单挂单失败: {e}")),
        }
    }

    Ok((
        RunResult {
            equity: plan.equity,
            long_leg: plan.long_leg.clone(),
            short_leg: plan.short_leg.clone(),
            per_coin: plan.per_coin,
            tp_ref: Some(tp_ref_out),
            plan_lines,
            executed,
            live,
        },
        records,
    ))
}

pub fn now_ms_pub() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}


/// 逐币把仓位转成全仓：平掉 → 切 cross → 按目标重新开仓。
///
/// Hyperliquid 不允许在持仓状态下切换保证金模式（"Cannot switch leverage
/// while a position is open"），所以必须平掉再开。逐币处理的好处是组合里
/// 其余仓位始终保持对冲，任一时刻只有 1 个仓位处于裸奔状态。
pub async fn rebuild_cross(
    store: &crate::store::Store,
    state: &LiveState,
    markets: &HashMap<String, MarketInfo>,
) -> Result<Vec<String>> {
    let cfg = state.config.clone();
    anyhow::ensure!(cfg.armed, "实盘未启用（需先打开「启用实盘」开关）");
    anyhow::ensure!(cfg.can_sign(), "未配置 API 钱包密钥");
    let exec = Exec::signer(&cfg.account, std::path::Path::new(&cfg.key_path)).await?;
    let tc = cfg.trade_config();

    // 先读账户：仓位数上限（cap）依赖净值，必须在算权重之前拿到。
    let acct = exec.account().await?;
    if acct.positions.is_empty() {
        return Ok(vec!["账户没有持仓，无需转换".into()]);
    }
    let panel = trader::load_panel(store)?;
    let (weights, _) = trader::target_weights(
        &panel,
        &tc,
        acct.equity,
        crate::live::now_ms_pub() as i64,
    );
    anyhow::ensure!(!weights.is_empty(), "流动性过滤后没有候选");
    let coins: Vec<String> = acct.positions.keys().cloned().collect();
    let mids = trader::fetch_mids(&exec, &coins).await;
    // 按各自权重还原仓位。之前用「净值×杠杆÷仓位数」等权重建，会把权重抹平，
    // 总敞口也从 Σ|w| 变成 1 —— 点一次「转为全仓」就悄悄改了权重和杠杆。
    let deployable = acct.equity * cfg.margin_buffer;
    let wmap: HashMap<String, f64> = weights.iter().cloned().collect();
    let per_coin = deployable * cfg.leverage / weights.len().max(1) as f64;

    let mut log = vec![format!(
        "逐币转全仓：{} 个持仓 · 目标每仓 ${:.2}",
        acct.positions.len(),
        per_coin
    )];
    for coin in &coins {
        let pos = &acct.positions[coin];
        let Some(&mid) = mids.get(coin) else {
            log.push(format!("{coin}: 取价失败，跳过"));
            continue;
        };
        let Some(m) = markets.get(coin) else {
            log.push(format!("{coin}: 缺市场元数据，跳过"));
            continue;
        };
        let lev = (cfg.leverage.round() as u32).clamp(1, m.max_leverage.max(1));

        // 1) 平仓
        let size = crate::exchange::round_size(pos.size.abs(), m.sz_decimals);
        if size * mid >= 10.0 {
            let buy = pos.size < 0.0;
            match exec
                .ioc(coin, buy, true, size, mid, cfg.slippage, m.sz_decimals)
                .await
            {
                Ok(s) => log.push(format!("平 {coin} {size} · {}", crate::exchange::describe(&s))),
                Err(e) => {
                    log.push(format!("平 {coin} 失败：{e}（保持原样）"));
                    continue;
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // 2) 切全仓
        match exec.set_leverage(coin, lev).await {
            Ok(_) => log.push(format!("{coin}: 已切 {lev}x 全仓")),
            Err(e) => log.push(format!("{coin}: 切全仓失败 {e}")),
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // 3) 若仍在目标腿里，按目标重新开仓
        let w = weights.iter().find(|(c, _)| c == coin).map(|(_, w)| *w).unwrap_or(0.0);
        let want_long = w > 0.0;
        let want_short = w < 0.0;
        if want_long || want_short {
            // 每个币按自己的 |w| 还原，而不是等权
            let w = wmap.get(coin).map(|x| x.abs()).unwrap_or(0.0);
            let notional = w * deployable * cfg.leverage;
            let tsize = crate::exchange::round_size(notional / mid, m.sz_decimals);
            if tsize * mid >= 10.0 {
                match exec
                    .ioc(coin, want_long, false, tsize, mid, cfg.slippage, m.sz_decimals)
                    .await
                {
                    Ok(s) => log.push(format!(
                        "重开 {coin} {tsize} · {}",
                        crate::exchange::describe(&s)
                    )),
                    Err(e) => log.push(format!("重开 {coin} 失败：{e}")),
                }
            }
        } else {
            log.push(format!("{coin}: 不在目标名单，保持平仓"));
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    Ok(log)
}


/// 撤掉本程序挂出的止盈单。
///
/// 只撤带 cloid 前缀的单，不会误撤用户手动挂的单。返回撤销数量。
/// 注意：`open_orders` 失败会直接冒泡 —— 撤不掉旧单就绝不能继续下单，
/// 否则同一个仓位上会同时存在两张只减仓单（交易所只按仓位校验，不会去重）。
pub async fn cancel_our_take_profits(exec: &Exec) -> Result<usize> {
    let orders = exec.open_order_details().await?;
    let ours: Vec<(String, u64)> = orders
        .iter()
        .filter(|o| crate::exchange::is_ours(o.cloid.as_deref()))
        .map(|o| (o.coin.clone(), o.oid))
        .collect();
    if ours.is_empty() {
        return Ok(0);
    }
    exec.cancel_orders(&ours).await
}

/// 为当前所有持仓挂止盈单。
///
/// `reference` 是**调仓时**记录的参考价：止盈幅度必须相对那个价格算，而不是
/// 相对当前中间价。否则手动「刷新止盈单」会把止盈线随行情一起搬走 —— 例如
/// 100 建仓、现价 80 的多头会被挂到 88，变成一笔自动的亏损出场。
/// 参考价缺失（或已越过止盈线）时退回当前中间价，避免挂出立即成交的单。
pub async fn place_take_profits(
    exec: &Exec,
    markets: &HashMap<String, MarketInfo>,
    tp_pct: f64,
    reference: &HashMap<String, f64>,
) -> Result<Vec<String>> {
    let mut log = Vec::new();
    if tp_pct <= 0.0 {
        log.push("未启用止盈".into());
        return Ok(log);
    }
    anyhow::ensure!(!markets.is_empty(), "市场元数据为空，拒绝挂止盈单");
    let acct = exec.account().await?;
    if acct.positions.is_empty() {
        log.push("无持仓，不挂止盈".into());
        return Ok(log);
    }
    let coins: Vec<String> = acct.positions.keys().cloned().collect();
    let mids = trader::fetch_mids(exec, &coins).await;
    anyhow::ensure!(!mids.is_empty(), "取不到中间价，拒绝挂止盈单");

    let mut placed = 0usize;
    let mut filled = 0usize;
    let mut failed = Vec::new();
    for (coin, pos) in acct.positions.iter() {
        let Some(m) = markets.get(coin.as_str()) else {
            failed.push(format!("{coin}: 缺市场元数据"));
            continue;
        };
        let Some(&mid) = mids.get(coin) else {
            failed.push(format!("{coin}: 缺中间价"));
            continue;
        };
        if mid <= 0.0 {
            continue;
        }
        let is_long = pos.size > 0.0;
        // 基准价优先用调仓时记录的参考价
        let mut base = reference.get(coin).copied().unwrap_or(mid);
        // 参考价已被突破（行情反向走过头）时不能用它：挂出来会立刻成交。
        if is_long && base * (1.0 + tp_pct) <= mid {
            base = mid;
        }
        if !is_long && base * (1.0 - tp_pct) >= mid {
            base = mid;
        }
        let (buy, px) = take_profit_order(is_long, base, tp_pct, m.sz_decimals);
        if px <= 0.0 {
            failed.push(format!("{coin}: 价位精度不足，无法挂出有效止盈价"));
            continue;
        }
        let size = round_size(pos.size.abs(), m.sz_decimals);
        if size <= 0.0 || size * mid < 10.0 {
            continue;
        }
        match exec.resting_reduce_order(coin, buy, size, px).await {
            // oid 为 0 表示挂出去的瞬间就成交了（价格已经越过止盈线）。
            // 这不是失败，但也不能报成「挂上」。
            Ok(0) => filled += 1,
            Ok(_) => placed += 1,
            Err(e) => failed.push(format!("{coin}: {e}")),
        }
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    }
    log.push(format!(
        "止盈单：挂上 {placed} 个{}（±{:.0}%）{}",
        if filled > 0 {
            format!("，{filled} 个挂出即成交")
        } else {
            String::new()
        },
        tp_pct * 100.0,
        if failed.is_empty() {
            String::new()
        } else {
            format!("，未挂 {}", failed.join("; "))
        }
    ));
    Ok(log)
}

/// 供外部按钮调用：先撤自己的旧单，再按参考价重挂。
pub async fn refresh_take_profits(
    exec: &Exec,
    markets: &HashMap<String, MarketInfo>,
    tp_pct: f64,
    reference: &HashMap<String, f64>,
) -> Result<Vec<String>> {
    let mut log = Vec::new();
    // 先确认「一定挂得上」再去撤旧单：撤单成功而挂单失败（取不到价、缺元数据）
    // 会让账户在下次调仓前完全失去止盈保护，而且日志还会显示成功。
    if tp_pct > 0.0 {
        anyhow::ensure!(!markets.is_empty(), "市场元数据为空，拒绝撤销现有止盈单");
        let acct = exec.account().await?;
        if !acct.positions.is_empty() {
            let coins: Vec<String> = acct.positions.keys().cloned().collect();
            let mids = trader::fetch_mids(exec, &coins).await;
            anyhow::ensure!(!mids.is_empty(), "取不到中间价，拒绝撤销现有止盈单");
        }
    }
    match cancel_our_take_profits(exec).await {
        Ok(n) if n > 0 => log.push(format!("撤销旧止盈单 {n} 个")),
        Ok(_) => {}
        Err(e) => return Err(e.context("撤销旧止盈单失败")),
    }
    log.extend(place_take_profits(exec, markets, tp_pct, reference).await?);
    Ok(log)
}

/// 交易所允许的最小价位（和 `order_price` 的精度规则一致）。
fn price_tick(px: f64, sz_decimals: u32) -> f64 {
    let dp = (4 - px.log10().floor() as i32).min(6 - sz_decimals as i32);
    10_f64.powi(-dp)
}

/// 止盈单的方向与挂单价。多头要「卖出」且挂在市价上方，空头要「买入」且挂在
/// 市价下方 —— 方向写反会变成加仓，价格写反会立刻成交变成市价单。
///
/// 低价币的价位精度可能不够（一个 tick 就超过止盈幅度），取整会把价格推到
/// 市价的错误一侧。这时至少推离一个 tick；推不动就返回 0，让调用方跳过该币。
pub fn take_profit_order(is_long: bool, mid: f64, tp_pct: f64, sz_decimals: u32) -> (bool, f64) {
    if !mid.is_finite() || mid <= 0.0 {
        return (true, 0.0);
    }
    let (buy, raw) = if is_long {
        (false, mid * (1.0 + tp_pct))
    } else {
        (true, mid * (1.0 - tp_pct))
    };
    if !raw.is_finite() || raw <= 0.0 {
        return (buy, 0.0);
    }
    let tick = price_tick(raw, sz_decimals);
    // 挂单要和 IOC 相反：取整方向朝「远离市价」，避免变成立即成交的市价单。
    let mut px = order_price(raw, sz_decimals, !buy);
    let right_side = |p: f64| p > 0.0 && if is_long { p > mid } else { p < mid };
    for _ in 0..4 {
        if right_side(px) {
            return (buy, px);
        }
        px = if is_long { px + tick } else { px - tick };
    }
    if right_side(px) { (buy, px) } else { (buy, 0.0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_profit_sells_above_market_for_longs() {
        let (buy, px) = take_profit_order(true, 100.0, 0.10, 2);
        assert!(!buy, "多头止盈必须是卖出");
        assert!(px > 100.0, "多头止盈必须挂在市价上方，实际 {px}");
    }

    #[test]
    fn take_profit_buys_below_market_for_shorts() {
        let (buy, px) = take_profit_order(false, 100.0, 0.10, 2);
        assert!(buy, "空头止盈必须是买入");
        assert!(px < 100.0, "空头止盈必须挂在市价下方，实际 {px}");
    }

    #[test]
    fn take_profit_stays_on_the_correct_side_after_rounding() {
        for &mid in &[0.00001234_f64, 0.5, 3.14159, 87.65, 1234.5, 98765.4] {
            for &sd in &[0u32, 1, 2, 4, 6] {
                let (buy, px) = take_profit_order(true, mid, 0.10, sd);
                assert!(!buy);
                assert!(px > mid, "多头 mid={mid} sd={sd} 得到 {px}");
                let (buy, px) = take_profit_order(false, mid, 0.10, sd);
                assert!(buy);
                assert!(px < mid, "空头 mid={mid} sd={sd} 得到 {px}");
            }
        }
    }

    #[test]
    fn take_profit_price_is_never_zero_or_negative() {
        for &mid in &[0.000001_f64, 1.0, 100.0] {
            for &tp in &[0.01, 0.1, 0.5] {
                for &sd in &[0u32, 2, 6] {
                    let (_, up) = take_profit_order(true, mid, tp, sd);
                    let (_, dn) = take_profit_order(false, mid, tp, sd);
                    assert!(up >= 0.0 && dn >= 0.0, "mid={mid} tp={tp} sd={sd} 得到 {up}/{dn}");
                    assert!(up > mid || up == 0.0, "多头挂单价必须高于市价: {up} vs {mid}");
                    assert!(dn < mid || dn == 0.0, "空头挂单价必须低于市价: {dn} vs {mid}");
                }
            }
        }
    }
}
