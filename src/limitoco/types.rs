//! Configuration, risk tier presets, and state models for Limit-OCO V2.

use super::{now_ms, utc_day_str, ENGINE_VERSION, HOUR_MS, STRATEGY_ID};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

// ---------------------------------------------------------------------------
// 配置与风险档位
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RiskTierPreset {
    pub id: String,
    pub label: String,
    pub risk_per_trade: f64,
    pub max_pos_lev: f64,
    pub max_total_gross_lev: f64,
    pub bt_ann_pct: f64,
    #[serde(alias = "bt_ret_23m_pct")]
    pub bt_ret_35m_pct: f64,
    pub bt_max_dd_pct: f64,
    pub bt_worst_day_pct: f64,
    pub bt_sharpe: f64,
}

pub fn tier_presets() -> Vec<RiskTierPreset> {
    vec![
        RiskTierPreset {
            id: "1.0x".into(),
            label: "1.0x 稳健基线".into(),
            risk_per_trade: 0.008,
            max_pos_lev: 1.5,
            max_total_gross_lev: 4.5,
            bt_ann_pct: 72.7,
            bt_ret_35m_pct: 368.0,
            bt_max_dd_pct: 15.5,
            bt_worst_day_pct: -5.1,
            bt_sharpe: 1.91,
        },
        RiskTierPreset {
            id: "1.5x".into(),
            label: "1.5x 甜点默认".into(),
            risk_per_trade: 0.012,
            max_pos_lev: 2.2,
            max_total_gross_lev: 6.0,
            bt_ann_pct: 120.1,
            bt_ret_35m_pct: 828.6,
            bt_max_dd_pct: 22.4,
            bt_worst_day_pct: -7.6,
            bt_sharpe: 1.92,
        },
        RiskTierPreset {
            id: "2.0x".into(),
            label: "2.0x 进取翻倍".into(),
            risk_per_trade: 0.016,
            max_pos_lev: 3.0,
            max_total_gross_lev: 8.0,
            bt_ann_pct: 175.0,
            bt_ret_35m_pct: 1641.1,
            bt_max_dd_pct: 29.0,
            bt_worst_day_pct: -10.1,
            bt_sharpe: 1.93,
        },
        RiskTierPreset {
            id: "2.5x".into(),
            label: "2.5x 高波激进".into(),
            risk_per_trade: 0.020,
            max_pos_lev: 3.5,
            max_total_gross_lev: 9.0,
            bt_ann_pct: 236.3,
            bt_ret_35m_pct: 2973.7,
            bt_max_dd_pct: 35.1,
            bt_worst_day_pct: -12.5,
            bt_sharpe: 1.93,
        },
    ]
}

fn d_capital() -> f64 { 10_000.0 }
fn d_tier() -> String { "1.5x".into() }
fn d_rpt() -> f64 { 0.012 }
fn d_max_pos_lev() -> f64 { 2.2 }
fn d_max_gross_lev() -> f64 { 6.0 }
fn d_true() -> bool { true }
fn d_max_total_pos() -> usize { 5 }
fn d_a_max_pos() -> usize { 3 }
fn d_a_max_ss() -> usize { 2 }
fn d_b_max_pos() -> usize { 2 }
fn d_b_max_ss() -> usize { 2 }
fn d_min_sl() -> f64 { 0.005 }
fn d_max_sl() -> f64 { 0.14 }
fn d_cooldown() -> i64 { 72 }
fn d_daily_loss_r() -> f64 { 3.0 }
fn d_top_n() -> usize { 80 }
fn d_min_vol() -> f64 { 15_000_000.0 }
fn d_split_jump() -> f64 { 0.35 }
fn d_split_hours() -> usize { 72 }
fn d_entry_pen() -> f64 { 0.0005 }
fn d_tp_pen() -> f64 { 0.0002 }
fn d_maker_fee() -> f64 { 0.0002 }
fn d_sl_cost() -> f64 { 0.0010 }
fn d_time_cost() -> f64 { 0.0008 }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LimitOcoConfig {
    #[serde(default)]
    pub running: bool,
    #[serde(default = "d_capital")]
    pub capital: f64,
    #[serde(default = "d_tier")]
    pub risk_tier: String,
    #[serde(default = "d_rpt")]
    pub risk_per_trade: f64,
    #[serde(default = "d_max_pos_lev")]
    pub max_pos_lev: f64,
    #[serde(default = "d_max_gross_lev")]
    pub max_total_gross_lev: f64,
    #[serde(default = "d_true")]
    pub compounding: bool,
    #[serde(default = "d_max_total_pos")]
    pub max_total_positions: usize,
    #[serde(default = "d_a_max_pos")]
    pub sleeve_a_max_pos: usize,
    #[serde(default = "d_a_max_ss")]
    pub sleeve_a_max_same_side: usize,
    #[serde(default = "d_b_max_pos")]
    pub sleeve_b_max_pos: usize,
    #[serde(default = "d_b_max_ss")]
    pub sleeve_b_max_same_side: usize,
    #[serde(default = "d_min_sl")]
    pub min_sl_pct: f64,
    #[serde(default = "d_max_sl")]
    pub max_sl_pct: f64,
    #[serde(default = "d_cooldown")]
    pub cooldown_hours: i64,
    #[serde(default = "d_daily_loss_r")]
    pub daily_loss_limit_r: f64,
    #[serde(default = "d_top_n")]
    pub top_n_liquid: usize,
    #[serde(default = "d_min_vol")]
    pub min_daily_vol_usd: f64,
    #[serde(default = "d_split_jump")]
    pub split_jump_pct: f64,
    #[serde(default = "d_split_hours")]
    pub split_block_hours: usize,
    #[serde(default = "d_true")]
    pub enabled_a: bool,
    #[serde(default = "d_true")]
    pub enabled_b: bool,
    #[serde(default = "d_entry_pen")]
    pub entry_pen: f64,
    #[serde(default = "d_tp_pen")]
    pub tp_pen: f64,
    #[serde(default = "d_maker_fee")]
    pub maker_fee: f64,
    #[serde(default = "d_sl_cost")]
    pub sl_cost: f64,
    #[serde(default = "d_time_cost")]
    pub time_exit_cost: f64,
    #[serde(default)]
    pub proxy_url: String,
}

impl Default for LimitOcoConfig {
    fn default() -> Self {
        Self {
            running: false,
            capital: d_capital(),
            risk_tier: d_tier(),
            risk_per_trade: d_rpt(),
            max_pos_lev: d_max_pos_lev(),
            max_total_gross_lev: d_max_gross_lev(),
            compounding: true,
            max_total_positions: d_max_total_pos(),
            sleeve_a_max_pos: d_a_max_pos(),
            sleeve_a_max_same_side: d_a_max_ss(),
            sleeve_b_max_pos: d_b_max_pos(),
            sleeve_b_max_same_side: d_b_max_ss(),
            min_sl_pct: d_min_sl(),
            max_sl_pct: d_max_sl(),
            cooldown_hours: d_cooldown(),
            daily_loss_limit_r: d_daily_loss_r(),
            top_n_liquid: d_top_n(),
            min_daily_vol_usd: d_min_vol(),
            split_jump_pct: d_split_jump(),
            split_block_hours: d_split_hours(),
            enabled_a: true,
            enabled_b: true,
            entry_pen: d_entry_pen(),
            tp_pen: d_tp_pen(),
            maker_fee: d_maker_fee(),
            sl_cost: d_sl_cost(),
            time_exit_cost: d_time_cost(),
            proxy_url: String::new(),
        }
    }
}

impl LimitOcoConfig {
    pub fn apply_tier_preset(&mut self, tier: &str) {
        let t = tier.trim();
        if let Some(p) = tier_presets().into_iter().find(|x| x.id == t) {
            self.risk_tier = p.id;
            self.risk_per_trade = p.risk_per_trade;
            self.max_pos_lev = p.max_pos_lev;
            self.max_total_gross_lev = p.max_total_gross_lev;
        } else if t == "custom" {
            self.risk_tier = "custom".into();
        }
    }

    pub fn sanitize(&mut self) {
        if !self.capital.is_finite() || self.capital <= 0.0 {
            self.capital = d_capital();
        }
        self.capital = self.capital.clamp(100.0, 100_000_000.0);
        if !self.risk_per_trade.is_finite() {
            self.risk_per_trade = d_rpt();
        }
        self.risk_per_trade = self.risk_per_trade.clamp(0.002, 0.05);
        if !self.max_pos_lev.is_finite() {
            self.max_pos_lev = d_max_pos_lev();
        }
        self.max_pos_lev = self.max_pos_lev.clamp(0.5, 10.0);
        if !self.max_total_gross_lev.is_finite() {
            self.max_total_gross_lev = d_max_gross_lev();
        }
        self.max_total_gross_lev = self.max_total_gross_lev.clamp(1.0, 20.0);
        self.max_total_positions = self.max_total_positions.clamp(1, 12);
        self.sleeve_a_max_pos = self.sleeve_a_max_pos.clamp(1, 8);
        self.sleeve_a_max_same_side = self.sleeve_a_max_same_side.clamp(1, 6);
        self.sleeve_b_max_pos = self.sleeve_b_max_pos.clamp(1, 8);
        self.sleeve_b_max_same_side = self.sleeve_b_max_same_side.clamp(1, 6);
        self.min_sl_pct = self.min_sl_pct.clamp(0.002, 0.05);
        self.max_sl_pct = self.max_sl_pct.clamp(0.04, 0.25);
        self.cooldown_hours = self.cooldown_hours.clamp(0, 240);
        self.daily_loss_limit_r = self.daily_loss_limit_r.clamp(0.0, 20.0);
        self.top_n_liquid = self.top_n_liquid.clamp(20, 150);
        self.min_daily_vol_usd = self.min_daily_vol_usd.clamp(1_000_000.0, 200_000_000.0);
        self.proxy_url = self.proxy_url.trim().to_string();
    }
}

// ---------------------------------------------------------------------------
// 核心数据模型
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bar1h {
    pub open_time_ms: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub quote_volume: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct BtcRegime {
    pub bar_ts: i64,
    pub price: f64,
    pub ema168: f64,
    pub ema336: f64,
    pub bull_ok: bool,
    pub short_ok: bool,
    pub funding_rate: f64,
    pub label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UniversePreviewRow {
    pub symbol: String,
    pub daily_qv7_m: f64,
    pub price: f64,
    pub ret24h: f64,
    pub funding_bp: f64,
    pub is_commodity: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct UniverseInfo {
    pub total_perps: usize,
    pub excluded_stocks: usize,
    pub eligible_count: usize,
    pub liquid_count: usize,
    pub top80_preview: Vec<UniversePreviewRow>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SleeveStat {
    pub open: usize,
    pub max: usize,
    pub pending: usize,
    pub trades: usize,
    pub wins: usize,
    pub win_rate: f64,
    pub sum_r: f64,
    pub net_pnl: f64,
    pub pf: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpenPosition {
    pub sleeve: String,
    pub symbol: String,
    pub asset_class: String,
    pub side: i8,
    pub entry_ts: i64,
    pub last_fund_ts: i64,
    pub entry_px: f64,
    pub mark_px: f64,
    pub qty: f64,
    pub notional: f64,
    pub risk_usd: f64,
    pub sl_px: f64,
    pub tp_px: f64,
    pub sl_pct: f64,
    pub tp_pct: f64,
    pub max_hold_bars: usize,
    pub max_hold_hours: usize,
    pub hold_bars: usize,
    pub hold_hours: i64,
    pub fee_usd: f64,
    pub funding_usd: f64,
    pub unreal_pnl: f64,
    pub unreal_r: f64,
    pub progress_pct: f64,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingOrder {
    pub sleeve: String,
    pub symbol: String,
    pub side: i8,
    pub limit_px: f64,
    pub mark_px: f64,
    pub dist_to_fill_pct: f64,
    pub sl_px: f64,
    pub tp_px: f64,
    pub sl_pct: f64,
    pub tp_pct: f64,
    pub expire_hours: usize,
    pub max_hold_bars: usize,
    pub age_hours: usize,
    pub score: f64,
    pub funding_bp: f64,
    pub created_ts: i64,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClosedTrade {
    pub sleeve: String,
    pub symbol: String,
    pub side: i8,
    pub entry_ts: i64,
    pub exit_ts: i64,
    pub hold_hours: i64,
    pub entry_px: f64,
    pub exit_px: f64,
    pub sl_px: f64,
    pub tp_px: f64,
    pub sl_pct: f64,
    pub tp_pct: f64,
    pub qty: f64,
    pub notional: f64,
    pub risk_usd: f64,
    pub net_pnl: f64,
    pub r_mult: f64,
    pub fee_usd: f64,
    pub funding_usd: f64,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShadowEvent {
    pub ts: i64,
    pub kind: String,
    pub symbol: String,
    pub sleeve: String,
    pub msg: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SignalFunnel {
    pub bars_scanned: u64,
    pub a_raw_cross: u64,
    pub a_vol_blocked: u64,
    pub a_btc_blocked: u64,
    pub a_fund_blocked: u64,
    pub a_sl_rejected: u64,
    pub a_placed: u64,
    pub b_raw_rip: u64,
    pub b_rsi_blocked: u64,
    pub b_fund_blocked: u64,
    pub b_sl_rejected: u64,
    pub b_placed: u64,
    pub filled: u64,
    pub expired: u64,
    pub runaway: u64,
    pub breaker_skipped: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignalOrder {
    pub sleeve: String,
    pub symbol: String,
    pub side: i8,
    pub limit_px: f64,
    pub sl_px: f64,
    pub tp_px: f64,
    pub expire_bars: usize,
    pub max_hold_bars: usize,
    pub score: f64,
    pub sl_pct: f64,
    pub tp_pct: f64,
    pub signal_close: f64,
    pub atr24: f64,
    pub funding_rate: f64,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LimitOcoState {
    pub strategy: String,
    pub version: String,
    pub running: bool,
    pub status_text: String,
    pub capital: f64,
    pub cash: f64,
    pub equity: f64,
    pub pnl: f64,
    pub pnl_pct: f64,
    pub realized_pnl: f64,
    pub unrealized_pnl: f64,
    pub total_fees: f64,
    pub total_funding: f64,
    pub peak_equity: f64,
    pub max_dd_pct: f64,
    pub gross_notional: f64,
    pub net_notional: f64,
    pub gross_leverage: f64,
    pub win_rate: f64,
    pub profit_factor: f64,
    pub avg_r: f64,
    pub total_r: f64,
    pub started_at: i64,
    pub last_update: i64,
    pub last_bar_ms: i64,
    pub next_bar_ms: i64,
    pub scan_duration_ms: u64,
    pub last_error: Option<String>,
    pub days_elapsed: f64,
    pub daily_utc_day: String,
    pub daily_r_pnl: f64,
    pub daily_breaker_active: bool,
    pub btc_regime: BtcRegime,
    pub universe: UniverseInfo,
    pub sleeve_stats: BTreeMap<String, SleeveStat>,
    pub positions: BTreeMap<String, OpenPosition>,
    pub pending: BTreeMap<String, PendingOrder>,
    pub cooldowns: BTreeMap<String, i64>,
    /// `[ts_ms, equity, gross_leverage]`
    pub history: Vec<(i64, f64, f64)>,
    pub trades: Vec<ClosedTrade>,
    pub events: Vec<ShadowEvent>,
    pub funnel: SignalFunnel,
    pub config: LimitOcoConfig,
    #[serde(default = "tier_presets")]
    pub tier_presets: Vec<RiskTierPreset>,
}

impl Default for LimitOcoState {
    fn default() -> Self {
        Self::new_with_config(LimitOcoConfig::default(), now_ms())
    }
}

impl LimitOcoState {
    pub fn new_with_config(mut cfg: LimitOcoConfig, ts: i64) -> Self {
        cfg.sanitize();
        let cap = cfg.capital;
        let mut sleeve_stats = BTreeMap::new();
        sleeve_stats.insert(
            "A".into(),
            SleeveStat { max: cfg.sleeve_a_max_pos, ..Default::default() },
        );
        sleeve_stats.insert(
            "B".into(),
            SleeveStat { max: cfg.sleeve_b_max_pos, ..Default::default() },
        );
        let mut st = Self {
            strategy: STRATEGY_ID.into(),
            version: ENGINE_VERSION.into(),
            running: cfg.running,
            status_text: if cfg.running { "运行中".into() } else { "待启动".into() },
            capital: cap,
            cash: cap,
            equity: cap,
            pnl: 0.0,
            pnl_pct: 0.0,
            realized_pnl: 0.0,
            unrealized_pnl: 0.0,
            total_fees: 0.0,
            total_funding: 0.0,
            peak_equity: cap,
            max_dd_pct: 0.0,
            gross_notional: 0.0,
            net_notional: 0.0,
            gross_leverage: 0.0,
            win_rate: 0.0,
            profit_factor: 0.0,
            avg_r: 0.0,
            total_r: 0.0,
            started_at: ts,
            last_update: ts,
            last_bar_ms: 0,
            next_bar_ms: (ts.div_euclid(HOUR_MS) + 1) * HOUR_MS,
            scan_duration_ms: 0,
            last_error: None,
            days_elapsed: 0.0,
            daily_utc_day: utc_day_str(ts),
            daily_r_pnl: 0.0,
            daily_breaker_active: false,
            btc_regime: BtcRegime {
                label: "等待首次扫描".into(),
                ..Default::default()
            },
            universe: UniverseInfo::default(),
            sleeve_stats,
            positions: BTreeMap::new(),
            pending: BTreeMap::new(),
            cooldowns: BTreeMap::new(),
            history: vec![(ts, cap, 0.0)],
            trades: Vec::new(),
            events: Vec::new(),
            funnel: SignalFunnel::default(),
            config: cfg.clone(),
            tier_presets: tier_presets(),
        };
        st.log_event(
            ts,
            "INIT",
            "",
            "",
            format!(
                "初始化纯 Rust 影子账本 · 本金 ${:.0} · 档位 {} ({:.1}%R)",
                cap,
                cfg.risk_tier,
                cfg.risk_per_trade * 100.0
            ),
        );
        st
    }

    pub fn load(path: &Path) -> Self {
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(mut st) = serde_json::from_slice::<Self>(&bytes) {
                st.config.sanitize();
                st.strategy = STRATEGY_ID.into();
                st.version = ENGINE_VERSION.into();
                st.tier_presets = tier_presets();
                return st;
            }
        }
        Self::default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
        let bytes = serde_json::to_vec(self)?;
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn sync_config(&mut self, mut cfg: LimitOcoConfig, ts: i64) {
        cfg.sanitize();
        self.running = cfg.running;
        self.status_text = if cfg.running { "运行中".into() } else { "已暂停".into() };
        self.config = cfg;
        self.tier_presets = tier_presets();
        if let Some(a) = self.sleeve_stats.get_mut("A") {
            a.max = self.config.sleeve_a_max_pos;
        }
        if let Some(b) = self.sleeve_stats.get_mut("B") {
            b.max = self.config.sleeve_b_max_pos;
        }
        self.last_update = ts;
    }

    pub fn log_event(&mut self, ts: i64, kind: &str, symbol: &str, sleeve: &str, msg: String) {
        self.events.push(ShadowEvent {
            ts,
            kind: kind.into(),
            symbol: symbol.into(),
            sleeve: sleeve.into(),
            msg,
        });
        if self.events.len() > 140 {
            let drain = self.events.len() - 140;
            self.events.drain(0..drain);
        }
    }

    pub(super) fn roll_daily_breaker(&mut self, ts: i64) {
        let day = utc_day_str(ts);
        if self.daily_utc_day != day {
            self.daily_utc_day = day.clone();
            self.daily_r_pnl = 0.0;
            if self.daily_breaker_active {
                self.daily_breaker_active = false;
                self.log_event(
                    ts,
                    "BREAKER",
                    "",
                    "",
                    format!("新 UTC 交易日 ({day}) 开始，日内熔断已自动解除"),
                );
            }
        }
    }
}

