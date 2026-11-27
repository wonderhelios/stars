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
    http: reqwest::Client,
    trading: Option<ExchangeClient>,
    account: Option<Address>,
}

/// 解析地址：容忍 "HL:0x..." 前缀（HL 网页复制常带 ✗ 直接 from_str 会失败）。
fn parse_addr(s: &str) -> Result<Address> {
    let t = s.trim();
    let t = t.strip_prefix("HL:").or_else(|| t.strip_prefix("hl:")).unwrap_or(t);
    Address::from_str(t).context("invalid HL_ACCOUNT_ADDRESS")
}

impl Exec {
    pub async fn signer_config(cfg: &crate::live::LiveConfig) -> Result<Self> {
        Self::signer(
            &cfg.account,
            Path::new(&cfg.key_path),
            cfg.vault_address.as_deref(),
        )
        .await
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
            Some(a) => Some(parse_addr(a)?),
            None => None,
        };
        Ok(Self {
            http,
            trading: None,
            account,
        })
    }

    pub async fn reader_config(cfg: &crate::live::LiveConfig) -> Result<Self> {
        // 有子账户就查子账户，否则查主账户
        let addr = cfg.vault_address.as_deref().unwrap_or(&cfg.account);
        Self::reader_for(Some(addr)).await
    }

    pub async fn reader_for(account: Option<&str>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()?;
        let account = match account {
            Some(a) => Some(parse_addr(a)?),
            None => None,
        };
        Ok(Self {
            http,
            trading: None,
            account,
        })
    }

    /// Build a signing client from an authorized API wallet (never the main key).
    /// `vault_address` 是子账户地址：下单和查询都作用在子账户上（None = 主账户）。
    pub async fn signer(account: &str, key_path: &Path, vault_address: Option<&str>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()?;
        let address = parse_addr(account).context("invalid account address")?;
        let key = std::fs::read_to_string(key_path)
            .with_context(|| format!("read {}", key_path.display()))?
            .trim()
            .to_string();
        let wallet = PrivateKeySigner::from_str(&key).context("invalid API wallet key")?;
        anyhow::ensure!(
            wallet.address() != address,
            "refusing to use the main wallet key; supply an authorized API wallet"
        );
        // 子账户地址（None = 主账户）
        let vault = vault_address
            .map(|v| parse_addr(v).context("invalid vault_address"))
            .transpose()?;
        let trading = tokio::time::timeout(
            Duration::from_secs(40),
            ExchangeClient::new(None, wallet, Some(BaseUrl::Mainnet), None, vault),
        )
        .await
        .context("exchange client init timed out")??;
        // 查询余额/仓位时，有子账户就查子账户
        let query_addr = vault.unwrap_or(address);
        Ok(Self {
            http,
            trading: Some(trading),
            account: Some(query_addr),
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
        let account = self.account.context("read-only client has no account")?;
        let addr = account.to_string();
        // Both views in one round trip.
        let (perp_res, spot_res) = tokio::join!(
            self.info_post(json!({"type": "clearinghouseState", "user": addr})),
            self.info_post(json!({"type": "spotClearinghouseState", "user": addr}))
        );
        let perp = perp_res?;
        // 响应缺字段/格式错误必须失败，不能回退成 0 后再用现货余额冒充账户总净值。
        let mut equity = parse_perp_equity(&perp)?;
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
            // Unified accounts can have most collateral in spot and only a small
            // positive perp subaccount value. Never silently size from that partial value.
            return Err(e).context("读取统一账户总余额失败（现货接口），拒绝使用永续分账户净值");
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

    /// 最近的成交明细。止盈单是被动挂单、由交易所自动成交的，程序不经手，
    /// 所以必须回头拉成交才能把这块盈亏算进来。
    pub async fn user_fills(&self, limit: usize) -> Result<serde_json::Value> {
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

    /// 先挂被动限价单（Alo / Post-Only）吃 Maker 费率（~0.015%），超时未成交再撤单改用 IOC 补齐。
    ///
    /// - 挂单价严格位于 `mid` 被动一侧至少 `max(1bp, 1 tick)`（买单 `< mid`，卖单 `> mid`），
    ///   避免在 1-tick 价差盘口上直接挂在 `mid` 触发 Post-Only 立即拒单；
    /// - 最多进行 1–2 次快速被动重挂（刷新实时 `mid` 或按 tick 退让），最后残余再回退到 `ioc`；
    /// - 撤单返回 `Ok(0)` 说明刚巧成交，绝不重复补单。
    #[allow(clippy::too_many_arguments)]
    pub async fn maker_then_ioc(
        &self,
        coin: &str,
        buy: bool,
        reduce_only: bool,
        size: f64,
        mid: f64,
        slippage: f64,
        sz_decimals: i32,
        wait_secs: u64,
    ) -> Result<String> {
        let trading = self.trading.as_ref().context("not a signing client")?;
        if !mid.is_finite() || mid <= 0.0 || wait_secs == 0 {
            let st = self
                .ioc(coin, buy, reduce_only, size, mid, slippage, sz_decimals)
                .await?;
            return Ok(describe(&st));
        }

        const MAX_ATTEMPTS: usize = 3; // 1 次初始挂单 + 最多 2 次快速被动重挂
        let total_deadline = tokio::time::Instant::now() + Duration::from_secs(wait_secs);
        let coin_owned = coin.to_string();
        let mut cur_mid = mid;
        let mut remaining = round_size(size, sz_decimals);
        if remaining <= 0.0 {
            remaining = size;
        }
        let mut extra_ticks: u32 = 0;
        let mut last_px = 0.0f64;
        let mut any_rejected = false;

        for attempt in 0..MAX_ATTEMPTS {
            if remaining <= 0.0 {
                return Ok(format!("Maker成交 {size:.6}@{last_px}"));
            }
            if attempt > 0 && remaining * cur_mid < 10.0 {
                let filled = (size - remaining).max(0.0);
                return Ok(format!("Maker成交 {filled:.6}@{last_px}"));
            }
            let now = tokio::time::Instant::now();
            if now >= total_deadline {
                break;
            }
            let px = passive_alo_price(cur_mid, sz_decimals, buy, extra_ticks);
            if px <= 0.0 {
                break;
            }
            last_px = px;

            let cloid = Uuid::new_v4();
            let resp = sdk_retry(&format!("被动挂单 {coin}"), || {
                let order = ClientOrderRequest {
                    asset: coin_owned.clone(),
                    is_buy: buy,
                    reduce_only,
                    limit_px: px,
                    sz: remaining,
                    cloid: Some(cloid),
                    order_type: ClientOrder::Limit(ClientLimit { tif: "Alo".into() }),
                };
                trading.order(order, None)
            })
            .await;

            let status = match resp.and_then(one_status) {
                Ok(st) => st,
                Err(e) => {
                    let msg = format!("{e:#}");
                    let explicit_reject = msg.contains("exchange order error:")
                        || msg.contains("exchange action rejected:");
                    if explicit_reject {
                        any_rejected = true;
                        if attempt + 1 < MAX_ATTEMPTS && tokio::time::Instant::now() < total_deadline {
                            if let Ok(fresh) = self.all_mids().await {
                                if let Some(&m) = fresh.get(coin) {
                                    if m.is_finite() && m > 0.0 {
                                        cur_mid = m;
                                    }
                                }
                            }
                            extra_ticks = extra_ticks.saturating_add(1);
                            tokio::time::sleep(Duration::from_millis(150)).await;
                            continue;
                        }
                        break;
                    }
                    // 网络超时或未知错误：先按 cloid 查交易所状态，确认未挂单/未成交才允许重试或回退 IOC
                    let cloid_hex = format!("0x{}", cloid.simple());
                    let acct = self.account.context("missing account for orderStatus")?;
                    let st_resp = self
                        .info_post(json!({
                            "type": "orderStatus",
                            "user": acct.to_string(),
                            "oid": cloid_hex,
                        }))
                        .await
                        .with_context(|| format!("Alo 状态未知且查询失败 ({coin}): {msg}"))?;
                    match parse_order_status(&st_resp, sz_decimals) {
                        OrderStatusCheck::UnknownOid => {
                            any_rejected = true;
                            break;
                        }
                        OrderStatusCheck::Filled => {
                            return Ok(format!("Maker成交 {size:.6}@{px}"));
                        }
                        OrderStatusCheck::Open { oid, rem } => {
                            if let Some(r) = rem {
                                remaining = r;
                            }
                            ExchangeDataStatus::Resting(hyperliquid_rust_sdk::RestingOrder { oid })
                        }
                        OrderStatusCheck::TerminalUnfilled { rem } => {
                            if let Some(r) = rem {
                                remaining = r;
                            }
                            if remaining <= 0.0 {
                                return Ok(format!("Maker成交 {size:.6}@{px}"));
                            }
                            any_rejected = true;
                            if attempt + 1 < MAX_ATTEMPTS && tokio::time::Instant::now() < total_deadline {
                                if let Ok(fresh) = self.all_mids().await {
                                    if let Some(&m) = fresh.get(coin) {
                                        if m.is_finite() && m > 0.0 {
                                            cur_mid = m;
                                        }
                                    }
                                }
                                extra_ticks = extra_ticks.saturating_add(1);
                                tokio::time::sleep(Duration::from_millis(150)).await;
                                continue;
                            }
                            break;
                        }
                        OrderStatusCheck::Ambiguous(detail) => {
                            bail!("Alo 下单状态未确认 ({coin}, {detail}): {msg}");
                        }
                    }
                }
            };

            let oid = match status {
                ExchangeDataStatus::Filled(_) => {
                    return Ok(format!("Maker · {}", describe(&status)));
                }
                ExchangeDataStatus::Resting(r) => r.oid,
                _ => {
                    any_rejected = true;
                    break;
                }
            };

            let left_budget = total_deadline.saturating_duration_since(tokio::time::Instant::now());
            let attempts_left = (MAX_ATTEMPTS - attempt) as u64;
            let slice_dur = if attempt + 1 < MAX_ATTEMPTS {
                let split = (left_budget.as_secs() / attempts_left).max(3);
                Duration::from_secs(split).min(left_budget)
            } else {
                left_budget
            };
            let slice_deadline = tokio::time::Instant::now() + slice_dur;

            while tokio::time::Instant::now() < slice_deadline {
                let step = Duration::from_secs(2)
                    .min(slice_deadline.saturating_duration_since(tokio::time::Instant::now()));
                if step.is_zero() {
                    break;
                }
                tokio::time::sleep(step).await;
                if let Ok(open) = self.open_order_details().await {
                    if !open.iter().any(|x| x.oid == oid) {
                        // 不在未成交挂单列表里，说明已全部成交
                        return Ok(format!("Maker成交 {size:.6}@{px}"));
                    }
                }
            }

            // 撤单：Ok(0) 表示 MissingOrder（撤单前刚成交），绝不重复下 IOC 或重挂
            match self.cancel_orders(&[(coin_owned.clone(), oid)]).await {
                Ok(0) => return Ok(format!("Maker成交 {size:.6}@{px}")),
                Ok(_) => {
                    // 撤单返回 Ok(n) 后必须查 orderStatus 确认最终剩余未成交量：
                    // 若最后一次轮询与撤单之间发生部分成交，不查就会用过期 remaining 造成双倍下单！
                    let acct = self.account.context("missing account for post-cancel orderStatus")?;
                    let st_resp = self
                        .info_post(json!({
                            "type": "orderStatus",
                            "user": acct.to_string(),
                            "oid": oid,
                        }))
                        .await
                        .with_context(|| format!("撤单后查询 orderStatus 失败 ({coin}, oid={oid})，中止补单防双倍成交"))?;
                    match parse_order_status(&st_resp, sz_decimals) {
                        OrderStatusCheck::Filled => {
                            return Ok(format!("Maker成交 {size:.6}@{px}"));
                        }
                        OrderStatusCheck::TerminalUnfilled { rem: Some(r) } => {
                            remaining = r;
                            if remaining <= 0.0 {
                                return Ok(format!("Maker成交 {size:.6}@{px}"));
                            }
                        }
                        OrderStatusCheck::TerminalUnfilled { rem: None } => {
                            bail!("撤单后 orderStatus 缺少剩余数量 sz ({coin}, oid={oid})，中止补单防双倍成交");
                        }
                        OrderStatusCheck::Open { .. }
                        | OrderStatusCheck::UnknownOid
                        | OrderStatusCheck::Ambiguous(_) => {
                            bail!("撤单后订单状态异常 ({coin}, oid={oid}, resp={st_resp})，中止补单防双倍成交");
                        }
                    }
                }
                Err(e) => {
                    return Err(e.context(format!("Maker 超时撤单失败 {coin} (oid={oid})")));
                }
            }

            // 撤单后再刷新最新 mid 准备重挂或 IOC 回退
            if let Ok(fresh) = self.all_mids().await {
                if let Some(&m) = fresh.get(coin) {
                    if m.is_finite() && m > 0.0 {
                        cur_mid = m;
                    }
                }
            }
            // 上一笔成功挂上盘口（未被 Post-Only 拒单），重挂时回到最新 mid 的标准被动档位
            extra_ticks = 0;
        }

        let rem = round_size(remaining, sz_decimals);
        let filled_maker = (size - rem).max(0.0);
        if rem <= 0.0 || (filled_maker > 1e-12 && rem * cur_mid < 10.0) {
            return Ok(format!("Maker成交 {filled_maker:.6}@{last_px}"));
        }
        let st = self
            .ioc(coin, buy, reduce_only, rem, cur_mid, slippage, sz_decimals)
            .await?;
        if filled_maker > 1e-12 {
            Ok(format!("Maker {filled_maker:.6} + IOC {rem:.6} · {}", describe(&st)))
        } else if any_rejected {
            Ok(format!("IOC回退 · {}", describe(&st)))
        } else {
            Ok(format!("超时转IOC · {}", describe(&st)))
        }
    }
}

#[derive(Debug, PartialEq)]
enum OrderStatusCheck {
    UnknownOid,
    Filled,
    Open { oid: u64, rem: Option<f64> },
    TerminalUnfilled { rem: Option<f64> },
    Ambiguous(String),
}

fn parse_order_status(st_resp: &Value, sz_decimals: i32) -> OrderStatusCheck {
    let order_status = st_resp["status"].as_str().unwrap_or("");
    if order_status == "unknownOid" {
        return OrderStatusCheck::UnknownOid;
    }
    if order_status != "order" {
        return OrderStatusCheck::Ambiguous(format!("status={order_status}"));
    }
    let inner_status = st_resp["order"]["status"].as_str().unwrap_or("");
    let rem = st_resp["order"]["order"]["sz"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .or_else(|| st_resp["order"]["order"]["sz"].as_f64())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map(|v| round_size(v, sz_decimals));
    match inner_status {
        "filled" => OrderStatusCheck::Filled,
        "open" => match st_resp["order"]["order"]["oid"].as_u64() {
            Some(oid) => OrderStatusCheck::Open { oid, rem },
            None => OrderStatusCheck::Ambiguous("open missing oid".into()),
        },
        "canceled" | "rejected" | "marginCanceled" | "reduceOnlyCanceled" | "minTradeNtlRejected" => {
            OrderStatusCheck::TerminalUnfilled { rem }
        }
        other => OrderStatusCheck::Ambiguous(format!("status={order_status}/{other}")),
    }
}

fn parse_perp_equity(perp: &Value) -> Result<f64> {
    let raw = perp["marginSummary"]["accountValue"]
        .as_str()
        .context("clearinghouseState 缺少 marginSummary.accountValue")?;
    let equity: f64 = raw.parse().context("marginSummary.accountValue 不是有效数字")?;
    anyhow::ensure!(equity.is_finite() && equity >= 0.0, "账户净值无效: {equity}");
    Ok(equity)
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

/// Hyperliquid 单档最小价格步进（遵循 5 位有效数字与 `6 - sz_decimals` 小数位规则）。
pub fn price_tick(px: f64, sz_decimals: i32) -> f64 {
    if !px.is_finite() || px <= 0.0 {
        return 0.0;
    }
    let decimal_places = (4 - px.log10().floor() as i32).min(6 - sz_decimals);
    10_f64.powi(-decimal_places)
}

/// 构造严格被动（Post-Only / Alo）挂单价：
/// 至少相对 `mid` 退让 `max(1bp, 1 tick)`，并支持 `extra_ticks` 档额外退让。
/// 保证买单严格 `< mid`、卖单严格 `> mid`，同时满足 HL tick/sig-fig 规则。
pub fn passive_alo_price(mid: f64, sz_decimals: i32, buy: bool, extra_ticks: u32) -> f64 {
    if !mid.is_finite() || mid <= 0.0 {
        return 0.0;
    }
    let tick = price_tick(mid, sz_decimals);
    if !tick.is_finite() || tick <= 0.0 {
        return 0.0;
    }
    let base_offset = (mid * 0.0001).max(tick);
    if buy {
        let raw = mid - base_offset;
        if raw <= 0.0 {
            return 0.0;
        }
        let mut px = order_price(raw, sz_decimals, false);
        while px >= mid && px > 0.0 {
            let step = price_tick(px * (1.0 - 1e-9), sz_decimals).max(tick);
            let next = order_price((px - step * 0.5).max(0.0), sz_decimals, false);
            if next >= px {
                let next2 = order_price((px - step).max(0.0), sz_decimals, false);
                if next2 >= px {
                    break;
                }
                px = next2;
            } else {
                px = next;
            }
        }
        for _ in 0..extra_ticks {
            if px <= 0.0 {
                return 0.0;
            }
            let step = price_tick(px * (1.0 - 1e-9), sz_decimals);
            if step <= 0.0 {
                break;
            }
            let next = order_price((px - step * 0.5).max(0.0), sz_decimals, false);
            px = if next < px {
                next
            } else {
                order_price((px - step).max(0.0), sz_decimals, false)
            };
        }
        if px > 0.0 && px < mid {
            px
        } else {
            0.0
        }
    } else {
        let raw = mid + base_offset;
        let mut px = order_price(raw, sz_decimals, true);
        while px <= mid {
            let step = price_tick(px, sz_decimals).max(tick);
            let next = order_price(px + step * 0.5, sz_decimals, true);
            if next <= px {
                let next2 = order_price(px + step, sz_decimals, true);
                if next2 <= px {
                    break;
                }
                px = next2;
            } else {
                px = next;
            }
        }
        for _ in 0..extra_ticks {
            let step = price_tick(px, sz_decimals);
            if step <= 0.0 {
                break;
            }
            let next = order_price(px + step * 0.5, sz_decimals, true);
            px = if next > px {
                next
            } else {
                order_price(px + step, sz_decimals, true)
            };
        }
        if px > mid {
            px
        } else {
            0.0
        }
    }
}

/// Truncate a size to the coin's lot size.
pub fn round_size(size: f64, sz_decimals: i32) -> f64 {
    if !size.is_finite() || size <= 0.0 {
        return 0.0;
    }
    if sz_decimals < 0 {
        // Integer lot sizes must use an integer step, not its inexact reciprocal.
        // 593 / 0.00001 becomes 59299999.99999999 and violates PEPE lot sizing.
        let step = 10_f64.powi(-sz_decimals);
        return (size / step).floor() * step;
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

#[cfg(test)]
mod account_value_tests {
    use super::*;

    #[test]
    fn perp_equity_requires_a_valid_account_value() {
        assert_eq!(parse_perp_equity(&json!({"marginSummary":{"accountValue":"2500.25"}})).unwrap(), 2500.25);
        // 统一账户的永续视图可以合法为 0，后续仍取永续/现货两者较大值。
        assert_eq!(parse_perp_equity(&json!({"marginSummary":{"accountValue":"0.0"}})).unwrap(), 0.0);
        assert!(parse_perp_equity(&json!({"status":"err","response":"rate limited"})).is_err());
        assert!(parse_perp_equity(&json!({"marginSummary":{}})).is_err());
        assert!(parse_perp_equity(&json!({"marginSummary":{"accountValue":"not-a-number"}})).is_err());
        assert!(parse_perp_equity(&json!({"marginSummary":{"accountValue":"-1"}})).is_err());
    }

    #[test]
    fn passive_alo_price_is_strictly_passive_across_scales() {
        let cases = [
            (0.0000123, 0),
            (0.000512, 0),
            (0.084119, 1),
            (1.0, 2),
            (10.0, 2),
            (100.0, 2),
            (256.78, 3),
            (3250.5, 4),
            (85000.0, 5),
            (99999.0, 5),
        ];
        for (mid, sz_dec) in cases {
            let buy0 = passive_alo_price(mid, sz_dec, true, 0);
            let sell0 = passive_alo_price(mid, sz_dec, false, 0);
            assert!(
                buy0 > 0.0 && buy0 < mid,
                "mid={mid} sz_dec={sz_dec}: buy_px {buy0} 必须严格 < mid"
            );
            assert!(
                sell0 > mid,
                "mid={mid} sz_dec={sz_dec}: sell_px {sell0} 必须严格 > mid"
            );
            let tick = price_tick(mid, sz_dec);
            let min_gap = (mid * 0.0001).max(tick) * (1.0 - 1e-9);
            assert!(
                mid - buy0 >= min_gap - tick * 1e-6,
                "mid={mid} sz_dec={sz_dec}: buy_px={buy0} 退让不足 (gap={}, min={min_gap})",
                mid - buy0
            );
            assert!(
                sell0 - mid >= min_gap - tick * 1e-6,
                "mid={mid} sz_dec={sz_dec}: sell_px={sell0} 退让不足 (gap={}, min={min_gap})",
                sell0 - mid
            );
            let buy1 = passive_alo_price(mid, sz_dec, true, 1);
            let sell1 = passive_alo_price(mid, sz_dec, false, 1);
            assert!(buy1 < buy0, "mid={mid}: extra_ticks=1 买价 {buy1} 应 < {buy0}");
            assert!(sell1 > sell0, "mid={mid}: extra_ticks=1 卖价 {sell1} 应 > {sell0}");
        }
        assert_eq!(passive_alo_price(0.0, 2, true, 0), 0.0);
        assert_eq!(passive_alo_price(f64::NAN, 2, false, 0), 0.0);
    }

    #[test]
    fn parse_order_status_extracts_remaining_after_partial_fill_cancel() {
        let filled = json!({"status": "order", "order": {"status": "filled", "order": {"oid": 42, "sz": "0"}}});
        assert_eq!(parse_order_status(&filled, 2), OrderStatusCheck::Filled);

        let partial_canceled = json!({"status": "order", "order": {"status": "canceled", "order": {"oid": 42, "sz": "3.456"}}});
        assert_eq!(
            parse_order_status(&partial_canceled, 2),
            OrderStatusCheck::TerminalUnfilled { rem: Some(3.45) }
        );

        let open = json!({"status": "order", "order": {"status": "open", "order": {"oid": 99, "sz": "1.2"}}});
        assert_eq!(
            parse_order_status(&open, 2),
            OrderStatusCheck::Open { oid: 99, rem: Some(1.2) }
        );

        let unknown = json!({"status": "unknownOid"});
        assert_eq!(parse_order_status(&unknown, 2), OrderStatusCheck::UnknownOid);
    }
}
