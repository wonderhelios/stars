//! Fixed hypotheses on completed hourly observations. Never grants live authorization.
//! Price studies have no leverage, capacity, stop, fills, or funding settlements.
#[cfg(test)]
use crate::paper_store::FundingSnapshotRow;
#[cfg(test)]
use rust_decimal::prelude::ToPrimitive;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

const HOUR: i64 = 3_600_000;
const COST: f64 = 0.15;
const MIN_VOLUME: f64 = 500_000.0;
const VERSION: &str = "parallel-v2";

#[derive(Clone, Debug)]
pub(crate) struct Point {
    at: i64,
    hour: i64,
    coin: String,
    price: f64,
    prior: f64,
    rate8: f64,
    volume: f64,
}
impl Point {
    pub(crate) fn read_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let number = |i| -> rusqlite::Result<f64> {
            let raw: String = row.get(i)?;
            raw.parse::<f64>().map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    i,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })
        };
        Ok(Self {
            at: row.get(0)?,
            hour: row.get(7)?,
            coin: row.get(1)?,
            price: number(2)?,
            prior: number(3)?,
            rate8: number(4)? * 8.0 / row.get::<_, i64>(5)? as f64,
            volume: number(6)?,
        })
    }
    #[cfg(test)]
    fn from(row: FundingSnapshotRow) -> Option<Self> {
        let p = Self {
            at: row.observed_at,
            hour: row.snapshot_hour,
            coin: row.inst_id,
            price: row.reference_price.to_f64()?,
            prior: row.prior_24h_return.to_f64()?,
            rate8: row.funding_rate.to_f64()? * 8.0 / row.funding_period_hours as f64,
            volume: row.volume_quote_24h.to_f64()?,
        };
        (p.price > 0.0 && row.funding_period_hours > 0 && p.rate8.is_finite()).then_some(p)
    }
}
#[derive(Clone, Copy)]
struct Hypothesis {
    id: &'static str,
    name: &'static str,
    rule: &'static str,
    direction: f64,
    mode: u8,
}
const LINES: [Hypothesis; 7] = [
    Hypothesis {
        id: "positive-reversal",
        name: "正费率反转做空",
        rule: "24h涨幅≥3%，8h等效费率>0.05%",
        direction: -1.0,
        mode: 0,
    },
    Hypothesis {
        id: "negative-reversal",
        name: "负费率反转做多",
        rule: "24h跌幅≤−3%，8h等效费率<−0.05%",
        direction: 1.0,
        mode: 1,
    },
    Hypothesis {
        id: "momentum-long",
        name: "纯动量做多",
        rule: "24h涨幅≥3%，不限制费率",
        direction: 1.0,
        mode: 2,
    },
    Hypothesis {
        id: "momentum-short",
        name: "纯动量做空",
        rule: "24h跌幅≤−3%，不限制费率",
        direction: -1.0,
        mode: 3,
    },
    Hypothesis {
        id: "funding-only",
        name: "纯正费率做空",
        rule: "8h等效费率>0.05%，不限制涨幅",
        direction: -1.0,
        mode: 4,
    },
    Hypothesis {
        id: "confirmed-reversal",
        name: "跨平台确认反转",
        rule: "正费率反转条件，并且另一平台同币在此前30分钟内也满足",
        direction: -1.0,
        mode: 5,
    },
    Hypothesis {
        id: "opposite-control",
        name: "反转信号反向对照",
        rule: "与正费率反转相同信号，改为做多",
        direction: 1.0,
        mode: 0,
    },
];
fn qualifies(p: &Point, mode: u8) -> bool {
    if p.volume < MIN_VOLUME {
        return false;
    }
    match mode {
        0 | 5 => p.prior >= 3.0 && p.rate8 > 0.0005,
        1 => p.prior <= -3.0 && p.rate8 < -0.0005,
        2 => p.prior >= 3.0,
        3 => p.prior <= -3.0,
        4 => p.rate8 > 0.0005,
        _ => false,
    }
}
// Keep HIP-3 venue prefixes in identities; only cross-match native crypto contracts.
fn base(coin: &str) -> Option<&str> {
    if coin.contains(':') {
        return None;
    }
    Some(
        coin.strip_suffix("-USDT-SWAP")
            .or_else(|| coin.strip_suffix("USDT"))
            .unwrap_or(coin),
    )
}
type Market = BTreeMap<String, Vec<Point>>;
fn before(points: &[Point], at: i64, tolerance: i64) -> Option<&Point> {
    let n = points.partition_point(|p| p.at <= at);
    points
        .get(n.checked_sub(1)?)
        .filter(|p| at - p.at <= tolerance)
}
// Each hour stores its last real snapshot. Match the target bucket rather than
// discarding a valid bucket when its collection minute precedes the entry minute.
// This is a coarse hourly study, not an exact holding-time execution replay.
fn at_hour(points: &[Point], hour: i64) -> Option<&Point> {
    points
        .get(points.partition_point(|p| p.hour < hour))
        .filter(|p| p.hour == hour)
}
fn mean(v: &[f64]) -> Option<f64> {
    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
}
fn median(v: &[f64]) -> Option<f64> {
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let n = s.len();
    (n > 0).then(|| (s[(n - 1) / 2] + s[n / 2]) / 2.0)
}
#[derive(Clone)]
struct Outcome {
    at: i64,
    coin: String,
    net: f64,
    btc: Option<f64>,
    market: Option<f64>,
    regime: &'static str,
}
#[derive(Serialize)]
pub struct Evidence {
    n: usize,
    days: usize,
    coins: usize,
    mean: Option<f64>,
    median: Option<f64>,
    win_pct: Option<f64>,
    best: Option<f64>,
    worst: Option<f64>,
    without_best: Option<f64>,
    without_top3: Option<f64>,
    top3_profit_share: Option<f64>,
    largest_day_share: Option<f64>,
    btc_matched: usize,
    btc_excess: Option<f64>,
    market_matched: usize,
    market_excess: Option<f64>,
    day_bootstrap_95: Option<[f64; 2]>,
    dates: Vec<DateEvidence>,
    regimes: Vec<RegimeEvidence>,
}
#[derive(Serialize)]
struct DateEvidence {
    day: i64,
    n: usize,
    mean: Option<f64>,
}
#[derive(Serialize)]
struct RegimeEvidence {
    regime: &'static str,
    n: usize,
    mean: Option<f64>,
}
fn evidence(rows: &[Outcome]) -> Evidence {
    let values: Vec<_> = rows.iter().map(|r| r.net).collect();
    let mut sorted = values.clone();
    sorted.sort_by(|a, b| b.total_cmp(a));
    let profits: Vec<_> = sorted.iter().copied().filter(|x| *x > 0.0).collect();
    let mut dates: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
    let mut regimes: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    for r in rows {
        dates
            .entry(r.at.div_euclid(24 * HOUR))
            .or_default()
            .push(r.net);
        regimes.entry(r.regime).or_default().push(r.net);
    }
    let btc: Vec<_> = rows
        .iter()
        .filter_map(|r| r.btc.map(|b| r.net - b))
        .collect();
    let market: Vec<_> = rows
        .iter()
        .filter_map(|r| r.market.map(|b| r.net - b))
        .collect();
    Evidence {
        n: rows.len(),
        days: dates.len(),
        coins: rows
            .iter()
            .map(|r| &r.coin)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        mean: mean(&values),
        median: median(&values),
        win_pct: (!rows.is_empty()).then(|| {
            values.iter().filter(|v| **v > 0.0).count() as f64 / rows.len() as f64 * 100.0
        }),
        best: sorted.first().copied(),
        worst: sorted.last().copied(),
        without_best: mean(sorted.get(1..).unwrap_or(&[])),
        without_top3: mean(sorted.get(3..).unwrap_or(&[])),
        top3_profit_share: (!profits.is_empty())
            .then(|| profits.iter().take(3).sum::<f64>() / profits.iter().sum::<f64>() * 100.0),
        largest_day_share: (!rows.is_empty()).then(|| {
            dates.values().map(Vec::len).max().unwrap_or(0) as f64 / rows.len() as f64 * 100.0
        }),
        btc_matched: btc.len(),
        btc_excess: mean(&btc),
        market_matched: market.len(),
        market_excess: mean(&market),
        day_bootstrap_95: day_interval(&dates),
        dates: dates
            .into_iter()
            .map(|(day, v)| DateEvidence {
                day,
                n: v.len(),
                mean: mean(&v),
            })
            .collect(),
        regimes: regimes
            .into_iter()
            .map(|(regime, v)| RegimeEvidence {
                regime,
                n: v.len(),
                mean: mean(&v),
            })
            .collect(),
    }
}
// Resample whole UTC date clusters, not individual coins from the same market event.
// This is an uncertainty diagnostic, not a significance test corrected for all hypotheses.
fn day_interval(dates: &BTreeMap<i64, Vec<f64>>) -> Option<[f64; 2]> {
    if dates.len() < 5 {
        return None;
    }
    let clusters: Vec<_> = dates
        .values()
        .map(|v| (v.iter().sum::<f64>(), v.len()))
        .collect();
    let mut seed = 47191_u64;
    let mut means = Vec::with_capacity(1000);
    for _ in 0..1000 {
        let mut sum = 0.0;
        let mut n = 0;
        for _ in 0..clusters.len() {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let (s, k) = clusters[(seed >> 32) as usize % clusters.len()];
            sum += s;
            n += k;
        }
        means.push(sum / n as f64);
    }
    means.sort_by(f64::total_cmp);
    Some([means[24], means[974]])
}

#[derive(Serialize)]
pub struct Study {
    id: String,
    venue: String,
    name: &'static str,
    rule: &'static str,
    hold_hours: i64,
    direction: &'static str,
    frozen_at: i64,
    missing_exit: usize,
    pending_exit: usize,
    historical_missing_exit: usize,
    forward_missing_exit: usize,
    actual_hold_min_hours: Option<f64>,
    actual_hold_max_hours: Option<f64>,
    extreme_moves: usize,
    historical: Evidence,
    forward: Evidence,
}
#[derive(Serialize)]
pub struct FeeStudy {
    coin: String,
    pair: String,
    short: String,
    long: String,
    n: usize,
    days: usize,
    first_at: i64,
    last_at: i64,
    mean_daily_spread: f64,
    positive_pct: f64,
    longest_positive_hours: f64,
    basis_range: Option<f64>,
    gaps: usize,
}
#[derive(Serialize)]
pub struct Report {
    pub version: &'static str,
    pub generated_at: i64,
    pub window_days: usize,
    pub exit_method: &'static str,
    pub studies: Vec<Study>,
    pub fee_studies: Vec<FeeStudy>,
    pub coverage: Vec<serde_json::Value>,
    pub errors: Vec<String>,
}

/// Every research line uses actual observations from the selected collection period.
pub fn cycle_points(rows: Vec<Point>, since: i64) -> Vec<Point> {
    rows.into_iter().filter(|p| p.at >= since).collect()
}

pub fn analyze(inputs: Vec<(&str, i64, Vec<Point>)>, errors: Vec<String>) -> Report {
    let mut data: BTreeMap<String, Market> = BTreeMap::new();
    let mut frozen = HashMap::new();
    let mut coverage = Vec::new();
    for (venue, cutoff, rows) in inputs {
        frozen.insert(venue.to_string(), cutoff);
        let mut m: Market = BTreeMap::new();
        for p in rows
            .into_iter()
            .filter(|p| p.price > 0.0 && p.price.is_finite() && p.rate8.is_finite())
        {
            m.entry(p.coin.clone()).or_default().push(p);
        }
        for points in m.values_mut() {
            points.sort_by_key(|p| p.at);
        }
        coverage.push(serde_json::json!({"venue":venue,"markets":m.len(),"observations":m.values().map(Vec::len).sum::<usize>(),"latest":m.values().filter_map(|v|v.last().map(|p|p.at)).max(),"frozen_at":cutoff}));
        data.insert(venue.to_string(), m);
    }
    let mut studies = Vec::new();
    for (venue, markets) in &data {
        // A contemporaneous native-market baseline, fixed at entry; never select controls by future returns.
        let mut controls: HashMap<(i64, i64), Option<f64>> = HashMap::new();
        for line in LINES {
            for hold in [4, 24] {
                let mut outcomes = Vec::new();
                let mut missing_exit = 0;
                let mut pending_exit = 0;
                let mut historical_missing_exit = 0;
                let mut forward_missing_exit = 0;
                let mut bad = 0;
                let mut actual_holds = Vec::new();
                for (coin, points) in markets {
                    let mut available_at = i64::MIN;
                    for p in points {
                        if p.at < available_at || !qualifies(p, line.mode) {
                            continue;
                        }
                        if line.mode == 5 {
                            let Some(c) = base(coin) else {
                                continue;
                            };
                            let confirmed =
                                data.iter().filter(|(v, _)| *v != venue).any(|(_, m)| {
                                    m.iter().filter(|(k, _)| base(k) == Some(c)).any(|(_, v)| {
                                        before(v, p.at, HOUR / 2).is_some_and(|q| qualifies(q, 0))
                                    })
                                });
                            if !confirmed {
                                continue;
                            }
                        }
                        // Reserve the whole observation interval even if its exit is missing.
                        let target_hour = p.hour + hold * HOUR;
                        available_at = target_hour;
                        let Some(exit) = at_hour(points, target_hour) else {
                            if target_hour >= crate::paper::now_ms().div_euclid(HOUR) * HOUR {
                                pending_exit += 1;
                            } else {
                                missing_exit += 1;
                                if p.at < frozen[venue] {
                                    historical_missing_exit += 1;
                                } else {
                                    forward_missing_exit += 1;
                                }
                            }
                            continue;
                        };
                        available_at = exit.at;
                        actual_holds.push((exit.at - p.at) as f64 / HOUR as f64);
                        let ret = (exit.price / p.price - 1.0) * 100.0;
                        if !ret.is_finite() {
                            continue;
                        }
                        if ret.abs() > 50.0 {
                            bad += 1;
                        }
                        let btc = markets
                            .iter()
                            .find(|(c, _)| base(c) == Some("BTC"))
                            .and_then(|(_, v)| {
                                let a = before(v, p.at, HOUR / 2)?;
                                let b = before(v, exit.at, HOUR / 2)?;
                                Some((b.price / a.price - 1.0) * 100.0)
                            });
                        // Cache by exact entry and exit timestamps so benchmarks use the same interval.
                        let market = *controls.entry((p.at, exit.at)).or_insert_with(|| {
                            let v: Vec<_> = markets
                                .iter()
                                .filter(|(c, _)| base(c).is_some())
                                .filter_map(|(_, v)| {
                                    let a = before(v, p.at, HOUR / 2)?;
                                    if a.volume < MIN_VOLUME {
                                        return None;
                                    }
                                    let b = before(v, exit.at, HOUR / 2)?;
                                    let r = (b.price / a.price - 1.0) * 100.0;
                                    r.is_finite().then_some(r)
                                })
                                .collect();
                            (v.len() >= 10).then(|| mean(&v)).flatten()
                        });
                        let regime = markets
                            .iter()
                            .find(|(c, _)| base(c) == Some("BTC"))
                            .and_then(|(_, v)| before(v, p.at, HOUR / 2))
                            .map(|b| {
                                if b.prior > 3.0 {
                                    "BTC上涨"
                                } else if b.prior < -3.0 {
                                    "BTC下跌"
                                } else {
                                    "BTC震荡"
                                }
                            })
                            .unwrap_or("基准缺失");
                        outcomes.push(Outcome {
                            at: p.at,
                            coin: coin.clone(),
                            net: line.direction * ret - COST,
                            btc: base(coin).and(btc).map(|r| line.direction * r - COST),
                            market: base(coin).and(market).map(|r| line.direction * r - COST),
                            regime,
                        });
                    }
                }
                let cutoff = frozen[venue];
                let (old, new): (Vec<_>, Vec<_>) =
                    outcomes.into_iter().partition(|r| r.at < cutoff);
                studies.push(Study {
                    id: format!("{VERSION}-{venue}-{}-{hold}h", line.id),
                    venue: venue.clone(),
                    name: line.name,
                    rule: line.rule,
                    hold_hours: hold,
                    direction: if line.direction < 0.0 {
                        "做空"
                    } else {
                        "做多"
                    },
                    frozen_at: cutoff,
                    missing_exit,
                    pending_exit,
                    historical_missing_exit,
                    forward_missing_exit,
                    actual_hold_min_hours: actual_holds.iter().copied().reduce(f64::min),
                    actual_hold_max_hours: actual_holds.iter().copied().reduce(f64::max),
                    extreme_moves: bad,
                    historical: evidence(&old),
                    forward: evidence(&new),
                });
            }
        }
    }
    Report {
        version: VERSION,
        generated_at: crate::paper::now_ms(),
        window_days: 30,
        exit_method: "target-hour-v2",
        fee_studies: fee_evidence(&data),
        studies,
        coverage,
        errors,
    }
}

fn fee_evidence(data: &BTreeMap<String, Market>) -> Vec<FeeStudy> {
    let mut result = Vec::new();
    let venues: Vec<_> = data.keys().collect();
    for i in 0..venues.len() {
        for j in i + 1..venues.len() {
            let (lv, rv) = (venues[i], venues[j]);
            for (coin, left) in &data[lv] {
                let Some(c) = base(coin) else {
                    continue;
                };
                let Some((_, right)) = data[rv].iter().find(|(k, _)| base(k) == Some(c)) else {
                    continue;
                };
                let mut samples = Vec::new();
                let mut used = i64::MIN;
                for a in left {
                    if a.volume < MIN_VOLUME {
                        continue;
                    }
                    let Some(b) = before(right, a.at, HOUR / 2) else {
                        continue;
                    };
                    if b.volume < MIN_VOLUME || b.at == used {
                        continue;
                    }
                    used = b.at;
                    // USD contract price units are not guaranteed identical; reject obvious scaling mismatches.
                    let basis = (a.price / b.price - 1.0) * 100.0;
                    if basis.abs() > 20.0 {
                        continue;
                    }
                    samples.push((a.at, (a.rate8 - b.rate8) * 3.0 * 100.0, basis));
                }
                if samples.is_empty() {
                    continue;
                }
                // Fix leg orientation from the first snapshot; never flip retroactively to make every spread positive.
                let direction = if samples[0].1 >= 0.0 { 1.0 } else { -1.0 };
                let mut longest: f64 = 0.0;
                let mut duration = 0.0;
                let mut previous = None;
                let mut gaps = 0;
                for &(at, diff, _) in &samples {
                    if let Some((last, positive)) = previous {
                        let dt = at - last;
                        if dt > HOUR + HOUR / 2 {
                            gaps += 1;
                            duration = 0.0;
                        } else if positive && diff * direction > 0.0 {
                            duration += dt as f64 / HOUR as f64;
                        } else {
                            duration = 0.0;
                        }
                    }
                    longest = longest.max(duration);
                    previous = Some((at, diff * direction > 0.0));
                }
                let min = samples.iter().map(|s| s.2).fold(f64::INFINITY, f64::min);
                let max = samples
                    .iter()
                    .map(|s| s.2)
                    .fold(f64::NEG_INFINITY, f64::max);
                result.push(FeeStudy {
                    coin: c.to_string(),
                    pair: format!("{lv} / {rv}"),
                    short: if direction > 0.0 {
                        lv.clone()
                    } else {
                        rv.clone()
                    },
                    long: if direction > 0.0 {
                        rv.clone()
                    } else {
                        lv.clone()
                    },
                    n: samples.len(),
                    days: samples
                        .iter()
                        .map(|s| s.0.div_euclid(24 * HOUR))
                        .collect::<std::collections::HashSet<_>>()
                        .len(),
                    first_at: samples[0].0,
                    last_at: samples.last().unwrap().0,
                    mean_daily_spread: samples.iter().map(|s| s.1 * direction).sum::<f64>()
                        / samples.len() as f64,
                    positive_pct: samples.iter().filter(|s| s.1 * direction > 0.0).count() as f64
                        / samples.len() as f64
                        * 100.0,
                    longest_positive_hours: longest,
                    basis_range: Some(max - min),
                    gaps,
                });
            }
        }
    }
    result.sort_by(|a, b| a.pair.cmp(&b.pair).then(a.coin.cmp(&b.coin)));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn outliers_do_not_define_the_median() {
        let rows: Vec<_> = [1.0, 1.0, 2.0, 2.0, 20.0]
            .into_iter()
            .enumerate()
            .map(|(i, net)| Outcome {
                at: i as i64 * 24 * HOUR,
                coin: i.to_string(),
                net,
                btc: Some(0.0),
                market: Some(0.0),
                regime: "BTC震荡",
            })
            .collect();
        let e = evidence(&rows);
        assert_eq!(e.median, Some(2.0));
        assert_eq!(e.without_best, Some(1.5));
        assert_eq!(e.without_top3, Some(1.0));
        assert_eq!(e.days, 5);
    }
    #[test]
    fn cycle_filters_old_prices_and_fee_snapshots_at_exact_boundary() {
        let points = [99, 100, 101]
            .into_iter()
            .map(|at| Point {
                at,
                hour: 0,
                coin: "BTC".into(),
                price: 1.0,
                prior: 0.0,
                rate8: 0.01,
                volume: 1e6,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            cycle_points(points.clone(), 100)
                .iter()
                .map(|p| p.at)
                .collect::<Vec<_>>(),
            vec![100, 101]
        );
        assert_eq!(cycle_points(points.clone(), 0).len(), 3);
        assert!(cycle_points(points, 102).is_empty());
    }
    #[test]
    fn benchmark_cannot_use_future_observations() {
        let p = Point {
            at: 100,
            hour: 0,
            coin: "BTC".into(),
            price: 1.0,
            prior: 0.0,
            rate8: 0.0,
            volume: 1e6,
        };
        assert!(before(&[p.clone()], 99, HOUR).is_none());
        assert!(before(&[p], 100, HOUR).is_some());
    }
    #[test]
    fn negative_funding_and_momentum_are_distinct() {
        let p = Point {
            at: 0,
            hour: 0,
            coin: "X".into(),
            price: 1.0,
            prior: -4.0,
            rate8: -0.001,
            volume: 1e6,
        };
        assert!(qualifies(&p, 1));
        assert!(qualifies(&p, 3));
        assert!(!qualifies(&p, 0));
        assert_eq!(base("xyz:BTC"), None);
        assert_eq!(base("BTC-USDT-SWAP"), Some("BTC"));
    }
    #[test]
    fn fixed_funding_legs_can_become_negative() {
        let make = |at, rate8| Point {
            at,
            hour: at.div_euclid(HOUR) * HOUR,
            coin: "BTC".into(),
            price: 100.0,
            prior: 0.0,
            rate8,
            volume: 1e6,
        };
        let mut a = Market::new();
        a.insert("BTC".into(), vec![make(0, 0.01), make(HOUR, -0.02)]);
        let mut b = Market::new();
        b.insert("BTC".into(), vec![make(0, 0.0), make(HOUR, 0.0)]);
        let r = fee_evidence(&BTreeMap::from([("a".into(), a), ("b".into(), b)]));
        assert_eq!(r[0].short, "a");
        assert_eq!(r[0].positive_pct, 50.0);
        assert!(r[0].mean_daily_spread < 0.0);
    }
    fn row(at: i64, price: i64) -> FundingSnapshotRow {
        FundingSnapshotRow {
            snapshot_hour: at,
            observed_at: at,
            inst_id: "X".into(),
            funding_rate: rust_decimal::Decimal::new(1, 3),
            funding_period_hours: 8,
            next_funding_at: None,
            prior_24h_return: rust_decimal::Decimal::from(4),
            reference_price: rust_decimal::Decimal::from(price),
            bid_price: None,
            ask_price: None,
            quote_kind: None,
            quote_observed_at: None,
            volume_quote_24h: rust_decimal::Decimal::from(1_000_000),
            open_interest_base: None,
        }
    }
    #[test]
    fn future_results_are_separate_and_large_losses_are_not_removed() {
        let r = analyze(
            vec![(
                "Hyperliquid",
                24 * HOUR,
                vec![row(0, 100), row(24 * HOUR, 200), row(48 * HOUR, 210)]
                    .into_iter()
                    .filter_map(Point::from)
                    .collect(),
            )],
            vec![],
        );
        let s = r
            .studies
            .iter()
            .find(|s| s.id.ends_with("positive-reversal-24h"))
            .unwrap();
        assert_eq!(s.historical.n, 1);
        assert_eq!(s.forward.n, 1);
        assert_eq!(s.extreme_moves, 1);
        assert!((s.historical.mean.unwrap() + 100.15).abs() < 1e-9);
        assert!((s.forward.mean.unwrap() + 5.15).abs() < 1e-9);
        assert_eq!(s.missing_exit, 1);
        assert_eq!(s.forward.btc_matched, 0);
    }
    #[test]
    fn hourly_exit_uses_real_target_bucket_despite_collection_minute_drift() {
        let minute = HOUR / 60;
        let mut entry = row(0, 100);
        entry.observed_at = 50 * minute;
        let mut exit = row(4 * HOUR, 90);
        exit.observed_at = 4 * HOUR + 10 * minute;
        // The target snapshot precedes entry+4h by 40 minutes; it is present,
        // and its actual 3h20 observation duration must be disclosed.
        let r = analyze(
            vec![(
                "OKX",
                HOUR,
                vec![entry, exit]
                    .into_iter()
                    .filter_map(Point::from)
                    .collect(),
            )],
            vec![],
        );
        let s = r
            .studies
            .iter()
            .find(|s| s.id.ends_with("positive-reversal-4h"))
            .unwrap();
        assert_eq!(s.historical.n, 1);
        assert!((s.historical.mean.unwrap() - 9.85).abs() < 1e-9);
        assert!((s.actual_hold_min_hours.unwrap() - 10.0 / 3.0).abs() < 1e-9);
        assert_eq!(s.historical_missing_exit, 0);
        assert_eq!(s.forward_missing_exit, 1);
    }

    #[test]
    fn missing_target_hour_is_not_filled_with_a_later_price() {
        let r = analyze(
            vec![(
                "OKX",
                HOUR,
                vec![row(0, 100), row(5 * HOUR, 90)]
                    .into_iter()
                    .filter_map(Point::from)
                    .collect(),
            )],
            vec![],
        );
        let s = r
            .studies
            .iter()
            .find(|s| s.id.ends_with("positive-reversal-4h"))
            .unwrap();
        assert_eq!(s.historical.n, 0);
        assert_eq!(s.historical_missing_exit, 1);
        assert_eq!(s.forward_missing_exit, 1);
        assert_eq!(s.actual_hold_min_hours, None);
    }

    #[test]
    fn uncertainty_needs_multiple_dates() {
        assert!(day_interval(&BTreeMap::from([(0, vec![1.0; 100])])).is_none());
        let d = (0..5).map(|i| (i, vec![2.0])).collect();
        assert_eq!(day_interval(&d), Some([2.0, 2.0]));
    }
}
