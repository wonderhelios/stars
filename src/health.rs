//! 策略健康度：用最近的因子 IC 和多空价差判断策略是否在失效。
//!
//! 口径（与回测一致）：
//!   · 每天取「30 日均额 ≥ $10M 且非 HIP-3」的币
//!   · IC = F1 分数与【次日收益】的截面相关系数
//!   · 多空价差 = 前 k 名次日平均收益 − 后 k 名次日平均收益
//!   · 取最近 90 个交易日的均值 ✗ 和历史 5% 分位比较
use std::collections::BTreeMap;

fn num_ok(x: f64) -> bool {
    x.is_finite()
}

/// 健康度结果（直接序列化成 JSON ✗ 前端只读）
#[derive(Clone, Debug, serde::Serialize, Default)]
pub struct Health {
    pub status: String,      // green / yellow / red
    pub reason: String,
    pub ic90: Option<f64>,   // 滚动 90 日平均 IC
    pub ic60: Option<f64>,
    pub spread90: Option<f64>, // 滚动 90 日多空价差（%/天）
    pub dd_pct: Option<f64>,   // 当前回撤（相对峰值 ✗ %）
    pub days: usize,
}

/// 用日线面板 + 净值历史算健康度。
pub fn compute(
    panel: &[crate::momentum::PanelEntry],
    equity: &[(i64, f64)],
) -> Health {
    const W: usize = 90;
    // 汇总成 closes/dvol 表
    let mut ts: Vec<i64> = Vec::new();
    let mut closes: BTreeMap<String, BTreeMap<i64, f64>> = BTreeMap::new();
    let mut dvol: BTreeMap<String, BTreeMap<i64, f64>> = BTreeMap::new();
    let mut stamp: BTreeMap<i64, ()> = BTreeMap::new();
    for e in panel {
        if e.coin.contains(':') {
            continue;
        }
        let c = closes.entry(e.coin.clone()).or_default();
        let v = dvol.entry(e.coin.clone()).or_default();
        for k in e.candles.iter() {
            if !num_ok(k.c) || k.c <= 0.0 {
                continue;
            }
            c.insert(k.t, k.c);
            v.insert(k.t, k.c * k.v);
            stamp.insert(k.t, ());
        }
    }
    ts.extend(stamp.keys().copied());
    ts.sort_unstable();
    let n = ts.len();
    if n < 60 {
        return Health {
            status: "green".into(),
            reason: "样本不足，按正常处理".into(),
            days: 0,
            ..Default::default()
        };
    }
    let coins: Vec<&String> = closes.keys().collect();
    let col = |m: &BTreeMap<String, BTreeMap<i64, f64>>, c: &String, i: usize| -> Option<f64> {
        m.get(c).and_then(|x| x.get(&ts[i])).copied()
    };
    let mut ics: Vec<f64> = Vec::new();
    let mut spreads: Vec<f64> = Vec::new();
    let start = n.saturating_sub(150).max(31);
    for i in start..n.saturating_sub(1) {
        // 宇宙：30 日均额（回看 i-30..i）
        let mut uni: Vec<(f64, f64, f64)> = Vec::new(); // (f1, ret_next, dvol30)
        for c in coins.iter() {
            let (Some(now), Some(past)) = (col(&closes, c, i), col(&closes, c, i.saturating_sub(14)))
            else {
                continue;
            };
            if past <= 0.0 {
                continue;
            }
            // 20 日波动
            let mut rets = Vec::with_capacity(20);
            for j in (i + 1 - 20)..=i {
                if j == 0 {
                    continue;
                }
                if let (Some(a), Some(b)) = (col(&closes, c, j), col(&closes, c, j - 1)) {
                    if b > 0.0 {
                        rets.push(a / b - 1.0);
                    }
                }
            }
            if rets.len() < 5 {
                continue;
            }
            let mean = rets.iter().sum::<f64>() / rets.len() as f64;
            let var = rets.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (rets.len() - 1) as f64;
            let vol = var.sqrt().max(1e-9);
            // 30 日均额
            let mut sum = 0.0;
            let mut cnt = 0usize;
            for j in i.saturating_sub(30)..i {
                if let Some(v) = col(&dvol, c, j) {
                    sum += v;
                    cnt += 1;
                }
            }
            if cnt < 5 {
                continue;
            }
            let avg = sum / cnt as f64;
            if avg < 1e7 {
                continue;
            }
            let (Some(nx), Some(px)) = (col(&closes, c, i + 1), col(&closes, c, i)) else {
                continue;
            };
            if px <= 0.0 || nx <= 0.0 {
                continue;
            }
            uni.push(((now / past - 1.0) / vol, nx / px - 1.0, avg));
        }
        if uni.len() < 10 {
            continue;
        }
        // IC
        let m: usize = uni.len();
        let ma = uni.iter().map(|x| x.0).sum::<f64>() / m as f64;
        let mb = uni.iter().map(|x| x.1).sum::<f64>() / m as f64;
        let mut cov = 0.0;
        let mut va = 0.0;
        let mut vb = 0.0;
        for x in uni.iter() {
            cov += (x.0 - ma) * (x.1 - mb);
            va += (x.0 - ma).powi(2);
            vb += (x.1 - mb).powi(2);
        }
        if va > 1e-18 && vb > 1e-18 {
            ics.push(cov / (va.sqrt() * vb.sqrt()));
        }
        // 多空价差
        let mut sorted: Vec<(f64, f64)> = uni.iter().map(|x| (x.0, x.1)).collect();
        sorted.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let kk = ((m as f64 * 0.2).round() as usize).clamp(1, 10).min(m / 2);
        if kk >= 1 {
            let top: f64 = sorted[..kk].iter().map(|x| x.1).sum::<f64>() / kk as f64;
            let bot: f64 = sorted[m - kk..].iter().map(|x| x.1).sum::<f64>() / kk as f64;
            spreads.push((top - bot) * 100.0);
        }
    }
    let mean_last = |v: &[f64], w: usize| -> Option<f64> {
        if v.len() < w / 2 || v.is_empty() {
            return None;
        }
        let tail = &v[v.len().saturating_sub(w)..];
        Some(tail.iter().sum::<f64>() / tail.len() as f64)
    };
    let ic90 = mean_last(&ics, W);
    let ic60 = mean_last(&ics, 60);
    let spread90 = mean_last(&spreads, W);
    // 回撤
    let dd = if equity.len() > 1 {
        let peak = equity.iter().map(|x| x.1).fold(f64::MIN, f64::max);
        let last = equity.last().map(|x| x.1).unwrap_or(0.0);
        (peak > 0.0).then(|| (1.0 - last / peak) * 100.0)
    } else {
        None
    };
    let mut status = "green".to_string();
    let mut reason = if ic90.is_some() {
        "因子仍在有效区间".to_string()
    } else {
        format!("样本不足（{} 天）✗ 暂按正常显示", ics.len())
    };
    if let Some(ic) = ic90 {
        if ic < 0.0 {
            status = "red".into();
            reason = format!("滚动 90 日 IC 转负（{ic:+.3}）✗ 历史上从未出现");
        } else if ic < 0.009 {
            status = "yellow".into();
            reason = format!("滚动 90 日 IC {ic:+.3} 低于历史 5% 分位（0.009）");
        }
    }
    if let Some(d) = dd {
        if d > 44.4 {
            status = "red".into();
            reason = format!("回撤 {d:.0}% 超过回测最大回撤（44.4%）");
        } else if d > 30.0 && status == "green" {
            status = "yellow".into();
            reason = format!("回撤 {d:.0}% 已超过历史 80% 分位");
        }
    }
    Health {
        status,
        reason,
        ic90,
        ic60,
        spread90,
        dd_pct: dd,
        days: ics.len(),
    }
}
