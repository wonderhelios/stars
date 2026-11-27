//! 策略健康度：用最近的因子 IC 和多空价差判断策略是否在失效。
//!
//! 口径（与实盘三因子一致）：
//!   · 每天取「30 日均额 ≥ min_vol_usd 且非 HIP-3」的币
//!   · 若配置了 dd_weight / upfrac_weight，则按实盘 3 因子横截面 z-score 合成打分
//!     （mom14/vol20 + dd14 + upfrac14），否则退化为 F1 动量打分
//!   · IC = 合成打分与【次日收益】的截面相关系数
//!   · 多空价差 = 前 top_frac 次日平均收益 − 后 top_frac 次日平均收益
//!   · 取最近 90 个交易日的均值；回撤基于时间加权净值指数（TWR）计算
use std::collections::BTreeMap;

fn num_ok(x: f64) -> bool {
    x.is_finite()
}

/// 健康度计算参数（默认兼容历史 F1 口径，实盘调用时传入 LiveConfig）
#[derive(Clone, Debug)]
pub struct HealthParams {
    pub min_vol_usd: f64,
    pub lookback: usize,
    pub top_frac: f64,
    pub momentum_weight: f64,
    pub dd_weight: f64,
    pub upfrac_weight: f64,
}

impl Default for HealthParams {
    fn default() -> Self {
        Self {
            min_vol_usd: 1e7,
            lookback: 14,
            top_frac: 0.20,
            momentum_weight: 1.0,
            dd_weight: 0.0,
            upfrac_weight: 0.0,
        }
    }
}

impl HealthParams {
    pub fn from_live_config(cfg: &crate::live::LiveConfig) -> Self {
        Self {
            min_vol_usd: cfg.min_vol_usd.max(1.0),
            lookback: cfg.lookback.clamp(2, 120),
            top_frac: cfg.top_frac.clamp(0.05, 0.50),
            momentum_weight: cfg.momentum_weight.max(0.0),
            dd_weight: cfg.dd_weight.max(0.0),
            upfrac_weight: cfg.upfrac_weight.max(0.0),
        }
    }

    pub fn factor_label(&self) -> String {
        let mut parts = Vec::new();
        if self.momentum_weight > 0.0 {
            parts.push(format!("Mom{}", self.lookback));
        }
        if self.dd_weight > 0.0 {
            parts.push("DD14".into());
        }
        if self.upfrac_weight > 0.0 {
            parts.push("UpFrac14".into());
        }
        if parts.is_empty() {
            "F1 动量".into()
        } else if parts.len() == 1 {
            parts[0].clone()
        } else {
            format!("{} 因子合成 ({})", parts.len(), parts.join("+"))
        }
    }
}

/// 健康度结果（直接序列化成 JSON，前端只读）
#[derive(Clone, Debug, serde::Serialize, Default)]
pub struct Health {
    pub status: String,        // green / yellow / red
    pub reason: String,
    pub factor_label: String,  // 例如 "3 因子合成 (Mom14+DD14+UpFrac14)"
    pub ic90: Option<f64>,     // 滚动 90 日平均 IC
    pub ic60: Option<f64>,
    pub spread90: Option<f64>, // 滚动 90 日多空价差（%/天）
    pub dd_pct: Option<f64>,   // 当前回撤（相对峰值，%，基于 TWR 指数）
    pub max_dd_pct: Option<f64>, // 历史最大回撤（%，基于 TWR 指数）
    pub days: usize,
}

fn zscore_inplace(vals: &mut [f64]) {
    if vals.len() < 2 {
        for v in vals.iter_mut() {
            *v = 0.0;
        }
        return;
    }
    let n = vals.len() as f64;
    let mean = vals.iter().sum::<f64>() / n;
    let var = vals.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
    let std = var.sqrt();
    if std <= 1e-12 {
        for v in vals.iter_mut() {
            *v = 0.0;
        }
    } else {
        for v in vals.iter_mut() {
            *v = (*v - mean) / std;
        }
    }
}

/// 用日线面板 + TWR 净值指数 + 实盘因子配置算健康度。
pub fn compute_with_params(
    panel: &[crate::store::PanelEntry],
    equity: &[(i64, f64)],
    params: &HealthParams,
) -> Health {
    const W: usize = 90;
    let factor_label = params.factor_label();
    let lb = params.lookback.max(2);
    let use_composite = params.dd_weight > 0.0 || params.upfrac_weight > 0.0;

    let mut ts: Vec<i64> = Vec::new();
    let mut closes: BTreeMap<String, BTreeMap<i64, f64>> = BTreeMap::new();
    let mut highs: BTreeMap<String, BTreeMap<i64, f64>> = BTreeMap::new();
    let mut dvol: BTreeMap<String, BTreeMap<i64, f64>> = BTreeMap::new();
    let mut stamp: BTreeMap<i64, ()> = BTreeMap::new();
    for e in panel {
        if e.coin.contains(':') {
            continue;
        }
        let c = closes.entry(e.coin.clone()).or_default();
        let h = highs.entry(e.coin.clone()).or_default();
        let v = dvol.entry(e.coin.clone()).or_default();
        for k in e.candles.iter() {
            if !num_ok(k.c) || k.c <= 0.0 {
                continue;
            }
            c.insert(k.t, k.c);
            let hi = if num_ok(k.h) && k.h > 0.0 { k.h } else { k.c };
            h.insert(k.t, hi);
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
            factor_label,
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
    let start = n.saturating_sub(150).max(31.max(lb + 1));
    for i in start..n.saturating_sub(1) {
        // (mom_adj, dd14, upfrac14, ret_next)
        let mut raw: Vec<(f64, f64, f64, f64)> = Vec::new();
        for c in coins.iter() {
            let (Some(now), Some(past)) = (col(&closes, c, i), col(&closes, c, i.saturating_sub(lb)))
            else {
                continue;
            };
            if past <= 0.0 {
                continue;
            }
            // 20 日波动
            let mut rets = Vec::with_capacity(20);
            for j in (i + 1).saturating_sub(20)..=i {
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
            if avg < params.min_vol_usd {
                continue;
            }
            let (Some(nx), Some(px)) = (col(&closes, c, i + 1), col(&closes, c, i)) else {
                continue;
            };
            if px <= 0.0 || nx <= 0.0 {
                continue;
            }
            let mom_adj = (now / past - 1.0) / vol;
            // 14 日距高点 (close / high14 - 1)
            let mut hi14 = now;
            for j in i.saturating_sub(13)..=i {
                if let Some(hv) = col(&highs, c, j) {
                    if hv > hi14 {
                        hi14 = hv;
                    }
                }
            }
            let dd14 = if hi14 > 0.0 { now / hi14 - 1.0 } else { 0.0 };
            // 14 日上涨天数占比
            let mut up_cnt = 0usize;
            let mut tot_cnt = 0usize;
            for j in i.saturating_sub(13)..=i {
                if j == 0 {
                    continue;
                }
                if let (Some(a), Some(b)) = (col(&closes, c, j), col(&closes, c, j - 1)) {
                    tot_cnt += 1;
                    if a > b {
                        up_cnt += 1;
                    }
                }
            }
            let upfrac14 = if tot_cnt > 0 {
                up_cnt as f64 / tot_cnt as f64
            } else {
                0.5
            };
            raw.push((mom_adj, dd14, upfrac14, nx / px - 1.0));
        }
        if raw.len() < 10 {
            continue;
        }
        let m = raw.len();
        let scores: Vec<f64> = if use_composite {
            let mut z_mom: Vec<f64> = raw.iter().map(|r| r.0).collect();
            let mut z_dd: Vec<f64> = raw.iter().map(|r| r.1).collect();
            let mut z_up: Vec<f64> = raw.iter().map(|r| r.2).collect();
            zscore_inplace(&mut z_mom);
            zscore_inplace(&mut z_dd);
            zscore_inplace(&mut z_up);
            let wm = params.momentum_weight.max(0.0);
            let wd = params.dd_weight.max(0.0);
            let wu = params.upfrac_weight.max(0.0);
            (0..m)
                .map(|k| wm * z_mom[k] + wd * z_dd[k] + wu * z_up[k])
                .collect()
        } else {
            raw.iter().map(|r| r.0).collect()
        };

        // 截面 IC
        let ma = scores.iter().sum::<f64>() / m as f64;
        let mb = raw.iter().map(|x| x.3).sum::<f64>() / m as f64;
        let mut cov = 0.0;
        let mut va = 0.0;
        let mut vb = 0.0;
        for k in 0..m {
            cov += (scores[k] - ma) * (raw[k].3 - mb);
            va += (scores[k] - ma).powi(2);
            vb += (raw[k].3 - mb).powi(2);
        }
        if va > 1e-18 && vb > 1e-18 {
            ics.push(cov / (va.sqrt() * vb.sqrt()));
        }
        // 多空价差
        let mut sorted: Vec<(f64, f64)> = (0..m).map(|k| (scores[k], raw[k].3)).collect();
        sorted.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let kk = ((m as f64 * params.top_frac).round() as usize).clamp(1, 10).min(m / 2);
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

    // 基于 TWR 指数分别计算「当前回撤」与「历史最大回撤」
    let (cur_dd, max_dd) = if equity.len() > 1 {
        let mut peak = f64::NEG_INFINITY;
        let mut worst = 0.0f64;
        for &(_, v) in equity.iter() {
            if !v.is_finite() || v <= 0.0 {
                continue;
            }
            if v > peak {
                peak = v;
            }
            if peak > 0.0 {
                let d = (peak - v) / peak * 100.0;
                if d > worst {
                    worst = d;
                }
            }
        }
        let last = equity
            .iter()
            .rev()
            .find(|(_, v)| v.is_finite() && *v > 0.0)
            .map(|(_, v)| *v)
            .unwrap_or(0.0);
        if peak > 0.0 {
            (Some(((peak - last) / peak * 100.0).max(0.0)), Some(worst.max(0.0)))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    let mut status = "green".to_string();
    let mut reason = if ic90.is_some() {
        "因子与回撤均处于健康区间".to_string()
    } else {
        format!("样本不足（{} 天），暂按正常显示", ics.len())
    };
    if let Some(ic) = ic90 {
        if ic < 0.009 {
            reason = format!("因子近期偏弱（90 日 IC {ic:+.3}），历史上低 IC 后 20 天常反弹，不触发降杠杆");
        }
    }
    if let Some(d) = cur_dd {
        if d > 44.4 {
            status = "red".into();
            reason = format!("当前回撤 {d:.1}% 超过回测最大回撤（44.4%）");
        } else if d > 30.0 && status == "green" {
            status = "yellow".into();
            reason = format!("当前回撤 {d:.1}% 已超过历史 80% 分位");
        }
    }
    Health {
        status,
        reason,
        factor_label,
        ic90,
        ic60,
        spread90,
        dd_pct: cur_dd,
        max_dd_pct: max_dd,
        days: ics.len(),
    }
}
