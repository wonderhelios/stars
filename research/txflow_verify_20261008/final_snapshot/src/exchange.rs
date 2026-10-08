//! Minimal Hyperliquid execution layer for the momentum strategy.
//!
//! Read-only by default (`Exec::reader`); a signing client is only built when an
//! account + API-wallet key are supplied (`Exec::signer`). Order placement is
//! adapted from the same coding patterns used by the hyper-fly bot.

use alloy::{primitives::Address, signers::local::PrivateKeySigner};
use anyhow::{anyhow, bail, Context, Result};
use hyperliquid_rust_sdk::{
    BaseUrl, ClientCancelRequest, ClientLimit, ClientOrder, ClientOrderRequest, ExchangeClient,
    ExchangeDataStatus, ExchangeResponseStatus,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;
use uuid::Uuid;

#[derive(Clone, Copy, Debug)]
pub struct MarketInfo {
    pub sz_decimals: i32,
    pub max_leverage: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Pos {
    /// signed size (negative = short)
    pub size: f64,
    pub entry_px: f64,
    /// exchange-reported liquidation price, if any
    pub liq_px: Option<f64>,
    /// true = cross margin, false = isolated
    pub is_cross: bool,
    pub leverage: u32,
    /// 交易所报的仓位绝对值（USD）。这是「标记价 × 数量」，永远存在 ——
    /// 行情接口偶尔读不到某个币时，靠它反推标记价，而不是退回开仓价。
    pub position_value: f64,
    /// 交易所报的未实现盈亏。同样是权威值，不依赖我们自己的行情缓存。
    pub unrealized_pnl: f64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct OpenOrder {
    pub oid: u64,
    pub coin: String,
    pub side: String,
    pub px: f64,
    pub sz: f64,
    pub reduce_only: bool,
    /// 我们的止盈单都带固定 cloid 前缀，用来把「自己的单」和用户手动挂的单区分开。
    pub cloid: Option<String>,
}

/// 本程序挂出的订单统一使用的 cloid 前缀（Hyperliquid 要求 0x + 32 位十六进制）。
/// 撤单时只撤带这个前缀的，不会误撤用户手动挂的单。
const STARS_CLOID_PREFIX: &str = "0x5741";

/// 生成一个可识别归属的 cloid。SDK 要求 Uuid，所以把前两个字节固定成 'W''A'，
/// 序列化后就是 0x5741 开头的十六进制串。
fn stars_cloid() -> Uuid {
    let mut b = *Uuid::new_v4().as_bytes();
    b[0] = 0x57;
    b[1] = 0x41;
    Uuid::from_bytes(b)
}

/// 这个挂单是不是我们自己挂的。
pub fn is_ours(cloid: Option<&str>) -> bool {
    cloid.map(|c| c.starts_with(STARS_CLOID_PREFIX)).unwrap_or(false)
}

#[derive(Clone, Debug, Default)]
pub struct Acct {
    pub equity: f64,
    pub positions: HashMap<String, Pos>,
}

pub struct Exec {
    txflow: Option<crate::txflow::Client>,
    http: reqwest::Client,
    trading: Option<ExchangeClient>,
    account: Option<Address>,
}

impl Exec {
    #[cfg(test)]
    pub fn test_txflow(client: crate::txflow::Client) -> Self {
        Self { txflow:Some(client), http:reqwest::Client::new(), trading:None, account:None }
    }
    pub fn is_txflow(&self) -> bool { self.txflow.is_some() }
    pub async fn txflow_prices(&self, coins: &[String]) -> Option<Result<HashMap<String, f64>>> {
        if let Some(tx) = &self.txflow { Some(tx.prices(coins).await) } else { None }
    }
    pub async fn txflow_maintenance_margin(&self) -> Option<Result<f64>> {
        if let Some(tx) = &self.txflow { Some(tx.maintenance_margin().await) } else { None }
    }
    pub async fn signer_config(cfg: &crate::live::LiveConfig) -> Result<Self> {
        if cfg.txflow { Self::txflow(&cfg.account, Some(Path::new(&cfg.key_path))).await }
        else { Self::signer(&cfg.account, Path::new(&cfg.key_path)).await }
    }
    pub async fn txflow(account: &str, key: Option<&Path>) -> Result<Self> {
        let client = crate::txflow::Client::new(account, key).await?;
        Ok(Self { txflow: Some(client), http: reqwest::Client::new(), trading: None, account: None })
    }

    async fn info_post(&self, body: Value) -> Result<Value> {
        let mut last: Option<anyhow::Error> = None;
        for attempt in 0..5u32 {
            match self
                .http
                .post("https://api.hyperliquid.xyz/info")
                .json(&body)
                .send()
                .await
            {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        return Ok(resp.json().await?);
                    }
                    // Hyperliquid returns a plain 429 when the IP is over budget.
                    if status.as_u16() == 429 {
                        tokio::time::sleep(Duration::from_millis(
                            500 * 2u64.pow(attempt),
                        ))
                        .await;
                        last = Some(anyhow!("请求被限流 (429)，已重试 {attempt} 次"));
                        continue;
                    }
                    last = Some(anyhow!("HTTP {status}"));
                }
                Err(e) => last = Some(anyhow!("请求失败: {e}")),
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Err(last.unwrap_or_else(|| anyhow!("info 请求失败")))
    }

    pub async fn reader() -> Result<Self> {
        Self::reader_for(None).await
    }

    /// Read-only client bound to an account so equity/positions can be read
    /// without any signing key (used by dry runs).
    /// Build with a shared client so connections are reused across requests.
    pub async fn reader_shared(http: reqwest::Client, account: Option<&str>) -> Result<Self> {
        let account = match account {
            Some(a) => Some(Address::from_str(a).context("invalid HL_ACCOUNT_ADDRESS")?),
            None => None,
        };
        Ok(Self {
            txflow: None,
            http,
            trading: None,
            account,
        })
    }

    pub async fn reader_config(cfg: &crate::live::LiveConfig) -> Result<Self> {
        if cfg.txflow {Self::txflow(&cfg.account,None).await}
        else {Self::reader_for(Some(&cfg.account)).await}
    }

    pub async fn reader_for(account: Option<&str>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()?;
        let account = match account {
            Some(a) => Some(Address::from_str(a).context("invalid HL_ACCOUNT_ADDRESS")?),
            None => None,
        };
        Ok(Self {
            txflow: None,
            http,
            trading: None,
            account,
        })
    }

    /// Build a signing client from an authorized API wallet (never the main key).
    pub async fn signer(account: &str, key_path: &Path) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()?;
        let address = Address::from_str(account).context("invalid account address")?;
        let key = std::fs::read_to_string(key_path)
            .with_context(|| format!("read {}", key_path.display()))?
            .trim()
            .to_string();
        let wallet = PrivateKeySigner::from_str(&key).context("invalid API wallet key")?;
        anyhow::ensure!(
            wallet.address() != address,
            "refusing to use the main wallet key; supply an authorized API wallet"
        );
        let trading = tokio::time::timeout(
            Duration::from_secs(40),
            ExchangeClient::new(None, wallet, Some(BaseUrl::Mainnet), None, None),
        )
        .await
        .context("exchange client init timed out")??;
        Ok(Self {
            txflow: None,
            http,
            trading: Some(trading),
            account: Some(address),
        })
    }

    /// Coin -> size decimals / max leverage for the main DEX.
    pub async fn markets(&self) -> Result<HashMap<String, MarketInfo>> {
        let raw = self.info_post(json!({"type": "metaAndAssetCtxs"})).await?;
        let universe = raw[0]["universe"]
            .as_array()
            .context("bad meta response")?;
        let mut out = HashMap::new();
        for m in universe {
            let Some(name) = m["name"].as_str() else {
                continue;
            };
            if m["isDelisted"].as_bool().unwrap_or(false) {
                continue;
            }
            out.insert(
                name.to_string(),
                MarketInfo {
                    sz_decimals: m["szDecimals"].as_i64().unwrap_or(4) as i32,
                    max_leverage: m["maxLeverage"].as_u64().unwrap_or(10) as u32,
                },
            );
        }
        anyhow::ensure!(!out.is_empty(), "empty market universe");
        Ok(out)
    }

    /// Every mid price in one request (avoids one l2 call per coin).
    pub async fn all_mids(&self) -> Result<HashMap<String, f64>> {
        if let Some(tx) = &self.txflow { return tx.all_mids().await; }
        let v = self.info_post(json!({"type": "allMids"})).await?;
        let mut out = HashMap::new();
        if let Some(obj) = v.as_object() {
            for (coin, val) in obj {
                if coin.contains('@') {
                    continue;
                }
                if let Some(p) = val.as_str().and_then(|x| x.parse::<f64>().ok()) {
                    if p > 0.0 {
                        out.insert(coin.clone(), p);
                    }
                }
            }
        }
        anyhow::ensure!(!out.is_empty(), "empty allMids response");
        Ok(out)
    }

    pub async fn account(&self) -> Result<Acct> {
        if let Some(tx) = &self.txflow { return tx.account().await; }
        let account = self.account.context("read-only client has no account")?;
        let addr = account.to_string();
        // Both views in one round trip.
        let (perp_res, spot_res) = tokio::join!(
            self.info_post(json!({"type": "clearinghouseState", "user": addr})),
            self.info_post(json!({"type": "spotClearinghouseState", "user": addr}))
        );
        let perp = perp_res?;
        let mut equity: f64 = perp["marginSummary"]["accountValue"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        // A unified account keeps its collateral in the spot balance, so the
        // perp view alone reads 0. Retry rather than silently reporting zero.
        let mut last_err: Option<anyhow::Error> = None;
        let mut spot_opt = spot_res.ok();
        for attempt in 0..2 {
            let Some(spot) = spot_opt.take() else {
                match self
                    .info_post(json!({"type": "spotClearinghouseState", "user": addr}))
                    .await
                {
                    Ok(v) => spot_opt = Some(v),
                    Err(e) => {
                        last_err = Some(e);
                        if attempt == 0 {
                            tokio::time::sleep(Duration::from_millis(400)).await;
                        }
                        continue;
                    }
                }
                continue;
            };
            let usdc: f64 = spot["balances"]
                .as_array()
                .map(|b| {
                    b.iter()
                        .filter(|x| x["coin"].as_str() == Some("USDC"))
                        .filter_map(|x| x["total"].as_str()?.parse::<f64>().ok())
                        .sum()
                })
                .unwrap_or(0.0);
            if usdc > equity {
                equity = usdc;
            }
            last_err = None;
            break;
        }
        if let Some(e) = last_err {
            if equity <= 0.0 {
                return Err(e).context("读取账户余额失败（现货接口）");
            }
        }
        let mut positions = HashMap::new();
        if let Some(arr) = perp["assetPositions"].as_array() {
            for item in arr {
                let p = &item["position"];
                let Some(coin) = p["coin"].as_str() else { continue };
                let size = p["szi"].as_str().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
                if size.abs() <= 1e-10 {
                    continue;
                }
                positions.insert(
                    coin.to_string(),
                    Pos {
                        size,
                        entry_px: p["entryPx"]
                            .as_str()
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0.0),
                        liq_px: p["liquidationPx"].as_str().and_then(|v| v.parse().ok()),
                        is_cross: p["leverage"]["type"].as_str() == Some("cross"),
                        leverage: p["leverage"]["value"].as_u64().unwrap_or(0) as u32,
                        position_value: p["positionValue"]
                            .as_str()
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0.0),
                        unrealized_pnl: p["unrealizedPnl"]
                            .as_str()
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0.0),
                    },
                );
            }
        }
        Ok(Acct { equity, positions })
    }

    /// 设置杠杆并强制使用**全仓**（cross）保证金。
    ///
    /// 多空对冲组合必须用全仓：逐仓下每个仓位独立结算，一个币走反 20~30%
    /// 就会被单独强平，而组合整体其实还在盈亏相抵。第三个参数是 is_cross，
    /// 传 false 会变成逐仓（这里曾经写错成 false）。
    pub async fn set_leverage(&self, coin: &str, leverage: u32) -> Result<()> {
        if let Some(tx) = &self.txflow { return tx.set_leverage(coin, leverage).await; }
        let trading = self.trading.as_ref().context("not a signing client")?;
        let resp = sdk_retry(&format!("设置 {coin} 杠杆"), || {
            trading.update_leverage(leverage, coin, true, None)
        })
        .await?;
        match resp {
            ExchangeResponseStatus::Ok(_) => Ok(()),
            ExchangeResponseStatus::Err(e) => bail!("leverage rejected for {coin}: {e}"),
        }
    }

    /// 挂一个被动限价单（Gtc，留在盘口）用于止盈：只减仓。
    /// 返回交易所订单号，便于之后撤销。
    pub async fn resting_reduce_order(
        &self,
        coin: &str,
        buy: bool,
        size: f64,
        px: f64,
    ) -> Result<u64> {
        if let Some(tx) = &self.txflow { return tx.resting_reduce_order(coin, buy, size, px).await; }
        let trading = self.trading.as_ref().context("not a signing client")?;
        anyhow::ensure!(px > 0.0 && size > 0.0, "invalid resting order {coin}");
        let cloid = stars_cloid();
        let coin_owned = coin.to_string();
        let resp = sdk_retry(&format!("挂止盈单 {coin}"), || {
            let order = ClientOrderRequest {
                asset: coin_owned.clone(),
                is_buy: buy,
                reduce_only: true,
                limit_px: px,
                sz: size,
                cloid: Some(cloid),
                // Gtc = 一直挂在盘口，等价格来碰（被动成交 = maker 费率）
                order_type: ClientOrder::Limit(ClientLimit { tif: "Gtc".into() }),
            };
            trading.order(order, None)
        })
        .await?;
        match one_status(resp)? {
            ExchangeDataStatus::Resting(r) => Ok(r.oid),
            ExchangeDataStatus::Filled(_) => Ok(0),
            other => bail!("止盈单未挂上 {coin}: {other:?}"),
        }
    }

    /// TxFlow 的真实业绩汇总（已实现/手续费/净入金）。非 TxFlow 时返回 None。
    pub async fn txflow_pnl(&self, since_ms: i64) -> Option<anyhow::Result<crate::txflow::PnlSummary>> {
        match &self.txflow {
            Some(tx) => Some(tx.pnl_summary(since_ms).await),
            None => None,
        }
    }

    /// 最近的成交明细。止盈单是被动挂单、由交易所自动成交的，程序不经手，
    /// 所以必须回头拉成交才能把这块盈亏算进来。
    pub async fn user_fills(&self, limit: usize) -> Result<serde_json::Value> {
        if let Some(tx) = &self.txflow { return tx.user_fills(limit).await; }
        let account = self.account.context("read-only client has no account")?;
        self.info_post(json!({
            "type": "userFills",
            "user": account.to_string(),
            "aggregateByTime": false
        }))
        .await
        .map(|v| {
            let _ = limit;
            v
        })
    }

    /// 挂单详情，给前端展示。
    pub async fn open_order_details(&self) -> Result<Vec<OpenOrder>> {
        if let Some(tx) = &self.txflow { return tx.open_orders().await; }
        let account = self.account.context("read-only client has no account")?;
        let v = self
            .info_post(json!({"type": "openOrders", "user": account.to_string()}))
            .await?;
        let mut out = Vec::new();
        if let Some(arr) = v.as_array() {
            for o in arr {
                let coin = o["coin"].as_str().unwrap_or("").to_string();
                if coin.is_empty() {
                    continue;
                }
                out.push(OpenOrder {
                    oid: o["oid"].as_u64().unwrap_or(0),
                    coin,
                    side: if o["side"].as_str() == Some("B") { "买" } else { "卖" }.into(),
                    px: o["limitPx"].as_str().and_then(|x| x.parse().ok()).unwrap_or(0.0),
                    sz: o["sz"].as_str().and_then(|x| x.parse().ok()).unwrap_or(0.0),
                    // openOrders 不一定返回 reduceOnly。本程序每次调仓都先撤光
                    // 所有挂单，且只挂只减仓的止盈单，所以盘口上剩下的就是止盈单。
                    reduce_only: o["reduceOnly"].as_bool().unwrap_or(true),
                    cloid: o["cloid"].as_str().map(|c| c.to_string()),
                });
            }
        }
        Ok(out)
    }

    /// 批量撤单。返回成功撤销的数量。
    pub async fn cancel_orders(&self, orders: &[(String, u64)]) -> Result<usize> {
        if let Some(tx) = &self.txflow { return tx.cancel_orders(orders).await; }
        if orders.is_empty() {
            return Ok(0);
        }
        let trading = self.trading.as_ref().context("not a signing client")?;
        let mut n = 0usize;
        for chunk in orders.chunks(20) {
            // ClientCancelRequest 不实现 Clone，所以在闭包里每次重建
            let part: Vec<(String, u64)> = chunk.to_vec();
            let resp = sdk_retry("撤单", || {
                let reqs: Vec<ClientCancelRequest> = part
                    .iter()
                    .map(|(coin, oid)| ClientCancelRequest {
                        asset: coin.clone(),
                        oid: *oid,
                    })
                    .collect();
                trading.bulk_cancel(reqs, None)
            })
            .await?;
            match resp {
                // 顶层 Ok 不代表每个撤单都成功：官方文档明确说订单/撤单的错误
                // 放在逐项向量里。只看顶层会把"没撤掉"当成"撤掉了"，然后继续
                // 挂新单 —— 旧止盈单还留在盘口，会在新仓建好后被触发、破坏对冲。
                ExchangeResponseStatus::Ok(r) => {
                    let Some(d) = r.data else {
                        bail!("撤单响应缺少 data，无法确认 {} 笔是否已撤", chunk.len());
                    };
                    if d.statuses.len() != chunk.len() {
                        bail!(
                            "撤单响应条数不符：请求 {} 笔、返回 {} 条",
                            chunk.len(),
                            d.statuses.len()
                        );
                    }
                    let mut bad: Vec<String> = Vec::new();
                    let mut ok = 0usize;
                    for (i, st) in d.statuses.iter().enumerate() {
                        match st {
                            ExchangeDataStatus::Success => ok += 1,
                            // MissingOrder：已成交或已被撤，不是失败，但也不算成功
                            ExchangeDataStatus::Error(e)
                                if e.to_ascii_lowercase().contains("missing") => {}
                            ExchangeDataStatus::Error(e) => {
                                bad.push(format!("{}: {e}", chunk[i].0));
                            }
                            other => bad.push(format!("{}: 意外状态 {other:?}", chunk[i].0)),
                        }
                    }
                    if !bad.is_empty() {
                        bail!("部分撤单失败（{} 笔）: {}", bad.len(), bad.join("; "));
                    }
                    n += ok;
                }
                ExchangeResponseStatus::Err(e) => bail!("撤单失败: {e}"),
            }
        }
        Ok(n)
    }

    /// Immediate-or-cancel order with a slippage cap around `mid`.
    #[allow(clippy::too_many_arguments)]
    pub async fn ioc(
        &self,
        coin: &str,
        buy: bool,
        reduce_only: bool,
        size: f64,
        mid: f64,
        slippage: f64,
        sz_decimals: i32,
    ) -> Result<ExchangeDataStatus> {
        if let Some(tx) = &self.txflow { return tx.ioc(coin, buy, reduce_only, size, mid, slippage).await; }
        let trading = self.trading.as_ref().context("not a signing client")?;
        let aggressive = if buy {
            mid * (1.0 + slippage)
        } else {
            mid * (1.0 - slippage)
        };
        let mut px = order_price(aggressive, sz_decimals, !buy);
        if px <= 0.0 {
            // Very low-priced coins can round down to zero; step up one tick
            // instead of failing outright.
            px = order_price(aggressive, sz_decimals, true);
        }
        anyhow::ensure!(px > 0.0, "无法为 {coin} 构造有效价格（mid={mid}）");
        // cloid 在重试间保持不变，避免重复下单被当成两笔
        let cloid = Uuid::new_v4();
        let coin_owned = coin.to_string();
        let resp = sdk_retry(&format!("下单 {coin}"), || {
            let order = ClientOrderRequest {
                asset: coin_owned.clone(),
                is_buy: buy,
                reduce_only,
                limit_px: px,
                sz: size,
                cloid: Some(cloid),
                order_type: ClientOrder::Limit(ClientLimit { tif: "Ioc".into() }),
            };
            trading.order(order, None)
        })
        .await?;
        one_status(resp)
    }
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

/// Truncate a size to the coin's lot size.
pub fn round_size(size: f64, sz_decimals: i32) -> f64 {
    if !size.is_finite() || size <= 0.0 {
        return 0.0;
    }
    let factor = 10_f64.powi(sz_decimals);
    (size * factor).floor() / factor
}

/// SDK 调用重试：Hyperliquid 在配额紧张时返回 429，SDK 会把原始错误抛出来。
/// 下单和设杠杆都必须重试，否则用户看到的就是一句 429。
async fn sdk_retry<T, F, Fut>(label: &str, mut f: F) -> Result<T>
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

fn one_status(value: ExchangeResponseStatus) -> Result<ExchangeDataStatus> {
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
