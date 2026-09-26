use rust_decimal::Decimal;
use std::str::FromStr;
use std::time::Duration;
use tracing::error;

use crate::okx::{
    rest::{Candle1H, FundingHistoryItem},
    RestClient,
};

const THRESHOLD_STR: &str = "0.0005";
const HORIZONS_HOURS: [i64; 4] = [1, 4, 8, 24];
const CLUSTER_GAP_HOURS: i64 = 6;

pub async fn run(inst_ids: &[String]) -> anyhow::Result<()> {
    if inst_ids.is_empty() {
        eprintln!("用法: cargo run --release -- research <instId1> [instId2 ...]");
        std::process::exit(1);
    }

    let threshold = Decimal::from_str(THRESHOLD_STR)?;
    let client = RestClient::new();

    let mut all_events: Vec<EventOutcome> = Vec::new();
    let mut benchmark_samples: Vec<BenchmarkSample> = Vec::new();

    for inst_id in inst_ids {
        match analyze_one(&client, inst_id, threshold).await {
            Ok((events, bench)) => {
                all_events.extend(events);
                if let Some(b) = bench {
                    benchmark_samples.push(b);
                }
            }
            Err(e) => error!("{}: {}", inst_id, e),
        }
        tokio::time::sleep(Duration::from_millis(80)).await;
    }

    print_summary("汇总统计（处理组）", &all_events);
    print_by_prior_momentum("按事件前 24h 动量分层（处理组）", &all_events);
    print_alpha_vs_inst(&all_events, &benchmark_samples);

    Ok(())
}

pub async fn run_scan(top_n: usize) -> anyhow::Result<()> {
    let threshold = Decimal::from_str(THRESHOLD_STR)?;
    let client = RestClient::new();

    println!("拉取全市场 ticker（按 24h 成交额排序取 Top {}）...", top_n);
    let inst_ids = client.top_swap_by_volume(top_n).await?;
    println!("待扫描币种: {}\n", inst_ids.len());

    let mut all_events: Vec<EventOutcome> = Vec::new();
    let mut benchmark_samples: Vec<BenchmarkSample> = Vec::new();
    let mut ok = 0usize;
    let mut fail = 0usize;

    for (i, inst_id) in inst_ids.iter().enumerate() {
        match analyze_one(&client, inst_id, threshold).await {
            Ok((events, bench)) => {
                all_events.extend(events);
                if let Some(b) = bench {
                    benchmark_samples.push(b);
                }
                ok += 1;
            }
            Err(e) => {
                fail += 1;
                eprintln!("  [{}] {}: {}", i + 1, inst_id, e);
            }
        }
        if (i + 1) % 25 == 0 {
            println!(
                "  进度 {}/{}  成功 {}  失败 {}  事件 {}",
                i + 1,
                inst_ids.len(),
                ok,
                fail,
                all_events.len()
            );
        }
        tokio::time::sleep(Duration::from_millis(80)).await;
    }

    println!(
        "\n扫描完成: 成功 {} 失败 {} 事件 {}",
        ok,
        fail,
        all_events.len()
    );

    print_summary("汇总统计（处理组）", &all_events);
    print_by_prior_momentum("按事件前 24h 动量分层（处理组）", &all_events);
    print_alpha_vs_inst(&all_events, &benchmark_samples);

    Ok(())
}

#[derive(Debug, Clone)]
struct EventOutcome {
    #[allow(dead_code)]
    inst_id: String,
    #[allow(dead_code)]
    event_ts: i64,
    funding_rate: Decimal,
    prior_24h_return: Option<Decimal>,
    /// (horizon, 事件后 h 小时收益%)
    returns: Vec<(i64, Option<Decimal>)>,
    /// (horizon, 该币全时段平均 h 小时收益%)，用于算 alpha
    benchmark_mu: Vec<(i64, Option<Decimal>)>,
}

#[derive(Debug, Clone)]
struct BenchmarkSample {
    #[allow(dead_code)]
    inst_id: String,
    /// 该币全时段平均 h 小时收益
    mu: Vec<(i64, Decimal)>,
    /// 该币全时段 h 小时收益标准差
    sigma: Vec<(i64, Decimal)>,
}

async fn analyze_one(
    client: &RestClient,
    inst_id: &str,
    threshold: Decimal,
) -> anyhow::Result<(Vec<EventOutcome>, Option<BenchmarkSample>)> {
    let funding = client.funding_rate_history(inst_id, 100).await?;
    let candles = client.candles_1h(inst_id, 300).await?;
    if candles.len() < 30 {
        return Ok((vec![], None));
    }

    let earliest_ts = candles.first().map(|c| c.open_time).unwrap_or(0);

    // 币内基准：全时段平均 h 小时收益
    let benchmark = compute_benchmark(inst_id, &candles);

    let raw_events: Vec<&FundingHistoryItem> = funding
        .iter()
        .filter(|f| f.funding_rate.abs() > threshold)
        .filter(|f| {
            f.funding_time
                .parse::<i64>()
                .map(|ts| ts >= earliest_ts)
                .unwrap_or(false)
        })
        .collect();
    let raw_count = raw_events.len();
    let events = dedup_events(raw_events, CLUSTER_GAP_HOURS);

    if !events.is_empty() {
        println!("[{}] 原始 {} 去重 {}", inst_id, raw_count, events.len());
    }

    let mut outcomes = Vec::new();

    for ev in &events {
        let ts: i64 = match ev.funding_time.parse() {
            Ok(t) => t,
            Err(_) => continue,
        };

        let Some(base) = find_base_candle(&candles, ts) else {
            continue;
        };
        let base_close = base.close;

        let prior_ts = ts - 24 * 3_600_000;
        let prior_ret = find_candle_at(&candles, prior_ts)
            .map(|c| (base_close - c.close) / c.close * Decimal::from(100));

        let mut returns = Vec::new();
        for h in HORIZONS_HOURS {
            let target_ts = ts + h * 3_600_000;
            let ret = find_candle_at(&candles, target_ts)
                .map(|c| (c.close - base_close) / base_close * Decimal::from(100));
            returns.push((h, ret));
        }

        outcomes.push(EventOutcome {
            inst_id: inst_id.to_string(),
            event_ts: ts,
            funding_rate: ev.funding_rate,
            prior_24h_return: prior_ret,
            returns,
            benchmark_mu: benchmark.mu,
        });
    }

    Ok((outcomes, Some(benchmark)))
}

/// 计算该币全时段重叠窗口的 h 小时收益均值与标准差
fn compute_benchmark(inst_id: &str, candles: &[Candle1H]) -> BenchmarkSample {
    let mut mu = Vec::new();
    let mut sigma = Vec::new();

    for &h in &HORIZONS_HOURS {
        let h = h as usize;
        let mut values: Vec<Decimal> = Vec::new();
        for i in 0..candles.len().saturating_sub(h) {
            let start = candles[i].close;
            let end = candles[i + h].close;
            if start.is_zero() {
                continue;
            }
            values.push((end - start) / start * Decimal::from(100));
        }

        if values.is_empty() {
            mu.push((h as i64, Decimal::ZERO));
            sigma.push((h as i64, Decimal::ZERO));
            continue;
        }

        let n = Decimal::from(values.len() as i64);
        let mean: Decimal = values.iter().copied().sum::<Decimal>() / n;

        let variance: Decimal = values
            .iter()
            .map(|v| {
                let d = *v - mean;
                d * d
            })
            .sum::<Decimal>()
            / n;

        let std = decimal_sqrt(variance);

        mu.push((h as i64, mean));
        sigma.push((h as i64, std));
    }

    BenchmarkSample {
        inst_id: inst_id.to_string(),
        mu,
        sigma,
    }
}

/// Decimal 开平方（牛顿迭代，够用即可）
fn decimal_sqrt(x: Decimal) -> Decimal {
    if x <= Decimal::ZERO {
        return Decimal::ZERO;
    }
    let mut guess = x / Decimal::from(2);
    if guess.is_zero() {
        guess = Decimal::ONE;
    }
    for _ in 0..20 {
        let next = (guess + x / guess) / Decimal::from(2);
        if next == guess {
            break;
        }
        guess = next;
    }
    guess
}

fn dedup_events<'a>(
    mut events: Vec<&'a FundingHistoryItem>,
    gap_hours: i64,
) -> Vec<&'a FundingHistoryItem> {
    if events.is_empty() {
        return events;
    }

    events.sort_by_key(|e| e.funding_time.parse::<i64>().unwrap_or(0));

    let gap_ms = gap_hours * 3_600_000;
    let mut result: Vec<&FundingHistoryItem> = Vec::new();
    let mut current_cluster: Vec<&FundingHistoryItem> = Vec::new();
    let mut last_ts: i64 = i64::MIN;

    for e in events {
        let ts = e.funding_time.parse::<i64>().unwrap_or(0);
        if ts - last_ts > gap_ms && !current_cluster.is_empty() {
            result.push(pick_representative(&current_cluster));
            current_cluster.clear();
        }
        current_cluster.push(e);
        last_ts = ts;
    }
    if !current_cluster.is_empty() {
        result.push(pick_representative(&current_cluster));
    }

    result
}

fn pick_representative<'a>(cluster: &[&'a FundingHistoryItem]) -> &'a FundingHistoryItem {
    cluster
        .iter()
        .max_by(|a, b| a.funding_rate.abs().cmp(&b.funding_rate.abs()))
        .copied()
        .expect("non-empty cluster")
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

fn print_summary(title: &str, events: &[EventOutcome]) {
    println!("\n\n========== {} ==========", title);
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

    println!("\n  正费率: {} 个", pos.len());
    print_stats(&pos);

    println!("\n  负费率: {} 个", neg.len());
    print_stats(&neg);

    println!("\n  总数: {}", events.len());
    println!("  涉及币种数: {}", {
        let mut ids: Vec<&str> = events.iter().map(|e| e.inst_id.as_str()).collect();
        ids.sort();
        ids.dedup();
        ids.len()
    });
}

fn print_by_prior_momentum(title: &str, events: &[EventOutcome]) {
    println!("\n\n========== {} ==========", title);
    if events.is_empty() {
        return;
    }

    let buckets: Vec<(&str, Option<Decimal>, Option<Decimal>)> = vec![
        ("暴跌  (<-10%)", None, Some(Decimal::from(-10))),
        (
            "下跌  (-10~-3%)",
            Some(Decimal::from(-10)),
            Some(Decimal::from(-3)),
        ),
        (
            "横盘  (-3~+3%)",
            Some(Decimal::from(-3)),
            Some(Decimal::from(3)),
        ),
        (
            "上涨  (+3~+10%)",
            Some(Decimal::from(3)),
            Some(Decimal::from(10)),
        ),
        ("暴涨  (>+10%)", Some(Decimal::from(10)), None),
    ];

    for (label, lo, hi) in &buckets {
        let subset: Vec<&EventOutcome> = events
            .iter()
            .filter(|e| {
                let Some(r) = e.prior_24h_return else {
                    return false;
                };
                if let Some(lo) = lo {
                    if r < *lo {
                        return false;
                    }
                }
                if let Some(hi) = hi {
                    if r >= *hi {
                        return false;
                    }
                }
                true
            })
            .collect();

        if subset.is_empty() {
            continue;
        }

        let pos: Vec<&EventOutcome> = subset
            .iter()
            .filter(|e| e.funding_rate > Decimal::ZERO)
            .copied()
            .collect();
        let neg: Vec<&EventOutcome> = subset
            .iter()
            .filter(|e| e.funding_rate < Decimal::ZERO)
            .copied()
            .collect();

        println!(
            "\n  【{}】共 {} 个（正 {} / 负 {}）",
            label,
            subset.len(),
            pos.len(),
            neg.len()
        );

        if !pos.is_empty() {
            println!("    -- 正费率 --");
            print_stats(&pos);
        }
        if !neg.is_empty() {
            println!("    -- 负费率 --");
            print_stats(&neg);
        }
    }
}

/// 对比 alpha：事件收益 - 该币全时段平均收益
fn print_alpha_vs_inst(events: &[EventOutcome], benchmarks: &[BenchmarkSample]) {
    println!("\n\n========== Alpha 对比（事件收益 - 该币全时段平均） ==========");
    println!("  对每个事件，用其所属币的全时段 h 小时收益作为基准，差值即 alpha。");
    println!("  alpha < 0 说明事件后收益低于该币常态；alpha > 0 反之。\n");

    if events.is_empty() {
        println!("  无事件");
        return;
    }

    // 全局平均基准（用于参考）
    let mut global_mu: Vec<(i64, Decimal, usize)> = HORIZONS_HOURS
        .iter()
        .map(|h| (*h, Decimal::ZERO, 0))
        .collect();
    for b in benchmarks {
        for (idx, (h, v)) in b.mu.iter().enumerate() {
            if *h == HORIZONS_HOURS[idx] {
                global_mu[idx].1 += *v;
                global_mu[idx].2 += 1;
            }
        }
    }
    println!("  全部扫描币的全时段平均收益（作为整体市场参考）：");
    for (h, sum, n) in &global_mu {
        if *n > 0 {
            let avg = *sum / Decimal::from(*n as i64);
            println!("      T+{:>2}h: {:+.3}%", h, avg);
        }
    }

    let buckets: Vec<(&str, Option<Decimal>, Option<Decimal>)> = vec![
        ("暴跌  (<-10%)", None, Some(Decimal::from(-10))),
        (
            "下跌  (-10~-3%)",
            Some(Decimal::from(-10)),
            Some(Decimal::from(-3)),
        ),
        (
            "横盘  (-3~+3%)",
            Some(Decimal::from(-3)),
            Some(Decimal::from(3)),
        ),
        (
            "上涨  (+3~+10%)",
            Some(Decimal::from(3)),
            Some(Decimal::from(10)),
        ),
        ("暴涨  (>+10%)", Some(Decimal::from(10)), None),
    ];

    for (label, lo, hi) in &buckets {
        for (dir_label, sign) in [("正费率", 1i32), ("负费率", -1i32)] {
            let subset: Vec<&EventOutcome> = events
                .iter()
                .filter(|e| {
                    let Some(r) = e.prior_24h_return else {
                        return false;
                    };
                    if let Some(lo) = lo {
                        if r < *lo {
                            return false;
                        }
                    }
                    if let Some(hi) = hi {
                        if r >= *hi {
                            return false;
                        }
                    }
                    if sign > 0 {
                        e.funding_rate > Decimal::ZERO
                    } else {
                        e.funding_rate < Decimal::ZERO
                    }
                })
                .collect();

            if subset.len() < 5 {
                continue;
            }

            println!("\n  【{} · {}】n={}", label, dir_label, subset.len());

            for (idx, h) in HORIZONS_HOURS.iter().enumerate() {
                // 事件收益
                let rets: Vec<Decimal> = subset
                    .iter()
                    .filter_map(|e| e.returns.get(idx).and_then(|(_, r)| *r))
                    .collect();
                if rets.is_empty() {
                    continue;
                }
                let n_ret = rets.len();
                let avg_ret = rets.iter().copied().sum::<Decimal>() / Decimal::from(n_ret as i64);

                // 对应基准
                let mus: Vec<Decimal> = subset
                    .iter()
                    .filter_map(|e| {
                        e.benchmark_mu
                            .iter()
                            .find(|(hh, _)| hh == h)
                            .and_then(|(_, v)| *v)
                    })
                    .collect();
                if mus.is_empty() {
                    continue;
                }
                let avg_mu = mus.iter().copied().sum::<Decimal>() / Decimal::from(mus.len() as i64);

                let alpha = avg_ret - avg_mu;

                println!(
                    "      T+{:>2}h: 事件 {:+.2}%  基准 {:+.2}%  alpha {:+.2}%  n={}",
                    h, avg_ret, avg_mu, alpha, n_ret
                );
            }
        }
    }

    println!("\n  判断标准：alpha 绝对值 > 1.5% 且 n > 15 才算初步可交易。");
}

fn print_stats(events: &[&EventOutcome]) {
    if events.is_empty() {
        return;
    }

    for (idx, h) in HORIZONS_HOURS.iter().enumerate() {
        let values: Vec<Decimal> = events
            .iter()
            .filter_map(|e| e.returns.get(idx).and_then(|(_, r)| *r))
            .collect();

        if values.is_empty() {
            println!("      T+{:>2}h: N/A", h);
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

        let worst = sorted.first().copied().unwrap_or(Decimal::ZERO);
        let best = sorted.last().copied().unwrap_or(Decimal::ZERO);

        println!(
            "      T+{:>2}h: n={:>3}  均{:+.2}%  中位{:+.2}%  胜{:.1}%  范围[{:+.1}%, {:+.1}%]",
            h, n, avg, median, win_rate, worst, best
        );
    }
}
