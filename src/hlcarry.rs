//! HL 单账户 carry 影子引擎（2026-10）。
//!
//! 策略：**同一账户内**「买现货 + 等名义空永续」，赚取永续资金费。
//! 这是唯一在 HL 上被实测为正的方向（实盘 91 天资金费 +$324 ≈ +22%/年）。
//!
//! 本模块只做三件事：
//!   1. 拉真实行情（`allMids` 一次拿全永续+现货；`metaAndAssetCtxs` 拿资金费）
//!   2. 按固定篮子模拟持仓（不预测、不择时、不调仓）
//!   3. 核算三块损益：资金费、基差、手续费，并记录权益曲线
//!
//! **不发任何真实订单。** 纯影子。
//!
//! 成本口径（HL 实际费率，2026-10）：
//!   - 现货 maker 4.0bp/side，永续 maker 1.44bp/side
//!   - 每腿单边 5.44bp，往返 10.88bp（进场时一次性预扣，保守）
//!
//! 为什么需要它：`funding-carry` 之前的纸面引擎建在 OKX/币安现货上，
//! 而 HL 现在已有流动现货（UBTC/UETH/USOL/XMR1 等，$3~19M/24h），
//! 单账户即可完成，成本从币安的 24bp 降到 10.88bp。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

const HL_INFO: &str = "https://api.hyperliquid.xyz/info";

/// 现货 maker 4bp + 永续 maker 1.44bp，往返 ×2。
pub const ROUND_TRIP_FEE_BP: f64 = 10.88;

/// 篮子里的一个标的：永续币名 + 现货交易对 + 目标权重。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LegSpec {
    pub coin: String,
    pub spot: String,
    pub weight: f64,
}

/// 默认篮子：2026-10-27 实测「现货 24h 额 > $0.5M 且资金费为正」的 9 个里，
/// 取点差最紧、入场基差最有利的 5 个。
pub fn default_basket() -> Vec<LegSpec> {
    vec![
        LegSpec { coin: "XMR".into(), spot: "@260".into(), weight: 0.20 },
        LegSpec { coin: "BTC".into(), spot: "@142".into(), weight: 0.30 },
        LegSpec { coin: "ETH".into(), spot: "@151".into(), weight: 0.20 },
        LegSpec { coin: "SOL".into(), spot: "@156".into(), weight: 0.15 },
        LegSpec { coin: "ZEC".into(), spot: "@272".into(), weight: 0.15 },
    ]
}

/// 一条腿的模拟持仓。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Leg {
    pub coin: String,
    pub spot: String,
    pub weight: f64,
    /// 每腿名义（USD），进场的现货/永续两边各这么多。
    pub notional: f64,
    pub spot_qty: f64,
    pub perp_qty_abs: f64,
    pub spot_entry: f64,
    pub perp_entry: f64,
    /// 进场时记下的年化资金费（%），仅供参考。
    pub entry_funding_ann: f64,
    /// 累计净资金费（正 = 净收）。
    pub funding_accrued: f64,
    /// 基差盈亏（现货币 + 永续腿合计）。
    pub basis_pnl: f64,
    /// 已扣的手续费（往返一次性）。
    pub fee_paid: f64,
    pub last_accrual_ms: i64,
    // ---- 实时行情（每次 step 更新）----
    pub spot_px: f64,
    pub perp_px: f64,
    /// 当前年化资金费（%）。
    pub funding_ann: f64,
    /// 当前基差（bp，正 = 永续升水）。
    pub basis_bp: f64,
    /// 该腿当前合计盈亏 = 资金费 + 基差 − 手续费。
    pub leg_pnl: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CarryLog {
    pub ts: i64,
    pub msg: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HlCarryState {
    pub basket: Vec<LegSpec>,
    pub legs: Vec<Leg>,
    pub budget_usd: f64,
    pub start_equity: f64,
    pub closed_pnl: f64,
    /// (ts, 权益)，每小时一个点。
    pub history: Vec<(i64, f64)>,
    pub log: Vec<CarryLog>,
    pub updated: Option<i64>,
    /// 上次历史采样时间，用于按小时节流。
    #[serde(default)]
    pub last_history_ms: Option<i64>,
}

impl Default for HlCarryState {
    fn default() -> Self {
        HlCarryState {
            basket: default_basket(),
            legs: Vec::new(),
            budget_usd: 0.0,
            start_equity: 0.0,
            closed_pnl: 0.0,
            history: Vec::new(),
            log: Vec::new(),
            updated: None,
            last_history_ms: None,
        }
    }
}

impl HlCarryState {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(path, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("写入 {}", path.display()))?;
        Ok(())
    }

    /// 当前总权益 = 期初 + 各腿盈亏 + 已平仓盈亏。
    pub fn equity(&self) -> f64 {
        self.start_equity + self.legs.iter().map(|l| l.leg_pnl).sum::<f64>() + self.closed_pnl
    }

    /// 年化收益率（按已运行时长折算）。不足 1 小时返回 None。
    pub fn annualized_pct(&self) -> Option<f64> {
        let first = *self.history.first()?;
        let last = *self.history.last()?;
        let hours = (last.0 - first.0) as f64 / 3_600_000.0;
        if hours < 1.0 || self.start_equity <= 0.0 {
            return None;
        }
        let ret = self.equity() / self.start_equity - 1.0;
        Some(ret * (365.0 * 24.0 / hours) * 100.0)
    }

    fn log(&mut self, now: i64, msg: String) {
        self.log.push(CarryLog { ts: now, msg });
        if self.log.len() > 300 {
            let drop = self.log.len() - 300;
            self.log.drain(0..drop);
        }
    }

    /// 按预算开仓：每腿名义 = 预算 × 权重，现货与永续等名义。
    pub fn open(&mut self, budget_usd: f64, market: &Market, now: i64) {
        self.legs.clear();
        self.closed_pnl = 0.0;
        self.history.clear();
        self.budget_usd = budget_usd;
        self.start_equity = budget_usd;
        self.last_history_ms = Some(now);

        for spec in self.basket.clone() {
            let Some(q) = market.quote(&spec.coin, &spec.spot) else {
                self.log(now, format!("{} 行情缺失，跳过", spec.coin));
                continue;
            };
            let notional = budget_usd * spec.weight;
            if notional <= 0.0 || q.spot <= 0.0 || q.perp <= 0.0 {
                continue;
            }
            let fee = notional * ROUND_TRIP_FEE_BP / 10_000.0;
            self.legs.push(Leg {
                coin: spec.coin.clone(),
                spot: spec.spot.clone(),
                weight: spec.weight,
                notional,
                spot_qty: notional / q.spot,
                perp_qty_abs: notional / q.perp,
                spot_entry: q.spot,
                perp_entry: q.perp,
                entry_funding_ann: q.funding_ann,
                funding_accrued: 0.0,
                basis_pnl: 0.0,
                fee_paid: fee,
                last_accrual_ms: now,
                spot_px: q.spot,
                perp_px: q.perp,
                funding_ann: q.funding_ann,
                basis_bp: (q.perp - q.spot) / q.spot * 10_000.0,
                leg_pnl: -fee,
            });
        }
        let n = self.legs.len();
        self.log(now, format!("开仓：{} 腿，总名义 ${:.0}", n, budget_usd));
    }

    /// 推进一步：计提资金费、盯市、记录权益。
    pub fn step(&mut self, market: &Market, now: i64) {
        self.updated = Some(now);
        for leg in self.legs.iter_mut() {
            let Some(q) = market.quote(&leg.coin, &leg.spot) else {
                continue; // 行情缺失：本轮跳过，保留上次状态
            };
            if !(q.spot > 0.0 && q.perp > 0.0 && q.spot.is_finite() && q.perp.is_finite()) {
                continue;
            }
            // ① 资金费：做空永续，正费率则收钱。dt 按小时折算。
            let dt_hours = ((now - leg.last_accrual_ms).max(0) as f64) / 3_600_000.0;
            if dt_hours > 0.0 && q.funding_hourly.is_finite() {
                leg.funding_accrued += leg.notional * q.funding_hourly * dt_hours;
                leg.last_accrual_ms = now;
            }
            // ② 盯市
            leg.spot_px = q.spot;
            leg.perp_px = q.perp;
            leg.funding_ann = q.funding_ann;
            leg.basis_bp = (q.perp - q.spot) / q.spot * 10_000.0;
            leg.basis_pnl =
                (q.spot - leg.spot_entry) * leg.spot_qty + (leg.perp_entry - q.perp) * leg.perp_qty_abs;
            leg.leg_pnl = leg.basis_pnl + leg.funding_accrued - leg.fee_paid;
        }

        // ③ 权益曲线：每小时一个点
        let should = match self.last_history_ms {
            None => true,
            Some(t) => now - t >= 3_600_000,
        };
        if should {
            self.last_history_ms = Some(now);
            let eq = self.equity();
            self.history.push((now, eq));
            if self.history.len() > 24 * 400 {
                let drop = self.history.len() - 24 * 400;
                self.history.drain(0..drop);
            }
        }
    }

    /// 告警：资金费转负或基差恶化到需要人工判断的程度。
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        for leg in &self.legs {
            if leg.funding_ann < 0.0 {
                out.push(format!(
                    "{} 资金费转负（{:.2}%/年）——这条腿在付钱",
                    leg.coin, leg.funding_ann
                ));
            } else if leg.funding_ann < 3.0 {
                out.push(format!("{} 资金费偏低（{:.2}%/年）", leg.coin, leg.funding_ann));
            }
            if leg.basis_bp < -30.0 {
                out.push(format!("{} 永续贴水 {:.0}bp——基差不利", leg.coin, leg.basis_bp));
            }
        }
        out
    }
}

/// 一个标的的实时报价。
#[derive(Clone, Debug)]
pub struct Quote {
    pub spot: f64,
    pub perp: f64,
    /// 每小时资金费（小数，例如 0.0000125）。
    pub funding_hourly: f64,
    /// 年化资金费（%）。
    pub funding_ann: f64,
}

/// 一次抓取的全市场快照。
#[derive(Clone, Debug, Default)]
pub struct Market {
    /// 币名 -> 永续中间价
    pub perp_mid: std::collections::HashMap<String, f64>,
    /// 现货对名（"@142" 等）-> 中间价
    pub spot_mid: std::collections::HashMap<String, f64>,
    /// 币名 -> 每小时资金费
    pub funding: std::collections::HashMap<String, f64>,
}

impl Market {
    pub fn quote(&self, coin: &str, spot: &str) -> Option<Quote> {
        let perp = *self.perp_mid.get(coin)?;
        let sp = *self.spot_mid.get(spot)?;
        let fh = *self.funding.get(coin).unwrap_or(&0.0);
        Some(Quote {
            spot: sp,
            perp,
            funding_hourly: fh,
            funding_ann: fh * 24.0 * 365.0 * 100.0,
        })
    }
}

async fn post(http: &reqwest::Client, payload: Value) -> Result<Value> {
    let resp = http
        .post(HL_INFO)
        .json(&payload)
        .send()
        .await
        .context("HL info 请求失败")?;
    let v: Value = resp.json().await.context("HL info 解析失败")?;
    Ok(v)
}

/// 拉一次全市场快照：2 个公开请求。
pub async fn fetch_market(http: &reqwest::Client) -> Result<Market> {
    let mids = post(http, json!({"type": "allMids"})).await?;
    let ctxs = post(http, json!({"type": "metaAndAssetCtxs"})).await?;
    Ok(parse_market(&mids, &ctxs))
}

/// 纯解析：把两个接口的原始 JSON 变成 Market。抽出来是为了能用真实报文做单测
/// （本机 TLS 环境异常，无法直连 HL）。
pub fn parse_market(mids: &Value, ctxs: &Value) -> Market {
    let mut m = Market::default();
    if let Some(obj) = mids.as_object() {
        for (k, v) in obj {
            let Some(px) = v.as_str().and_then(|s| s.parse::<f64>().ok()) else {
                continue;
            };
            if !(px > 0.0) {
                continue;
            }
            if k.starts_with('@') || k.contains('/') {
                m.spot_mid.insert(k.clone(), px);
            } else {
                // 永续（含 HIP-3 的 "#N" 之类，用不到但不影响）
                m.perp_mid.insert(k.clone(), px);
            }
        }
    }
    if let Some(arr) = ctxs.as_array() {
        if arr.len() > 1 {
            if let (Some(meta), Some(list)) = (arr[0].get("universe"), arr[1].as_array()) {
                for (u, c) in meta.as_array().unwrap_or(&vec![]).iter().zip(list.iter()) {
                    let Some(coin) = u.get("name").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    let f = c
                        .get("funding")
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(0.0);
                    m.funding.insert(coin.to_string(), f);
                }
            }
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn market_with(spot: f64, perp: f64, fh: f64) -> Market {
        let mut m = Market::default();
        m.spot_mid.insert("@142".into(), spot);
        m.perp_mid.insert("BTC".into(), perp);
        m.funding.insert("BTC".into(), fh);
        m
    }

    #[test]
    fn open_then_accrue_funding() {
        let mut st = HlCarryState::default();
        st.basket = vec![LegSpec { coin: "BTC".into(), spot: "@142".into(), weight: 1.0 }];
        let m = market_with(80_000.0, 80_000.0, 0.000_012_5); // 10.95%/年
        st.open(1_000.0, &m, 1_000_000);
        assert_eq!(st.legs.len(), 1);
        let fee = st.legs[0].fee_paid;
        assert!((fee - 1_000.0 * ROUND_TRIP_FEE_BP / 10_000.0).abs() < 1e-9);
        // 10 小时后：应收到 10h 的资金费
        st.step(&m, 1_000_000 + 10 * 3_600_000);
        let expect = 1_000.0 * 0.000_012_5 * 10.0;
        assert!((st.legs[0].funding_accrued - expect).abs() < 1e-9);
        // 价格不动时，基差盈亏 ≈ 0
        assert!(st.legs[0].basis_pnl.abs() < 1e-6);
        assert!((st.equity() - (1_000.0 + expect - fee)).abs() < 1e-6);
    }

    #[test]
    fn basis_pnl_offsets_price_move() {
        let mut st = HlCarryState::default();
        st.basket = vec![LegSpec { coin: "BTC".into(), spot: "@142".into(), weight: 1.0 }];
        let m0 = market_with(80_000.0, 80_000.0, 0.0);
        st.open(1_000.0, &m0, 1_000_000);
        // 价格 +10%，现货赚、空永续亏，净≈0
        let m1 = market_with(88_000.0, 88_000.0, 0.0);
        st.step(&m1, 1_000_000 + 3_600_000);
        assert!(st.legs[0].basis_pnl.abs() < 1e-6, "delta 中性后基差盈亏应≈0");
    }

    #[test]
    fn parses_real_api_payload() {
        // 2026-10-27 从 HL 实际抓到的报文（截取相关字段）
        let mids = serde_json::json!({
            "BTC": "79948.5", "ETH": "2313.45", "XMR": "592.885",
            "@142": "79871.5", "@151": "2311.25", "@260": "591.63",
            "#114910": "0.5"
        });
        let ctxs = serde_json::json!([
            {"universe": [{"name": "BTC"}, {"name": "ETH"}, {"name": "XMR"}]},
            [{"funding": "0.0000125"}, {"funding": "0.0000125"}, {"funding": "0.0000477"}]
        ]);
        let m = parse_market(&mids, &ctxs);
        // 永续和现货要分到不同 map
        assert_eq!(m.perp_mid.get("BTC"), Some(&79948.5));
        assert_eq!(m.spot_mid.get("@142"), Some(&79871.5));
        assert!(!m.perp_mid.contains_key("@142"));
        let q = m.quote("BTC", "@142").expect("BTC 报价");
        assert!((q.funding_ann - 10.95).abs() < 0.01, "10.95%/年");
        let qx = m.quote("XMR", "@260").expect("XMR 报价");
        assert!(qx.funding_ann > 40.0, "XMR 40%+ 应被识别");
        // 缺现货对时不能给出报价（防止拿 0 价开仓）
        assert!(m.quote("BTC", "@999").is_none());
    }

    #[test]
    fn negative_funding_triggers_warning() {
        let mut st = HlCarryState::default();
        st.basket = vec![LegSpec { coin: "BTC".into(), spot: "@142".into(), weight: 1.0 }];
        let m = market_with(80_000.0, 80_000.0, -0.000_01);
        st.open(1_000.0, &m, 1_000_000);
        assert!(!st.warnings().is_empty());
    }
}
