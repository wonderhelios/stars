use anyhow::{Context,Result};
use std::collections::{HashMap,BTreeSet};
use crate::exchange::{Acct,MarketInfo,round_size};
#[derive(Clone, Debug)]
pub struct TradeConfig {
    pub lookback: usize,
    pub top_frac: f64,
    pub min_vol_usd: f64,
    pub leverage: f64,
    pub target_positions: usize,
    /// cap on adverse price move accepted for an IOC fill
    pub slippage: f64,
    /// exchange minimum order value
    pub min_order_usd: f64,
    /// **按净值比例的最小下单额**（0.01 = 1%）。
    ///
    /// 为什么需要它：`min_order_usd` 是绝对值，而仓位大小随账号缩放 ——
    /// 同一个 $10 门槛，对 $580 的账号是「一个仓位的 17.7%」（粗得几乎不调仓），
    /// 对 $2900 的账号是「3.8%」（每天微调一堆）。这就是"HL 不需要调、TxFlow 一大堆要调"
    /// 的真实原因：**两个账号用同一个策略，但执行粒度差了 5 倍**。
    ///
    /// 有效门槛 = max(min_order_usd, 净值 × min_order_pct)。
    /// 保留绝对下限是因为交易所本身有最小下单量，比例不能低于它。
    pub min_order_pct: f64,
    /// skip adjustments smaller than this share of the target notional
    pub rebalance_band: f64,
    /// fraction of equity actually deployed, leaving room for fees and slippage
    pub margin_buffer: f64,
    /// 每仓最小名义：低于此值的币会被剔除，否则仓位太小无法跟随复利
    pub min_position_usd: f64,
    /// 错峰调仓的档数。每个币每 N 天轮到一次，而不是每天全量调。
    ///
    /// 实测（相位平均、两个子区间均通过）：换手 29%→13%，成本 16.2%→7.3%，
    /// Sharpe 1.62→1.79，弱势区间 0.37→0.73。信号是 14 日动量，滞后几天
    /// 对它的影响远小于省下的成本。
    pub rebalance_slices: u32,
}

impl TradeConfig {
    /// 有效最小下单额：绝对下限与"净值的比例"取大者。
    ///
    /// 比例设成 0（默认）时行为与旧版完全一致，不会影响既有配置。
    pub fn effective_min_order_usd(&self, equity: f64) -> f64 {
        let pct = if self.min_order_pct.is_finite() && self.min_order_pct > 0.0 {
            equity * self.min_order_pct
        } else {
            0.0
        };
        self.min_order_usd.max(pct)
    }
}


impl Default for TradeConfig {
    fn default() -> Self {
        Self {
            lookback: 14,
            top_frac: 0.2,
            min_vol_usd: 5_000_000.0,
            leverage: 3.0,
            target_positions: 5,
            slippage: 0.005,
            min_order_usd: 10.0,
            min_order_pct: 0.0,
            rebalance_band: 0.02,
            margin_buffer: 0.90,
            min_position_usd: 15.0,
            rebalance_slices: 3,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Order {
    pub coin: String,
    pub buy: bool,
    pub reduce_only: bool,
    /// 带符号的目标仓位（币本位数量）。execute 在平仓后重读账户，用它把开仓
    /// 量算成「目标 − 实际」，避免部分成交/被拒导致方向做反。
    pub target: f64,
    pub size: f64,
    pub mid: f64,
    pub sz_decimals: i32,
    pub notional: f64,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct Plan {
    pub equity: f64,
    pub gross_notional: f64,
    pub per_coin: f64,
    pub long_leg: Vec<String>,
    pub short_leg: Vec<String>,
    /// Closes first, then opens/resizes.
    pub orders: Vec<Order>,
    pub notes: Vec<String>,
    /// 下单前已持有仓位的币（这些币无法切换保证金模式）
    pub held: Vec<String>,
    /// Admitted TxFlow final book, including positions retained by slicing/bands.
    pub txflow_targets: HashMap<String, (f64, f64, i32)>,
}

/// Keep a typed failure so resizing can only respond to the margin gate, never
/// swallow a missing quote, excessive concentration, or unbalanced book.
#[derive(Debug, Clone)]
struct TxFlowMarginExceeded {
    required: f64,
    budget: f64,
    positions: f64,
    retained: f64,
    orders: usize,
}
impl std::fmt::Display for TxFlowMarginExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TxFlow 保留仓＋可能成交挂单＋计划保证金超预算：预计 ${:.2}，预算 ${:.2}，超出 ${:.2}；其中目标仓位 ${:.2}（含保留仓 ${:.2}），{} 笔挂单额外预留 ${:.2}",
            self.required, self.budget, self.required-self.budget, self.positions,
            self.retained, self.orders, (self.required-self.positions).max(0.))
    }
}
impl std::error::Error for TxFlowMarginExceeded {}

/// TxFlow planning entry point; extracted from live::run to test portfolio admission offline.
pub fn build_txflow_plan(
    weights: &[(String,f64)], acct: &Acct, open_orders: &[crate::exchange::OpenOrder],
    markets: &HashMap<String,MarketInfo>, mids: &HashMap<String,f64>, cfg: &TradeConfig,
) -> Result<Plan> {
    let first_error = match build_txflow_plan_at_scale(weights, acct, open_orders, markets, mids, cfg, 1.) {
        Ok(plan) => return Ok(plan),
        Err(error) => error,
    };
    let Some(mut failure) = first_error.downcast_ref::<TxFlowMarginExceeded>().cloned() else { return Err(first_error); };
    // Resting orders and unexpected leverage require operator attention; do not
    // silently accommodate them by shrinking the strategy. Oversized input
    // weights must not be disguised as an ordinary minimum-order conflict.
    if !open_orders.is_empty()
        || weights.iter().map(|(_, w)| w.abs()).sum::<f64>() > 1. + 1e-8
        || acct.positions.values().any(|p| p.size.abs() > 1e-12 && (p.leverage as f64) < cfg.leverage) {
        return Err(first_error);
    }
    let mut scale = 1.;
    // A bounded, descending allocation adjustment, not a relaxed risk limit.
    // Each candidate retains the original slices, order floor and risk budget;
    // only the desired factor notionals shrink. Thresholds make this discrete.
    for _ in 0..64 {
        scale *= (failure.budget / failure.required * 0.995).min(0.99);
        if scale < 0.5 { break; }
        match build_txflow_plan_at_scale(weights, acct, open_orders, markets, mids, cfg, scale) {
            Ok(mut plan) => {
                plan.notes.push(format!("预算内缩减：原计划保证金 ${:.2} 超过预算 ${:.2}；目标权重统一缩至 {:.2}%，已重新核验实际保留仓、最小下单额、两腿和净敞口，保证金上限未提高",
                    first_error.downcast_ref::<TxFlowMarginExceeded>().unwrap().required,
                    failure.budget, scale * 100.));
                return Ok(plan);
            },
            Err(error) => match error.downcast_ref::<TxFlowMarginExceeded>() {
                Some(next) => failure = next.clone(),
                None => return Err(first_error.context(format!("目标缩减仍未通过风险检查：{error}"))),
            }
        }
    }
    Err(first_error.context("未找到预算内可执行计划（保留原错峰档位与最小下单额，最多缩减目标 50%）；未下单"))
}

fn build_txflow_plan_at_scale(
    weights: &[(String,f64)], acct: &Acct, open_orders: &[crate::exchange::OpenOrder],
    markets: &HashMap<String,MarketInfo>, mids: &HashMap<String,f64>, cfg: &TradeConfig, scale: f64,
) -> Result<Plan> {
    anyhow::ensure!(acct.equity.is_finite() && acct.equity>0. && cfg.leverage.is_finite()
        && cfg.leverage>=1. && cfg.leverage.fract()==0. && cfg.margin_buffer.is_finite()
        && cfg.margin_buffer>0. && cfg.margin_buffer<=1., "TxFlow 风险预算非法");
    // Reject incomplete inputs instead of letting build_plan silently drop names.
    let mut names = BTreeSet::new();
    for (coin, weight) in weights {
        anyhow::ensure!(weight.is_finite() && names.insert(coin), "TxFlow 权重非法或币名重复");
        let mid = mids.get(coin).context("TxFlow 目标缺价格")?;
        anyhow::ensure!(mid.is_finite() && *mid > 0. && markets.contains_key(coin),
            "TxFlow 目标价格/市场元数据非法");
    }
    for position in acct.positions.values() {
        anyhow::ensure!(position.size.is_finite(), "TxFlow 实际仓位数量非法");
        if position.size.abs() > 1e-12 {
            anyhow::ensure!(position.is_cross && position.leverage > 0,
                "TxFlow 已有逐仓或杠杆未知，必须先显式重建");
        }
    }
    let scaled: Vec<_> = weights.iter().map(|(coin, w)| (coin.clone(), w * scale)).collect();
    let mut plan=build_plan(&scaled,acct,markets,mids,cfg,None);
    let mut final_sizes:HashMap<String,f64>=acct.positions.iter().map(|(c,p)|(c.clone(),p.size)).collect();
    for o in &plan.orders {final_sizes.insert(o.coin.clone(),o.target);}
    for (coin,size) in &final_sizes {
        if size.abs() > 1e-12 {
            let mid=*mids.get(coin).context("TxFlow 保留目标缺价格")?;
            let decimals=markets.get(coin).context("TxFlow 保留目标缺市场")?.sz_decimals;
            plan.txflow_targets.insert(coin.clone(),(*size,mid,decimals));
        }
    }
    let mut ranges:HashMap<String,(f64,f64)>=final_sizes.iter().map(|(c,s)|(c.clone(),(*s,*s))).collect();
    // Resting orders may fill in any order, or not fill. Sum non-reduce
    // orders first; reduce-only fills may move either endpoint toward zero,
    // but must never be credited as guaranteed margin relief.
    let mut risk_prices = mids.clone();
    let mut reductions: HashMap<String, (f64, f64)> = HashMap::new();
    for o in open_orders {
        anyhow::ensure!(o.sz.is_finite() && o.sz>=0. && o.px.is_finite() && o.px>0., "TxFlow 挂单字段非法");
        anyhow::ensure!(o.side == "B" || o.side == "A", "TxFlow 挂单方向未知，拒绝计划");
        let mid = risk_prices.get_mut(&o.coin).context("TxFlow 挂单缺价格")?;
        anyhow::ensure!(mid.is_finite() && *mid > 0., "TxFlow 挂单风险价格非法");
        *mid = mid.max(o.px);
        let range = ranges.entry(o.coin.clone()).or_insert((0., 0.));
        if o.reduce_only {
            let reduction = reductions.entry(o.coin.clone()).or_default();
            if o.side == "B" { reduction.0 += o.sz; } else { reduction.1 += o.sz; }
        } else if o.side == "B" { range.1 += o.sz; } else { range.0 -= o.sz; }
    }
    for (coin, (buys, sells)) in reductions {
        let (low, high) = ranges.get_mut(&coin).expect("resting order range");
        if *high < 0. { *high = (*high + buys).min(0.); }
        if *low > 0. { *low = (*low - sells).max(0.); }
    }
    let mut margin=0.;let mut long=0.;let mut short=0.;let mut net_low=0.;let mut net_high=0.;
    let mut position_margin=0.; let mut retained_margin=0.;
    let mut long_names=0;let mut short_names=0;
    for (coin,(low,high)) in ranges {
        let mid=*risk_prices.get(&coin).with_context(||format!("TxFlow 缺价风险 {coin}"))?;
        anyhow::ensure!(mid.is_finite() && mid>0. && low.is_finite() && high.is_finite(),"TxFlow 风险价格/数量非法");
        let market=markets.get(&coin).context("TxFlow 风险市场元数据缺失")?;
        anyhow::ensure!(cfg.leverage<=market.max_leverage as f64,"TxFlow 市场杠杆上限不足");
        let worst=low.abs().max(high.abs())*mid;
        if worst<=1e-8 {continue;}
        let leverage=if let Some(p)=acct.positions.get(&coin).filter(|p|p.size.abs()>1e-12) {
            anyhow::ensure!(p.is_cross && p.leverage>0,"TxFlow 保留仓为逐仓或杠杆未知，必须先显式重建");
            (p.leverage as f64).min(cfg.leverage)
        } else {cfg.leverage};
        margin+=worst/leverage;
        let target_margin = final_sizes.get(&coin).copied().unwrap_or(0.).abs() * mids[&coin] / leverage;
        position_margin += target_margin;
        if !plan.orders.iter().any(|o| o.coin == coin) { retained_margin += target_margin; }
        anyhow::ensure!(worst<=acct.equity*0.5+1e-8,"TxFlow 最终单币名义超净值 50%");
        long+=high.max(0.)*mid;short+=(-low).max(0.)*mid;
        net_low+=low*mid;net_high+=high*mid;
        let target=final_sizes.get(&coin).copied().unwrap_or(0.);
        if target>1e-12 {long_names+=1;} else if target< -1e-12 {short_names+=1;}
    }
    anyhow::ensure!(long_names>=3 && short_names>=3,"TxFlow 最终组合每腿不足 3 个名字");
    anyhow::ensure!(long<=acct.equity*1.5+1e-8 && short<=acct.equity*1.5+1e-8
        && net_low.abs().max(net_high.abs())<=acct.equity*0.1+1e-8,"TxFlow 最终两腿/可能净敞口超限");
    let budget = acct.equity * cfg.margin_buffer;
    if margin > budget + 1e-8 {
        return Err(TxFlowMarginExceeded { required:margin, budget, positions:position_margin,
            retained:retained_margin, orders:open_orders.len() }.into());
    }
    plan.gross_notional = plan.txflow_targets.values().map(|(size, mid, _)| size.abs() * mid).sum();
    plan.per_coin = plan.gross_notional / plan.txflow_targets.len().max(1) as f64;
    Ok(plan)
}

/// 某个币属于哪一档。用币名的字节和取模，保证跨进程、跨重启都一致。
pub fn slice_of(coin: &str, slices: u32) -> u32 {
    if slices <= 1 {
        return 0;
    }
    let sum: u32 = coin.bytes().map(|b| b as u32).sum();
    sum % slices
}

/// Build the order list. `mids` must contain every coin we intend to trade.
#[allow(clippy::too_many_arguments)]
pub fn build_plan(
    weights: &[(String, f64)],
    acct: &Acct,
    markets: &HashMap<String, MarketInfo>,
    mids: &HashMap<String, f64>,
    cfg: &TradeConfig,
    equity_override: Option<f64>,
) -> Plan {
    let equity = equity_override.unwrap_or(acct.equity);
    // Deploy only part of the equity so fees and adverse fills cannot push the
    // book past the margin limit mid-run.
    let deployable = equity * cfg.margin_buffer;
    // 权重已归一化到 Σ|w| = 1，所以直接按权重缩放名义即可。
    let per_coin = if weights.is_empty() {
        0.0
    } else {
        deployable * cfg.leverage / weights.len() as f64
    };
    let long_leg: Vec<String> = weights
        .iter()
        .filter(|(_, w)| *w > 0.0)
        .map(|(c, _)| c.clone())
        .collect();
    let short_leg: Vec<String> = weights
        .iter()
        .filter(|(_, w)| *w < 0.0)
        .map(|(c, _)| c.clone())
        .collect();
    let mut plan = Plan {
        equity,
        per_coin,
        long_leg,
        short_leg,
        held: acct
            .positions
            .iter()
            .filter(|(_, p)| p.size.abs() > 1e-12)
            .map(|(c, _)| c.clone())
            .collect(),
        ..Default::default()
    };

    // 错峰调仓：今天只对属于本档的币下单，其余保持现状。
    //
    // 注意 wanted 里必须保留全部目标权重 —— 否则不在本档的持仓会被当成
    // 「目标为 0」而平掉，那就不是在错峰，而是在随机清仓。
    let slices = cfg.rebalance_slices.max(1);
    let day_no = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
        / 86_400_000) as u32;
    let in_slot = |coin: &str| slices <= 1 || (day_no + slice_of(coin, slices)) % slices == 0;

    // wanted: coin -> signed target size（按权重缩放名义）
    let mut wanted: HashMap<String, (f64, bool)> = HashMap::new();
    for (coin, w) in weights {
        if w.abs() < 1e-12 {
            continue;
        }
        let (Some(mid), Some(m)) = (mids.get(coin), markets.get(coin)) else {
            continue;
        };
        let notional = w.abs() * deployable * cfg.leverage;
        let size = round_size(notional / mid, m.sz_decimals);
        if size * mid >= cfg.effective_min_order_usd(acct.equity) {
            wanted.insert(coin.clone(), (if *w > 0.0 { size } else { -size }, *w > 0.0));
        } else {
            plan.notes.push(format!(
                "{coin}: 目标 ${:.0} 低于最小下单额，跳过",
                size * mid
            ));
        }
    }

    // 1) close positions that are gone or must flip side
    let mut closes: Vec<Order> = Vec::new();
    let mut opens: Vec<Order> = Vec::new();
    for (coin, pos) in acct.positions.iter() {
        let cur = pos.size;
        if cur.abs() < 1e-12 {
            continue;
        }
        if !in_slot(coin) {
            continue; // 不在今天的档位，保持现状
        }
        let Some(mid) = mids.get(coin) else {
            plan.notes
                .push(format!("{coin}: 缺价格，无法处理现有仓位 {cur:.6}"));
            continue;
        };
        let Some(m) = markets.get(coin) else {
            plan.notes.push(format!("{coin}: 缺市场元数据"));
            continue;
        };
        let target = wanted.get(coin).map(|w| w.0).unwrap_or(0.0);
        let flip = cur * target < 0.0;
        if target == 0.0 || flip {
            let size = round_size(cur.abs(), m.sz_decimals);
            if size * mid >= cfg.effective_min_order_usd(acct.equity) {
                closes.push(Order {
                    coin: coin.clone(),
                    buy: cur < 0.0, // buy to close a short
                    reduce_only: true,
                    target: 0.0,
                    size,
                    mid: *mid,
                    sz_decimals: m.sz_decimals,
                    notional: size * mid,
                    reason: if flip { "反手平仓".into() } else { "掉出名单平仓".into() },
                });
            } else {
                plan.notes
                    .push(format!("{coin}: 残余仓位 ${:.1} 低于最小下单额，无法平", size * mid));
            }
        } else {
            let delta = target - cur;
            let dsize = round_size(delta.abs(), m.sz_decimals);
            let dnotional = dsize * mid;
            // 复利门槛用「该仓自身的目标名义」算比例带，而不是全组合平均值：
            // 小仓位过去被平均值抬高门槛，长期跟不上净值增长。
            let pos_target = (cur + delta).abs() * mid;
            let threshold = cfg.effective_min_order_usd(acct.equity).max(pos_target * cfg.rebalance_band);
            if dnotional >= threshold {
                opens.push(Order {
                    coin: coin.clone(),
                    buy: delta > 0.0,
                    // 只有「delta 与原仓位反向」才是减仓。之前用 |delta| < |cur|
                    // 判断，会把「加仓」误标成只减仓，实盘会被交易所拒绝。
                    reduce_only: cur * delta < 0.0,
                    target,
                    size: dsize,
                    mid: *mid,
                    sz_decimals: m.sz_decimals,
                    notional: dnotional,
                    // 文案要看「相对原仓位是增还是减」，而不是买/卖方向：
                    // 空头买回是减仓、空头继续卖才是加仓。
                    reason: if cur * delta > 0.0 {
                        "加仓到目标".into()
                    } else {
                        "减仓到目标".into()
                    },
                });
            }
        }
    }

    // 2) open positions that are missing entirely
    for (coin, (target, is_long)) in wanted.iter() {
        if !in_slot(coin) {
            continue; // 新仓位也等轮到它那一档再开
        }
        let cur = acct.positions.get(coin).map(|p| p.size).unwrap_or(0.0);
        let flip = cur * target < 0.0;
        // A flip is handled by the close above; reopen here once it is flat.
        if cur.abs() < 1e-12 || flip {
            let Some(mid) = mids.get(coin) else { continue };
            let Some(m) = markets.get(coin) else { continue };
            let size = round_size(target.abs(), m.sz_decimals);
            if size * mid >= cfg.effective_min_order_usd(acct.equity) {
                opens.push(Order {
                    coin: coin.clone(),
                    buy: *is_long,
                    reduce_only: false,
                    target: if *is_long { size } else { -size },
                    size,
                    mid: *mid,
                    sz_decimals: m.sz_decimals,
                    notional: size * mid,
                    reason: if *is_long { "开多".into() } else { "开空".into() },
                });
            }
        }
    }

    // 真实的目标总名义 = Σ|w| × 可部署 × 杠杆。之前用「开仓单求和」，那是本次
    // 的增量而不是目标敞口，用来展示会误导（平均每仓 × 2 × 腿数同样不准）。
    plan.gross_notional = weights.iter().map(|(_, w)| w.abs()).sum::<f64>()
        * deployable
        * cfg.leverage;
    plan.orders = closes;
    plan.orders.extend(opens);
    plan
}

