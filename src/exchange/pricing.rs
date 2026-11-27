//! Hyperliquid tick-size pricing, rounding, order status parsing, and SDK retry helpers.

use anyhow::{anyhow, bail, Context, Result};
use hyperliquid_rust_sdk::{ExchangeDataStatus, ExchangeResponseStatus};
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, PartialEq)]
pub(super) enum OrderStatusCheck {
    UnknownOid,
    Filled,
    Open { oid: u64, rem: Option<f64> },
    TerminalUnfilled { rem: Option<f64> },
    Ambiguous(String),
}

pub(super) fn parse_order_status(st_resp: &Value, sz_decimals: i32) -> OrderStatusCheck {
    let order_status = st_resp["status"].as_str().unwrap_or("");
    if order_status == "unknownOid" {
        return OrderStatusCheck::UnknownOid;
    }
    if order_status != "order" {
        return OrderStatusCheck::Ambiguous(format!("status={order_status}"));
    }
    let inner_status = st_resp["order"]["status"].as_str().unwrap_or("");
    let rem = st_resp["order"]["order"]["sz"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .or_else(|| st_resp["order"]["order"]["sz"].as_f64())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map(|v| round_size(v, sz_decimals));
    match inner_status {
        "filled" => OrderStatusCheck::Filled,
        "open" => match st_resp["order"]["order"]["oid"].as_u64() {
            Some(oid) => OrderStatusCheck::Open { oid, rem },
            None => OrderStatusCheck::Ambiguous("open missing oid".into()),
        },
        "canceled" | "rejected" | "marginCanceled" | "reduceOnlyCanceled" | "minTradeNtlRejected" => {
            OrderStatusCheck::TerminalUnfilled { rem }
        }
        other => OrderStatusCheck::Ambiguous(format!("status={order_status}/{other}")),
    }
}

pub(super) fn parse_perp_equity(perp: &Value) -> Result<f64> {
    let raw = perp["marginSummary"]["accountValue"]
        .as_str()
        .context("clearinghouseState 缺少 marginSummary.accountValue")?;
    let equity: f64 = raw.parse().context("marginSummary.accountValue 不是有效数字")?;
    anyhow::ensure!(equity.is_finite() && equity >= 0.0, "账户净值无效: {equity}");
    Ok(equity)
}

/// Tick rounding: at most 5 significant figures and never finer than the
/// size decimals allow (same rule the exchange applies).
pub fn order_price(px: f64, sz_decimals: i32, round_up: bool) -> f64 {
    if !px.is_finite() || px <= 0.0 {
        return 0.0;
    }
    let decimal_places = (4 - px.log10().floor() as i32).min(6 - sz_decimals);
    let factor = 10_f64.powi(decimal_places);
    let scaled = px * factor;
    if round_up {
        scaled.ceil() / factor
    } else {
        scaled.floor() / factor
    }
}

/// Hyperliquid 单档最小价格步进（遵循 5 位有效数字与 `6 - sz_decimals` 小数位规则）。
pub fn price_tick(px: f64, sz_decimals: i32) -> f64 {
    if !px.is_finite() || px <= 0.0 {
        return 0.0;
    }
    let decimal_places = (4 - px.log10().floor() as i32).min(6 - sz_decimals);
    10_f64.powi(-decimal_places)
}

/// 构造严格被动（Post-Only / Alo）挂单价：
/// 至少相对 `mid` 退让 `max(1bp, 1 tick)`，并支持 `extra_ticks` 档额外退让。
/// 保证买单严格 `< mid`、卖单严格 `> mid`，同时满足 HL tick/sig-fig 规则。
pub fn passive_alo_price(mid: f64, sz_decimals: i32, buy: bool, extra_ticks: u32) -> f64 {
    if !mid.is_finite() || mid <= 0.0 {
        return 0.0;
    }
    let tick = price_tick(mid, sz_decimals);
    if !tick.is_finite() || tick <= 0.0 {
        return 0.0;
    }
    let base_offset = (mid * 0.0001).max(tick);
    if buy {
        let raw = mid - base_offset;
        if raw <= 0.0 {
            return 0.0;
        }
        let mut px = order_price(raw, sz_decimals, false);
        while px >= mid && px > 0.0 {
            let step = price_tick(px * (1.0 - 1e-9), sz_decimals).max(tick);
            let next = order_price((px - step * 0.5).max(0.0), sz_decimals, false);
            if next >= px {
                let next2 = order_price((px - step).max(0.0), sz_decimals, false);
                if next2 >= px {
                    break;
                }
                px = next2;
            } else {
                px = next;
            }
        }
        for _ in 0..extra_ticks {
            if px <= 0.0 {
                return 0.0;
            }
            let step = price_tick(px * (1.0 - 1e-9), sz_decimals);
            if step <= 0.0 {
                break;
            }
            let next = order_price((px - step * 0.5).max(0.0), sz_decimals, false);
            px = if next < px {
                next
            } else {
                order_price((px - step).max(0.0), sz_decimals, false)
            };
        }
        if px > 0.0 && px < mid {
            px
        } else {
            0.0
        }
    } else {
        let raw = mid + base_offset;
        let mut px = order_price(raw, sz_decimals, true);
        while px <= mid {
            let step = price_tick(px, sz_decimals).max(tick);
            let next = order_price(px + step * 0.5, sz_decimals, true);
            if next <= px {
                let next2 = order_price(px + step, sz_decimals, true);
                if next2 <= px {
                    break;
                }
                px = next2;
            } else {
                px = next;
            }
        }
        for _ in 0..extra_ticks {
            let step = price_tick(px, sz_decimals);
            if step <= 0.0 {
                break;
            }
            let next = order_price(px + step * 0.5, sz_decimals, true);
            px = if next > px {
                next
            } else {
                order_price(px + step, sz_decimals, true)
            };
        }
        if px > mid {
            px
        } else {
            0.0
        }
    }
}

/// Truncate a size to the coin's lot size.
pub fn round_size(size: f64, sz_decimals: i32) -> f64 {
    if !size.is_finite() || size <= 0.0 {
        return 0.0;
    }
    if sz_decimals < 0 {
        // Integer lot sizes must use an integer step, not its inexact reciprocal.
        // 593 / 0.00001 becomes 59299999.99999999 and violates PEPE lot sizing.
        let step = 10_f64.powi(-sz_decimals);
        return (size / step).floor() * step;
    }
    let factor = 10_f64.powi(sz_decimals);
    (size * factor).floor() / factor
}

/// SDK 调用重试：Hyperliquid 在配额紧张时返回 429，SDK 会把原始错误抛出来。
/// 下单和设杠杆都必须重试，否则用户看到的就是一句 429。
pub(super) async fn sdk_retry<T, F, Fut>(label: &str, mut f: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, hyperliquid_rust_sdk::Error>>,
{
    let mut last = String::new();
    for attempt in 0..6u32 {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                let msg = format!("{e}");
                let limited = msg.contains("429") || msg.contains("Too Many");
                last = msg;
                if !limited {
                    return Err(anyhow!(last));
                }
                let wait = 700u64 * 2u64.pow(attempt.min(4));
                tokio::time::sleep(Duration::from_millis(wait)).await;
            }
        }
    }
    Err(anyhow!("{label} 连续被限流（429），请稍后重试：{last}"))
}

pub(super) fn one_status(value: ExchangeResponseStatus) -> Result<ExchangeDataStatus> {
    let ExchangeResponseStatus::Ok(response) = value else {
        bail!("exchange action rejected: {value:?}");
    };
    let statuses = response
        .data
        .context("exchange response without data")?
        .statuses;
    anyhow::ensure!(
        statuses.len() == 1,
        "unexpected exchange statuses: {statuses:?}"
    );
    let status = statuses.into_iter().next().unwrap();
    if let ExchangeDataStatus::Error(s) = &status {
        bail!("exchange order error: {s}");
    }
    Ok(status)
}

pub fn describe(status: &ExchangeDataStatus) -> String {
    match status {
        ExchangeDataStatus::Filled(f) => format!("成交 {}@{}", f.total_sz, f.avg_px),
        ExchangeDataStatus::Resting(_) => "挂单未成交".into(),
        ExchangeDataStatus::WaitingForTrigger => "等待触发".into(),
        ExchangeDataStatus::Error(e) => format!("错误 {e}"),
        other => format!("{other:?}"),
    }
}

#[allow(dead_code)]
pub fn err(msg: &str) -> anyhow::Error {
    anyhow!(msg.to_string())
}

