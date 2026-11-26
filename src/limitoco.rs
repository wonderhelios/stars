//! 币安 USD-M 永续 A+B 双引擎纯限价 OCO 影子引擎（`Dyn_Top80 + Fix 1+2`，默认 1.5x 档位）。
//!
//! 纯 Rust 实现，零外部运行依赖，只读币安公开行情接口（绝不发真实下单请求）：
//!   - `/fapi/v1/exchangeInfo`（缓存 6h，自动区分加密/大宗商品并剔除美股永续与稳定币）
//!   - `/fapi/v1/ticker/24hr`（快速筛选流动性候选池）
//!   - `/fapi/v1/klines?interval=1h`（增量本地缓存已收盘 1h K 线，零未来函数）
//!   - `/fapi/v1/premiumIndex`（实时标记价与资金费率，每 60s 盯市）
//!   - `/fapi/v1/fundingRate`（严格 `< bar_close` 已结算资金费率 + 持仓真实资金费现金流）
//!
//! 策略核心参数（与 23 个月回测 & 锁死 OOS 100% 对齐）：
//!   - 标的池：加密货币 + 大宗商品（金银油气铜），排除美股永续；每整点动态取 7 日日均成交额 Top-80（>= $15M），排除 72h 内单小时振幅 >= 35% 标的
//!   - 策略 A（7d 趋势突破，最多 3 仓 / 同向最多 2 仓 / 冷却 72h）：
//!     - 168h 通道新突破 + 放量 >= 1.3x 24h 均量
//!     - 做多：个币 & BTC 均 > EMA168 且 > EMA336，且资金费 <= +0.5bp
//!     - 做空：个币 & BTC 均 < EMA168，且资金费 >= 0
//!     - 纯 Maker GTX 限价 `close ∓ 0.15*ATR24`（12h 过期）；SL `3.5*ATR24`（0.5%~14%），TP `2.2R`，最长持仓 `168h`
//!   - 策略 B（空头反弹急拉做空，最多 2 仓 / 仅做空 / 冷却 72h）：
//!     - `close < EMA336` + 72h 涨幅 >= +6% + 因果日线采样 `RSI(2) >= 90` + 资金费 >= -2bp
//!     - 纯 Maker GTX 挂卖 `close + 0.20*ATR24`（12h 过期）；SL `3.0*ATR24`（0.5%~14%），TP `1.5R`，最长持仓 `96h`
//!   - 全局风控：总计最多 5 仓、默认 1.5x 档位（单笔 1.2%R 复利，单仓上限 2.2x，总杠杆上限 6.0x）、日内 UTC 已实现亏损 `-3R` 自动熔断。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

pub const HOUR_MS: i64 = 3_600_000;
pub const DAY_MS: i64 = 86_400_000;
pub const STRATEGY_ID: &str = "binance_limit_oco_ab_v1";
const FAPI_BASE_DEFAULT: &str = "https://fapi.binance.com";
const MAX_BARS_CACHE: usize = 440;
const COLD_FETCH_LIMIT: usize = 400;

const COMMODITY_SYMBOLS: &[&str] = &[
    "XAUUSDT", "XAGUSDT", "CLUSDT", "BZUSDT", "NATGASUSDT", "COPPERUSDT", "PAXGUSDT",
    "XPTUSDT", "XPDUSDT",
];

const STABLE_OR_FX_EXCLUDED: &[&str] = &[
    "USDCUSDT", "BUSDUSDT", "USDPUSDT", "TUSDUSDT", "FDUSDUSDT", "EURUSDT",
    "DAIUSDT", "USD1USDT", "USDEUSDT", "GBPUSDT", "AUDUSDT", "AEURUSDT",
];

const US_STOCK_SYMBOLS: &[&str] = &[
    "AAPLUSDT", "NVDAUSDT", "TSLAUSDT", "MSFTUSDT", "AMZNUSDT", "GOOGLUSDT", "GOOGUSDT",
    "METAUSDT", "MSTRUSDT", "COINUSDT", "PLTRUSDT", "AMDUSDT", "INTCUSDT", "NFLXUSDT",
    "ORCLUSDT", "HOODUSDT", "MUUSDT", "CRCLUSDT", "SOXLUSDT", "SNDKUSDT", "SPYUSDT",
    "QQQUSDT", "IWMUSDT", "CRWDUSDT", "ARMUSDT", "AVGOUSDT", "SMCIUSDT", "LLYUSDT",
    "JPMUSDT", "BACUSDT", "XOMUSDT", "BABAUSDT", "PDDUSDT", "NIOUSDT", "TSMUSDT",
    "PYPLUSDT", "SHOPUSDT", "UBERUSDT", "SNOWUSDT", "MARAUSDT", "RIOTUSDT", "CLSKUSDT",
    "GMEUSDT", "AMCUSDT", "RDDTUSDT", "ASMLUSDT", "DISUSDT", "BAUSDT", "NKEUSDT",
    "WMTUSDT", "COSTUSDT", "VUSDT", "MAUSDT", "KOUSDT", "PEPUSDT", "MCDUSDT",
    "UNHUSDT", "JNJUSDT", "PFEUSDT", "MRNAUSDT", "CVXUSDT", "OXYUSDT", "CATUSDT",
    "GEUSDT", "LMTUSDT", "RTXUSDT", "IBMUSDT", "CSCOUSDT", "QCOMUSDT", "TXNUSDT",
    "ADBEUSDT", "CRMUSDT", "NOWUSDT", "PANWUSDT", "ABNBUSDT", "BKNGUSDT", "DASHUSDT",
    "SPOTUSDT", "ROKUUSDT", "SQUSDT", "XYZUSDT", "SOFIUSDT", "AFRMUSDT", "UPSTUSDT",
    "CVNAUSDT", "DKNGUSDT", "RBLXUSDT", "UUSDT", "NETUSDT", "DDOGUSDT", "MDBUSDT",
    "OKTAUSDT", "ZSUSDT", "FTNTUSDT", "ANETUSDT", "MRVLUSDT", "KLACUSDT", "LRCXUSDT",
    "AMATUSDT", "ONUSDT", "MCHPUSDT", "NXPIUSDT", "STMUSDT", "GFSUSDT", "SLVUSDT",
    "GLDUSDT", "TLTUSDT", "TQQQUSDT", "SQQQUSDT", "UVXYUSDT",
];

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// UTC calendar day string `YYYY-MM-DD` (Howard Hinnant civil-from-days algorithm, zero external crate).
pub fn utc_day_str(ts_ms: i64) -> String {
    let z = ts_ms.div_euclid(DAY_MS) + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    format!("{year:04}-{m:02}-{d:02}")
}

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
    pub bt_ret_23m_pct: f64,
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
            bt_ann_pct: 55.3,
            bt_ret_23m_pct: 127.3,
            bt_max_dd_pct: 18.7,
            bt_worst_day_pct: -5.9,
            bt_sharpe: 1.66,
        },
        RiskTierPreset {
            id: "1.5x".into(),
            label: "1.5x 甜点默认".into(),
            risk_per_trade: 0.012,
            max_pos_lev: 2.2,
            max_total_gross_lev: 6.0,
            bt_ann_pct: 87.8,
            bt_ret_23m_pct: 223.8,
            bt_max_dd_pct: 26.9,
            bt_worst_day_pct: -8.8,
            bt_sharpe: 1.67,
        },
        RiskTierPreset {
            id: "2.0x".into(),
            label: "2.0x 进取翻倍".into(),
            risk_per_trade: 0.016,
            max_pos_lev: 3.0,
            max_total_gross_lev: 8.0,
            bt_ann_pct: 122.5,
            bt_ret_23m_pct: 344.4,
            bt_max_dd_pct: 34.5,
            bt_worst_day_pct: -11.7,
            bt_sharpe: 1.67,
        },
        RiskTierPreset {
            id: "2.5x".into(),
            label: "2.5x 高波激进".into(),
            risk_per_trade: 0.020,
            max_pos_lev: 3.5,
            max_total_gross_lev: 9.0,
            bt_ann_pct: 158.5,
            bt_ret_23m_pct: 487.6,
            bt_max_dd_pct: 41.4,
            bt_worst_day_pct: -14.5,
            bt_sharpe: 1.67,
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
            version: "v1.0-rust-dyn80-fix12-sl14".into(),
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

    fn roll_daily_breaker(&mut self, ts: i64) {
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

// ---------------------------------------------------------------------------
// 合约分类 + 严格因果技术指标（零未来函数）
// ---------------------------------------------------------------------------

pub fn classify_binance_contract(sym_info: &Value) -> &'static str {
    let symbol = sym_info
        .get("symbol")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_uppercase();
    if !symbol.ends_with("USDT") {
        return "excluded";
    }
    if sym_info.get("status").and_then(|v| v.as_str()) != Some("TRADING") {
        return "excluded";
    }
    if sym_info.get("contractType").and_then(|v| v.as_str()) != Some("PERPETUAL") {
        return "excluded";
    }
    if sym_info.get("quoteAsset").and_then(|v| v.as_str()) != Some("USDT") {
        return "excluded";
    }
    if STABLE_OR_FX_EXCLUDED.contains(&symbol.as_str()) {
        return "excluded";
    }
    if COMMODITY_SYMBOLS.contains(&symbol.as_str()) {
        return "commodity";
    }
    if US_STOCK_SYMBOLS.contains(&symbol.as_str()) {
        return "stock";
    }
    if let Some(arr) = sym_info.get("underlyingSubType").and_then(|v| v.as_array()) {
        for item in arr {
            let up = item.as_str().unwrap_or("").to_uppercase();
            if up.contains("COMMODITY") || up.contains("METAL") || up.contains("ENERGY") || up.contains("PRECIOUS") {
                return "commodity";
            }
            if up.contains("EQUITY") || up.contains("STOCK") || up.contains("TRADFI") || up.contains("ETF") {
                return "stock";
            }
        }
    }
    let utype = sym_info
        .get("underlyingType")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_uppercase();
    if utype == "STOCK" || utype == "EQUITY" || utype == "TRADFI" {
        return "stock";
    }
    "crypto"
}

pub struct SeriesIndicators {
    pub ema168: Vec<f64>,
    pub ema336: Vec<f64>,
    pub atr24: Vec<f64>,
    pub rsi2_d: Vec<f64>,
    pub avg_qv24: Vec<f64>,
    pub avg_daily_qv7: Vec<f64>,
    pub split_block: Vec<bool>,
    pub last_funding: Vec<f64>,
}

fn ema_vec(vals: &[f64], span: usize) -> Vec<f64> {
    let n = vals.len();
    if n == 0 {
        return Vec::new();
    }
    let mut out = vec![0.0; n];
    let alpha = 2.0 / (span as f64 + 1.0);
    let one_m = 1.0 - alpha;
    let mut prev = vals[0];
    out[0] = prev;
    for i in 1..n {
        prev = alpha * vals[i] + one_m * prev;
        out[i] = prev;
    }
    out
}

fn wilder_atr_vec(bars: &[Bar1h], period: usize) -> Vec<f64> {
    let n = bars.len();
    if n == 0 {
        return Vec::new();
    }
    let mut tr = vec![0.0; n];
    tr[0] = bars[0].high - bars[0].low;
    for i in 1..n {
        let pc = bars[i - 1].close;
        let b = &bars[i];
        tr[i] = (b.high - b.low).max((b.high - pc).abs()).max((b.low - pc).abs());
    }
    let mut atr = vec![0.0; n];
    let mut running = 0.0;
    let lim = n.min(period);
    for i in 0..lim {
        running += tr[i];
        atr[i] = running / ((i + 1) as f64);
    }
    if n > period {
        let inv = 1.0 / (period as f64);
        let keep = ((period - 1) as f64) * inv;
        let mut prev = atr[period - 1];
        for i in period..n {
            prev = prev * keep + tr[i] * inv;
            atr[i] = prev;
        }
    }
    atr
}

fn rolling_mean_vec(vals: &[f64], win: usize) -> Vec<f64> {
    let n = vals.len();
    let mut out = vec![0.0; n];
    let mut acc = 0.0;
    for i in 0..n {
        acc += vals[i];
        if i >= win {
            acc -= vals[i - win];
            out[i] = acc / (win as f64);
        } else {
            out[i] = acc / ((i + 1) as f64);
        }
    }
    out
}

fn daily_rsi2_causal_vec(closes: &[f64]) -> Vec<f64> {
    let n = closes.len();
    let mut out = vec![50.0; n];
    if n < 72 {
        return out;
    }
    for i in 48..n {
        let d1 = closes[i] - closes[i - 24];
        let d2 = closes[i - 24] - closes[i - 48];
        let up = d1.max(0.0) + d2.max(0.0);
        let dn = (-d1).max(0.0) + (-d2).max(0.0);
        let tot = up + dn;
        out[i] = if tot > 1e-12 { 100.0 * up / tot } else { 50.0 };
    }
    out
}

/// Strictly causal funding alignment: latest settled rate with `fund_ts < open_time_ms + HOUR_MS`.
fn align_last_funding_strict(open_times: &[i64], funding_events: &[(i64, f64)]) -> Vec<f64> {
    let n = open_times.len();
    let mut out = vec![0.0; n];
    if funding_events.is_empty() || n == 0 {
        return out;
    }
    let mut ev = funding_events.to_vec();
    ev.sort_by_key(|x| x.0);
    for (i, &ot) in open_times.iter().enumerate() {
        let bar_close = ot + HOUR_MS;
        let idx = ev.partition_point(|x| x.0 < bar_close);
        if idx > 0 {
            out[i] = ev[idx - 1].1;
        }
    }
    out
}

pub fn compute_indicators(
    bars: &[Bar1h],
    funding_events: &[(i64, f64)],
    split_jump_pct: f64,
    split_block_hours: usize,
) -> SeriesIndicators {
    let n = bars.len();
    let closes: Vec<f64> = bars.iter().map(|b| b.close).collect();
    let qvs: Vec<f64> = bars.iter().map(|b| b.quote_volume).collect();
    let ots: Vec<i64> = bars.iter().map(|b| b.open_time_ms).collect();

    let ema168 = ema_vec(&closes, 168);
    let ema336 = ema_vec(&closes, 336);
    let atr24 = wilder_atr_vec(bars, 24);
    let rsi2_d = daily_rsi2_causal_vec(&closes);
    let avg_qv24 = rolling_mean_vec(&qvs, 24);
    let avg_daily_qv7: Vec<f64> = rolling_mean_vec(&qvs, 168).into_iter().map(|v| v * 24.0).collect();

    let mut jump = vec![0usize; n];
    for i in 1..n {
        let pc = closes[i - 1];
        if pc > 1e-12 && (closes[i] / pc - 1.0).abs() >= split_jump_pct {
            jump[i] = 1;
        }
    }
    let mut split_block = vec![false; n];
    let mut win_sum = 0usize;
    for i in 0..n {
        win_sum += jump[i];
        if i >= split_block_hours {
            win_sum -= jump[i - split_block_hours];
        }
        split_block[i] = win_sum > 0;
    }

    let last_funding = align_last_funding_strict(&ots, funding_events);
    SeriesIndicators {
        ema168,
        ema336,
        atr24,
        rsi2_d,
        avg_qv24,
        avg_daily_qv7,
        split_block,
        last_funding,
    }
}

pub fn detect_sleeve_a(
    symbol: &str,
    bars: &[Bar1h],
    ind: &SeriesIndicators,
    idx: usize,
    btc: &BtcRegime,
    cfg: &LimitOcoConfig,
) -> (Option<SignalOrder>, &'static str) {
    const LOOKBACK: usize = 168;
    const LIMIT_ATR: f64 = 0.15;
    const SL_ATR: f64 = 3.5;
    const TP_RR: f64 = 2.2;
    const VOL_MULT: f64 = 1.3;

    if idx < 340 || idx >= bars.len() {
        return (None, "warmup");
    }
    if ind.split_block[idx] {
        return (None, "split_block");
    }
    let atr = ind.atr24[idx];
    let cl = bars[idx].close;
    let qv = bars[idx].quote_volume;
    let avg_qv = ind.avg_qv24[idx - 1];
    if atr <= 0.0 || cl <= 0.0 || avg_qv <= 0.0 {
        return (None, "invalid_bar");
    }

    let mut prev_hi = f64::NEG_INFINITY;
    let mut prev_lo = f64::INFINITY;
    for b in &bars[(idx - LOOKBACK)..idx] {
        if b.high > prev_hi {
            prev_hi = b.high;
        }
        if b.low < prev_lo {
            prev_lo = b.low;
        }
    }
    let prev_cl = bars[idx - 1].close;
    let ema168 = ind.ema168[idx];
    let ema336 = ind.ema336[idx];

    let side: i8 = if cl > prev_hi && prev_cl <= prev_hi && cl > ema168 && cl > ema336 {
        1
    } else if cl < prev_lo && prev_cl >= prev_lo && cl < ema168 {
        -1
    } else {
        0
    };
    if side == 0 {
        return (None, "no_cross");
    }
    if qv < VOL_MULT * avg_qv {
        return (None, "a_vol_blocked");
    }
    if side == 1 && !btc.bull_ok {
        return (None, "a_btc_blocked");
    }
    if side == -1 && !btc.short_ok {
        return (None, "a_btc_blocked");
    }
    let fr = ind.last_funding[idx];
    if side == 1 && fr > 0.00005 {
        return (None, "a_fund_blocked");
    }
    if side == -1 && fr < 0.0 {
        return (None, "a_fund_blocked");
    }

    let lim = if side == 1 { cl - LIMIT_ATR * atr } else { cl + LIMIT_ATR * atr };
    if (side == 1 && lim >= cl * 0.9998) || (side == -1 && lim <= cl * 1.0002) {
        return (None, "a_passive_rejected");
    }
    let r = SL_ATR * atr;
    let sl = if side == 1 { lim - r } else { lim + r };
    let tp = if side == 1 { lim + TP_RR * r } else { lim - TP_RR * r };
    let sl_pct = (lim - sl).abs() / lim;
    if sl_pct < cfg.min_sl_pct || sl_pct > cfg.max_sl_pct {
        return (None, "a_sl_rejected");
    }
    let tp_pct = (tp - lim).abs() / lim;
    let vol_ratio = qv / avg_qv;
    let tag = if side == 1 { "7d向上放量突破" } else { "7d向下放量跌破" };
    (
        Some(SignalOrder {
            sleeve: "A".into(),
            symbol: symbol.into(),
            side,
            limit_px: lim,
            sl_px: sl,
            tp_px: tp,
            expire_bars: 12,
            max_hold_bars: 168,
            score: vol_ratio,
            sl_pct,
            tp_pct,
            signal_close: cl,
            atr24: atr,
            funding_rate: fr,
            reason: format!("{tag} · 量比{vol_ratio:.2}x · 费率{:+.2}bp", fr * 10_000.0),
        }),
        "ok",
    )
}

pub fn detect_sleeve_b(
    symbol: &str,
    bars: &[Bar1h],
    ind: &SeriesIndicators,
    idx: usize,
    cfg: &LimitOcoConfig,
) -> (Option<SignalOrder>, &'static str) {
    const DROP_HOURS: usize = 72;
    const RIP_PCT: f64 = 0.06;
    const RSI2_MIN: f64 = 90.0;
    const LIMIT_ATR: f64 = 0.20;
    const SL_ATR: f64 = 3.0;
    const TP_RR: f64 = 1.5;

    if idx < 340 || idx >= bars.len() {
        return (None, "warmup");
    }
    if ind.split_block[idx] {
        return (None, "split_block");
    }
    let atr = ind.atr24[idx];
    let cl = bars[idx].close;
    let base_cl = bars[idx - DROP_HOURS].close;
    if atr <= 0.0 || cl <= 0.0 || base_cl <= 1e-12 {
        return (None, "invalid_bar");
    }
    let ret72 = cl / base_cl - 1.0;
    if ret72 < RIP_PCT || cl >= ind.ema336[idx] {
        return (None, "no_rip");
    }
    let rsi2 = ind.rsi2_d[idx];
    if rsi2 < RSI2_MIN {
        return (None, "b_rsi_blocked");
    }
    let fr = ind.last_funding[idx];
    if fr < -0.0002 {
        return (None, "b_fund_blocked");
    }
    let lim = cl + LIMIT_ATR * atr;
    if lim <= cl * 1.0002 {
        return (None, "b_passive_rejected");
    }
    let r = SL_ATR * atr;
    let sl = lim + r;
    let tp = lim - TP_RR * r;
    let sl_pct = (lim - sl).abs() / lim;
    if sl_pct < cfg.min_sl_pct || sl_pct > cfg.max_sl_pct {
        return (None, "b_sl_rejected");
    }
    let tp_pct = (tp - lim).abs() / lim;
    (
        Some(SignalOrder {
            sleeve: "B".into(),
            symbol: symbol.into(),
            side: -1,
            limit_px: lim,
            sl_px: sl,
            tp_px: tp,
            expire_bars: 12,
            max_hold_bars: 96,
            score: ret72.abs(),
            sl_pct,
            tp_pct,
            signal_close: cl,
            atr24: atr,
            funding_rate: fr,
            reason: format!(
                "空头趋势3日急拉+{:.1}% · 日RSI2={:.0} · 费率{:+.2}bp",
                ret72 * 100.0,
                rsi2,
                fr * 10_000.0
            ),
        }),
        "ok",
    )
}

// ---------------------------------------------------------------------------
// 影子账本推演核心（完全离线可测试）
// ---------------------------------------------------------------------------

impl LimitOcoState {
    fn record_closed_trade(
        &mut self,
        ts_ms: i64,
        pos: &OpenPosition,
        exit_px: f64,
        exit_fee: f64,
        pnl_price: f64,
        reason: &str,
    ) {
        self.cash += pnl_price - exit_fee;
        self.total_fees += exit_fee;
        let total_fee = pos.fee_usd + exit_fee;
        let net_pnl = pnl_price - total_fee + pos.funding_usd;
        let risk_usd = pos.risk_usd.max(1e-9);
        let r_mult = net_pnl / risk_usd;
        self.realized_pnl += net_pnl;

        self.trades.push(ClosedTrade {
            sleeve: pos.sleeve.clone(),
            symbol: pos.symbol.clone(),
            side: pos.side,
            entry_ts: pos.entry_ts,
            exit_ts: ts_ms,
            hold_hours: ((ts_ms - pos.entry_ts) / HOUR_MS).max(0),
            entry_px: pos.entry_px,
            exit_px,
            sl_px: pos.sl_px,
            tp_px: pos.tp_px,
            sl_pct: pos.sl_pct,
            tp_pct: pos.tp_pct,
            qty: pos.qty,
            notional: pos.qty * pos.entry_px,
            risk_usd,
            net_pnl,
            r_mult,
            fee_usd: total_fee,
            funding_usd: pos.funding_usd,
            reason: reason.into(),
        });
        if self.trades.len() > 500 {
            let drain = self.trades.len() - 500;
            self.trades.drain(0..drain);
        }

        if self.config.cooldown_hours > 0 {
            self.cooldowns.insert(
                format!("{}:{}", pos.sleeve, pos.symbol),
                ts_ms + self.config.cooldown_hours * HOUR_MS,
            );
        }

        self.roll_daily_breaker(ts_ms);
        self.daily_r_pnl += r_mult;
        if self.config.daily_loss_limit_r > 0.0 && self.daily_r_pnl <= -self.config.daily_loss_limit_r {
            if !self.daily_breaker_active {
                self.daily_breaker_active = true;
                self.log_event(
                    ts_ms,
                    "BREAKER",
                    "",
                    "",
                    format!(
                        "触发日内熔断：今日已实现 {:+.2}R <= -{:.1}R，暂停新开仓至次日 UTC 00:00",
                        self.daily_r_pnl, self.config.daily_loss_limit_r
                    ),
                );
            }
        }

        let side_cn = if pos.side == 1 { "做多" } else { "做空" };
        self.log_event(
            ts_ms,
            reason,
            &pos.symbol,
            &pos.sleeve,
            format!(
                "[{}] {} {} {} 平仓 @ {:.6} · {:+.2} USDT ({:+.2}R)",
                pos.sleeve, pos.symbol, side_cn, reason, exit_px, net_pnl, r_mult
            ),
        );
    }

    pub fn recompute_stats(&mut self, mark_prices: &HashMap<String, f64>, ts_ms: i64, record_history: bool) -> f64 {
        let mut unreal = 0.0;
        let mut gross_notional = 0.0;
        let mut net_notional = 0.0;

        for (sym, pos) in self.positions.iter_mut() {
            let px = mark_prices.get(sym).copied().filter(|&v| v > 0.0).unwrap_or(pos.mark_px);
            pos.mark_px = px;
            let u_pnl = pos.qty * if pos.side == 1 { px - pos.entry_px } else { pos.entry_px - px };
            let notional = pos.qty * px;
            pos.notional = notional;
            pos.unreal_pnl = u_pnl;
            pos.unreal_r = u_pnl / pos.risk_usd.max(1e-9);
            pos.hold_hours = ((ts_ms - pos.entry_ts) / HOUR_MS).max(0);
            pos.progress_pct = if pos.side == 1 {
                if px >= pos.entry_px {
                    ((px - pos.entry_px) / (pos.tp_px - pos.entry_px).max(1e-12) * 100.0).min(100.0)
                } else {
                    (-((pos.entry_px - px) / (pos.entry_px - pos.sl_px).max(1e-12) * 100.0)).max(-100.0)
                }
            } else if px <= pos.entry_px {
                ((pos.entry_px - px) / (pos.entry_px - pos.tp_px).max(1e-12) * 100.0).min(100.0)
            } else {
                (-((px - pos.entry_px) / (pos.sl_px - pos.entry_px).max(1e-12) * 100.0)).max(-100.0)
            };
            unreal += u_pnl;
            gross_notional += notional;
            net_notional += notional * (pos.side as f64);
        }

        for (sym, pend) in self.pending.iter_mut() {
            let px = mark_prices.get(sym).copied().filter(|&v| v > 0.0).unwrap_or(pend.mark_px);
            pend.mark_px = px;
            pend.dist_to_fill_pct = if pend.side == 1 {
                (px - pend.limit_px) / px.max(1e-12) * 100.0
            } else {
                (pend.limit_px - px) / px.max(1e-12) * 100.0
            };
        }

        let equity = self.cash + unreal;
        let cap = self.capital.max(1e-9);
        self.equity = equity;
        self.unrealized_pnl = unreal;
        self.pnl = equity - cap;
        self.pnl_pct = (equity / cap - 1.0) * 100.0;
        self.gross_notional = gross_notional;
        self.net_notional = net_notional;
        self.gross_leverage = if equity > 0.0 { gross_notional / equity } else { 0.0 };

        if equity > self.peak_equity {
            self.peak_equity = equity;
        }
        let dd = if self.peak_equity > 0.0 {
            (self.peak_equity - equity) / self.peak_equity * 100.0
        } else {
            0.0
        };
        if dd > self.max_dd_pct {
            self.max_dd_pct = dd;
        }
        self.days_elapsed = ((ts_ms - self.started_at) as f64 / DAY_MS as f64).max(0.0);
        self.last_update = ts_ms;
        self.next_bar_ms = (ts_ms.div_euclid(HOUR_MS) + 1) * HOUR_MS;

        // Prune expired cooldowns
        self.cooldowns.retain(|_, &mut until| until > ts_ms);

        // Aggregate trade metrics
        if !self.trades.is_empty() {
            let mut wins = 0usize;
            let mut w_sum = 0.0;
            let mut l_sum = 0.0;
            let mut tot_r = 0.0;
            for t in &self.trades {
                tot_r += t.r_mult;
                if t.net_pnl > 0.0 {
                    wins += 1;
                    w_sum += t.net_pnl;
                } else {
                    l_sum += t.net_pnl.abs();
                }
            }
            self.win_rate = (wins as f64) / (self.trades.len() as f64) * 100.0;
            self.profit_factor = if l_sum > 1e-9 { w_sum / l_sum } else if w_sum > 0.0 { 99.0 } else { 0.0 };
            self.total_r = tot_r;
            self.avg_r = tot_r / (self.trades.len() as f64);
        } else {
            self.win_rate = 0.0;
            self.profit_factor = 0.0;
            self.total_r = 0.0;
            self.avg_r = 0.0;
        }

        for (slv, max_p) in [("A", self.config.sleeve_a_max_pos), ("B", self.config.sleeve_b_max_pos)] {
            let mut trades_n = 0usize;
            let mut wins_n = 0usize;
            let mut ws = 0.0;
            let mut ls = 0.0;
            let mut sum_r = 0.0;
            let mut net_pnl = 0.0;
            for t in &self.trades {
                if t.sleeve == slv {
                    trades_n += 1;
                    sum_r += t.r_mult;
                    net_pnl += t.net_pnl;
                    if t.net_pnl > 0.0 {
                        wins_n += 1;
                        ws += t.net_pnl;
                    } else {
                        ls += t.net_pnl.abs();
                    }
                }
            }
            let open_n = self.positions.values().filter(|p| p.sleeve == slv).count();
            let pend_n = self.pending.values().filter(|p| p.sleeve == slv).count();
            self.sleeve_stats.insert(
                slv.into(),
                SleeveStat {
                    open: open_n,
                    max: max_p,
                    pending: pend_n,
                    trades: trades_n,
                    wins: wins_n,
                    win_rate: if trades_n > 0 { (wins_n as f64) / (trades_n as f64) * 100.0 } else { 0.0 },
                    sum_r,
                    net_pnl,
                    pf: if ls > 1e-9 { ws / ls } else if ws > 0.0 { 99.0 } else { 0.0 },
                },
            );
        }

        if record_history {
            let pt = (ts_ms, (equity * 100.0).round() / 100.0, (self.gross_leverage * 1000.0).round() / 1000.0);
            if let Some(last) = self.history.last_mut() {
                if (last.0 - ts_ms).abs() < 55 * 60_000 {
                    *last = pt;
                } else {
                    self.history.push(pt);
                }
            } else {
                self.history.push(pt);
            }
            if self.history.len() > 4000 {
                let drain = self.history.len() - 4000;
                self.history.drain(0..drain);
            }
        }
        equity
    }

    #[cfg(test)]
    pub fn process_closed_bar(
        &mut self,
        bar_ts: i64,
        bars_by_sym: &HashMap<String, Vec<Bar1h>>,
        funding_by_sym: &HashMap<String, Vec<(i64, f64)>>,
        asset_class_by_sym: &HashMap<String, String>,
        live_marks: Option<&HashMap<String, f64>>,
    ) {
        self.process_closed_bar_with_opts(bar_ts, bars_by_sym, funding_by_sym, asset_class_by_sym, live_marks, true);
    }

    pub fn process_closed_bar_with_opts(
        &mut self,
        bar_ts: i64,
        bars_by_sym: &HashMap<String, Vec<Bar1h>>,
        funding_by_sym: &HashMap<String, Vec<(i64, f64)>>,
        asset_class_by_sym: &HashMap<String, String>,
        live_marks: Option<&HashMap<String, f64>>,
        allow_new_signals: bool,
    ) {
        self.roll_daily_breaker(bar_ts);
        let bar_close_ms = bar_ts + HOUR_MS;

        // Strictly slice up to `bar_ts` so future bars can never enter indicators
        let mut sliced: HashMap<String, (&[Bar1h], SeriesIndicators, usize)> = HashMap::new();
        for (sym, bars) in bars_by_sym {
            if let Some(idx) = bars.iter().rposition(|b| b.open_time_ms == bar_ts) {
                let prefix = &bars[..=idx];
                let fev = funding_by_sym.get(sym).map(|v| v.as_slice()).unwrap_or(&[]);
                let ind = compute_indicators(
                    prefix,
                    fev,
                    self.config.split_jump_pct,
                    self.config.split_block_hours,
                );
                sliced.insert(sym.clone(), (prefix, ind, idx));
            }
        }

        let Some((btc_bars, btc_ind, btc_idx)) = sliced.get("BTCUSDT") else {
            self.log_event(bar_close_ms, "WARN", "", "", "缺少 BTCUSDT 已收盘 K 线，跳过本小时扫描".into());
            self.last_bar_ms = bar_ts;
            return;
        };
        let btc_p = btc_bars[*btc_idx].close;
        let btc_e168 = btc_ind.ema168[*btc_idx];
        let btc_e336 = btc_ind.ema336[*btc_idx];
        let bull_ok = btc_p > btc_e168 && btc_p > btc_e336;
        let short_ok = btc_p < btc_e168;
        let btc_ctx = BtcRegime {
            bar_ts,
            price: btc_p,
            ema168: btc_e168,
            ema336: btc_e336,
            bull_ok,
            short_ok,
            funding_rate: btc_ind.last_funding[*btc_idx],
            label: if bull_ok {
                "多头趋势 (A多允许)".into()
            } else if short_ok {
                "空头趋势 (A空+B空活跃)".into()
            } else {
                "震荡过渡 (仅B空/观望)".into()
            },
        };
        self.btc_regime = btc_ctx.clone();

        // 1. Manage existing positions: Funding -> SL -> TP -> Time-Stop
        let open_syms: Vec<String> = self.positions.keys().cloned().collect();
        for sym in open_syms {
            let Some((s_bars, _, idx)) = sliced.get(&sym) else {
                continue;
            };
            let b = &s_bars[*idx];
            let mut pos = match self.positions.remove(&sym) {
                Some(p) => p,
                None => continue,
            };
            pos.hold_bars += 1;

            if let Some(fev) = funding_by_sym.get(&sym) {
                let f_sum: f64 = fev
                    .iter()
                    .filter(|&&(fts, _)| fts > pos.last_fund_ts && fts <= bar_close_ms)
                    .map(|&(_, r)| r)
                    .sum();
                if f_sum != 0.0 {
                    let f_cf = -(pos.side as f64) * pos.qty * b.close * f_sum;
                    self.cash += f_cf;
                    self.total_funding += f_cf;
                    pos.funding_usd += f_cf;
                    pos.last_fund_ts = bar_close_ms;
                }
            }

            // 1a. Hard SL first
            if (pos.side == 1 && b.low <= pos.sl_px) || (pos.side == -1 && b.high >= pos.sl_px) {
                let fill_px = if (pos.side == 1 && b.open < pos.sl_px) || (pos.side == -1 && b.open > pos.sl_px) {
                    b.open
                } else {
                    pos.sl_px
                };
                let pnl_p = pos.qty * if pos.side == 1 { fill_px - pos.entry_px } else { pos.entry_px - fill_px };
                let fee = pos.qty * fill_px * self.config.sl_cost;
                self.record_closed_trade(bar_close_ms, &pos, fill_px, fee, pnl_p, "SL");
                continue;
            }

            // 1b. Maker TP
            if (pos.side == 1 && b.high >= pos.tp_px * (1.0 + self.config.tp_pen))
                || (pos.side == -1 && b.low <= pos.tp_px * (1.0 - self.config.tp_pen))
            {
                let pnl_p = pos.qty * if pos.side == 1 { pos.tp_px - pos.entry_px } else { pos.entry_px - pos.tp_px };
                let fee = pos.qty * pos.tp_px * self.config.maker_fee;
                self.record_closed_trade(bar_close_ms, &pos, pos.tp_px, fee, pnl_p, "TP");
                continue;
            }

            // 1c. Time-stop
            if pos.hold_bars >= pos.max_hold_bars {
                let pnl_p = pos.qty * if pos.side == 1 { b.close - pos.entry_px } else { pos.entry_px - b.close };
                let fee = pos.qty * b.close * self.config.time_exit_cost;
                self.record_closed_trade(bar_close_ms, &pos, b.close, fee, pnl_p, "TIME");
                continue;
            }

            self.positions.insert(sym, pos);
        }

        let bar_closes: HashMap<String, f64> = sliced
            .iter()
            .map(|(s, (bs, _, idx))| (s.clone(), bs[*idx].close))
            .collect();
        let equity = self.recompute_stats(&bar_closes, bar_close_ms, false);
        let mut gross_notional = self.gross_notional;

        // 2. Process resting GTX pending orders
        let mut pend_order: Vec<(String, f64)> = self.pending.iter().map(|(s, p)| (s.clone(), p.score)).collect();
        pend_order.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));

        for (sym, _) in pend_order {
            let Some((s_bars, _, idx)) = sliced.get(&sym) else {
                continue;
            };
            let b = &s_bars[*idx];
            let mut pend = match self.pending.remove(&sym) {
                Some(p) => p,
                None => continue,
            };
            pend.age_hours += 1;

            let ran_away = (pend.side == 1 && b.high >= pend.tp_px) || (pend.side == -1 && b.low <= pend.tp_px);
            let lim = pend.limit_px;
            let mut filled = (pend.side == 1 && b.low <= lim * (1.0 - self.config.entry_pen))
                || (pend.side == -1 && b.high >= lim * (1.0 + self.config.entry_pen));
            if pend.age_hours == 1 && ((pend.side == 1 && b.open <= lim) || (pend.side == -1 && b.open >= lim)) {
                filled = false;
            }

            if filled {
                let slv_pos_n = self.positions.values().filter(|p| p.sleeve == pend.sleeve).count();
                let slv_same_n = self
                    .positions
                    .values()
                    .filter(|p| p.sleeve == pend.sleeve && p.side == pend.side)
                    .count();
                let (slv_max, slv_ss_max) = if pend.sleeve == "A" {
                    (self.config.sleeve_a_max_pos, self.config.sleeve_a_max_same_side)
                } else {
                    (self.config.sleeve_b_max_pos, self.config.sleeve_b_max_same_side)
                };
                if self.daily_breaker_active
                    || self.positions.len() >= self.config.max_total_positions
                    || slv_pos_n >= slv_max
                    || slv_same_n >= slv_ss_max
                    || equity <= 0.0
                    || self.positions.contains_key(&sym)
                {
                    continue;
                }
                let sl_dist = (lim - pend.sl_px).abs() / lim;
                if sl_dist < self.config.min_sl_pct || sl_dist > self.config.max_sl_pct {
                    continue;
                }
                let size_base = if self.config.compounding { equity } else { self.capital };
                let risk_usd = size_base * self.config.risk_per_trade;
                let notional = (risk_usd / sl_dist).min(size_base * self.config.max_pos_lev);
                if gross_notional + notional > equity * self.config.max_total_gross_lev {
                    continue;
                }

                let qty = notional / lim;
                let efee = notional * self.config.maker_fee;
                self.cash -= efee;
                self.total_fees += efee;
                self.funnel.filled += 1;

                let tp_pct = (pend.tp_px - lim).abs() / lim * 100.0;
                let pos_obj = OpenPosition {
                    sleeve: pend.sleeve.clone(),
                    symbol: sym.clone(),
                    asset_class: asset_class_by_sym.get(&sym).cloned().unwrap_or_else(|| "crypto".into()),
                    side: pend.side,
                    entry_ts: bar_close_ms,
                    last_fund_ts: bar_close_ms,
                    entry_px: lim,
                    mark_px: b.close,
                    qty,
                    notional,
                    risk_usd: notional * sl_dist,
                    sl_px: pend.sl_px,
                    tp_px: pend.tp_px,
                    sl_pct: sl_dist * 100.0,
                    tp_pct,
                    max_hold_bars: pend.max_hold_bars,
                    max_hold_hours: pend.max_hold_bars,
                    hold_bars: 0,
                    hold_hours: 0,
                    fee_usd: efee,
                    funding_usd: 0.0,
                    unreal_pnl: 0.0,
                    unreal_r: 0.0,
                    progress_pct: 0.0,
                    reason: pend.reason.clone(),
                };

                if (pend.side == 1 && b.low <= pend.sl_px) || (pend.side == -1 && b.high >= pend.sl_px) {
                    let pnl_p = qty * if pend.side == 1 { pend.sl_px - lim } else { lim - pend.sl_px };
                    let sfee = qty * pend.sl_px * self.config.sl_cost;
                    self.record_closed_trade(bar_close_ms, &pos_obj, pend.sl_px, sfee, pnl_p, "FILL_BAR_SL");
                    continue;
                }

                gross_notional += notional;
                let side_cn = if pend.side == 1 { "做多" } else { "做空" };
                self.log_event(
                    bar_close_ms,
                    "FILL",
                    &sym,
                    &pend.sleeve,
                    format!(
                        "[{}] {} {} Maker成交 @ {:.6} · 名义 ${:.0} · SL {:.6} (-{:.1}%) / TP {:.6} (+{:.1}%)",
                        pend.sleeve, sym, side_cn, lim, notional, pend.sl_px, sl_dist * 100.0, pend.tp_px, tp_pct
                    ),
                );
                self.positions.insert(sym, pos_obj);
                continue;
            }

            if ran_away {
                self.funnel.runaway += 1;
                continue;
            }
            if pend.age_hours >= pend.expire_hours {
                self.funnel.expired += 1;
                continue;
            }
            self.pending.insert(sym, pend);
        }

        // 3. Dynamic Top-80 ranking & new closed-bar signal generation
        let mut scored: Vec<(f64, String)> = Vec::new();
        for (sym, (_, ind, idx)) in &sliced {
            if *idx >= 340 && !ind.split_block[*idx] {
                let dqv7 = ind.avg_daily_qv7[*idx];
                if dqv7 >= self.config.min_daily_vol_usd {
                    scored.push((dqv7, sym.clone()));
                }
            }
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.1.cmp(&b.1)));
        let top_liquid: Vec<String> = scored.iter().take(self.config.top_n_liquid).map(|x| x.1.clone()).collect();

        let mut preview = Vec::new();
        for (dqv7, sym) in scored.iter().take(24) {
            if let Some((s_bars, ind, idx)) = sliced.get(sym) {
                let cl = s_bars[*idx].close;
                let prev24 = s_bars[idx.saturating_sub(24)].close;
                let ret24 = if prev24 > 0.0 { (cl / prev24 - 1.0) * 100.0 } else { 0.0 };
                let is_comm = asset_class_by_sym.get(sym).map(|s| s == "commodity").unwrap_or(false)
                    || COMMODITY_SYMBOLS.contains(&sym.as_str());
                preview.push(UniversePreviewRow {
                    symbol: sym.clone(),
                    daily_qv7_m: (dqv7 / 1e6 * 10.0).round() / 10.0,
                    price: cl,
                    ret24h: (ret24 * 100.0).round() / 100.0,
                    funding_bp: (ind.last_funding[*idx] * 10_000.0 * 100.0).round() / 100.0,
                    is_commodity: is_comm,
                });
            }
        }
        self.universe.liquid_count = top_liquid.len();
        self.universe.top80_preview = preview;
        self.funnel.bars_scanned += top_liquid.len() as u64;

        if !allow_new_signals {
            // 冷启动距上根整点收盘已过久，或历史补扫阶段：仅更新 BTC 罗盘与 Top-80，不回溯挂陈旧单
        } else if self.daily_breaker_active {
            self.funnel.breaker_skipped += 1;
        } else if self.positions.len() < self.config.max_total_positions {
            // Sleeve A
            if self.config.enabled_a
                && self.positions.values().filter(|p| p.sleeve == "A").count() < self.config.sleeve_a_max_pos
            {
                let mut cands = Vec::new();
                for sym in &top_liquid {
                    if self.positions.contains_key(sym) || self.pending.contains_key(sym) {
                        continue;
                    }
                    if self.cooldowns.get(&format!("A:{sym}")).copied().unwrap_or(0) > bar_close_ms {
                        continue;
                    }
                    if let Some((s_bars, ind, idx)) = sliced.get(sym) {
                        let (sig, tag) = detect_sleeve_a(sym, s_bars, ind, *idx, &btc_ctx, &self.config);
                        match tag {
                            "a_vol_blocked" => { self.funnel.a_raw_cross += 1; self.funnel.a_vol_blocked += 1; }
                            "a_btc_blocked" => { self.funnel.a_raw_cross += 1; self.funnel.a_btc_blocked += 1; }
                            "a_fund_blocked" => { self.funnel.a_raw_cross += 1; self.funnel.a_fund_blocked += 1; }
                            "a_sl_rejected" => { self.funnel.a_raw_cross += 1; self.funnel.a_sl_rejected += 1; }
                            "ok" => { self.funnel.a_raw_cross += 1; }
                            _ => {}
                        }
                        if let Some(s) = sig {
                            cands.push(s);
                        }
                    }
                }
                cands.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.symbol.cmp(&b.symbol)));
                for sig in cands {
                    if self.pending.values().filter(|p| p.sleeve == "A").count() >= self.config.sleeve_a_max_pos * 2 {
                        break;
                    }
                    if !self.pending.contains_key(&sig.symbol) {
                        let lm = live_marks.and_then(|m| m.get(&sig.symbol).copied());
                        if self.place_pending(sig, bar_close_ms, lm) {
                            self.funnel.a_placed += 1;
                        }
                    }
                }
            }

            // Sleeve B
            if self.config.enabled_b
                && self.positions.values().filter(|p| p.sleeve == "B").count() < self.config.sleeve_b_max_pos
            {
                let mut cands = Vec::new();
                for sym in &top_liquid {
                    if self.positions.contains_key(sym) || self.pending.contains_key(sym) {
                        continue;
                    }
                    if self.cooldowns.get(&format!("B:{sym}")).copied().unwrap_or(0) > bar_close_ms {
                        continue;
                    }
                    if let Some((s_bars, ind, idx)) = sliced.get(sym) {
                        let (sig, tag) = detect_sleeve_b(sym, s_bars, ind, *idx, &self.config);
                        match tag {
                            "b_rsi_blocked" => { self.funnel.b_raw_rip += 1; self.funnel.b_rsi_blocked += 1; }
                            "b_fund_blocked" => { self.funnel.b_raw_rip += 1; self.funnel.b_fund_blocked += 1; }
                            "b_sl_rejected" => { self.funnel.b_raw_rip += 1; self.funnel.b_sl_rejected += 1; }
                            "ok" => { self.funnel.b_raw_rip += 1; }
                            _ => {}
                        }
                        if let Some(s) = sig {
                            cands.push(s);
                        }
                    }
                }
                cands.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.symbol.cmp(&b.symbol)));
                for sig in cands {
                    if self.pending.values().filter(|p| p.sleeve == "B").count() >= self.config.sleeve_b_max_pos * 2 {
                        break;
                    }
                    if !self.pending.contains_key(&sig.symbol) {
                        let lm = live_marks.and_then(|m| m.get(&sig.symbol).copied());
                        if self.place_pending(sig, bar_close_ms, lm) {
                            self.funnel.b_placed += 1;
                        }
                    }
                }
            }
        }

        self.last_bar_ms = bar_ts;
        self.recompute_stats(live_marks.unwrap_or(&bar_closes), bar_close_ms, true);
    }

    /// 挂出纯 Maker GTX 限价单；若传入实时标记价 `live_mark`，严格校验 Post-Only 被动性
    ///（做多挂单价必须严格低于现价，做空挂单价必须严格高于现价，否则按币安 GTX 规则拒单）。
    fn place_pending(&mut self, sig: SignalOrder, ts_ms: i64, live_mark: Option<f64>) -> bool {
        if let Some(mp) = live_mark.filter(|&v| v > 0.0) {
            let violates_gtx = (sig.side == 1 && sig.limit_px >= mp * 0.9998)
                || (sig.side == -1 && sig.limit_px <= mp * 1.0002);
            if violates_gtx {
                let side_cn = if sig.side == 1 { "做多买单" } else { "做空卖单" };
                self.log_event(
                    ts_ms,
                    "GTX_REJECT",
                    &sig.symbol,
                    &sig.sleeve,
                    format!(
                        "[{}] {} {} GTX 拒单：挂单价 {:.6} 已不满足被动挂单条件（当前市价 {:.6}）",
                        sig.sleeve, sig.symbol, side_cn, sig.limit_px, mp
                    ),
                );
                return false;
            }
        }

        let mark_px = live_mark.filter(|&v| v > 0.0).unwrap_or(sig.signal_close);
        let dist_pct = if sig.side == 1 {
            (mark_px - sig.limit_px) / mark_px.max(1e-12) * 100.0
        } else {
            (sig.limit_px - mark_px) / mark_px.max(1e-12) * 100.0
        };
        let side_cn = if sig.side == 1 { "挂买单" } else { "挂卖单" };
        self.log_event(
            ts_ms,
            "SIGNAL",
            &sig.symbol,
            &sig.sleeve,
            format!(
                "[{}] {} {} GTX @ {:.6}（现价 {:.6}，距成交 +{:.2}%）· SL -{:.1}% / TP +{:.1}% ({})",
                sig.sleeve,
                sig.symbol,
                side_cn,
                sig.limit_px,
                mark_px,
                dist_pct,
                sig.sl_pct * 100.0,
                sig.tp_pct * 100.0,
                sig.reason
            ),
        );
        self.pending.insert(
            sig.symbol.clone(),
            PendingOrder {
                sleeve: sig.sleeve,
                symbol: sig.symbol,
                side: sig.side,
                limit_px: sig.limit_px,
                mark_px,
                dist_to_fill_pct: dist_pct,
                sl_px: sig.sl_px,
                tp_px: sig.tp_px,
                sl_pct: sig.sl_pct * 100.0,
                tp_pct: sig.tp_pct * 100.0,
                expire_hours: sig.expire_bars,
                max_hold_bars: sig.max_hold_bars,
                age_hours: 0,
                score: (sig.score * 10_000.0).round() / 10_000.0,
                funding_bp: (sig.funding_rate * 10_000.0 * 100.0).round() / 100.0,
                created_ts: ts_ms,
                reason: sig.reason,
            },
        );
        true
    }

    /// 60 秒实时盘中盯市推进：已挂在盘口的 GTX 限价单一旦被实时市价穿越即刻成交转持仓；
    /// 已持仓位一旦触及硬止损 SL 或 Maker 止盈 TP 即刻平仓落袋，无需干等到下一个整点。
    pub fn process_intrabar_marks(
        &mut self,
        mark_prices: &HashMap<String, f64>,
        asset_class_by_sym: &HashMap<String, String>,
        ts_ms: i64,
    ) {
        self.roll_daily_breaker(ts_ms);

        // 1. 持仓实时触发 SL / TP（硬止损优先）
        let open_syms: Vec<String> = self.positions.keys().cloned().collect();
        for sym in open_syms {
            let Some(&px) = mark_prices.get(&sym).filter(|&&v| v > 0.0) else {
                continue;
            };
            let mut pos = match self.positions.remove(&sym) {
                Some(p) => p,
                None => continue,
            };
            if (pos.side == 1 && px <= pos.sl_px) || (pos.side == -1 && px >= pos.sl_px) {
                let fill_px = if (pos.side == 1 && px < pos.sl_px) || (pos.side == -1 && px > pos.sl_px) {
                    px
                } else {
                    pos.sl_px
                };
                let pnl_p = pos.qty * if pos.side == 1 { fill_px - pos.entry_px } else { pos.entry_px - fill_px };
                let fee = pos.qty * fill_px * self.config.sl_cost;
                self.record_closed_trade(ts_ms, &pos, fill_px, fee, pnl_p, "SL");
                continue;
            }
            if (pos.side == 1 && px >= pos.tp_px * (1.0 + self.config.tp_pen))
                || (pos.side == -1 && px <= pos.tp_px * (1.0 - self.config.tp_pen))
            {
                let pnl_p = pos.qty * if pos.side == 1 { pos.tp_px - pos.entry_px } else { pos.entry_px - pos.tp_px };
                let fee = pos.qty * pos.tp_px * self.config.maker_fee;
                self.record_closed_trade(ts_ms, &pos, pos.tp_px, fee, pnl_p, "TP");
                continue;
            }
            pos.mark_px = px;
            self.positions.insert(sym, pos);
        }

        let equity = self.recompute_stats(mark_prices, ts_ms, false);
        let mut gross_notional = self.gross_notional;

        // 2. 已挂在盘口的 GTX 挂单：市价穿透挂单价即刻成交，或先触及 TP 则逃逸撤单
        let mut pend_order: Vec<(String, f64)> = self.pending.iter().map(|(s, p)| (s.clone(), p.score)).collect();
        pend_order.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));

        for (sym, _) in pend_order {
            let Some(&px) = mark_prices.get(&sym).filter(|&&v| v > 0.0) else {
                continue;
            };
            let pend = match self.pending.remove(&sym) {
                Some(p) => p,
                None => continue,
            };
            let lim = pend.limit_px;
            let ran_away = (pend.side == 1 && px >= pend.tp_px) || (pend.side == -1 && px <= pend.tp_px);
            let filled = (pend.side == 1 && px <= lim * (1.0 - self.config.entry_pen))
                || (pend.side == -1 && px >= lim * (1.0 + self.config.entry_pen));

            if filled {
                let slv_pos_n = self.positions.values().filter(|p| p.sleeve == pend.sleeve).count();
                let slv_same_n = self
                    .positions
                    .values()
                    .filter(|p| p.sleeve == pend.sleeve && p.side == pend.side)
                    .count();
                let (slv_max, slv_ss_max) = if pend.sleeve == "A" {
                    (self.config.sleeve_a_max_pos, self.config.sleeve_a_max_same_side)
                } else {
                    (self.config.sleeve_b_max_pos, self.config.sleeve_b_max_same_side)
                };
                if self.daily_breaker_active
                    || self.positions.len() >= self.config.max_total_positions
                    || slv_pos_n >= slv_max
                    || slv_same_n >= slv_ss_max
                    || equity <= 0.0
                    || self.positions.contains_key(&sym)
                {
                    continue;
                }
                let sl_dist = (lim - pend.sl_px).abs() / lim;
                if sl_dist < self.config.min_sl_pct || sl_dist > self.config.max_sl_pct {
                    continue;
                }
                let size_base = if self.config.compounding { equity } else { self.capital };
                let risk_usd = size_base * self.config.risk_per_trade;
                let notional = (risk_usd / sl_dist).min(size_base * self.config.max_pos_lev);
                if gross_notional + notional > equity * self.config.max_total_gross_lev {
                    continue;
                }

                let qty = notional / lim;
                let efee = notional * self.config.maker_fee;
                self.cash -= efee;
                self.total_fees += efee;
                self.funnel.filled += 1;

                let tp_pct = (pend.tp_px - lim).abs() / lim * 100.0;
                let pos_obj = OpenPosition {
                    sleeve: pend.sleeve.clone(),
                    symbol: sym.clone(),
                    asset_class: asset_class_by_sym.get(&sym).cloned().unwrap_or_else(|| "crypto".into()),
                    side: pend.side,
                    entry_ts: ts_ms,
                    last_fund_ts: ts_ms,
                    entry_px: lim,
                    mark_px: px,
                    qty,
                    notional,
                    risk_usd: notional * sl_dist,
                    sl_px: pend.sl_px,
                    tp_px: pend.tp_px,
                    sl_pct: sl_dist * 100.0,
                    tp_pct,
                    max_hold_bars: pend.max_hold_bars,
                    max_hold_hours: pend.max_hold_bars,
                    hold_bars: 0,
                    hold_hours: 0,
                    fee_usd: efee,
                    funding_usd: 0.0,
                    unreal_pnl: 0.0,
                    unreal_r: 0.0,
                    progress_pct: 0.0,
                    reason: pend.reason.clone(),
                };

                if (pend.side == 1 && px <= pend.sl_px) || (pend.side == -1 && px >= pend.sl_px) {
                    let pnl_p = qty * if pend.side == 1 { pend.sl_px - lim } else { lim - pend.sl_px };
                    let sfee = qty * pend.sl_px * self.config.sl_cost;
                    self.record_closed_trade(ts_ms, &pos_obj, pend.sl_px, sfee, pnl_p, "FILL_BAR_SL");
                    continue;
                }

                gross_notional += notional;
                let side_cn = if pend.side == 1 { "做多" } else { "做空" };
                self.log_event(
                    ts_ms,
                    "FILL",
                    &sym,
                    &pend.sleeve,
                    format!(
                        "[{}] {} {} 盘中 Maker 成交 @ {:.6}（市价 {:.6}）· 名义 ${:.0} · SL {:.6} / TP {:.6}",
                        pend.sleeve, sym, side_cn, lim, px, notional, pend.sl_px, pend.tp_px
                    ),
                );
                self.positions.insert(sym, pos_obj);
                continue;
            }

            if ran_away {
                self.funnel.runaway += 1;
                self.log_event(
                    ts_ms,
                    "RUNAWAY",
                    &sym,
                    &pend.sleeve,
                    format!("[{}] {} 市价 {:.6} 已直接触及止盈位 {:.6}，撤销未成交挂单", pend.sleeve, sym, px, pend.tp_px),
                );
                continue;
            }

            self.pending.insert(sym, pend);
        }

        self.recompute_stats(mark_prices, ts_ms, false);
    }
}

// ---------------------------------------------------------------------------
// 增量 K 线缓存 + 币安 USD-M 异步行情拉取（纯 Rust reqwest + tokio）
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct MarketDataCache {
    exchange_updated_ms: i64,
    eligible: HashMap<String, String>,
    total_perps: usize,
    excluded_stocks: usize,
    bars: HashMap<String, Vec<Bar1h>>,
    funding: HashMap<String, Vec<(i64, f64)>>,
}

impl MarketDataCache {
    fn load(path: &Path) -> Self {
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(c) = serde_json::from_slice::<Self>(&bytes) {
                return c;
            }
        }
        Self::default()
    }

    fn save(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
        if let Ok(bytes) = serde_json::to_vec(self) {
            if std::fs::write(&tmp, bytes).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
    }

    fn merge_bars(&mut self, sym: &str, new_bars: Vec<Bar1h>) -> Vec<Bar1h> {
        let mut by_ts: BTreeMap<i64, Bar1h> = BTreeMap::new();
        if let Some(old) = self.bars.get(sym) {
            for b in old {
                by_ts.insert(b.open_time_ms, b.clone());
            }
        }
        for b in new_bars {
            by_ts.insert(b.open_time_ms, b);
        }
        let all: Vec<Bar1h> = by_ts.into_values().collect();
        let trimmed = if all.len() > MAX_BARS_CACHE {
            all[all.len() - MAX_BARS_CACHE..].to_vec()
        } else {
            all
        };
        self.bars.insert(sym.to_string(), trimmed.clone());
        trimmed
    }

    fn merge_funding(&mut self, sym: &str, events: Vec<(i64, f64)>) -> Vec<(i64, f64)> {
        let mut by_ts: BTreeMap<i64, f64> = BTreeMap::new();
        if let Some(old) = self.funding.get(sym) {
            for &(t, r) in old {
                by_ts.insert(t, r);
            }
        }
        for (t, r) in events {
            by_ts.insert(t, r);
        }
        let all: Vec<(i64, f64)> = by_ts.into_iter().collect();
        let trimmed = if all.len() > 120 {
            all[all.len() - 120..].to_vec()
        } else {
            all
        };
        self.funding.insert(sym.to_string(), trimmed.clone());
        trimmed
    }
}

fn build_binance_http(proxy_url: &str) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .pool_max_idle_per_host(16)
        .user_agent("stars-limitoco-rust/1.0");
    let proxy = if !proxy_url.trim().is_empty() {
        proxy_url.trim().to_string()
    } else {
        std::env::var("BINANCE_PROXY")
            .or_else(|_| std::env::var("HTTPS_PROXY"))
            .or_else(|_| std::env::var("https_proxy"))
            .unwrap_or_default()
    };
    if !proxy.is_empty() {
        builder = builder.proxy(reqwest::Proxy::all(&proxy).context("无效的代理地址")?);
    }
    builder.build().context("创建币安 HTTP 客户端失败")
}

fn fapi_base() -> String {
    std::env::var("BINANCE_FAPI_BASE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| FAPI_BASE_DEFAULT.into())
        .trim_end_matches('/')
        .to_string()
}

async fn get_json_retry(client: &reqwest::Client, url: &str) -> Result<Value> {
    let mut last_err = None;
    for attempt in 0..3 {
        match client.get(url).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    return resp.json::<Value>().await.context("解析 JSON 失败");
                }
                last_err = Some(anyhow::anyhow!("HTTP {} {}", resp.status(), url));
            }
            Err(e) => {
                last_err = Some(anyhow::anyhow!("{e}"));
            }
        }
        if attempt < 2 {
            tokio::time::sleep(Duration::from_millis(450 * (attempt + 1))).await;
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("请求失败: {url}")))
}

async fn fetch_premium_marks(client: &reqwest::Client, base: &str) -> Result<HashMap<String, f64>> {
    let v = get_json_retry(client, &format!("{base}/fapi/v1/premiumIndex")).await?;
    let mut out = HashMap::new();
    if let Some(arr) = v.as_array() {
        for item in arr {
            let sym = item.get("symbol").and_then(|x| x.as_str()).unwrap_or("").to_uppercase();
            let mp = item
                .get("markPrice")
                .and_then(|x| x.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            if !sym.is_empty() && mp > 0.0 {
                out.insert(sym, mp);
            }
        }
    }
    Ok(out)
}

async fn fetch_exchange_universe(client: &reqwest::Client, base: &str) -> Result<(HashMap<String, String>, usize, usize)> {
    let v = get_json_retry(client, &format!("{base}/fapi/v1/exchangeInfo")).await?;
    let mut eligible = HashMap::new();
    let mut total_perps = 0usize;
    let mut excluded_stocks = 0usize;
    if let Some(arr) = v.get("symbols").and_then(|x| x.as_array()) {
        for s in arr {
            if s.get("contractType").and_then(|x| x.as_str()) == Some("PERPETUAL")
                && s.get("quoteAsset").and_then(|x| x.as_str()) == Some("USDT")
                && s.get("status").and_then(|x| x.as_str()) == Some("TRADING")
            {
                total_perps += 1;
            }
            let cls = classify_binance_contract(s);
            if cls == "crypto" || cls == "commodity" {
                if let Some(sym) = s.get("symbol").and_then(|x| x.as_str()) {
                    eligible.insert(sym.to_uppercase(), cls.into());
                }
            } else if cls == "stock" {
                excluded_stocks += 1;
            }
        }
    }
    Ok((eligible, total_perps, excluded_stocks))
}

async fn fetch_24h_qv(client: &reqwest::Client, base: &str) -> Result<HashMap<String, f64>> {
    let v = get_json_retry(client, &format!("{base}/fapi/v1/ticker/24hr")).await?;
    let mut out = HashMap::new();
    if let Some(arr) = v.as_array() {
        for item in arr {
            let sym = item.get("symbol").and_then(|x| x.as_str()).unwrap_or("").to_uppercase();
            let qv = item
                .get("quoteVolume")
                .and_then(|x| x.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            if !sym.is_empty() {
                out.insert(sym, qv);
            }
        }
    }
    Ok(out)
}

async fn fetch_closed_1h_bars(
    client: &reqwest::Client,
    base: &str,
    sym: &str,
    cur_hour_open: i64,
    limit: usize,
) -> Result<Vec<Bar1h>> {
    let url = format!("{base}/fapi/v1/klines?symbol={sym}&interval=1h&limit={limit}");
    let v = get_json_retry(client, &url).await?;
    let mut bars = Vec::new();
    if let Some(arr) = v.as_array() {
        for row in arr {
            let Some(r) = row.as_array() else { continue };
            if r.len() < 8 {
                continue;
            }
            let ot = r[0].as_i64().unwrap_or(0);
            if ot <= 0 || ot >= cur_hour_open {
                continue;
            }
            let p = |i: usize| -> f64 {
                r[i].as_str()
                    .and_then(|s| s.parse::<f64>().ok())
                    .or_else(|| r[i].as_f64())
                    .unwrap_or(0.0)
            };
            bars.push(Bar1h {
                open_time_ms: ot,
                open: p(1),
                high: p(2),
                low: p(3),
                close: p(4),
                volume: p(5),
                quote_volume: p(7),
            });
        }
    }
    bars.sort_by_key(|b| b.open_time_ms);
    Ok(bars)
}

async fn fetch_funding_history(
    client: &reqwest::Client,
    base: &str,
    sym: &str,
    start_ms: i64,
    end_ms: i64,
    limit: usize,
) -> Result<Vec<(i64, f64)>> {
    let url = format!(
        "{base}/fapi/v1/fundingRate?symbol={sym}&startTime={start_ms}&endTime={end_ms}&limit={limit}"
    );
    let v = get_json_retry(client, &url).await?;
    let mut out = Vec::new();
    if let Some(arr) = v.as_array() {
        for item in arr {
            let ts = item.get("fundingTime").and_then(|x| x.as_i64()).unwrap_or(0);
            let rate = item
                .get("fundingRate")
                .and_then(|x| x.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            if ts > 0 {
                out.push((ts, rate));
            }
        }
    }
    out.sort_by_key(|x| x.0);
    Ok(out)
}

/// 推进一步影子引擎（由后台 tokio 定时任务或前端手动点击「启动/立即扫描」调用）。
pub async fn step_once(
    state: &mut LimitOcoState,
    state_path: &Path,
    force_scan: bool,
) {
    let started = Instant::now();
    let now = now_ms();
    if !state.config.running && !force_scan {
        state.running = false;
        state.status_text = "已暂停".into();
        let _ = state.save(state_path);
        return;
    }

    let client = match build_binance_http(&state.config.proxy_url) {
        Ok(c) => c,
        Err(e) => {
            state.last_error = Some(format!("{e}"));
            state.status_text = "代理配置异常".into();
            state.log_event(now, "ERROR", "", "", format!("HTTP 客户端初始化失败: {e}"));
            let _ = state.save(state_path);
            return;
        }
    };
    let base = fapi_base();
    let cache_path = state_path.with_file_name("limitoco_kline_cache.json");

    let step_res: Result<()> = async {
        let mark_prices = fetch_premium_marks(&client, &base).await?;
        let cur_hour_open = now.div_euclid(HOUR_MS) * HOUR_MS;
        let latest_closed_bar_ts = cur_hour_open - HOUR_MS;
        let need_hourly_scan = force_scan || latest_closed_bar_ts > state.last_bar_ms;

        if need_hourly_scan {
            let mut cache = MarketDataCache::load(&cache_path);
            if now - cache.exchange_updated_ms > 6 * HOUR_MS || cache.eligible.is_empty() {
                let (el, tot, ex_st) = fetch_exchange_universe(&client, &base).await?;
                cache.eligible = el;
                cache.total_perps = tot;
                cache.excluded_stocks = ex_st;
                cache.exchange_updated_ms = now;
            }
            state.universe.total_perps = cache.total_perps;
            state.universe.excluded_stocks = cache.excluded_stocks;
            state.universe.eligible_count = cache.eligible.len();

            let qv24 = fetch_24h_qv(&client, &base).await?;
            let mut ranked: Vec<(f64, String)> = cache
                .eligible
                .keys()
                .map(|s| (qv24.get(s).copied().unwrap_or(0.0), s.clone()))
                .collect();
            ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.1.cmp(&b.1)));

            let n_fetch = ((state.config.top_n_liquid as f64 * 1.45).round() as usize).max(95);
            let mut targets: HashSet<String> = ranked.into_iter().take(n_fetch).map(|x| x.1).collect();
            targets.insert("BTCUSDT".into());
            for &c in COMMODITY_SYMBOLS {
                if cache.eligible.contains_key(c) {
                    targets.insert(c.into());
                }
            }
            for s in state.positions.keys() {
                targets.insert(s.clone());
            }
            for s in state.pending.keys() {
                targets.insert(s.clone());
            }

            let sem = Arc::new(Semaphore::new(12));
            let mut join_set = JoinSet::new();
            for sym in targets {
                let c = client.clone();
                let b_url = base.clone();
                let permit_sem = sem.clone();
                let cached_bars_len = cache.bars.get(&sym).map(|v| v.len()).unwrap_or(0);
                let cached_last_bar = cache.bars.get(&sym).and_then(|v| v.last()).map(|b| b.open_time_ms).unwrap_or(0);
                let cached_last_fund = cache.funding.get(&sym).and_then(|v| v.last()).map(|x| x.0).unwrap_or(0);
                let k_lim = if cached_bars_len >= 360 && cached_last_bar >= latest_closed_bar_ts - 6 * HOUR_MS {
                    8usize
                } else {
                    COLD_FETCH_LIMIT
                };
                let (f_start, f_lim) = if cached_last_fund >= cur_hour_open - 9 * HOUR_MS {
                    (cached_last_fund + 1, 12usize)
                } else {
                    (cur_hour_open - 7 * DAY_MS, 30usize)
                };

                join_set.spawn(async move {
                    let _permit = permit_sem.acquire_owned().await.ok()?;
                    let nb = fetch_closed_1h_bars(&c, &b_url, &sym, cur_hour_open, k_lim).await.ok()?;
                    let nf = fetch_funding_history(&c, &b_url, &sym, f_start, cur_hour_open, f_lim)
                        .await
                        .unwrap_or_default();
                    Some((sym, nb, nf))
                });
            }

            let mut bars_by_sym: HashMap<String, Vec<Bar1h>> = HashMap::new();
            let mut funding_by_sym: HashMap<String, Vec<(i64, f64)>> = HashMap::new();
            while let Some(res) = join_set.join_next().await {
                if let Ok(Some((sym, nb, nf))) = res {
                    let mb = cache.merge_bars(&sym, nb);
                    let mf = cache.merge_funding(&sym, nf);
                    if !mb.is_empty() {
                        bars_by_sym.insert(sym.clone(), mb);
                        funding_by_sym.insert(sym, mf);
                    }
                }
            }
            cache.save(&cache_path);

            let bars_to_run: Vec<i64> = if state.last_bar_ms == 0 {
                vec![latest_closed_bar_ts]
            } else {
                let start_bar = (state.last_bar_ms + HOUR_MS).max(latest_closed_bar_ts - 11 * HOUR_MS);
                let mut v = Vec::new();
                let mut t = start_bar;
                while t <= latest_closed_bar_ts {
                    v.push(t);
                    t += HOUR_MS;
                }
                v
            };

            if bars_to_run.is_empty() {
                // Already processed current closed hour; refresh BTC regime + live marks
                if let Some(btc_b) = bars_by_sym.get("BTCUSDT") {
                    let fev = funding_by_sym.get("BTCUSDT").map(|v| v.as_slice()).unwrap_or(&[]);
                    let ind = compute_indicators(btc_b, fev, state.config.split_jump_pct, state.config.split_block_hours);
                    let bi = btc_b.len() - 1;
                    let bp = btc_b[bi].close;
                    let e168 = ind.ema168[bi];
                    let e336 = ind.ema336[bi];
                    let bull_ok = bp > e168 && bp > e336;
                    let short_ok = bp < e168;
                    state.btc_regime = BtcRegime {
                        bar_ts: btc_b[bi].open_time_ms,
                        price: bp,
                        ema168: e168,
                        ema336: e336,
                        bull_ok,
                        short_ok,
                        funding_rate: ind.last_funding[bi],
                        label: if bull_ok {
                            "多头趋势 (A多允许)".into()
                        } else if short_ok {
                            "空头趋势 (A空+B空活跃)".into()
                        } else {
                            "震荡过渡 (仅B空/观望)".into()
                        },
                    };
                }
                state.process_intrabar_marks(&mark_prices, &cache.eligible, now);
            } else {
                const LIVE_SIGNAL_WINDOW_MS: i64 = 10 * 60_000;
                let was_cold = state.last_bar_ms == 0;
                for &bts in &bars_to_run {
                    let age_since_close = now - (bts + HOUR_MS);
                    let is_latest = bts == latest_closed_bar_ts;
                    let allow_new = is_latest && age_since_close <= LIVE_SIGNAL_WINDOW_MS;
                    state.process_closed_bar_with_opts(
                        bts,
                        &bars_by_sym,
                        &funding_by_sym,
                        &cache.eligible,
                        Some(&mark_prices),
                        allow_new,
                    );
                    if was_cold && is_latest && !allow_new {
                        let mins = (age_since_close / 60_000).max(1);
                        state.log_event(
                            now,
                            "INIT",
                            "",
                            "",
                            format!(
                                "冷启动行情与 Top-{} 标的池已同步（距上根 1h 收盘已 {} 分钟，跳过陈旧信号，等待下根整点收盘后挂新单）",
                                state.universe.liquid_count, mins
                            ),
                        );
                    }
                }
                state.process_intrabar_marks(&mark_prices, &cache.eligible, now);
            }
        } else {
            let cache = MarketDataCache::load(&cache_path);
            state.process_intrabar_marks(&mark_prices, &cache.eligible, now);
        }

        state.last_error = None;
        state.running = state.config.running;
        state.status_text = if state.config.running {
            "运行中".into()
        } else {
            "已暂停（手动扫描完成）".into()
        };
        Ok(())
    }
    .await;

    if let Err(e) = step_res {
        state.last_error = Some(format!("{e}"));
        state.status_text = "行情连接异常".into();
        state.log_event(now, "ERROR", "", "", format!("币安行情拉取失败：{e}"));
    }

    state.scan_duration_ms = started.elapsed().as_millis() as u64;
    let _ = state.save(state_path);
}

// ---------------------------------------------------------------------------
// 单元测试：分类 / 截断不变性（零未来函数）/ 严格资金费边界 / 完整影子生命周期
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_bars(closes: &[f64], spread: f64, qv: f64, start_ms: i64) -> Vec<Bar1h> {
        closes
            .iter()
            .enumerate()
            .map(|(i, &c)| {
                let o = if i > 0 { closes[i - 1] } else { c };
                Bar1h {
                    open_time_ms: start_ms + (i as i64) * HOUR_MS,
                    open: o,
                    high: o.max(c) * (1.0 + spread),
                    low: o.min(c) * (1.0 - spread),
                    close: c,
                    volume: qv / c.max(1e-9),
                    quote_volume: qv,
                }
            })
            .collect()
    }

    #[test]
    fn contract_classification_excludes_us_stocks_and_keeps_commodities() {
        assert_eq!(
            classify_binance_contract(&json!({"symbol":"BTCUSDT","status":"TRADING","contractType":"PERPETUAL","quoteAsset":"USDT"})),
            "crypto"
        );
        assert_eq!(
            classify_binance_contract(&json!({"symbol":"XAUUSDT","status":"TRADING","contractType":"PERPETUAL","quoteAsset":"USDT","underlyingSubType":["TRADFI"]})),
            "commodity"
        );
        assert_eq!(
            classify_binance_contract(&json!({"symbol":"CLUSDT","status":"TRADING","contractType":"PERPETUAL","quoteAsset":"USDT"})),
            "commodity"
        );
        assert_eq!(
            classify_binance_contract(&json!({"symbol":"NVDAUSDT","status":"TRADING","contractType":"PERPETUAL","quoteAsset":"USDT"})),
            "stock"
        );
        assert_eq!(
            classify_binance_contract(&json!({"symbol":"FOOEQUSDT","status":"TRADING","contractType":"PERPETUAL","quoteAsset":"USDT","underlyingSubType":["EQUITY"]})),
            "stock"
        );
        assert_eq!(
            classify_binance_contract(&json!({"symbol":"USDCUSDT","status":"TRADING","contractType":"PERPETUAL","quoteAsset":"USDT"})),
            "excluded"
        );
    }

    #[test]
    fn truncation_invariance_proves_zero_lookahead() {
        let closes: Vec<f64> = (0..520)
            .map(|i| 100.0 + 15.0 * ((i as f64) / 25.0).sin() + 0.03 * (i as f64))
            .collect();
        let bars_full = make_bars(&closes, 0.005, 2_000_000.0, 1_700_000_000_000);
        let funding_full: Vec<(i64, f64)> = (0..520)
            .step_by(8)
            .map(|i| (bars_full[i].open_time_ms, 0.0001 * (i as f64).cos()))
            .collect();

        let cut = 381usize;
        let idx = cut - 1;
        let cut_close = bars_full[idx].open_time_ms + HOUR_MS;
        let bars_trunc = &bars_full[..cut];
        let funding_trunc: Vec<(i64, f64)> = funding_full.iter().copied().filter(|&(t, _)| t < cut_close).collect();

        let full = compute_indicators(&bars_full, &funding_full, 0.35, 72);
        let trunc = compute_indicators(bars_trunc, &funding_trunc, 0.35, 72);

        assert!((full.ema168[idx] - trunc.ema168[idx]).abs() < 1e-12);
        assert!((full.ema336[idx] - trunc.ema336[idx]).abs() < 1e-12);
        assert!((full.atr24[idx] - trunc.atr24[idx]).abs() < 1e-12);
        assert!((full.rsi2_d[idx] - trunc.rsi2_d[idx]).abs() < 1e-12);
        assert!((full.avg_qv24[idx] - trunc.avg_qv24[idx]).abs() < 1e-12);
        assert!((full.avg_daily_qv7[idx] - trunc.avg_daily_qv7[idx]).abs() < 1e-12);
        assert_eq!(full.split_block[idx], trunc.split_block[idx]);
        assert!((full.last_funding[idx] - trunc.last_funding[idx]).abs() < 1e-12);
    }

    #[test]
    fn funding_alignment_is_strictly_before_bar_close() {
        let bars = make_bars(&[100.0; 10], 0.002, 1_000_000.0, 0);
        let ind = compute_indicators(&bars, &[(0, 0.0001), (HOUR_MS, 0.0009)], 0.35, 72);
        assert!((ind.last_funding[0] - 0.0001).abs() < 1e-12);
        assert!((ind.last_funding[1] - 0.0009).abs() < 1e-12);
    }

    #[test]
    fn sleeve_a_btc_ema336_guard_and_sleeve_b_short_rip() {
        let cfg = LimitOcoConfig::default();
        let mut closes = vec![100.0; 350];
        for i in 0..10 {
            closes.push(100.0 + ((i + 1) as f64) * 0.2);
        }
        let mut bars = make_bars(&closes, 0.006, 1_500_000.0, 1_700_000_000_000);
        let last_idx = bars.len() - 1;
        bars[last_idx] = Bar1h {
            open_time_ms: bars[last_idx].open_time_ms,
            open: 102.0,
            high: 106.0,
            low: 101.8,
            close: 105.5,
            volume: 30_000.0,
            quote_volume: 3_500_000.0,
        };
        let ind = compute_indicators(&bars, &[(bars[0].open_time_ms, 0.00002)], 0.35, 72);

        // Fix 1: BTC < EMA336 blocks A_Long
        let btc_chop = BtcRegime {
            bar_ts: bars[last_idx].open_time_ms,
            price: 95_000.0,
            ema168: 94_000.0,
            ema336: 98_000.0,
            bull_ok: false,
            short_ok: false,
            funding_rate: 0.0,
            label: String::new(),
        };
        let (sig_blk, tag_blk) = detect_sleeve_a("ETHUSDT", &bars, &ind, last_idx, &btc_chop, &cfg);
        assert!(sig_blk.is_none());
        assert_eq!(tag_blk, "a_btc_blocked");

        // BTC > EMA168 & EMA336 allows A_Long
        let btc_bull = BtcRegime { bull_ok: true, ..btc_chop };
        let (sig_ok, tag_ok) = detect_sleeve_a("ETHUSDT", &bars, &ind, last_idx, &btc_bull, &cfg);
        assert_eq!(tag_ok, "ok");
        let s = sig_ok.unwrap();
        assert_eq!(s.side, 1);
        assert!((s.tp_pct / s.sl_pct - 2.2).abs() < 1e-6);

        // Sleeve B ShortRip: downtrend (< EMA336) + 72h +10% rip + RSI2=100
        let mut b_closes: Vec<f64> = (0..300).map(|i| 200.0 - 0.25 * (i as f64)).collect();
        let rip_base = *b_closes.last().unwrap();
        for k in 1..=72 {
            b_closes.push(rip_base * (1.0 + 0.10 * (k as f64) / 72.0));
        }
        let b_bars = make_bars(&b_closes, 0.005, 2_000_000.0, 1_700_000_000_000);
        let b_ind = compute_indicators(&b_bars, &[(b_bars[0].open_time_ms, 0.0001)], 0.35, 72);
        let (sig_b, tag_b) = detect_sleeve_b("SOLUSDT", &b_bars, &b_ind, b_bars.len() - 1, &cfg);
        assert_eq!(tag_b, "ok");
        let sb = sig_b.unwrap();
        assert_eq!(sb.side, -1);
        assert!((sb.tp_pct / sb.sl_pct - 1.5).abs() < 1e-6);
    }

    #[test]
    fn shadow_lifecycle_fill_tp_cooldown_and_daily_breaker() {
        let cfg = LimitOcoConfig {
            running: true,
            capital: 10_000.0,
            risk_tier: "1.5x".into(),
            risk_per_trade: 0.012,
            max_pos_lev: 2.2,
            max_total_gross_lev: 6.0,
            cooldown_hours: 72,
            daily_loss_limit_r: 2.0,
            ..Default::default()
        };
        let t0 = 1_735_689_600_000i64; // 2025-01-01 00:00 UTC
        let mut st = LimitOcoState::new_with_config(cfg, t0);

        let btc_bars = make_bars(&[100_000.0; 365], 0.002, 50_000_000.0, t0);
        let mut eth_bars = make_bars(&[100.0; 365], 0.004, 5_000_000.0, t0);

        st.pending.insert(
            "ETHUSDT".into(),
            PendingOrder {
                sleeve: "A".into(),
                symbol: "ETHUSDT".into(),
                side: 1,
                limit_px: 99.0,
                mark_px: 100.0,
                dist_to_fill_pct: 1.0,
                sl_px: 95.0,
                tp_px: 107.8,
                sl_pct: 4.04,
                tp_pct: 8.89,
                expire_hours: 12,
                max_hold_bars: 168,
                age_hours: 0,
                score: 2.0,
                funding_bp: 0.0,
                created_ts: btc_bars[350].open_time_ms,
                reason: "test".into(),
            },
        );

        let b351_ts = eth_bars[351].open_time_ms;
        eth_bars[351] = Bar1h {
            open_time_ms: b351_ts,
            open: 100.0,
            high: 101.0,
            low: 98.5,
            close: 100.2,
            volume: 50_000.0,
            quote_volume: 5_000_000.0,
        };
        let mut bm = HashMap::new();
        bm.insert("BTCUSDT".into(), btc_bars.clone());
        bm.insert("ETHUSDT".into(), eth_bars.clone());
        let mut ac = HashMap::new();
        ac.insert("BTCUSDT".into(), "crypto".into());
        ac.insert("ETHUSDT".into(), "crypto".into());

        st.process_closed_bar(b351_ts, &bm, &HashMap::new(), &ac, None);
        assert!(st.positions.contains_key("ETHUSDT"));
        assert!((st.positions["ETHUSDT"].risk_usd - 120.0).abs() < 0.05);

        let b352_ts = eth_bars[352].open_time_ms;
        eth_bars[352] = Bar1h {
            open_time_ms: b352_ts,
            open: 100.2,
            high: 108.5,
            low: 100.0,
            close: 108.0,
            volume: 50_000.0,
            quote_volume: 5_000_000.0,
        };
        bm.insert("ETHUSDT".into(), eth_bars);
        st.process_closed_bar(b352_ts, &bm, &HashMap::new(), &ac, None);
        assert!(!st.positions.contains_key("ETHUSDT"));
        assert_eq!(st.trades.len(), 1);
        assert_eq!(st.trades[0].reason, "TP");
        assert!(st.trades[0].r_mult > 2.1);
        assert_eq!(st.cooldowns["A:ETHUSDT"], b352_ts + HOUR_MS + 72 * HOUR_MS);

        // Trigger daily breaker via simulated SLs on same UTC day
        let dummy = OpenPosition {
            sleeve: "B".into(),
            symbol: "SOLUSDT".into(),
            asset_class: "crypto".into(),
            side: -1,
            entry_ts: b352_ts,
            last_fund_ts: b352_ts,
            entry_px: 100.0,
            mark_px: 100.0,
            qty: 20.0,
            notional: 2000.0,
            risk_usd: 100.0,
            sl_px: 105.0,
            tp_px: 92.5,
            sl_pct: 5.0,
            tp_pct: 7.5,
            max_hold_bars: 96,
            max_hold_hours: 96,
            hold_bars: 1,
            hold_hours: 1,
            fee_usd: 0.4,
            funding_usd: 0.0,
            unreal_pnl: 0.0,
            unreal_r: 0.0,
            progress_pct: 0.0,
            reason: "test".into(),
        };
        for k in 1..=5 {
            st.record_closed_trade(b352_ts + k * HOUR_MS, &dummy, 105.0, 2.1, -100.0, "SL");
        }
        assert!(st.daily_breaker_active);
        st.roll_daily_breaker(b352_ts + 2 * DAY_MS);
        assert!(!st.daily_breaker_active);
    }

    #[test]
    fn gtx_passivity_rejects_impossible_post_only_orders() {
        let mut st = LimitOcoState::new_with_config(LimitOcoConfig::default(), 1_735_689_600_000);
        // Short signal @ 0.045752 when live market is already 0.046100 -> MUST be rejected (GTX Sell must be > mark!)
        let short_sig = SignalOrder {
            sleeve: "B".into(),
            symbol: "SKYUSDT".into(),
            side: -1,
            limit_px: 0.045752,
            sl_px: 0.048928,
            tp_px: 0.040988,
            expire_bars: 12,
            max_hold_bars: 96,
            score: 0.126,
            sl_pct: 0.069,
            tp_pct: 0.104,
            signal_close: 0.04554,
            atr24: 0.00106,
            funding_rate: 0.00005,
            reason: "test".into(),
        };
        assert!(!st.place_pending(short_sig.clone(), 1_735_689_600_000, Some(0.0461)));
        assert!(st.pending.is_empty());
        assert_eq!(st.events.last().unwrap().kind, "GTX_REJECT");

        // Same Short signal @ 0.045752 when live market is 0.04550 (below ask) -> ACCEPTED!
        assert!(st.place_pending(short_sig, 1_735_689_600_000, Some(0.04550)));
        assert!(st.pending.contains_key("SKYUSDT"));
        assert!(st.pending["SKYUSDT"].dist_to_fill_pct > 0.0);

        // Long signal @ 100.0 when live market is already 99.5 -> MUST be rejected (GTX Buy must be < mark!)
        let long_sig = SignalOrder {
            sleeve: "A".into(),
            symbol: "ETHUSDT".into(),
            side: 1,
            limit_px: 100.0,
            sl_px: 95.0,
            tp_px: 111.0,
            expire_bars: 12,
            max_hold_bars: 168,
            score: 2.0,
            sl_pct: 0.05,
            tp_pct: 0.11,
            signal_close: 100.5,
            atr24: 1.4,
            funding_rate: 0.0,
            reason: "test".into(),
        };
        assert!(!st.place_pending(long_sig, 1_735_689_600_000, Some(99.5)));
        assert!(!st.pending.contains_key("ETHUSDT"));
    }

    #[test]
    fn intrabar_marks_fill_resting_gtx_and_close_oco_immediately() {
        let mut st = LimitOcoState::new_with_config(LimitOcoConfig::default(), 1_735_689_600_000);
        let sig = SignalOrder {
            sleeve: "B".into(),
            symbol: "SKYUSDT".into(),
            side: -1,
            limit_px: 0.045752,
            sl_px: 0.048928,
            tp_px: 0.040988,
            expire_bars: 12,
            max_hold_bars: 96,
            score: 0.126,
            sl_pct: 0.0694,
            tp_pct: 0.1041,
            signal_close: 0.04554,
            atr24: 0.00106,
            funding_rate: 0.00005,
            reason: "test".into(),
        };
        // Legitimately placed at 01:00 while mark was 0.04554 (below sell limit)
        assert!(st.place_pending(sig, 1_735_689_600_000, Some(0.04554)));
        assert!(st.pending.contains_key("SKYUSDT"));

        // 15 minutes later mark rises to 0.0461 -> fills immediately into active short position!
        let mut marks = HashMap::new();
        marks.insert("SKYUSDT".into(), 0.0461);
        let mut ac = HashMap::new();
        ac.insert("SKYUSDT".into(), "crypto".into());
        st.process_intrabar_marks(&marks, &ac, 1_735_689_600_000 + 15 * 60_000);
        assert!(!st.pending.contains_key("SKYUSDT"));
        assert!(st.positions.contains_key("SKYUSDT"));
        assert!((st.positions["SKYUSDT"].entry_px - 0.045752).abs() < 1e-9);

        // Later mark drops to 0.04090 (< tp_px * 0.9998) -> TP triggers immediately!
        marks.insert("SKYUSDT".into(), 0.04090);
        st.process_intrabar_marks(&marks, &ac, 1_735_689_600_000 + 45 * 60_000);
        assert!(!st.positions.contains_key("SKYUSDT"));
        assert_eq!(st.trades.len(), 1);
        assert_eq!(st.trades[0].reason, "TP");
        assert!(st.trades[0].r_mult > 1.4);
    }
}
