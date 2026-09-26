use rust_decimal::Decimal;
use std::str::FromStr;
use std::time::Duration;
use tracing::error;

use crate::okx::{
    rest::{Candle1H, FundingHistoryItem},
    RestClient,
};

/// 极端费率阈值：0.05%
const THRESHOLD_STR: &str = "0.0005";
/// 观察窗口：事件后 N 小时
const HORIZONS_HOURS: [i64; 4] = [1, 4, 8, 24];

pub async fn run(inst_ids: &[String]) -> anyhow::Result<()> {
    if inst_ids.is_empty() {
        eprintln!("用法: cargo run --release -- research <instId1> [instId2 ...]");
        eprintln!("示例: cargo run --release -- research ONE-USDT-SWAP 2Z-USDT-SWAP");
        std::process::exit(1);
    }

    let threshold = Decimal::from_str(THRESHOLD_STR)?;
    let client = RestClient::new();

    let mut all_events: Vec<EventOutcome> = Vec::new();

    for inst_id in inst_ids {
        match analyze_one(&client, inst_id, threshold).await {
            Ok(events) => all_events.extend(events),
            Err(e) => error!("{}: {}", inst_id, e),
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    print_summary(&all_events);
    Ok(())
}

#[derive(Debug, Clone)]
struct EventOutcome {
    #[allow(dead_code)]
    inst_id: String,
    #[allow(dead_code)]
    event_ts: i64,
    funding_rate: Decimal,
    returns: Vec<(i64, Option<Decimal>)>,
}

async fn analyze_one(
    client: &RestClient,
    inst_id: &str,
    threshold: Decimal,
) -> anyhow::Result<Vec<EventOutcome>> {
    println!("\n========== {} ==========", inst_id);

    let funding = client.funding_rate_history(inst_id, 100).await?;
    println!("  历史费率条数: {}", funding.len());

    let candles = client.candles_1h(inst_id, 300).await?;
    println!("  历史 K线(1H): {}", candles.len());
    if candles.is_empty() {
        return Ok(vec![]);
    }

    let earliest_ts = candles.first().map(|c| c.open_time).unwrap_or(0);

    let events: Vec<&FundingHistoryItem> = funding
        .iter()
        .filter(|f| f.funding_rate.abs() > threshold)
        .filter(|f| {
            f.funding_time
                .parse::<i64>()
                .map(|ts| ts >= earliest_ts)
                .unwrap_or(false)
        })
        .collect();

    println!("  极端事件数（K线覆盖内）: {}", events.len());
    if events.is_empty() {
        return Ok(vec![]);
    }

    let mut outcomes = Vec::new();

    for ev in events {
        let ts: i64 = match ev.funding_time.parse() {
            Ok(t) => t,
            Err(_) => continue,
        };

        let Some(base) = find_base_candle(&candles, ts) else {
            continue;
        };
        let base_close = base.close;

        let mut returns = Vec::new();
        for h in HORIZONS_HOURS {
            let target_ts = ts + h * 3_600_000;
            let ret = find_candle_at(&candles, target_ts)
                .map(|c| (c.close - base_close) / base_close * Decimal::from(100));
            returns.push((h, ret));
        }

        print!(
            "  {} funding={:+.4}%  ",
            format_ts_cn(ts),
            ev.funding_rate * Decimal::from(100)
        );
        for (h, r) in &returns {
            match r {
                Some(x) => print!("T+{}h {:+.2}%  ", h, x),
                None => print!("T+{}h N/A  ", h),
            }
        }
        println!();

        outcomes.push(EventOutcome {
            inst_id: inst_id.to_string(),
            event_ts: ts,
            funding_rate: ev.funding_rate,
            returns,
        });
    }

    Ok(outcomes)
}

fn find_base_candle(candles: &[Candle1H], ts: i64) -> Option<&Candle1H> {
    candles
        .iter()
        .filter(|c| c.open_time <= ts)
        .max_by_key(|c| c.open_time)
}

fn find_candle_at(candles: &[Candle1H], ts: i64) -> Option<&Candle1H> {
    candles
        .iter()
        .filter(|c| c.open_time >= ts)
        .min_by_key(|c| c.open_time)
}

fn print_summary(events: &[EventOutcome]) {
    println!("\n\n========== 汇总统计 ==========");
    if events.is_empty() {
        println!("  无事件");
        return;
    }

    let pos: Vec<_> = events
        .iter()
        .filter(|e| e.funding_rate > Decimal::ZERO)
        .collect();
    let neg: Vec<_> = events
        .iter()
        .filter(|e| e.funding_rate < Decimal::ZERO)
        .collect();

    println!("\n  正极端费率事件 (funding > +0.05%): {} 个", pos.len());
    print_stats(&pos);

    println!("\n  负极端费率事件 (funding < -0.05%): {} 个", neg.len());
    print_stats(&neg);

    println!("\n  收益定义: (T+h 收盘价 - 事件时收盘价) / 事件时收盘价");
    println!("  注意: 样本仅为'当前极端费率'的币，存在选择偏差。");
}

fn print_stats(events: &[&EventOutcome]) {
    if events.is_empty() {
        println!("    (无)");
        return;
    }

    for (idx, h) in HORIZONS_HOURS.iter().enumerate() {
        let values: Vec<Decimal> = events
            .iter()
            .filter_map(|e| e.returns.get(idx).and_then(|(_, r)| *r))
            .collect();

        if values.is_empty() {
            println!("    T+{:>2}h: N/A", h);
            continue;
        }

        let n = values.len();
        let sum: Decimal = values.iter().copied().sum();
        let avg = sum / Decimal::from(n as i64);

        let mut sorted = values.clone();
        sorted.sort();
        let median = sorted[n / 2];

        let win = values.iter().filter(|v| **v > Decimal::ZERO).count();
        let win_rate = Decimal::from(win as i64) / Decimal::from(n as i64) * Decimal::from(100);

        println!(
            "    T+{:>2}h: 样本 {:>3}  平均 {:+.2}%  中位数 {:+.2}%  胜率 {:.1}%",
            h, n, avg, median, win_rate
        );
    }
}

fn format_ts_cn(ms: i64) -> String {
    let secs = ms / 1000 + 8 * 3600;
    let days = secs / 86400;
    let rem = secs % 86400;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let (y, mo, d) = days_to_ymd(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02}", y, mo, d, h, m)
}

fn days_to_ymd(days: i64) -> (i64, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}
