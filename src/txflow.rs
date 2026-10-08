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

#[cfg(not(test))]
const BASE: &str = "https://api.txflow.com";
#[cfg(test)]
static MOCK_PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);
fn base_url() -> String {
    #[cfg(not(test))] { BASE.into() }
    #[cfg(test)] { format!("http://127.0.0.1:{}", MOCK_PORT.load(std::sync::atomic::Ordering::SeqCst)) }
}
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

/// TxFlow 的真实业绩汇总（全部来自交易所流水，不依赖我们自己的记账）。
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct PnlSummary {
    pub fills: usize,
    /// 已实现盈亏（交易所口径，不含手续费）
    pub realized: f64,
    pub fees: f64,
    pub volume: f64,
    /// 净入金（入金 − 出金）。流水接口可能截断，因此是下界。
    pub net_deposit: f64,
    pub ledger_rows: usize,
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
    _account_lock: Option<std::fs::File>,
}

/// 进程级限速闸门与业绩缓存。
///
/// **必须是全局的**：限速原本挂在 Client 实例上，而 Client 每次访问都会重建
/// （`Exec::reader_for` → `Client::new`），所以每个实例都带着一个全新的空闸门、
/// 空缓存 —— 限速等于没做，缓存永远不命中。实测总流量因此到 1.5~2 次/秒且
/// 不可控（回填 + 页面轮询各算各的）。放到进程级之后总速率硬顶在 MIN_GAP。
struct RequestScheduler {
    timing: Mutex<(Option<Instant>, Option<Instant>)>,
    execution_waiters: std::sync::atomic::AtomicUsize,
}
fn scheduler() -> &'static RequestScheduler {
    static SCHEDULER: OnceLock<RequestScheduler> = OnceLock::new();
    SCHEDULER.get_or_init(|| RequestScheduler {
        timing: Mutex::new((None,None)), execution_waiters: std::sync::atomic::AtomicUsize::new(0),
    })
}
struct PriorityTicket(Option<&'static RequestScheduler>);
impl Drop for PriorityTicket {
    fn drop(&mut self) {
        if let Some(s)=self.0 { s.execution_waiters.fetch_sub(1,std::sync::atomic::Ordering::SeqCst); }
    }
}
async fn schedule_request(execution: bool) {
    let s=scheduler();
    let _ticket=if execution {
        s.execution_waiters.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
        PriorityTicket(Some(s))
    } else { PriorityTicket(None) };
    loop {
        let mut timing=s.timing.lock().await;
        if !execution && s.execution_waiters.load(std::sync::atomic::Ordering::SeqCst)>0 {
            drop(timing); tokio::time::sleep(Duration::from_millis(1)).await; continue;
        }
        let now=Instant::now();
        let due=timing.0.into_iter().chain(timing.1).max().unwrap_or(now);
        if due>now { drop(timing);tokio::time::sleep_until(due).await;continue; }
        timing.0=Some(now+MIN_GAP);
        return;
    }
}
async fn observe_rate_limit(response: &reqwest::Response) {
    if response.status()!=reqwest::StatusCode::TOO_MANY_REQUESTS {return;}
    let seconds=response.headers().get(reqwest::header::RETRY_AFTER)
        .and_then(|v|v.to_str().ok()).and_then(|v|v.parse::<u64>().ok()).unwrap_or(3).min(86400);
    let mut timing=scheduler().timing.lock().await;
    let until=Instant::now()+Duration::from_secs(seconds);
    timing.1=Some(timing.1.map(|old|old.max(until)).unwrap_or(until));
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PnlCacheKey {
    // The endpoint identifies the venue/network. This cache belongs only to TxFlow.
    venue: String,
    account: String,
    // The interval is [since_ms, latest]; its rolling end is bounded by the TTL.
    since_ms: i64,
}
type PnlCache = Option<(std::time::Instant, PnlCacheKey, PnlSummary)>;
static PNL_CACHE: OnceLock<Mutex<PnlCache>> = OnceLock::new();

fn pnl_cache() -> &'static Mutex<PnlCache> {
    PNL_CACHE.get_or_init(|| tokio::sync::Mutex::new(None))
}

/// 两次 TxFlow 请求之间的最小间隔。
///
/// 300ms（3.3 req/s）实测仍然触发 429 —— TxFlow 的限制比这更严。放宽到 1s。
/// 代价是刷新 223 个市场要 ~4 分钟，但那件事 30 分钟才做一次；
/// 调仓只需约 20 个请求（~20 秒），可以接受。
const MIN_GAP: std::time::Duration = std::time::Duration::from_millis(1000);

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
        schedule_request(true).await;
        let response = http.post(endpoint).json(body)
            .timeout(Duration::from_secs(25)).send().await?;
        observe_rate_limit(&response).await;
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
        let http = reqwest::Client::builder()
                .user_agent("Mozilla/5.0 stars/0.3")
                .timeout(Duration::from_secs(25));
        // Tests use loopback mocks; a system proxy must not route them externally.
        #[cfg(test)]
        let http = http.no_proxy();
        let mut client = Self {
            http: http.build()?,
            account,
            signer,
            markets: HashMap::new(),
            _account_lock: None,
            owned_path: std::env::var("STARS_TXFLOW_ORDERS")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    let live_path = std::env::var("STARS_TXFLOW_LIVE")
                        .or_else(|_| std::env::var("STARS_LIVE"))
                        .unwrap_or_else(|_| "/var/lib/stars/live.json".into());
                    PathBuf::from(live_path).with_file_name("txflow-orders.json")
                }),
        };
        if client.signer.is_some() {
            let dir=Path::new("/var/tmp/stars-txflow-account-locks");
            std::fs::create_dir_all(dir).context("无法创建 TxFlow 账户锁目录")?;
            let lock=std::fs::OpenOptions::new().read(true).write(true).create(true)
                .open(dir.join(format!("{}.lock",client.account.to_ascii_lowercase())))
                .context("无法打开 TxFlow 账户锁")?;
            lock.try_lock().map_err(|e|anyhow::anyhow!("TxFlow 账户锁被其他实例占用或不可用: {e}"))?;
            client._account_lock=Some(lock);
        }
        client.markets = client.load_markets().await?;
        Ok(client)
    }

    pub async fn info(&self, payload: Value) -> Result<Value> {
        let mut last: Option<anyhow::Error> = None;
        let mut retried_429=false;
        for attempt in 0..3u32 {
            schedule_request(self.signer.is_some()).await;
            let resp = match self
                .http
                .post(format!("{}/info", base_url()))
                .json(&payload)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    last = Some(e.into());
                    tokio::time::sleep(Duration::from_millis(300 * (attempt as u64 + 1))).await;
                    continue;
                }
            };
            if resp.status()==reqwest::StatusCode::TOO_MANY_REQUESTS {
                observe_rate_limit(&resp).await;
                if retried_429 { bail!("TxFlow 429 Too Many Requests（已重试一次）"); }
                retried_429=true;
                last=Some(anyhow::anyhow!("TxFlow 429 Too Many Requests"));
                continue;
            }
            let result = async {
                data(resp.error_for_status()?.json::<Value>().await?)
            }
            .await;
            match result {
                Ok(v) => return Ok(v),
                Err(e) => last = Some(e),
            }
            if attempt < 2 {
                tokio::time::sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
            }
        }
        Err(last.unwrap_or_else(|| anyhow::anyhow!("TxFlow 请求失败")))
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
        let results = self.price_results(coins).await;
        Ok(results.into_iter().filter_map(|(coin, result)| match result {
            Ok(mid) => Some((coin, mid)),
            Err(e) => { tracing::warn!("TxFlow {coin} 无盘口: {e}"); None }
        }).collect())
    }

    pub async fn price_results(&self, coins: &[String]) -> Vec<(String, Result<f64>)> {
        use futures::{stream, StreamExt};
        stream::iter(coins.iter().cloned().map(|coin| async move {
            let result = self.book_price(coin.clone()).await.map(|(_, mid)| mid);
            (coin, result)
        })).buffer_unordered(2).collect().await
    }

    /// 存取款流水。用于算「净入金」—— 没有它就没法把入金和盈亏分开。
    pub async fn ledger(&self) -> Result<Vec<Value>> {
        anyhow::ensure!(!self.account.is_empty(), "未配置 TxFlow 账户");
        let v = self
            .info(json!({"type":"userNonFundingLedgerUpdates","user":self.account}))
            .await?;
        v.as_array().cloned().context("TxFlow ledger 格式错误")
    }

    /// 真实业绩汇总：已实现盈亏、手续费、成交量、净入金。
    ///
    /// 页面之前只显示未实现盈亏，于是一个赚了 $1,945 的账户看起来像在亏钱。
    /// `net_deposit` 单独记账，才能把「入金」和「策略赚的钱」分开。
    pub async fn pnl_summary(&self, since_ms: i64) -> Result<PnlSummary> {
        // 只统计**策略上线之后**的成交。账户在策略之前就有自己的交易，
        // 把它们算进来会把用户自己赚的钱记成策略业绩（真实发生过：
        // 显示 +$1,945，全部来自部署前的手动下单）。
        //
        // 60 秒缓存：页面轮询很密，每次都拉两次接口会触发限流。
        const TTL: std::time::Duration = std::time::Duration::from_secs(60);
        let key = PnlCacheKey {
            venue: base_url(),
            account: self.account.clone(),
            since_ms,
        };
        {
            let c = pnl_cache().lock().await;
            if let Some((t, cached_key, v)) = c.as_ref() {
                if t.elapsed() < TTL && cached_key == &key {
                    return Ok(v.clone());
                }
            }
        }
        let raw = self.user_fills(1000).await?;
        let fills: Vec<Value> = raw.as_array().context("TxFlow fills 格式错误")?.clone();
        let mut realized = 0.0;
        let mut fees = 0.0;
        let mut volume = 0.0;
        let mut used = 0usize;
        for f in &fills {
            let t = f["time"].as_i64().unwrap_or(0);
            if since_ms > 0 && t < since_ms {
                continue; // 策略上线前的成交，不计入策略业绩
            }
            used += 1;
            realized += number(&f["closedPnl"]).unwrap_or(0.0);
            fees += number(&f["fee"]).unwrap_or(0.0);
            volume += number(&f["px"]).unwrap_or(0.0).abs() * number(&f["sz"]).unwrap_or(0.0).abs();
        }
        // 流水接口可能只返回最近若干条；入金合计因此是**下界**，页面要标注。
        let led = self.ledger().await?;
        let mut net_deposit = 0.0;
        for l in &led {
            let d = &l["delta"];
            let amt = number(&d["amount"]).unwrap_or(0.0);
            match d["type"].as_str().unwrap_or("") {
                "deposit" => net_deposit += amt,
                "withdraw" => net_deposit -= amt,
                _ => {}
            }
        }
        let out = PnlSummary {
            fills: used,
            realized,
            fees,
            volume,
            net_deposit,
            ledger_rows: led.len(),
        };
        // Only a complete successful read may replace the cached summary.
        *pnl_cache().lock().await = Some((std::time::Instant::now(), key, out.clone()));
        Ok(out)
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
        schedule_request(true).await;
        let result = self
            .http
            .post(format!("{}/exchange", base_url()))
            .json(&body)
            .send()
            .await;
        let result = async {
            let response=result?;
            observe_rate_limit(&response).await;
            data(response.error_for_status()?.json::<Value>().await?)
        }.await;
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

/// 只刷新市场元数据（一次 perpMeta 请求）。
///
/// 调仓路径上用它，不要用整轮 refresh —— 后者要遍历几十个市场拉日线，
/// 在 1 秒限速下是分钟级，会把调仓拖到超时。元数据只有一个请求。
pub async fn refresh_meta_only(state: &crate::web::AppState) -> Result<()> {
    let _refresh = state.refresh_gate.lock().await;
    let client = Client::new("", None).await?;
    publish_metadata(state, &client).await
}

async fn publish_metadata(state: &crate::web::AppState, client: &Client) -> Result<()> {
    let mut markets: Vec<_> = client.markets.values().cloned().collect();
    markets.sort_by(|a, b| a.name.cmp(&b.name));
    let universe: Vec<_> = markets.iter().map(|m| crate::hl::CoinMeta {
        name: m.name.clone(), sz_decimals: m.decimals,
        max_leverage: m.max_leverage, is_delisted: false,
    }).collect();
    let mut meta = state.meta.lock().await;
    // A shrinking listing requires explicit review: never accept a silent partial response.
    anyhow::ensure!(universe.len() >= 20 && universe.len() >= meta.universe.len(),
        "TxFlow 元数据不完整：{} 个市场，之前 {} 个；拒绝交易", universe.len(), meta.universe.len());
    anyhow::ensure!(markets.iter().all(|m| !m.name.is_empty() && m.max_leverage > 0
        && (-8..=8).contains(&m.decimals)), "TxFlow 元数据字段非法");
    let mut indices = std::collections::HashSet::new();
    anyhow::ensure!(markets.iter().all(|m| indices.insert(m.index)), "TxFlow 重复市场索引");
    meta.liquid = universe.iter().map(|m| m.name.clone()).collect();
    meta.universe = universe;
    meta.refreshed_at = crate::live::now_ms_pub();
    meta.fee_taker = 0.00045;
    meta.fee_maker = 0.00015;
    Ok(())
}

fn rotation_start(store: &crate::store::Store, total: usize, _now: i64) -> Result<usize> {
    store.txflow_cursor(total,0)
}
fn advance_rotation(store: &crate::store::Store, total: usize, processed: usize) -> Result<()> {
    store.txflow_cursor(total,processed)?;Ok(())
}

async fn refresh_inner(state: &crate::web::AppState) -> Result<()> {
    let client = Client::new("", None).await?;
    publish_metadata(state, &client).await?;
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

    // 每轮只刷一部分，并逐轮轮换起点。
    //
    // 全量 223 个市场在 1 次/秒的限速下要 3.7 分钟，超过本函数外层 180 秒的
    // 硬超时，于是整轮被判失败、refreshed_at 不更新，页面永远显示"行情更新失败"。
    // 取 140 个（约 2.3 分钟）留出余量；起点每轮往后挪，所有币最终都会被覆盖，
    // 冷门币只是更新得慢一些（约 30~45 分钟一次），不会像"只刷流动币"那样丢历史。
    // 每个请求的实际耗时 = max(MIN_GAP, 网络延迟)。实测延迟约 1 秒，
    // 所以按 2 秒/请求估：60 个 ≈ 120 秒，稳稳落在 180 秒超时内。
    // （之前取 140，只按 1 秒算，实际 280 秒，必然超时。）
    const PER_CYCLE: usize = 60;
    let total = markets.len();
    let start = rotation_start(&state.store,total,now)?;
    let picked: Vec<_> = if total <= PER_CYCLE {
        markets.clone()
    } else {
        (0..PER_CYCLE).map(|k| markets[(start + k) % total].clone()).collect()
    };
    let markets = picked;
    // 进度总数必须等于**本轮实际要刷的数量**，否则 60 个刷完显示成
    // "60/140"，看起来像卡住/失败。上面那句只是在裁剪前占位。
    {
        let mut r = state.refresh.lock().await;
        r.coins_total = markets.len();
    }
    let mut failed = 0usize;
    let started = std::time::Instant::now();
    // 留 20 秒余量给收尾；超了就主动停止本轮（而不是让外层超时把整轮判失败，
    // 那样 refreshed_at 不会更新，页面会一直显示"行情更新失败"）。
    let budget = std::time::Duration::from_secs(155);
    let mut done = 0usize;
    for (i, m) in markets.iter().enumerate() {
        if started.elapsed() > budget {
            tracing::warn!(
                "TxFlow 回填达时间预算（{}/{}），本轮提前收尾，剩余下轮继续",
                done, markets.len()
            );
            break;
        }
        done += 1;
        state.refresh.lock().await.current = m.name.clone();
        // 单个币失败**不能**中止整轮回填。
        //
        // 之前用 `?` 直接冒泡：任何一个币拉到限流或空数据，整轮就断在那里，
        // 后面的 `meta.refreshed_at = now` 永远执行不到 —— 于是 background 每
        // 60 秒判定"数据过期"再重来一次，页面看起来**永远停在"回填数据"**。
        // （和调仓里"一个市场没盘口就整轮放弃"是同一个毛病。）
        let body = json!({"type":"candleSnapshot","req":{"coin":m.index.to_string(),"interval":"1d","startTime":yesterday - (cfg.lookback.max(30) as i64 + 5)*DAY,"endTime":now}});
        let result=client.info(body).await;
        advance_rotation(&state.store,total,1)?;
        let raw = match result {
            Ok(v) => v,
            Err(e) => {
                failed += 1;
                tracing::warn!("TxFlow 回填 {} 失败，跳过: {e}", m.name);
                state.refresh.lock().await.coins_done = i + 1;
                continue;
            }
        };
        let mut candles: Vec<crate::hl::Candle> = match serde_json::from_value(raw) {
            Ok(c) => c,
            Err(e) => {
                failed += 1;
                tracing::warn!("TxFlow 回填 {} 解析失败，跳过: {e}", m.name);
                state.refresh.lock().await.coins_done = i + 1;
                continue;
            }
        };
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
    meta.candle_available = universe.iter().map(|m| m.name.clone()).collect();
    if failed > 0 {
        tracing::warn!("TxFlow 回填完成，{failed}/{} 个市场失败（已跳过）", markets.len());
    }
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
        let recovering = state.live.lock().await.pending_recovery.is_some();
        if due && recovering {
            if let Err(e)=run_auto(&state).await { tracing::warn!("TxFlow 优先恢复失败: {e}"); }
        } else if due || stale {
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
        // 每个循环补一个净值点（record_equity 内部按小时去重，所以一小时只会写一次）。
        // 这样曲线与页面是否打开无关 —— 之前只在 handler 里记，不看页面就断档。
        let _ = crate::live::record_equity_now(&state.live, &state.live_path).await;
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

fn auto_due(st: &crate::live::LiveState, now: i64) -> bool {
    st.config.armed && (st.pending_recovery.is_some() || (st.config.auto_run
        && now % DAY >= 5 * 60 * 1000
        && st.last_run_at.map(|t| t / DAY < now / DAY).unwrap_or(true)))
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
        st.begin_txflow_execution(&state.live_path)?;
        st.last_plan = vec!["TxFlow 自动调仓执行中…".into()];
        if let Err(e) = st.save(&state.live_path) {
            *st = previous;
            anyhow::bail!("无法保存自动调仓记录，未开始下单: {e}");
        }
        previous
    };
    refresh_meta_only(state).await?;
    let markets = crate::live::markets_from_meta(&state.meta.lock().await.universe);
    let result = crate::live::run(&state.store, state.hl_store.as_deref(), &snapshot, &markets, true).await;
    let mut st = state.live.lock().await;
    match result {
        Ok((r, records)) => {
            st.complete_txflow_execution(r.aborted, now);
            st.last_plan = r.plan_lines;
            st.records.extend(records);
            st.last_live = true;
            if let Some(r) = r.tp_ref { st.tp_ref = r; }
        }
        Err(e) => st.last_plan = vec![format!("TxFlow 自动调仓失败（等待优先恢复）: {e}")],
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

#[cfg(test)] mod fix_regressions {
 use super::*;
 use axum::{Router,routing::post,Json};
 use std::sync::atomic::{AtomicUsize,Ordering};
 static MOCK_LOCK: Mutex<()> = Mutex::const_new(());
 #[tokio::test] async fn backfill_preserves_complete_metadata() {
 let _mock = MOCK_LOCK.lock().await;
 let writes=Arc::new(AtomicUsize::new(0)); let w=writes.clone();
 let app=Router::new().route("/info",post(|Json(v):Json<Value>| async move {
 match v["type"].as_str().unwrap() {
 "perpMeta"=>Json(json!({"universe":(0..80).map(|i|json!({"name":format!("C{i}-USDC"),"index":i,"priceTick":0.01,"szDecimals":2,"maxLeverage":10})).collect::<Vec<_>>()})),
 "candleSnapshot"=>Json(json!([])),
 "clearinghouseState"=>Json(json!({"marginSummary":{"accountValue":"2900"},"assetPositions":[]})),
 _=>panic!("unexpected read {v}")
 }
 })).route("/exchange",post(move |Json(v):Json<Value>| {let w=w.clone(); async move {
 if v["action"]["type"]=="order" {w.fetch_add(1,Ordering::SeqCst); Json(json!({"status":"ok","response":{"data":{"statuses":[{"filled":{"totalSz":"2","avgPx":"100","oid":1}}]}}}))}
 else {Json(json!({"status":"ok"}))}
 }}));
 let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
 MOCK_PORT.store(listener.local_addr().unwrap().port(), Ordering::SeqCst);
 let server=tokio::spawn(async move {axum::serve(listener,app).await.unwrap()});
 let root=std::env::temp_dir().join(format!("stars-audit-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&root).unwrap();
 let state=crate::web::AppState {
 store:Arc::new(crate::store::Store::open(&root.join("db")).unwrap()),hl_store:None,
 paper:Arc::new(Mutex::new(Default::default())),paper_path:Arc::new(root.join("paper")),
 live:Arc::new(Mutex::new(Default::default())),live_path:Arc::new(root.join("live")),
 meta:Arc::new(Mutex::new(Default::default())),refresh:Arc::new(Mutex::new(Default::default())),http:reqwest::Client::builder().no_proxy().build().unwrap(),
 exec_gate:Arc::new(Mutex::new(())),refresh_gate:Arc::new(Mutex::new(())),run_gate:Arc::new(Mutex::new(()))};
 refresh_meta_only(&state).await.unwrap(); assert_eq!(state.meta.lock().await.universe.len(),80);
 refresh_inner(&state).await.unwrap(); assert_eq!(state.meta.lock().await.universe.len(),80);
 assert_eq!(state.refresh.lock().await.phase,"ready"); println!("full meta 80 -> backfill meta 0, status ready");
 server.abort(); let _=server.await; std::fs::remove_dir_all(root).unwrap();
 }
 #[tokio::test] async fn partial_books_preserve_successful_prices() {
 let _mock = MOCK_LOCK.lock().await;
 let app=Router::new().route("/info",post(|Json(v):Json<Value>| async move {
     Json(if v["coin"] == "0" { json!({"levels":[[{"px":"99"}],[{"px":"101"}]]}) } else { json!({"levels":[[],[]]}) })
 }));
 let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
 MOCK_PORT.store(listener.local_addr().unwrap().port(), Ordering::SeqCst);
 let server=tokio::spawn(async move {axum::serve(listener,app).await.unwrap()});
 let markets = (0..2).map(|i| { let name=format!("C{i}-USDC"); (name.clone(), Market { name,index:i,decimals:2,max_leverage:10,tick:0.01,max_order_size:1000. }) }).collect();
 let client=Client { http:reqwest::Client::builder().no_proxy().build().unwrap(),account:String::new(),signer:None,markets,owned_path:PathBuf::new(),_account_lock:None };
 let result=client.prices(&["C0-USDC".into(),"C1-USDC".into()]).await;
 server.abort(); let _=server.await;
 let prices=result.expect("one failed book must not erase the successful book");
 assert_eq!(prices.len(),1); assert_eq!(prices["C0-USDC"],100.);
 }
 async fn execution_scenario(mode: &str) -> (crate::trader::Outcome, usize) {
 let _mock = MOCK_LOCK.lock().await;
 let sizes=Arc::new(Mutex::new(HashMap::<String,f64>::new()));
 if mode == "residual" { sizes.lock().await.insert("C0-USDC".into(),10.); }
 let reads=Arc::new(AtomicUsize::new(0)); let r=reads.clone(); let a=sizes.clone();
 let writes=Arc::new(AtomicUsize::new(0)); let w=writes.clone(); let b=sizes.clone();
 let scenario=mode.to_string();
 let app=Router::new().route("/info",post(move |Json(v):Json<Value>| {let a=a.clone(); let r=r.clone(); async move {
     assert_eq!(v["type"],"clearinghouseState"); r.fetch_add(1,Ordering::SeqCst);
     let ps:Vec<_>=a.lock().await.iter().filter(|(_,sz)|sz.abs()>1e-9).map(|(coin,sz)|json!({"position":{"coin":coin,"szi":sz.to_string(),"entryPx":"100","positionValue":(sz.abs()*100.).to_string(),"unrealizedPnl":"0","leverage":{"type":"cross","value":3}}})).collect();
     Json(json!({"marginSummary":{"accountValue":"2900"},"assetPositions":ps}))
 }})).route("/exchange",post(move |Json(v):Json<Value>| {let b=b.clone();let w=w.clone();let scenario=scenario.clone(); async move {
     use axum::response::IntoResponse;
     if v["action"]["type"] != "order" { return Json(json!({"status":"ok"})).into_response(); }
     let n=w.fetch_add(1,Ordering::SeqCst);
     if scenario=="429" && n==1 {return (axum::http::StatusCode::TOO_MANY_REQUESTS,"limited").into_response();}
     let o=&v["action"]["orders"][0]; let requested=o["s"].as_str().unwrap().parse::<f64>().unwrap();
     let filled=if scenario=="amplified" {requested*1000.} else if scenario=="rounding" {requested+0.009} else if scenario=="partial" || scenario=="residual" { requested.min(2.) } else {requested};
     let coin=format!("C{}-USDC",o["a"].as_u64().unwrap());
     *b.lock().await.entry(coin).or_default() += if o["b"]==true {filled} else {-filled};
     if scenario=="extra_balanced" && n==1 {let mut positions=b.lock().await;positions.insert("C6-USDC".into(),100.);positions.insert("C7-USDC".into(),-100.);}
     if scenario=="timeout_filled" && n==1 {tokio::time::sleep(Duration::from_millis(200)).await;}
     Json(json!({"status":"ok","response":{"data":{"statuses":[{"filled":{"totalSz":filled.to_string(),"avgPx":"100","oid":n+1}}]}}})).into_response()
 }}));
 let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
 MOCK_PORT.store(listener.local_addr().unwrap().port(), Ordering::SeqCst);
 let server=tokio::spawn(async move {axum::serve(listener,app).await.unwrap()});
 let markets=(0..8).map(|i| {let name=format!("C{i}-USDC"); (name.clone(),Market{name,index:i,decimals:2,max_leverage:10,tick:0.01,max_order_size:1000.})}).collect();
 let client=Client {http:reqwest::Client::builder().no_proxy().timeout(Duration::from_millis(100)).build().unwrap(),account:"mock".into(),signer:Some(PrivateKeySigner::random()),markets,owned_path:PathBuf::new(),_account_lock:None};
 let exec=crate::exchange::Exec::test_txflow(client);
 let mut acct=Acct {equity:if mode=="amplified" {2000./2.7} else {2900.},..Default::default()};
 if mode=="residual" {acct.positions.insert("C0-USDC".into(),Pos{size:10.,..Default::default()});}
 let cfg=crate::trader::TradeConfig{rebalance_slices:1,..Default::default()};
 let weights=vec![("C0-USDC".into(),if mode=="residual" {-0.5} else {0.5}),("C1-USDC".into(),-0.5)];
 let ms=(0..2).map(|i|(format!("C{i}-USDC"),crate::exchange::MarketInfo{sz_decimals:2,max_leverage:10})).collect();
 let mids=(0..2).map(|i|(format!("C{i}-USDC"),100.)).collect();
 let mut plan=crate::trader::build_plan(&weights,&acct,&ms,&mids,&cfg,None);
 if mode=="amplified" || mode=="rounding" {for order in &mut plan.orders {order.target=if order.buy {10.}else{-10.};order.size=10.;order.notional=1000.;}assert_eq!(plan.orders.len(),2);}

 let outcome=crate::trader::execute(&exec,&plan,&cfg,&ms,true).await.unwrap();
 server.abort(); let _=server.await; (outcome,reads.load(Ordering::SeqCst))
 }
 #[tokio::test] async fn report5_realistic_rounding_is_accepted() {let(o,_)=execution_scenario("rounding").await;assert!(!o.aborted,"0.009 unit / $0.90 rounding drift must be accepted");}
 #[tokio::test] async fn report5_unplanned_balanced_gross_must_abort() {let(o,reads)=execution_scenario("extra_balanced").await;println!("prelim={:?} orders={:?}",o.prelim,o.orders);assert!(o.aborted,"extra +/-$10000 must abort despite zero net");assert!(reads>=2);}
 #[tokio::test] async fn report5_balanced_1000x_must_abort_and_recover() {
    let (o,reads)=execution_scenario("amplified").await;
    assert!(o.aborted,"targets +10/-10, actual +10000/-10000 must abort");assert!(reads>=2);
    let mut state=crate::live::LiveState::default();state.pending_recovery=Some(crate::live::RecoveryState{account:"mock".into(),started_at:1,reason:"in progress".into()});state.complete_txflow_execution(o.aborted,2);assert!(state.pending_recovery.is_some());assert!(state.last_run_at.is_none());
 }
 #[tokio::test] async fn execution_partial_fill_requires_final_account_verification() {
    let (o,reads)=execution_scenario("partial").await;
    assert!(o.aborted,"partial IOC is not a completed risk target"); assert!(reads>=2,"missing final account verification");
 }
 #[tokio::test] async fn execution_second_leg_429_requires_final_account_verification() {
    let (o,reads)=execution_scenario("429").await;
    assert!(o.aborted); assert!(reads>=2,"missing final account verification after first leg filled");
 }
 #[tokio::test] async fn execution_timeout_already_filled_is_confirmed_from_account() {
    let (o,reads)=execution_scenario("timeout_filled").await;
    assert!(!o.aborted,"account confirms both targets despite transport timeout"); assert!(reads>=2);
 }
 #[tokio::test] async fn execution_residual_reverse_position_is_not_success() {
    let (o,reads)=execution_scenario("residual").await;
    assert!(o.aborted,"residual opposite position is not success"); assert!(reads>=2);
 }
 #[test] fn pending_recovery_takes_priority_over_today_success_timestamp() {
    let now=10*DAY+6*60*1000;
    let mut st=crate::live::LiveState::default();
    st.config.armed=true; st.config.auto_run=true; st.last_run_at=Some(now);
    st.pending_recovery=Some(crate::live::RecoveryState{account:String::new(),started_at:now,reason:"partial".into()});
    assert!(auto_due(&st,now),"pending recovery must not wait until tomorrow or next slice");
 }
 #[tokio::test] async fn scheduler_reads_retry_429_only_once() {
 let _mock=MOCK_LOCK.lock().await;
 let calls=Arc::new(AtomicUsize::new(0));let c=calls.clone();
 let app=Router::new().route("/info",post(move || {let c=c.clone();async move {
     c.fetch_add(1,Ordering::SeqCst); (axum::http::StatusCode::TOO_MANY_REQUESTS,[("retry-after","1")],"limited")
 }}));
 let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); MOCK_PORT.store(listener.local_addr().unwrap().port(),Ordering::SeqCst);
 let server=tokio::spawn(async move {axum::serve(listener,app).await.unwrap()});
 let client=Client{http:reqwest::Client::builder().no_proxy().build().unwrap(),account:String::new(),signer:None,markets:HashMap::new(),owned_path:PathBuf::new(),_account_lock:None};
 assert!(client.info(json!({"type":"test"})).await.is_err());
 server.abort();let _=server.await;
 assert_eq!(calls.load(Ordering::SeqCst),2,"429 permits exactly one retry");
 }
 #[tokio::test] async fn scheduler_info_submit_approve_share_spacing() {
 let _mock=MOCK_LOCK.lock().await;
 let arrivals=Arc::new(Mutex::new(Vec::<Instant>::new()));let a=arrivals.clone();let b=arrivals.clone();
 let app=Router::new().route("/info",post(move ||{let a=a.clone();async move{a.lock().await.push(Instant::now());Json(json!({}))}}))
 .route("/exchange",post(move ||{let b=b.clone();async move{b.lock().await.push(Instant::now());Json(json!({"status":"ok"}))}}));
 let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); MOCK_PORT.store(listener.local_addr().unwrap().port(),Ordering::SeqCst);
 let server=tokio::spawn(async move {axum::serve(listener,app).await.unwrap()});
 let client=Client{http:reqwest::Client::builder().no_proxy().build().unwrap(),account:"mock".into(),signer:Some(PrivateKeySigner::random()),markets:HashMap::new(),owned_path:PathBuf::new(),_account_lock:None};
 client.info(json!({"type":"test"})).await.unwrap();
 client.submit("test", &json!({})).await.unwrap();
 approve_agent(&client.http,&json!({}),&format!("{}/exchange",base_url())).await.unwrap();
 server.abort();let _=server.await;
 let times=arrivals.lock().await;
 assert_eq!(times.len(),3);
 assert!(times.windows(2).all(|w|w[1]-w[0]>=Duration::from_millis(990)),"read/write/approve did not share the one-second scheduler: {times:?}");
 }
 #[tokio::test] async fn account_lock_child() {
     let Ok(key)=std::env::var("TXFLOW_FIX_CHILD_KEY") else {return;};
     MOCK_PORT.store(std::env::var("TXFLOW_FIX_CHILD_PORT").unwrap().parse().unwrap(),Ordering::SeqCst);
     let result=Client::new(&std::env::var("TXFLOW_FIX_CHILD_ACCOUNT").unwrap(),Some(Path::new(&key))).await;
     assert!(result.err().map(|e|e.to_string().contains("账户锁")).unwrap_or(false),"second process must fail at account lock");
 }
 #[tokio::test] async fn account_lock_excludes_a_second_process_and_releases_on_drop() {
     let _mock=MOCK_LOCK.lock().await;
     let app=Router::new().route("/info",post(|| async {Json(json!({"universe":(0..80).map(|i|json!({"name":format!("C{i}-USDC"),"index":i,"priceTick":0.01,"szDecimals":2,"maxLeverage":10})).collect::<Vec<_>>()}))}));
     let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let port=listener.local_addr().unwrap().port();MOCK_PORT.store(port,Ordering::SeqCst);
     let server=tokio::spawn(async move {axum::serve(listener,app).await.unwrap()});
     let root=std::env::temp_dir().join(format!("stars-lock-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&root).unwrap();
     let key=root.join("key");std::fs::write(&key,format!("{:#x}",PrivateKeySigner::random().to_bytes())).unwrap();
     let owned=root.join("orders.json");let old=std::env::var_os("STARS_TXFLOW_ORDERS");std::env::set_var("STARS_TXFLOW_ORDERS",&owned);
     let account=PrivateKeySigner::random().address().to_string();
     let first=Client::new(&account,Some(&key)).await.unwrap();
     let child=tokio::process::Command::new(std::env::current_exe().unwrap())
         .arg("--exact").arg("txflow::fix_regressions::account_lock_child").arg("--nocapture")
         .env("TXFLOW_FIX_CHILD_ACCOUNT",&account).env("TXFLOW_FIX_CHILD_KEY",&key).env("TXFLOW_FIX_CHILD_PORT",port.to_string()).env("STARS_TXFLOW_ORDERS",&owned)
         .output().await.unwrap();
     drop(first);
     let next=Client::new(&account,Some(&key)).await;
     if let Some(old)=old {std::env::set_var("STARS_TXFLOW_ORDERS",old);} else {std::env::remove_var("STARS_TXFLOW_ORDERS");}
     let released=next.is_ok();drop(next);
     let _=std::fs::remove_file(Path::new("/var/tmp/stars-txflow-account-locks").join(format!("{}.lock",account.to_ascii_lowercase())));
     server.abort();let _=server.await;std::fs::remove_dir_all(root).unwrap();
     assert!(child.status.success(),"child stdout: {} stderr: {}",String::from_utf8_lossy(&child.stdout),String::from_utf8_lossy(&child.stderr));
     assert!(released,"lock must release when owner exits");
 }
 async fn pnl_scenario() -> (f64,f64,bool) {
 let _mock=MOCK_LOCK.lock().await;
 *pnl_cache().lock().await=None;
 let app=Router::new().route("/info",post(|Json(v):Json<Value>| async move {
     use axum::response::IntoResponse;
     if v["type"]=="userFills" && v["user"]=="bad" {return (axum::http::StatusCode::SERVICE_UNAVAILABLE,"failed").into_response();}
     if v["type"]=="userFills" {return Json(json!([{"coin":"C0-USDC","time":9001,"closedPnl":if v["user"]=="A" {"1"}else{"2"},"fee":"0","px":"100","sz":"1"}])).into_response();}
     assert_eq!(v["type"],"userNonFundingLedgerUpdates");Json(json!([])).into_response()
 }));
 let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();MOCK_PORT.store(listener.local_addr().unwrap().port(),Ordering::SeqCst);
 let server=tokio::spawn(async move {axum::serve(listener,app).await.unwrap()});
 let mut client=Client{http:reqwest::Client::builder().no_proxy().build().unwrap(),account:"A".into(),signer:None,markets:HashMap::from([("C0-USDC".into(),Market{name:"C0-USDC".into(),index:0,decimals:2,max_leverage:10,tick:0.01,max_order_size:1000.})]),owned_path:PathBuf::new(),_account_lock:None};
 let a=client.pnl_summary(9000).await.unwrap();client.account="B".into();let b=client.pnl_summary(9000).await.unwrap();
 client.account="bad".into();let error=client.pnl_summary(8999).await;
 server.abort();let _=server.await;
 (a.realized,b.realized,error.is_err())
 }
 #[tokio::test] async fn pnl_cache_is_account_scoped() {
     let (a,b,_)=pnl_scenario().await;assert_eq!(a,1.);assert_eq!(b,2.,"same interval must not reuse another account PNL");
 }
 #[tokio::test] async fn pnl_read_errors_are_not_zero_profit() {
     let (_,_,failed)=pnl_scenario().await;assert!(failed,"read errors must not become cached zero profit");
 }
 #[tokio::test]
 async fn pnl_ledger_failure_recovery_and_venue_interval_isolation() {
     let _mock = MOCK_LOCK.lock().await;
     *pnl_cache().lock().await = None;
     let mode = Arc::new(AtomicUsize::new(0));
     let calls = Arc::new(AtomicUsize::new(0));
     let handler_mode = mode.clone();
     let handler_calls = calls.clone();
     let app = Router::new().route("/info", post(move |Json(v): Json<Value>| {
         let mode = handler_mode.clone();
         let calls = handler_calls.clone();
         async move {
             use axum::response::IntoResponse;
             if v["type"] == "userFills" {
                 calls.fetch_add(1, Ordering::SeqCst);
                 return Json(json!([{"coin":"C0-USDC","time":9001,"closedPnl":"7"}])).into_response();
             }
             match mode.load(Ordering::SeqCst) {
                 0 => (axum::http::StatusCode::SERVICE_UNAVAILABLE, "ledger failed").into_response(),
                 1 => Json(json!({"unexpected":"not an array"})).into_response(),
                 _ => Json(json!([{"delta":{"type":"deposit","amount":"12"}}])).into_response(),
             }
         }
     }));
     let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
     MOCK_PORT.store(listener.local_addr().unwrap().port(), Ordering::SeqCst);
     let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
     let client = Client {
         http: reqwest::Client::builder().no_proxy().build().unwrap(),
         account: "pnl-recovery".into(), signer: None,
         markets: HashMap::from([("C0-USDC".into(), Market {
             name: "C0-USDC".into(), index: 0, decimals: 2,
             max_leverage: 10, tick: 0.01, max_order_size: 1000.,
         })]),
         owned_path: PathBuf::new(), _account_lock: None,
     };
     assert!(client.pnl_summary(9000).await.is_err());
     assert!(pnl_cache().lock().await.is_none());
     mode.store(1, Ordering::SeqCst);
     assert!(client.pnl_summary(9000).await.unwrap_err().to_string().contains("ledger 格式错误"));
     assert!(pnl_cache().lock().await.is_none());
     mode.store(2, Ordering::SeqCst);
     let recovered = client.pnl_summary(9000).await.unwrap();
     assert_eq!(recovered.realized, 7.);
     assert_eq!(recovered.net_deposit, 12.);
     let successful_calls = calls.load(Ordering::SeqCst);
     client.pnl_summary(9000).await.unwrap();
     assert_eq!(calls.load(Ordering::SeqCst), successful_calls);
     // Simulate a cache entry belonging to a different venue/network.
     pnl_cache().lock().await.as_mut().unwrap().1.venue = "other-venue".into();
     assert_eq!(client.pnl_summary(9000).await.unwrap().realized, 7.);
     assert_eq!(calls.load(Ordering::SeqCst), successful_calls + 1);
     assert_eq!(client.pnl_summary(9002).await.unwrap().realized, 0.);
     assert_eq!(calls.load(Ordering::SeqCst), successful_calls + 2);
     server.abort();
     let _ = server.await;
 }
 #[test] fn persistent_rotation_covers_resonant_universe_and_survives_restart() {
     let root=std::env::temp_dir().join(format!("stars-cursor-{}",uuid::Uuid::new_v4()));let path=root.join("db");
     let store=crate::store::Store::open(&path).unwrap();let mut seen=std::collections::HashSet::new();
     for cycle in 0..1000 {
         let start=rotation_start(&store,240,cycle*1920*1000).unwrap();
         for k in 0..60 {seen.insert((start+k)%240);}
         advance_rotation(&store,240,60).unwrap();
     }
     assert_eq!(seen.len(),240,"resonant wall clock must not starve 180 markets");
     advance_rotation(&store,240,7).unwrap();let cursor=rotation_start(&store,240,0).unwrap();drop(store);
     let restarted=crate::store::Store::open(&path).unwrap();assert_eq!(rotation_start(&restarted,240,0).unwrap(),cursor);
     drop(restarted);std::fs::remove_dir_all(root).unwrap();
 }
 #[tokio::test] async fn equity_and_pnl_reader_select_txflow_without_signing_key() {
     let _mock=MOCK_LOCK.lock().await;
     let app=Router::new().route("/info",post(|| async {Json(json!({"universe":(0..80).map(|i|json!({"name":format!("C{i}-USDC"),"index":i,"priceTick":0.01,"szDecimals":2,"maxLeverage":10})).collect::<Vec<_>>()}))}));
     let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();MOCK_PORT.store(listener.local_addr().unwrap().port(),Ordering::SeqCst);
     let server=tokio::spawn(async move {axum::serve(listener,app).await.unwrap()});
     let mut cfg=crate::live::LiveConfig{txflow:true,account:"0x0707070707070707070707070707070707070707".into(),..Default::default()};
     let tx=crate::exchange::Exec::reader_config(&cfg).await.unwrap();cfg.txflow=false;
     let hl=crate::exchange::Exec::reader_config(&cfg).await.unwrap();
     server.abort();let _=server.await;
     assert!(tx.is_txflow(),"TxFlow equity/PNL factory selected Hyperliquid");assert!(!hl.is_txflow());
 }
 #[tokio::test] async fn incomplete_metadata_is_rejected_atomically() {
 let _mock = MOCK_LOCK.lock().await;
 let w=Arc::new(AtomicUsize::new(0));
 let app=Router::new().route("/info",post(|Json(v):Json<Value>| async move {
 match v["type"].as_str().unwrap() {
 "perpMeta"=>Json(json!({"universe":(0..5).map(|i|json!({"name":format!("C{i}-USDC"),"index":i,"priceTick":0.01,"szDecimals":2,"maxLeverage":10})).collect::<Vec<_>>()})),
 "candleSnapshot"=>Json(json!([])),
 "clearinghouseState"=>Json(json!({"marginSummary":{"accountValue":"2900"},"assetPositions":[]})),
 _=>panic!("unexpected read {v}")
 }
 })).route("/exchange",post(move |Json(v):Json<Value>| {let w=w.clone(); async move {
 if v["action"]["type"]=="order" {w.fetch_add(1,Ordering::SeqCst); Json(json!({"status":"ok","response":{"data":{"statuses":[{"filled":{"totalSz":"2","avgPx":"100","oid":1}}]}}}))}
 else {Json(json!({"status":"ok"}))}
 }}));
 let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
 MOCK_PORT.store(listener.local_addr().unwrap().port(), Ordering::SeqCst);
 let server=tokio::spawn(async move {axum::serve(listener,app).await.unwrap()});
 let root=std::env::temp_dir().join(format!("stars-audit-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&root).unwrap();
 let state=crate::web::AppState {
 store:Arc::new(crate::store::Store::open(&root.join("db")).unwrap()),hl_store:None,
 paper:Arc::new(Mutex::new(Default::default())),paper_path:Arc::new(root.join("paper")),
 live:Arc::new(Mutex::new(Default::default())),live_path:Arc::new(root.join("live")),
 meta:Arc::new(Mutex::new(Default::default())),refresh:Arc::new(Mutex::new(Default::default())),http:reqwest::Client::builder().no_proxy().build().unwrap(),
 exec_gate:Arc::new(Mutex::new(())),refresh_gate:Arc::new(Mutex::new(())),run_gate:Arc::new(Mutex::new(()))};
 let result = refresh_meta_only(&state).await;
 server.abort(); let _=server.await; std::fs::remove_dir_all(root).unwrap();
 assert!(result.is_err(), "five-market partial metadata must fail closed");
 assert!(state.meta.lock().await.universe.is_empty());
 }
}
