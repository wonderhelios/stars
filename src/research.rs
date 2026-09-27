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
const HISTORY_DAYS: u32 = 60;
const TRADE_COST_PCT: &str = "0.15";

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
        tokio::time::sleep(Duration::from_millis(120)).await;
    }

    print_summary("汇总统计（处理组）", &all_events);
    print_by_prior_momentum("按事件前 24h 动量分层", &all_events);
    print_by_strength("按费率强度分层", &all_events);
    print_alpha_vs_inst(&all_events, &benchmark_samples);
    print_net_short(&all_events);
    print_out_of_sample(&all_events);

    Ok(())
}

pub async fn run_scan(top_n: usize) -> anyhow::Result<()> {
    let threshold = Decimal::from_str(THRESHOLD_STR)?;
    let client = RestClient::new();

    println!(
        "拉取全市场 ticker（按 24h 成交额排序取 Top {}），目标历史 {} 天...",
        top_n, HISTORY_DAYS
    );
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
        if (i + 1) % 10 == 0 {
            println!(
                "  进度 {}/{}  成功 {}  失败 {}  事件 {}",
                i + 1,
                inst_ids.len(),
                ok,
                fail,
                all_events.len()
            );
        }
        tokio::time::sleep(Duration::from_millis(120)).await;
    }

    println!(
        "\n扫描完成: 成功 {} 失败 {} 事件 {}",
        ok,
        fail,
        all_events.len()
    );

    print_summary("汇总统计（处理组）", &all_events);
    print_by_prior_momentum("按事件前 24h 动量分层", &all_events);
    print_by_strength("按费率强度分层", &all_events);
    print_alpha_vs_inst(&all_events, &benchmark_samples);
    print_net_short(&all_events);
    print_out_of_sample(&all_events);

    Ok(())
}

#[derive(Debug, Clone)]
struct EventOutcome {
    #[allow(dead_code)]
    inst_id: String,
    event_ts: i64,
    funding_rate: Decimal,
    prior_24h_return: Option<Decimal>,
    /// (horizon_hours, 事件后 h 小时收益%)，未到期为 None
    returns: Vec<(i64, Option<Decimal>)>,
    /// (horizon_hours, 该币全时段平均 h 小时收益%)
    benchmark_mu: Vec<(i64, Decimal)>,
}

#[derive(Debug, Clone)]
struct BenchmarkSample {
    #[allow(dead_code)]
    inst_id: String,
    mu: Vec<(i64, Decimal)>,
    #[allow(dead_code)]
    sigma: Vec<(i64, Decimal)>,
}

async fn analyze_one(
    client: &RestClient,
    inst_id: &str,
    threshold: Decimal,
) -> anyhow::Result<(Vec<EventOutcome>, Option<BenchmarkSample>)> {
    let candles = client.candles_1h_history(inst_id, HISTORY_DAYS).await?;
    if candles.len() < 30 {
        println!("  [{}] K线不足 {} 根，跳过", inst_id, candles.len());
        return Ok((vec![], None));
    }

    let funding = client.funding_rate_history_paged(inst_id).await?;
    println!(
        "  [{}] 费率 {} 条，K线 {} 根",
        inst_id,
        funding.len(),
        candles.len()
    );

    let earliest_ts = candles.first().map(|c| c.open_time).unwrap_or(0);
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
        println!(
            "  [{}] 原始极端 {} 去重 {}",
            inst_id,
            raw_count,
            events.len()
        );
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
            benchmark_mu: benchmark.mu.clone(),
        });
    }

    Ok((outcomes, Some(benchmark)))
}

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

// ========== 统计输出 ==========

fn print_values(values: &[Decimal], indent: &str) {
    if values.is_empty() {
        return;
    }
    let n = values.len();
    let sum: Decimal = values.iter().copied().sum();
    let avg = sum / Decimal::from(n as i64);

    let mut sorted = values.to_vec();
    sorted.sort();
    let q = |p: f64| -> Decimal {
        let idx = ((n as f64) * p).floor() as usize;
        sorted[idx.min(n - 1)]
    };
    let p10 = q(0.10);
    let median = q(0.50);
    let p90 = q(0.90);

    let win = values.iter().filter(|v| **v > Decimal::ZERO).count();
    let win_rate = Decimal::from(win as i64) / Decimal::from(n as i64) * Decimal::from(100);

    println!(
        "{}n={:>3}  均{:+.2}%  中位{:+.2}%  胜{:.1}%  P10{:+.1}%  P90{:+.1}%",
        indent, n, avg, median, win_rate, p10, p90
    );
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
        print!("      T+{:>2}h: ", h);
        print_values(&values, "");
    }
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

fn print_by_strength(title: &str, events: &[EventOutcome]) {
    println!("\n\n========== {} ==========", title);
    if events.is_empty() {
        return;
    }

    let pos_events: Vec<&EventOutcome> = events
        .iter()
        .filter(|e| e.funding_rate > Decimal::ZERO)
        .collect();

    let pos_buckets: Vec<(&str, Option<Decimal>, Option<Decimal>)> = vec![
        (
            "正费率 弱 0.05~0.10%",
            Some(Decimal::from_str("0.0005").unwrap()),
            Some(Decimal::from_str("0.0010").unwrap()),
        ),
        (
            "正费率 中 0.10~0.20%",
            Some(Decimal::from_str("0.0010").unwrap()),
            Some(Decimal::from_str("0.0020").unwrap()),
        ),
        (
            "正费率 强 >0.20%",
            Some(Decimal::from_str("0.0020").unwrap()),
            None,
        ),
    ];

    for (label, lo, hi) in &pos_buckets {
        let subset: Vec<&EventOutcome> = pos_events
            .iter()
            .filter(|e| {
                if let Some(lo) = lo {
                    if e.funding_rate < *lo {
                        return false;
                    }
                }
                if let Some(hi) = hi {
                    if e.funding_rate >= *hi {
                        return false;
                    }
                }
                true
            })
            .copied()
            .collect();

        if subset.is_empty() {
            continue;
        }
        println!("\n  【{}】n={}", label, subset.len());
        print_stats(&subset);
    }

    let neg_events: Vec<&EventOutcome> = events
        .iter()
        .filter(|e| e.funding_rate < Decimal::ZERO)
        .collect();

    let neg_buckets: Vec<(&str, Option<Decimal>, Option<Decimal>)> = vec![
        (
            "负费率 弱 -0.05~-0.10%",
            Some(Decimal::from_str("-0.0010").unwrap()),
            Some(Decimal::from_str("-0.0005").unwrap()),
        ),
        (
            "负费率 中 -0.10~-0.20%",
            Some(Decimal::from_str("-0.0020").unwrap()),
            Some(Decimal::from_str("-0.0010").unwrap()),
        ),
        (
            "负费率 强 <-0.20%",
            None,
            Some(Decimal::from_str("-0.0020").unwrap()),
        ),
    ];

    for (label, lo, hi) in &neg_buckets {
        let subset: Vec<&EventOutcome> = neg_events
            .iter()
            .filter(|e| {
                if let Some(lo) = lo {
                    if e.funding_rate < *lo {
                        return false;
                    }
                }
                if let Some(hi) = hi {
                    if e.funding_rate >= *hi {
                        return false;
                    }
                }
                true
            })
            .copied()
            .collect();

        if subset.is_empty() {
            continue;
        }
        println!("\n  【{}】n={}", label, subset.len());
        print_stats(&subset);
    }
}

fn print_alpha_vs_inst(events: &[EventOutcome], benchmarks: &[BenchmarkSample]) {
    println!("\n\n========== Alpha 对比（事件收益 - 该币全时段平均） ==========");

    if events.is_empty() {
        println!("  无事件");
        return;
    }

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
    println!("  全部扫描币的全时段平均收益（整体市场基准）：");
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
                let rets: Vec<Decimal> = subset
                    .iter()
                    .filter_map(|e| e.returns.get(idx).and_then(|(_, r)| *r))
                    .collect();
                if rets.is_empty() {
                    continue;
                }
                let n_ret = rets.len();
                let avg_ret = rets.iter().copied().sum::<Decimal>() / Decimal::from(n_ret as i64);

                let mus: Vec<Decimal> = subset
                    .iter()
                    .filter_map(|e| {
                        e.benchmark_mu
                            .iter()
                            .find(|(hh, _)| hh == h)
                            .map(|(_, v)| *v)
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
}

fn print_net_short(events: &[EventOutcome]) {
    println!(
        "\n\n========== 净收益估算（正费率做空，扣 {}% 成本） ==========",
        TRADE_COST_PCT
    );
    println!(
        "  假设：正费率事件 → 做空 → 扣 {}% 单次交易成本（taker 双边+滑点）",
        TRADE_COST_PCT
    );
    println!("  注意：做空时资金费率是收入，此处未计入（保守估计）\n");

    let cost = Decimal::from_str(TRADE_COST_PCT).unwrap();

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
                if e.funding_rate <= Decimal::ZERO {
                    return false;
                }
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

        if subset.len() < 5 {
            continue;
        }

        println!("  【{} · 正费率做空】n={}", label, subset.len());
        for (idx, h) in HORIZONS_HOURS.iter().enumerate() {
            let net: Vec<Decimal> = subset
                .iter()
                .filter_map(|e| e.returns.get(idx).and_then(|(_, r)| *r))
                .map(|raw| -raw - cost)
                .collect();

            if net.is_empty() {
                continue;
            }
            let n = net.len();
            let avg = net.iter().copied().sum::<Decimal>() / Decimal::from(n as i64);
            let mut sorted = net.clone();
            sorted.sort();
            let median = sorted[n / 2];
            let win = net.iter().filter(|v| **v > Decimal::ZERO).count();
            let wr = Decimal::from(win as i64) / Decimal::from(n as i64) * Decimal::from(100);

            println!(
                "      T+{:>2}h: 净均{:+.2}%  中位{:+.2}%  胜率{:.1}%  n={}",
                h, avg, median, wr, n
            );
        }
    }
}

fn print_out_of_sample(events: &[EventOutcome]) {
    println!("\n\n========== 分样本外验证（前 50% vs 后 50% 时间） ==========");
    if events.len() < 20 {
        println!("  样本不足 20，跳过");
        return;
    }

    let mut sorted = events.to_vec();
    sorted.sort_by_key(|e| e.event_ts);
    let mid = sorted.len() / 2;
    let first_half = &sorted[..mid];
    let second_half = &sorted[mid..];

    let first_ts = sorted.first().map(|e| e.event_ts).unwrap_or(0);
    let second_ts = sorted[mid].event_ts;
    let last_ts = sorted.last().map(|e| e.event_ts).unwrap_or(0);

    println!(
        "  前段: {} 事件，{} → {}",
        first_half.len(),
        format_ts_cn(first_ts),
        format_ts_cn(second_ts)
    );
    println!(
        "  后段: {} 事件，{} → {}",
        second_half.len(),
        format_ts_cn(second_ts),
        format_ts_cn(last_ts)
    );

    println!("\n  -- 前段 --");
    print_summary("前段", first_half);
    println!("\n  -- 后段 --");
    print_summary("后段", second_half);
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
