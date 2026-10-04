//! Minimal Hyperliquid execution layer for the momentum strategy.
//!
//! Read-only by default (`Exec::reader`); a signing client is only built when an
//! account + API-wallet key are supplied (`Exec::signer`). Order placement is
//! adapted from the same coding patterns used by the hyper-fly bot.

use alloy::{primitives::Address, signers::local::PrivateKeySigner};
use anyhow::{anyhow, bail, Context, Result};
use hyperliquid_rust_sdk::{
    BaseUrl, ClientLimit, ClientOrder, ClientOrderRequest, ExchangeClient, ExchangeDataStatus,
    ExchangeResponseStatus, InfoClient,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;
use uuid::Uuid;

#[derive(Clone, Copy, Debug)]
pub struct MarketInfo {
    pub sz_decimals: u32,
    pub max_leverage: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Pos {
    /// signed size (negative = short)
    pub size: f64,
    pub entry_px: f64,
    /// exchange-reported liquidation price, if any
    pub liq_px: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct Acct {
    pub equity: f64,
    pub positions: HashMap<String, Pos>,
}

pub struct Exec {
    info: InfoClient,
    http: reqwest::Client,
    trading: Option<ExchangeClient>,
    account: Option<Address>,
}

impl Exec {
    async fn info_post(&self, body: Value) -> Result<Value> {
        let v: Value = self
            .http
            .post("https://api.hyperliquid.xyz/info")
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(v)
    }

    pub async fn reader() -> Result<Self> {
        Self::reader_for(None).await
    }

    /// Read-only client bound to an account so equity/positions can be read
    /// without any signing key (used by dry runs).
    pub async fn reader_for(account: Option<&str>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()?;
        let info = InfoClient::new(Some(http.clone()), Some(BaseUrl::Mainnet)).await?;
        let account = match account {
            Some(a) => Some(Address::from_str(a).context("invalid HL_ACCOUNT_ADDRESS")?),
            None => None,
        };
        Ok(Self {
            info,
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
        let info = InfoClient::new(Some(http.clone()), Some(BaseUrl::Mainnet)).await?;
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
            info,
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
                    sz_decimals: m["szDecimals"].as_u64().unwrap_or(4) as u32,
                    max_leverage: m["maxLeverage"].as_u64().unwrap_or(10) as u32,
                },
            );
        }
        anyhow::ensure!(!out.is_empty(), "empty market universe");
        Ok(out)
    }

    /// Mid price from the l2 book.
    pub async fn mid(&self, coin: &str) -> Result<f64> {
        let book = self.info.l2_snapshot(coin.to_string()).await?;
        anyhow::ensure!(book.levels.len() == 2, "bad book for {coin}");
        let bid: f64 = book.levels[0]
            .first()
            .context("empty bids")?
            .px
            .parse()?;
        let ask: f64 = book.levels[1]
            .first()
            .context("empty asks")?
            .px
            .parse()?;
        anyhow::ensure!(bid > 0.0 && ask > 0.0, "invalid book for {coin}");
        Ok((bid + ask) / 2.0)
    }

    pub async fn account(&self) -> Result<Acct> {
        let account = self.account.context("read-only client has no account")?;
        let addr = account.to_string();
        let perp = self
            .info_post(json!({"type": "clearinghouseState", "user": addr}))
            .await?;
        let mut equity: f64 = perp["marginSummary"]["accountValue"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        // Unified accounts hold the USDC balance in the spot view.
        if let Ok(spot) = self
            .info_post(json!({"type": "spotClearinghouseState", "user": addr}))
            .await
        {
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
                    },
                );
            }
        }
        Ok(Acct { equity, positions })
    }

    pub async fn set_leverage(&self, coin: &str, leverage: u32) -> Result<()> {
        let trading = self.trading.as_ref().context("not a signing client")?;
        let resp = trading.update_leverage(leverage, coin, false, None).await?;
        match resp {
            ExchangeResponseStatus::Ok(_) => Ok(()),
            ExchangeResponseStatus::Err(e) => bail!("leverage rejected for {coin}: {e}"),
        }
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
        sz_decimals: u32,
    ) -> Result<ExchangeDataStatus> {
        let trading = self.trading.as_ref().context("not a signing client")?;
        let aggressive = if buy {
            mid * (1.0 + slippage)
        } else {
            mid * (1.0 - slippage)
        };
        let px = order_price(aggressive, sz_decimals, !buy);
        anyhow::ensure!(px > 0.0, "bad limit price for {coin}");
        let order = ClientOrderRequest {
            asset: coin.to_string(),
            is_buy: buy,
            reduce_only,
            limit_px: px,
            sz: size,
            cloid: Some(Uuid::new_v4()),
            order_type: ClientOrder::Limit(ClientLimit { tif: "Ioc".into() }),
        };
        let resp = trading.order(order, None).await?;
        one_status(resp)
    }
}

/// Tick rounding: at most 5 significant figures and never finer than the
/// size decimals allow (same rule the exchange applies).
pub fn order_price(px: f64, sz_decimals: u32, round_up: bool) -> f64 {
    if !px.is_finite() || px <= 0.0 {
        return 0.0;
    }
    let decimal_places = (4 - px.log10().floor() as i32).min(6 - sz_decimals as i32);
    let factor = 10_f64.powi(decimal_places);
    let scaled = px * factor;
    if round_up {
        scaled.ceil() / factor
    } else {
        scaled.floor() / factor
    }
}

/// Truncate a size to the coin's lot size.
pub fn round_size(size: f64, sz_decimals: u32) -> f64 {
    if !size.is_finite() || size <= 0.0 {
        return 0.0;
    }
    let factor = 10_f64.powi(sz_decimals as i32);
    (size * factor).floor() / factor
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
