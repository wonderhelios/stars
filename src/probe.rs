//! 挂单成交探测器 —— 验证「4 小时反转」策略的 maker 挂单到底能不能成交。
//!
//! 默认是**零风险观测模式**：每 4 小时算一次目标组合、记录当时价格，然后在
//! 窗口结束后用真实的小时最高/最低价判断「如果我们当时挂了限价单，会不会被
//! 触及」。触及率是成交率的上界（忽略了排队位置）。
//!
//! 真实下单模式需要显式开启，且用小额、post-only 挂单。

use crate::hl::Candle;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Serialize, Deserialize)]
pub struct ProbeConfig {
    /// 信号回看小时数
    pub lookback_h: usize,
    /// 换仓间隔小时数
    pub rebal_h: usize,
    /// 多头/空头各取多少比例
    pub top_frac: f64,
    /// 小时成交额门槛（美元）
    pub min_vol_usd: f64,
    pub maker_fee: f64,
    pub taker_fee: f64,
    pub enabled: bool,
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            lookback_h: 4,
            rebal_h: 4,
            top_frac: 0.25,
            min_vol_usd: 150_000.0,
            maker_fee: 0.00015,
            taker_fee: 0.00045,
            enabled: true,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct ProbeRound {
    pub ts: i64,
    /// true = 用历史小时线回放出来的（样本内），false = 向前实时观测
    #[serde(default)]
    pub replayed: bool,
    pub longs: Vec<String>,
    pub shorts: Vec<String>,
    /// 决策时点的中间价（= 我们假设的挂单价）
    pub mids: HashMap<String, f64>,
    /// 窗口结束后回填
    pub evaluated: bool,
    /// 被触及的挂单数（= 会成交）
    pub touched: usize,
    pub total: usize,
    /// 理想口径：全部成交时的毛收益（市场中性组合，权重 ±0.5/k）
    pub gross_ideal: f64,
    /// 现实口径：只有被触及的才算建仓
    pub gross_real: f64,
    /// 实际建仓部分的换手（用于算手续费）
    pub turnover: f64,
    pub end_ts: i64,
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct ProbeState {
    pub config: ProbeConfig,
    pub rounds: Vec<ProbeRound>,
    pub last_run_at: Option<i64>,
    pub last_error: Option<String>,
}

impl ProbeState {
    pub fn load(path: &std::path::Path) -> Self {
        let mut s: Self = std::fs::read_to_string(path)
            .ok()
            .and_then(|x| serde_json::from_str(&x).ok())
            .unwrap_or_default();
        if s.config.rebal_h == 0 {
            s.config = ProbeConfig::default();
        }
        s
    }

    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// 用小时 K 线算 4h 反转目标：按最近 `lookback_h` 的涨跌幅排序，
/// 做空涨得最多的、做多跌得最多的。
pub fn targets(
    panel: &[(String, Vec<Candle>)],
    cfg: &ProbeConfig,
) -> (Vec<String>, Vec<String>, HashMap<String, f64>) {
    // 统一时间轴
    let mut all_ts: Vec<i64> = panel
        .iter()
        .flat_map(|(_, cs)| cs.iter().map(|c| c.t))
        .collect();
    all_ts.sort_unstable();
    all_ts.dedup();
    if all_ts.len() < cfg.lookback_h + 24 {
        return (Vec::new(), Vec::new(), HashMap::new());
    }
    let t_now = all_ts[all_ts.len() - 1];
    let t_past = all_ts[all_ts.len() - 1 - cfg.lookback_h];
    // 成交量窗口必须是「该时点之前 30 天」。若按数组末尾算，回放时会
    // 用未来的成交额筛币 —— 前视偏差。
    let vol_from_ts = t_now - 720 * 3_600_000;

    let mut sigs: Vec<(String, f64)> = Vec::new();
    let mut mids: HashMap<String, f64> = HashMap::new();
    for (coin, cs) in panel {
        if coin.contains(':') {
            continue; // 只做主 DEX
        }
        let Some(now) = cs.iter().find(|c| c.t == t_now) else {
            continue;
        };
        let Some(past) = cs.iter().find(|c| c.t == t_past) else {
            continue;
        };
        if now.c <= 0.0 || past.c <= 0.0 {
            continue;
        }
        // 滚动小时成交额
        let mut sum = 0.0;
        let mut n = 0usize;
        for c in cs.iter().rev() {
            if c.t > t_now {
                continue; // 未来数据不能参与成交量窗口
            }
            if c.t < vol_from_ts {
                break;
            }
            sum += c.v * c.c;
            n += 1;
        }
        let avg_vol = if n == 0 { 0.0 } else { sum / n as f64 };
        if avg_vol < cfg.min_vol_usd {
            continue;
        }
        sigs.push((coin.clone(), now.c / past.c - 1.0));
        // 挂单价用最新收盘价（真实运行时由盘口中间价覆盖）
        mids.insert(coin.clone(), now.c);
    }
    sigs.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    if sigs.len() < 8 {
        return (Vec::new(), Vec::new(), HashMap::new());
    }
    let k = ((sigs.len() as f64 * cfg.top_frac).round() as usize).max(1);
    let long: Vec<String> = sigs[..k].iter().map(|s| s.0.clone()).collect();
    let short: Vec<String> = sigs[sigs.len() - k..].iter().map(|s| s.0.clone()).collect();
    (long, short, mids)
}

/// 窗口结束后评估上一轮：用窗口内的小时最高/最低价判断挂单是否被触及。
pub fn evaluate(round: &mut ProbeRound, panel: &[(String, Vec<Candle>)], cfg: &ProbeConfig) {
    let by_coin: HashMap<&str, &Vec<Candle>> =
        panel.iter().map(|(c, v)| (c.as_str(), v)).collect();
    let k = round.longs.len().max(1);
    let mut touched = 0usize;
    let mut total = 0usize;
    let mut gross_ideal = 0.0;
    let mut gross_real = 0.0;

    for (coins, sign) in [(&round.longs, 1.0f64), (&round.shorts, -1.0f64)] {
        for coin in coins.iter() {
            let Some(cs) = by_coin.get(coin.as_str()) else {
                continue;
            };
            let Some(entry) = round.mids.get(coin).copied() else {
                continue;
            };
            let window: Vec<&Candle> = cs
                .iter()
                .filter(|c| c.t > round.ts && c.t <= round.ts + cfg.rebal_h as i64 * 3_600_000)
                .collect();
            if window.is_empty() {
                continue;
            }
            total += 1;
            let last = window.last().unwrap().c;
            gross_ideal += sign * (last / entry - 1.0) * 0.5 / k as f64;
            // 买单挂在 entry：low <= entry 才成交；卖单挂在 entry：high >= entry
            let hit = if sign > 0.0 {
                window.iter().any(|c| c.l <= entry)
            } else {
                window.iter().any(|c| c.h >= entry)
            };
            if hit {
                touched += 1;
                gross_real += sign * (last / entry - 1.0) * 0.5 / k as f64;
            }
        }
    }
    round.touched = touched;
    round.total = total;
    round.gross_ideal = gross_ideal;
    round.gross_real = gross_real;
    round.turnover = touched as f64 / (2.0 * k as f64).max(1.0);
    round.end_ts = round.ts + cfg.rebal_h as i64 * 3_600_000;
    round.evaluated = total > 0;
}

/// 统计汇总，给前端展示。
#[derive(Serialize)]
pub struct ProbeSummary {
    pub rounds: usize,
    pub evaluated: usize,
    /// 其中属于历史回放的数量（样本内）
    pub replayed: usize,
    pub touched: usize,
    pub total: usize,
    pub touch_rate: f64,
    /// 理想（全部成交，maker 费）
    pub ideal_ann: f64,
    pub ideal_t: f64,
    /// 现实（只有触及的成交，maker 费）
    pub real_ann: f64,
    pub real_t: f64,
    /// 现实口径按 taker 计费（保守下界）
    pub real_taker_ann: f64,
}

pub fn summarize(state: &ProbeState) -> ProbeSummary {
    let cfg = &state.config;
    let ev: Vec<&ProbeRound> = state.rounds.iter().filter(|r| r.evaluated).collect();
    let touched: usize = ev.iter().map(|r| r.touched).sum();
    let total: usize = ev.iter().map(|r| r.total).sum();
    let ppy = 24.0 * 365.0 / cfg.rebal_h.max(1) as f64;
    let mk = |f: &dyn Fn(&ProbeRound) -> f64| -> (f64, f64) {
        let xs: Vec<f64> = ev.iter().map(|r| f(r)).collect();
        if xs.len() < 3 {
            return (0.0, 0.0);
        }
        let m = xs.iter().sum::<f64>() / xs.len() as f64;
        let var = xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (xs.len() as f64 - 1.0);
        let sd = var.sqrt();
        let t = if sd > 0.0 {
            m / (sd / (xs.len() as f64).sqrt())
        } else {
            0.0
        };
        (m * ppy * 100.0, t)
    };
    let (ideal_ann, ideal_t) = mk(&|r| r.gross_ideal - 2.0 * r.turnover * cfg.maker_fee);
    let (real_ann, real_t) = mk(&|r| r.gross_real - 2.0 * r.turnover * cfg.maker_fee);
    let (real_taker_ann, _) = mk(&|r| r.gross_real - 2.0 * r.turnover * cfg.taker_fee);
    ProbeSummary {
        rounds: state.rounds.len(),
        evaluated: ev.len(),
        replayed: ev.iter().filter(|r| r.replayed).count(),
        touched,
        total,
        touch_rate: if total > 0 {
            touched as f64 / total as f64 * 100.0
        } else {
            0.0
        },
        ideal_ann,
        ideal_t,
        real_ann,
        real_t,
        real_taker_ann,
    }
}


/// 用历史小时线回放：从 `days` 天前开始，每 `rebal_h` 小时算一轮目标并立即评估。
/// 这样不用等几天就能看到触及率（但属于样本内，与向前观测要区分）。
pub fn replay(
    panel: &[(String, Vec<Candle>)],
    cfg: &ProbeConfig,
    days: i64,
) -> Vec<ProbeRound> {
    let mut all_ts: Vec<i64> = panel
        .iter()
        .flat_map(|(_, cs)| cs.iter().map(|c| c.t))
        .collect();
    all_ts.sort_unstable();
    all_ts.dedup();
    let step = cfg.rebal_h.max(1) as i64 * 3_600_000;
    let warmup = (cfg.lookback_h + 720) as i64 * 3_600_000;
    if all_ts.len() < 100 {
        return Vec::new();
    }
    let end = *all_ts.last().unwrap();
    let start = (end - days * 86_400_000).max(all_ts[0] + warmup);
    let by_coin: HashMap<&str, &Vec<Candle>> =
        panel.iter().map(|(c, v)| (c.as_str(), v)).collect();
    let mut out = Vec::new();
    let mut t = start;
    while t + step <= end {
        // 定位到该时刻
        let idx = match all_ts.binary_search(&t) {
            Ok(i) => i,
            Err(i) => i,
        };
        if idx >= all_ts.len() {
            break;
        }
        let t_now = all_ts[idx];
        let t_past = t_now - cfg.lookback_h as i64 * 3_600_000;
        let vol_from = t_now - 720 * 3_600_000;
        let mut sigs: Vec<(String, f64)> = Vec::new();
        let mut mids: HashMap<String, f64> = HashMap::new();
        for (coin, cs) in panel {
            if coin.contains(':') {
                continue;
            }
            let Some(now) = cs.iter().find(|c| c.t == t_now) else {
                continue;
            };
            let Some(past) = cs.iter().find(|c| c.t == t_past) else {
                continue;
            };
            if now.c <= 0.0 || past.c <= 0.0 {
                continue;
            }
            let mut sum = 0.0;
            let mut n = 0usize;
            for c in cs.iter().rev() {
                if c.t > t_now {
                    continue; // 未来数据不能参与成交量窗口
                }
                if c.t < vol_from {
                    break;
                }
                sum += c.v * c.c;
                n += 1;
            }
            let avg = if n == 0 { 0.0 } else { sum / n as f64 };
            if avg < cfg.min_vol_usd {
                continue;
            }
            sigs.push((coin.clone(), now.c / past.c - 1.0));
            mids.insert(coin.clone(), now.c);
        }
        if sigs.len() >= 8 {
            sigs.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            let k = ((sigs.len() as f64 * cfg.top_frac).round() as usize).max(1);
            let mut r = ProbeRound {
                ts: t_now,
                replayed: true,
                longs: sigs[..k].iter().map(|x| x.0.clone()).collect(),
                shorts: sigs[sigs.len() - k..].iter().map(|x| x.0.clone()).collect(),
                mids,
                ..Default::default()
            };
            evaluate(&mut r, panel, cfg);
            // 回放里对缺失数据的币也标记为已评估，避免污染前向统计
            if r.total == 0 {
                r.evaluated = false;
                r.replayed = true;
            }
            if r.evaluated {
                out.push(r);
            }
        }
        t += step;
    }
    // 只让 by_coin 被使用，避免未使用告警
    let _ = by_coin.len();
    out
}
