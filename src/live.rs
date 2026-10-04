//! Live trading on top of the same signal the paper engine uses.
//!
//! This is the only place in the project that can send real orders. It reuses
//! `trader::ranking` (identical to the paper engine) and `exchange::Exec`, so the
//! live book cannot drift from what was validated.

use crate::exchange::{Exec, MarketInfo};
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
                    sz_decimals: c.sz_decimals.unwrap_or(4),
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
            armed: false,
            auto_run: false,
        }
    }
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
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct LiveState {
    pub config: LiveConfig,
    pub history: Vec<EquityPoint>,
    pub records: Vec<LiveRecord>,
    pub last_run_at: Option<i64>,
    pub last_plan: Vec<String>,
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
    pub config: LiveConfig,
    pub history: Vec<EquityPoint>,
    pub records: Vec<LiveRecord>,
    pub last_run_at: Option<i64>,
    pub last_plan: Vec<String>,
    pub last_live: bool,
}

/// Read-only view of the live account: positions, exposure and liquidation risk.
pub async fn snapshot(
    state: &LiveState,
    markets: &HashMap<String, MarketInfo>,
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
    let exec = match Exec::reader_for(Some(&cfg.account)).await {
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
        });
    }
    snap.positions.sort_by(|a, b| a.coin.cmp(&b.coin));
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
    let (long, short, liquid) = trader::ranking(&panel, &tc);
    anyhow::ensure!(!long.is_empty(), "流动性过滤后没有候选");

    let acct = exec.account().await?;
    let mut coins: Vec<String> = long.iter().chain(short.iter()).cloned().collect();
    coins.extend(acct.positions.keys().cloned());
    coins.sort();
    coins.dedup();
    let mids = trader::fetch_mids(&exec, &coins).await;

    let plan = trader::build_plan(&long, &short, &acct, markets, &mids, &tc, None);

    let mut plan_lines = vec![format!(
        "流动宇宙 {} 币 · 多头腿 {} · 空头腿 {} · 账户净值 ${:.2} · 每仓 ${:.2} · 杠杆 {:.0}x · 缓冲 {:.0}%",
        liquid.len(),
        long.len(),
        short.len(),
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
    if plan.orders.is_empty() {
        plan_lines.push("无需调仓（已在目标状态）".into());
    }

    let now = now_ms_pub();
    let mut records = Vec::new();
    for o in &plan.orders {
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
        });
    }

    let executed = trader::execute(&exec, &plan, &tc, markets, live).await?;
    if live {
        // Update recorded results with what the exchange reported.
        for (i, line) in executed.iter().enumerate() {
            if let Some(r) = records.get_mut(i) {
                r.result = line.clone();
            }
        }
    }

    Ok((
        RunResult {
            equity: plan.equity,
            long_leg: plan.long_leg.clone(),
            short_leg: plan.short_leg.clone(),
            per_coin: plan.per_coin,
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
