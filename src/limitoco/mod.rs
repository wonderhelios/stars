//! 币安 USD-M 永续 A+B 双引擎纯限价 OCO 影子引擎（`V2_UltraWR · Dyn_Top80 + Fix 1+2`，默认 1.5x 档位）。
//!
//! 纯 Rust 实现，零外部运行依赖，只读币安公开行情接口（绝不发真实下单请求）：
//!   - `/fapi/v1/exchangeInfo`（缓存 6h，自动区分加密/大宗商品并剔除美股永续与稳定币）
//!   - `/fapi/v1/ticker/24hr`（快速筛选流动性候选池）
//!   - `/fapi/v1/klines?interval=1h`（增量本地缓存已收盘 1h K 线，零未来函数）
//!   - `/fapi/v1/premiumIndex`（实时标记价与资金费率，每 60s 盯市）
//!   - `/fapi/v1/fundingRate`（严格 `< bar_close` 已结算资金费率 + 持仓真实资金费现金流）
//!
//! 策略核心参数（与 35 个月跨牛熊回测 & 2024 盲测牛市 100% 对齐，高胜率 52.4% / 中位持仓 12h / 3 年最大连亏仅 8 笔）：
//!   - 标的池：加密货币 + 大宗商品（金银油气铜），排除美股永续；每整点动态取 7 日日均成交额 Top-80（>= $15M），排除 72h 内单小时振幅 >= 35% 标的
//!   - 策略 A（7d 趋势突破，最多 3 仓 / 同向最多 2 仓 / 冷却 72h）：
//!     - 168h 通道新突破 + 放量 >= 1.3x 24h 均量
//!     - 做多：个币 & BTC 均 > EMA168 且 > EMA336，且资金费 <= +0.5bp
//!     - 做空：个币 & BTC 均 < EMA168，且资金费 >= 0
//!     - 纯 Maker GTX 限价 `close ∓ 0.15*ATR24`（12h 过期）；SL `2.8*ATR24`（0.5%~14%），TP `1.2R`，最长持仓 `168h`
//!   - 策略 B（空头反弹急拉做空，最多 2 仓 / 仅做空 / 冷却 72h）：
//!     - `close < EMA336` + 72h 涨幅 >= +6% + 因果日线采样 `RSI(2) >= 90` + 资金费 >= -2bp
//!     - 纯 Maker GTX 挂卖 `close + 0.20*ATR24`（12h 过期）；SL `2.7*ATR24`（0.5%~14%），TP `0.9R`，最长持仓 `96h`
//!   - 全局风控：总计最多 5 仓、默认 1.5x 档位（单笔 1.2%R 复利，单仓上限 2.2x，总杠杆上限 6.0x）、日内 UTC 已实现亏损 `-3R` 自动熔断。

use std::time::{SystemTime, UNIX_EPOCH};

pub const HOUR_MS: i64 = 3_600_000;
pub const DAY_MS: i64 = 86_400_000;
pub const STRATEGY_ID: &str = "binance_limit_oco_ab_v2_ultrawr";
pub const ENGINE_VERSION: &str = "v2.0-rust-ultrawr-dyn80-sl14";
pub(crate) const FAPI_BASE_DEFAULT: &str = "https://fapi.binance.com";
pub(crate) const MAX_BARS_CACHE: usize = 440;
pub(crate) const COLD_FETCH_LIMIT: usize = 400;

pub(crate) const COMMODITY_SYMBOLS: &[&str] = &[
    "XAUUSDT", "XAGUSDT", "CLUSDT", "BZUSDT", "NATGASUSDT", "COPPERUSDT", "PAXGUSDT",
    "XPTUSDT", "XPDUSDT",
];

pub(crate) const STABLE_OR_FX_EXCLUDED: &[&str] = &[
    "USDCUSDT", "BUSDUSDT", "USDPUSDT", "TUSDUSDT", "FDUSDUSDT", "EURUSDT",
    "DAIUSDT", "USD1USDT", "USDEUSDT", "GBPUSDT", "AUDUSDT", "AEURUSDT",
];

pub(crate) const US_STOCK_SYMBOLS: &[&str] = &[
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


mod engine;
mod feed;
mod signals;
mod types;

pub use feed::step_once;
pub use signals::{classify_binance_contract, compute_indicators};
#[cfg(test)]
use signals::{detect_sleeve_a, detect_sleeve_b};
pub use types::{
    tier_presets, Bar1h, BtcRegime, ClosedTrade, LimitOcoConfig, LimitOcoState, OpenPosition,
    PendingOrder, SignalOrder, SleeveStat, UniversePreviewRow,
};

#[cfg(test)]
mod tests;
