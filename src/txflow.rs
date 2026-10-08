//! TxFlow mainnet adapter. Public data is independent of the Hyperliquid cache.
//! Ordered MessagePack payloads follow the current TxFlow web client's wire format.
use crate::exchange::{Acct, OpenOrder, Pos};
use alloy::{
    primitives::{keccak256, Address, B256},
    signers::{local::PrivateKeySigner, SignerSync},
};
use alloy_sol_types::{sol, Eip712Domain, SolStruct};
use anyhow::{bail, Context, Result};
use hyperliquid_rust_sdk::ExchangeDataStatus;
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::{sync::Mutex, time::Instant};

const BASE: &str = "https://api.txflow.com";
const DAY: i64 = 86_400_000;
pub const ORDER_INTERVAL: Duration = Duration::from_millis(300);

sol! {
    struct Agent {
        string txflowNetwork;
        uint32 chainId;
        uint32 apiVersion;
        bytes32 connectionId;
    }
}

#[derive(Default)]
struct SubmissionGate {
    completed: Option<Instant>,
    nonce: u64,
}
impl SubmissionGate {
    async fn next(&mut self) -> u64 {
        if let Some(last) = self.completed {
            tokio::time::sleep_until(last + ORDER_INTERVAL).await;
        }
        self.nonce = (crate::live::now_ms_pub() as u64).max(self.nonce + 1);
        self.nonce
    }
}
fn gate() -> Arc<Mutex<SubmissionGate>> {
    static GATE: OnceLock<Arc<Mutex<SubmissionGate>>> = OnceLock::new();
    GATE.get_or_init(|| Arc::new(Mutex::new(SubmissionGate::default())))
        .clone()
}

#[derive(Clone, Debug)]
pub struct Market {
    pub name: String,
    pub index: u32,
    pub decimals: i32,
    pub max_leverage: u32,
    pub tick: f64,
    pub max_order_size: f64,
}

pub struct Client {
    http: reqwest::Client,
    account: String,
    signer: Option<PrivateKeySigner>,
    pub markets: HashMap<String, Market>,
    owned_path: PathBuf,
}

fn number(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str()?.parse().ok())
        .filter(|x| x.is_finite())
}
fn data(v: Value) -> Result<Value> {
    if v.get("code").is_some() {
        anyhow::ensure!(v["code"] == 200, "TxFlow: {v}");
        return v
            .get("data")
            .cloned()
            .context("TxFlow response missing data");
    }
    Ok(v)
}

/// Main-wallet authorization shares the trading throttle. Never retry a signed write.
pub async fn approve_agent(http: &reqwest::Client, body: &Value, endpoint: &str) -> Result<()> {
    let gate = gate();
    let mut guard = gate.lock().await;
    guard.next().await;
    guard.completed = Some(Instant::now());
    let result = async {
        let response = http.post(endpoint).json(body)
            .timeout(Duration::from_secs(25)).send().await?;
        let response = data(response.error_for_status()?.json::<Value>().await?)?;
        anyhow::ensure!(response["status"] == "ok", "TxFlow 拒绝授权: {response}");
        Ok(())
    }.await;
    guard.completed = Some(Instant::now());
    result
}

impl Client {
    pub async fn new(account: &str, key_path: Option<&Path>) -> Result<Self> {
        let account = if account.is_empty() {
            String::new()
        } else {
            Address::from_str(account)
                .context("无效的 TxFlow 主账户地址")?
                .to_string()
        };
        let signer = if let Some(path) = key_path {
            let key = std::fs::read_to_string(path).context("无法读取 TxFlow Agent 密钥文件")?;
            let key = key.trim();
            let s = PrivateKeySigner::from_str(key)
                .map_err(|_| anyhow::anyhow!("TxFlow Agent 密钥格式错误"))?;
            anyhow::ensure!(
                s.address().to_string() != account,
                "请使用已授权的 Agent 钱包，不能使用主账户密钥"
            );
            Some(s)
        } else {
            None
        };
        let mut client = Self {
            http: reqwest::Client::builder()
                .user_agent("Mozilla/5.0 stars/0.3")
                .timeout(Duration::from_secs(25))
                .build()?,
            account,
            signer,
            markets: HashMap::new(),
            owned_path: std::env::var("STARS_TXFLOW_ORDERS")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    let live_path = std::env::var("STARS_TXFLOW_LIVE")
                        .or_else(|_| std::env::var("STARS_LIVE"))
                        .unwrap_or_else(|_| "/var/lib/stars/live.json".into());
                    PathBuf::from(live_path).with_file_name("txflow-orders.json")
                }),
        };
        client.markets = client.load_markets().await?;
        Ok(client)
    }

    pub async fn info(&self, payload: Value) -> Result<Value> {
        // Reads can be retried; signed writes are deliberately never auto-retried.
        let mut last = None;
        for attempt in 0..3 {
            let result = async {
                let resp = self
                    .http
                    .post(format!("{BASE}/info"))
                    .json(&payload)
                    .send()
                    .await?;
                data(resp.error_for_status()?.json::<Value>().await?)
            }
            .await;
            match result {
                Ok(v) => return Ok(v),
                Err(e) => last = Some(e),
            }
            if attempt < 2 {
                tokio::time::sleep(Duration::from_millis(500 * (attempt + 1))).await;
            }
        }
        Err(last.unwrap())
    }

    async fn load_markets(&self) -> Result<HashMap<String, Market>> {
        let raw = self.info(json!({"type":"perpMeta","dex":""})).await?;
        let mut markets = HashMap::new();
        for m in raw["universe"]
            .as_array()
            .context("TxFlow perpMeta 缺少 universe")?
        {
            if m["haltTrading"] == true || m["delisted"] == true || m["onlyIsolated"] == true {
                continue;
            }
            let name = m["name"]
                .as_str()
                .context("market missing name")?
                .to_string();
            let tick = number(&m["priceTick"]).context("market missing priceTick")?;
            anyhow::ensure!(tick > 0., "invalid priceTick");
            markets.insert(
                name.clone(),
                Market {
                    name,
                    index: m["index"].as_u64().context("market missing index")? as u32,
                    decimals: m["szDecimals"]
                        .as_i64()
                        .context("market missing szDecimals")? as i32,
                    max_leverage: m["maxLeverage"]
                        .as_u64()
                        .context("market missing leverage")?
                        as u32,
                    tick,
                    max_order_size: number(&m["maxMarketOrderSize"]).unwrap_or(f64::INFINITY),
                },
            );
        }
        anyhow::ensure!(!markets.is_empty(), "TxFlow 可交易市场为空");
        Ok(markets)
    }

    async fn book_price(&self, coin: String) -> Result<(String, f64)> {
        let m = self
            .markets
            .get(&coin)
            .with_context(|| format!("未知 TxFlow 市场 {coin}"))?;
        let book = self
            .info(json!({"type":"l2Book","coin":m.index.to_string()}))
            .await?;
        let bid = number(&book["levels"][0][0]["px"]).context("TxFlow 买盘为空")?;
        let ask = number(&book["levels"][1][0]["px"]).context("TxFlow 卖盘为空")?;
        anyhow::ensure!(ask >= bid && bid > 0., "TxFlow 盘口价格无效");
        Ok((coin, (bid + ask) / 2.))
    }
    pub async fn prices(&self, coins: &[String]) -> Result<HashMap<String, f64>> {
        use futures::{stream, StreamExt, TryStreamExt};
        let requests: Vec<_> = coins
            .iter()
            .map(|coin| self.book_price(coin.clone()))
            .collect();
        stream::iter(requests)
            .buffer_unordered(4)
            .try_collect()
            .await
    }

    pub async fn all_mids(&self) -> Result<HashMap<String, f64>> {
        let coins: Vec<String> = self.markets.keys().cloned().collect();
        self.prices(&coins).await
    }

    fn coin(&self, value: &Value) -> Result<String> {
        let id = value["instrumentId"]
            .as_u64()
            .or_else(|| value["coinIndex"].as_u64());
        let coin = value["coin"].as_str().unwrap_or("");
        self.markets
            .values()
            .find(|m| {
                id == Some(m.index as u64)
                    || coin == m.name
                    || coin == m.index.to_string()
                    || format!("{coin}-USDC") == m.name
            })
            .map(|m| m.name.clone())
            .with_context(|| format!("未知或不可交易的 TxFlow 持仓/订单: {coin} {id:?}"))
    }

    pub async fn account(&self) -> Result<Acct> {
        anyhow::ensure!(!self.account.is_empty(), "未配置 TxFlow 主账户地址");
        let v = self.info(json!({"type":"clearinghouseState","user":self.account,"accountType":"perps","exchangeId":0})).await?;
        let equity =
            number(&v["marginSummary"]["accountValue"]).context("TxFlow 账户缺少净值字段")?;
        let mut positions = HashMap::new();
        for item in v["assetPositions"]
            .as_array()
            .context("TxFlow 账户缺少持仓字段")?
        {
            let p = &item["position"];
            let size = number(&p["szi"]).context("TxFlow 持仓缺少 szi，无法安全调仓")?;
            if size.abs() < 1e-10 {
                continue;
            }
            let coin = self.coin(p)?;
            anyhow::ensure!(
                !positions.contains_key(&coin),
                "TxFlow 双向持仓不支持，请先切换单向持仓"
            );
            positions.insert(
                coin,
                Pos {
                    size,
                    entry_px: number(&p["entryPx"]).context("position missing entryPx")?,
                    liq_px: number(&p["liquidationPx"]),
                    is_cross: p["leverage"]["type"]
                        .as_str()
                        .map(|s| s.eq_ignore_ascii_case("cross"))
                        .unwrap_or(false),
                    leverage: number(&p["leverage"]["value"]).unwrap_or(0.) as u32,
                    position_value: number(&p["positionValue"])
                        .context("position missing value")?,
                    unrealized_pnl: number(&p["unrealizedPnl"]).context("position missing pnl")?,
                },
            );
        }
        Ok(Acct { equity, positions })
    }

    async fn submit<T: Serialize>(&self, kind: &str, payload: &T) -> Result<Value> {
        let encoded = rmp_serde::to_vec_named(payload)?;
        self.submit_encoded(kind, serde_json::to_value(payload)?, encoded)
            .await
    }

    async fn submit_encoded(&self, kind: &str, payload: Value, encoded: Vec<u8>) -> Result<Value> {
        let signer = self
            .signer
            .as_ref()
            .context("TxFlow 客户端未配置 Agent 密钥")?;
        let gate = gate();
        let mut guard = gate.lock().await;
        let nonce = guard.next().await;
        let digest = signing_hash(&encoded, nonce);
        let sig = signer.sign_hash_sync(&digest)?;
        let mut action = payload;
        action["type"] = json!(kind);
        let body = json!({"action":action,"nonce":nonce,"vaultAddress":null,
            "signature":{"r":format!("{:#066x}",sig.r()),"s":format!("{:#066x}",sig.s()),"v":27 + u8::from(sig.v())}});
        // Hold the shared gate through the response, including transport failures.
        guard.completed = Some(Instant::now());
        let result = self
            .http
            .post(format!("{BASE}/exchange"))
            .json(&body)
            .send()
            .await;
        let result = async { data(result?.error_for_status()?.json::<Value>().await?) }.await;
        guard.completed = Some(Instant::now());
        let response =
            result.context("TxFlow 发单结果未知；请核对账户后再操作，系统不会自动重发")?;
        anyhow::ensure!(response["status"] == "ok", "TxFlow 拒绝: {response}");
        Ok(response)
    }

    pub async fn set_leverage(&self, coin: &str, leverage: u32) -> Result<()> {
        let m = self.markets.get(coin).context("unknown TxFlow market")?;
        anyhow::ensure!(
            leverage <= m.max_leverage && leverage > 0,
            "杠杆超出市场限制"
        );
        #[derive(Serialize)]
        struct Update {
            asset: u32,
            leverage: u32,
            #[serde(rename = "marginMode")]
            margin_mode: &'static str,
        }
        self.submit(
            "updateLeverage",
            &Update {
                asset: m.index,
                leverage,
                margin_mode: "cross",
            },
        )
        .await?;
        Ok(())
    }

    async fn order(
        &self,
        coin: &str,
        buy: bool,
        reduce: bool,
        size: f64,
        px: f64,
        tif: &str,
    ) -> Result<ExchangeDataStatus> {
        let m = self.markets.get(coin).context("unknown TxFlow market")?;
        anyhow::ensure!(
            size.is_finite() && size > 0. && px.is_finite() && px > 0.,
            "无效下单数量/价格"
        );
        anyhow::ensure!(
            tif != "ioc" || size <= m.max_order_size,
            "{coin} 数量超过市场单笔上限"
        );
        let px = tick_price(px, m.tick, buy)?;
        let sz = crate::exchange::round_size(size, m.decimals);
        anyhow::ensure!(sz > 0., "数量小于最小步长");
        let payload = OrderPayload {
            grouping: "na",
            orders: vec![WireOrder {
                a: m.index,
                b: buy,
                p: decimal(px),
                s: decimal(sz),
                r: reduce,
                t: OrderType {
                    limit: Limit { tif },
                },
                m: "cross",
            }],
        };
        let result = self.submit("order", &payload).await?;
        let statuses = result["response"]["data"]["statuses"]
            .as_array()
            .context("TxFlow 缺少逐单回执，不能确认成交")?;
        anyhow::ensure!(statuses.len() == 1, "TxFlow 逐单回执数量错误");
        let status: ExchangeDataStatus =
            serde_json::from_value(statuses[0].clone()).context("未知 TxFlow 订单回执")?;
        if let ExchangeDataStatus::Error(ref e) = status {
            bail!("TxFlow 订单拒绝: {e}");
        }
        Ok(status)
    }

    pub async fn ioc(
        &self,
        coin: &str,
        buy: bool,
        reduce: bool,
        size: f64,
        mid: f64,
        slippage: f64,
    ) -> Result<ExchangeDataStatus> {
        let px = mid * if buy { 1. + slippage } else { 1. - slippage };
        self.order(coin, buy, reduce, size, px, "ioc").await
    }

    // TxFlow does not document client order ids. Persist the returned order ids
    // instead of relying on an unverified `cloid` field for ownership.
    fn owned(&self) -> Result<HashMap<String, Vec<u64>>> {
        match std::fs::read_to_string(&self.owned_path) {
            Ok(s) => Ok(serde_json::from_str(&s).context("TxFlow 订单归属文件损坏")?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(e) => Err(e.into()),
        }
    }
    fn remember(&self, oid: u64) -> Result<()> {
        let mut owned = self.owned()?;
        owned.entry(self.account.clone()).or_default().push(oid);
        if let Some(parent) = self.owned_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = self.owned_path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(&owned)?)?;
        std::fs::rename(tmp, &self.owned_path)?;
        Ok(())
    }
    pub async fn resting_reduce_order(
        &self,
        coin: &str,
        buy: bool,
        size: f64,
        px: f64,
    ) -> Result<u64> {
        // Check persistence before placing an order whose ownership must survive restart.
        self.owned()?;
        if let Some(parent) = self.owned_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match self.order(coin, buy, true, size, px, "gtc").await? {
            ExchangeDataStatus::Resting(r) => {
                self.remember(r.oid)?;
                Ok(r.oid)
            }
            ExchangeDataStatus::Filled(_) => Ok(0),
            other => bail!("TxFlow 止盈单未挂上: {other:?}"),
        }
    }
    pub async fn open_orders(&self) -> Result<Vec<OpenOrder>> {
        let v = self.info(json!({"type":"frontendOpenOrders","user":self.account,"exchangeId":0,"accountType":["perps"]})).await?;
        let owned = self.owned()?.remove(&self.account).unwrap_or_default();
        let mut orders = Vec::new();
        for o in v.as_array().context("TxFlow openOrders 格式错误")? {
            let oid = o["oid"]
                .as_u64()
                .or_else(|| o["oid"].as_str()?.parse().ok())
                .context("order missing oid")?;
            orders.push(OpenOrder {
                oid,
                coin: self.coin(o)?,
                side: o["side"].as_str().unwrap_or("").to_string(),
                px: number(&o["limitPx"])
                    .or_else(|| number(&o["px"]))
                    .context("order missing price")?,
                sz: number(&o["sz"]).context("order missing size")?,
                reduce_only: o["reduceOnly"].as_bool().unwrap_or(false),
                cloid: owned.contains(&oid).then(|| "0x5741txflow".to_string()),
            });
        }
        Ok(orders)
    }
    pub async fn cancel_orders(&self, orders: &[(String, u64)]) -> Result<usize> {
        let mut count = 0;
        for (coin, oid) in orders {
            let m = self.markets.get(coin).context("unknown cancel market")?;
            // Browser encodes `o` as BigInt: MessagePack uint64 even for small ids.
            let mut encoded = rmp_serde::to_vec_named(&CancelPayload {
                cancels: vec![Cancel { a: m.index, o: 0 }],
            })?;
            encoded.pop(); // remove integer zero
            encoded.push(0xcf);
            encoded.extend_from_slice(&oid.to_be_bytes());
            let response = self
                .submit_encoded(
                    "cancel",
                    json!({"cancels":[{"a":m.index,"o":oid.to_string()}]}),
                    encoded,
                )
                .await?;
            let statuses = response["response"]["data"]["statuses"]
                .as_array()
                .context("TxFlow 缺少撤单回执")?;
            anyhow::ensure!(
                statuses.len() == 1 && statuses[0] == "success",
                "TxFlow 撤单未确认: {response}"
            );
            count += 1;
        }
        Ok(count)
    }
    pub async fn user_fills(&self, limit: usize) -> Result<Value> {
        let mut v = self
            .info(json!({"type":"userFills","user":self.account,"limit":limit}))
            .await?;
        if let Some(fills) = v.as_array_mut() {
            for fill in fills {
                fill["coin"] = json!(self.coin(fill)?);
            }
        }
        Ok(v)
    }

    pub async fn maintenance_margin(&self) -> Result<f64> {
        let v = self.info(json!({"type":"clearinghouseState","user":self.account,"accountType":"perps","exchangeId":0})).await?;
        number(&v["crossMaintenanceMarginUsed"]).context("TxFlow 缺少维持保证金字段")
    }
}

#[derive(Serialize)]
struct OrderPayload<'a> {
    grouping: &'static str,
    orders: Vec<WireOrder<'a>>,
}
#[derive(Serialize)]
struct WireOrder<'a> {
    a: u32,
    b: bool,
    p: String,
    s: String,
    r: bool,
    t: OrderType<'a>,
    m: &'static str,
}
#[derive(Serialize)]
struct OrderType<'a> {
    limit: Limit<'a>,
}
#[derive(Serialize)]
struct Limit<'a> {
    tif: &'a str,
}
#[derive(Serialize)]
struct CancelPayload {
    cancels: Vec<Cancel>,
}
#[derive(Serialize)]
struct Cancel {
    a: u32,
    o: u64,
}
fn decimal(v: f64) -> String {
    format!("{v:.8}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}
fn tick_price(px: f64, tick: f64, buy: bool) -> Result<f64> {
    anyhow::ensure!(
        px.is_finite() && tick.is_finite() && px > 0. && tick > 0.,
        "invalid tick/price"
    );
    // Inward rounding never exceeds the configured slippage cap.
    let units = px / tick;
    let rounded = if buy { units.floor() } else { units.ceil() } * tick;
    anyhow::ensure!(rounded > 0., "价格低于最小 tick");
    Ok(rounded)
}
fn signing_hash(encoded: &[u8], nonce: u64) -> B256 {
    let mut bytes = encoded.to_vec();
    bytes.extend_from_slice(&nonce.to_be_bytes());
    bytes.push(0); // no vault address
    let agent = Agent {
        txflowNetwork: "TxFlow-Mainnet".into(),
        chainId: 869,
        apiVersion: 1,
        connectionId: keccak256(bytes),
    };
    let domain = Eip712Domain {
        name: Some("TxFlow-Mainnet".into()),
        version: Some("1".into()),
        chain_id: Some(alloy::primitives::U256::from(869u64)),
        verifying_contract: Some(Address::ZERO),
        salt: None,
    };
    agent.eip712_signing_hash(&domain)
}

/// Refresh all TxFlow markets before calculating a signal; reject stale/gapped daily data.
pub async fn refresh(state: &crate::web::AppState) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(180), async {
        let _refresh = state.refresh_gate.lock().await;
        refresh_inner(state).await
    }).await
        .context("TxFlow 行情更新超过 3 分钟，请稍后重试")?
}

async fn refresh_inner(state: &crate::web::AppState) -> Result<()> {
    let client = Client::new("", None).await?;
    let cfg = state.live.lock().await.config.clone();
    let now = crate::live::now_ms_pub();
    let yesterday = now / DAY * DAY - DAY;
    let count = client.markets.len();
    *state.refresh.lock().await = crate::web::RefreshStatus {
        phase: "backfill".into(),
        coins_total: count,
        ..Default::default()
    };
    let mut universe = Vec::new();
    let mut markets: Vec<_> = client.markets.values().cloned().collect();
    markets.sort_by(|a, b| a.name.cmp(&b.name));
    for (i, m) in markets.iter().enumerate() {
        state.refresh.lock().await.current = m.name.clone();
        let raw = client.info(json!({"type":"candleSnapshot","req":{"coin":m.index.to_string(),"interval":"1d","startTime":yesterday - (cfg.lookback.max(30) as i64 + 5)*DAY,"endTime":now}})).await?;
        let mut candles: Vec<crate::hl::Candle> = serde_json::from_value(raw)?;
        candles.retain(|c| {
            c.t <= yesterday && c.c.is_finite() && c.c > 0. && c.v.is_finite() && c.v >= 0.
        });
        candles.sort_by_key(|c| c.t);
        candles.dedup_by_key(|c| c.t);
        // Require contiguous candles for both momentum and the 30-day liquidity filter.
        let need = cfg.lookback.max(30) + 3;
        let recent = &candles[candles.len().saturating_sub(need)..];
        if recent.len() >= need
            && recent.last().map(|c| c.t) == Some(yesterday)
            && recent.windows(2).all(|w| w[1].t - w[0].t == DAY)
        {
            state.store.upsert_candles(&m.name, &candles)?;
            universe.push(crate::hl::CoinMeta {
                name: m.name.clone(),
                sz_decimals: m.decimals,
                max_leverage: m.max_leverage,
                is_delisted: false,
            });
        }
        state.refresh.lock().await.coins_done = i + 1;
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    let mut meta = state.meta.lock().await;
    meta.liquid = universe.iter().map(|m| m.name.clone()).collect();
    meta.universe = universe;
    meta.refreshed_at = now;
    meta.fee_taker = 0.00045;
    meta.fee_maker = 0.00015;
    state.refresh.lock().await.phase = "ready".into();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn every_submission_waits_300ms_even_after_failure() {
        let mut g = SubmissionGate::default();
        let first = g.next().await;
        g.completed = Some(Instant::now());
        let before = Instant::now();
        let second = g.next().await;
        assert!(before.elapsed() >= ORDER_INTERVAL);
        assert!(second > first);
    }
    #[tokio::test(start_paused = true)]
    async fn concurrent_clients_share_submission_spacing() {
        let g = Arc::new(Mutex::new(SubmissionGate::default()));
        let start = Instant::now();
        let mut tasks = Vec::new();
        for _ in 0..4 {
            let g = g.clone();
            tasks.push(tokio::spawn(async move {
                let mut g = g.lock().await;
                g.next().await;
                let at = Instant::now();
                g.completed = Some(at);
                at
            }));
        }
        let mut times = Vec::new();
        for t in tasks {
            times.push(t.await.unwrap());
        }
        times.sort();
        assert!(times[3] - start >= Duration::from_millis(900));
        assert!(times.windows(2).all(|w| w[1] - w[0] >= ORDER_INTERVAL));
    }
    #[test]
    fn tick_rounding_respects_slippage_and_decimal_normalization() {
        assert!((tick_price(89.235, 0.01, true).unwrap() - 89.23).abs() < 1e-9);
        assert!((tick_price(89.235, 0.01, false).unwrap() - 89.24).abs() < 1e-9);
        assert_eq!(decimal(1.20), "1.2");
        assert!(tick_price(f64::NAN, 0.01, true).is_err());
    }
    // Independent fixtures produced by the browser's @msgpack/msgpack (BigInt64)
    // and ethers TypedDataEncoder; catches domain, key order and uint64 differences.
    #[test]
    fn signatures_match_browser_wire_fixtures() {
        let encoded = rmp_serde::to_vec_named(&OrderPayload {
            grouping: "na",
            orders: vec![WireOrder {
                a: 53,
                b: true,
                p: "89.2".into(),
                s: "0.01".into(),
                r: false,
                t: OrderType {
                    limit: Limit { tif: "ioc" },
                },
                m: "cross",
            }],
        })
        .unwrap();
        assert_eq!(
            signing_hash(&encoded, 1791420000000),
            B256::from_str("0x6503ac63f5a5bd929a07e6351cbaefe4550efede2facc5bb994247ba0f6d4362")
                .unwrap()
        );
        let mut cancel = rmp_serde::to_vec_named(&CancelPayload {
            cancels: vec![Cancel { a: 53, o: 0 }],
        })
        .unwrap();
        assert_eq!(cancel.pop(), Some(0));
        cancel.push(0xcf);
        cancel.extend_from_slice(&12345u64.to_be_bytes());
        assert_eq!(
            signing_hash(&cancel, 1791420000000),
            B256::from_str("0x4185389bcf9092bb5c4465d89a05420e240b24af26e2acb069232309d2d6cf2b")
                .unwrap()
        );
        assert_eq!(crate::exchange::round_size(123.45, -1), 120.);
        assert_eq!(crate::exchange::round_size(1234.5, -2), 1200.);
        assert_eq!(crate::exchange::round_size(12345.6, -3), 12000.);
    }

    #[test]
    fn signing_nonce_and_order_key_order_are_binding() {
        let encoded = rmp_serde::to_vec_named(&OrderPayload {
            grouping: "na",
            orders: vec![],
        })
        .unwrap();
        assert_eq!(encoded, b"\x82\xa8grouping\xa2na\xa6orders\x90");
        assert_ne!(signing_hash(&encoded, 1), signing_hash(&encoded, 2));
    }
}

pub async fn background(state: crate::web::AppState) {
    loop {
        let now = crate::live::now_ms_pub();
        let due = auto_due(&*state.live.lock().await, now);
        let stale = now - state.meta.lock().await.refreshed_at > 30 * 60 * 1000;
        if due || stale {
            // Read-only backfill must not block saving settings or creating an Agent.
            match refresh(&state).await {
                Ok(()) if due => {
                    if let Err(e) = run_auto(&state).await {
                        tracing::warn!("TxFlow 自动调仓未完成: {e}");
                    }
                }
                Ok(()) => {}
                Err(e) => {
                    state.refresh.lock().await.phase = format!("error: {e}");
                    tracing::warn!("TxFlow 日线更新失败: {e}");
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

fn auto_due(st: &crate::live::LiveState, now: i64) -> bool {
    st.config.armed && st.config.auto_run && now % DAY >= 5 * 60 * 1000
        && st.last_run_at.map(|t| t / DAY < now / DAY).unwrap_or(true)
}

async fn run_auto(state: &crate::web::AppState) -> Result<()> {
    let _job = match state.run_gate.try_lock() { Ok(g) => g, Err(_) => return Ok(()) };
    let _execution = match state.exec_gate.try_lock() { Ok(g) => g, Err(_) => return Ok(()) };
    let now = crate::live::now_ms_pub();
    let snapshot = {
        let mut st = state.live.lock().await;
        // Settings may have been disabled while public candles were refreshing.
        if !auto_due(&st, now) { return Ok(()); }
        let previous = st.clone();
        st.last_run_at = Some(now);
        st.last_plan = vec!["TxFlow 自动调仓执行中…".into()];
        if let Err(e) = st.save(&state.live_path) {
            *st = previous;
            anyhow::bail!("无法保存自动调仓记录，未开始下单: {e}");
        }
        st.clone()
    };
    let markets = crate::live::markets_from_meta(&state.meta.lock().await.universe);
    let result = crate::live::run(&state.store, &snapshot, &markets, true).await;
    let mut st = state.live.lock().await;
    match result {
        Ok((r, records)) => {
            st.last_plan = r.plan_lines;
            st.records.extend(records);
            st.last_live = true;
            if let Some(r) = r.tp_ref { st.tp_ref = r; }
        }
        Err(e) => st.last_plan = vec![format!("TxFlow 自动调仓失败（本日不自动重试）: {e}")],
    }
    st.save(&state.live_path)
}

#[cfg(test)]
mod public_tests {
    use super::*;
    #[tokio::test]
    #[ignore = "read-only mainnet integration; requires network"]
    async fn mainnet_public_reads() {
        let client = Client::new("0x0000000000000000000000000000000000000001", None)
            .await
            .unwrap();
        let market = client.markets.get("CL-USDC").unwrap();
        assert_eq!(market.index, 53);
        assert_eq!(market.decimals, 2);
        let prices = client.prices(&[market.name.clone()]).await.unwrap();
        assert!(prices[&market.name] > 0.);
        let account = client.account().await.unwrap();
        assert_eq!(account.equity, 0.);
        assert!(client.open_orders().await.unwrap().is_empty());
        assert!(client
            .user_fills(10)
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(client.maintenance_margin().await.unwrap(), 0.);
    }
}
