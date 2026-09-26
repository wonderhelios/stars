use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

use crate::error::Result;
use crate::types::{FundingRate, Symbol, Ticker};

const WS_URL: &str = "wss://ws.okx.com:8443/ws/v5/public";
const PING_INTERVAL: Duration = Duration::from_secs(10);
const BATCH_SIZE: usize = 50;

#[derive(Debug, Clone)]
pub enum WsEvent {
    Ticker(Ticker),
    Funding(FundingRate),
    Subscribed(String, String), // (channel, inst_id)
    Error(String, String),      // (code, msg)
}

#[derive(Clone, Copy)]
pub enum Channel {
    Tickers,
    FundingRate,
}

impl Channel {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Tickers => "tickers",
            Self::FundingRate => "funding-rate",
        }
    }
}

pub struct WsClient {
    inst_ids: Vec<String>,
    channels: Vec<Channel>,
}

impl WsClient {
    pub fn new(inst_ids: Vec<String>, channels: Vec<Channel>) -> Self {
        Self { inst_ids, channels }
    }

    /// 连接、订阅、心跳、消费，一条龙。直到 receiver 被 drop 或连接断开。
    pub async fn run(&self, tx: mpsc::UnboundedSender<WsEvent>) -> Result<()> {
        info!("ws: HTTPS_PROXY = {:?}", std::env::var("HTTPS_PROXY").ok());

        let (stream, _) = connect_async(WS_URL).await?;
        let (mut write, mut read) = stream.split();

        // ---- 订阅（分批） ----
        let args: Vec<serde_json::Value> = self
            .channels
            .iter()
            .flat_map(|c| {
                self.inst_ids
                    .iter()
                    .map(move |id| serde_json::json!({"channel": c.as_str(), "instType":"SWAP","instId": id}))
            })
            .collect();

        info!("ws: subscribing {} args", args.len());
        for chunk in args.chunks(BATCH_SIZE) {
            let msg = serde_json::json!({"op": "subscribe", "args": chunk});
            write.send(Message::Text(msg.to_string().into())).await?;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        // ---- 心跳任务 ----
        let ping_task = tokio::spawn(async move {
            let mut tick = tokio::time::interval(PING_INTERVAL);
            loop {
                tick.tick().await;
                if write.send(Message::Text("ping".into())).await.is_err() {
                    break;
                }
            }
        });

        // ---- 消费循环 ----
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if text == "pong" {
                        continue;
                    }
                    handle_message(&text, &tx);
                }
                Ok(Message::Close(_)) => {
                    info!("ws: closed by server");
                    break;
                }
                Err(e) => {
                    error!("ws: {}", e);
                    break;
                }
                _ => {}
            }
            if tx.is_closed() {
                info!("ws: receiver dropped");
                break;
            }
        }

        ping_task.abort();
        Ok(())
    }
}

fn handle_message(text: &str, tx: &mpsc::UnboundedSender<WsEvent>) {
    tracing::debug!("ws raw: {}", text);
    let v: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            warn!("ws: bad json: {}", e);
            return;
        }
    };

    // 事件消息
    if let Some(event) = v.get("event").and_then(|e| e.as_str()) {
        match event {
            "subscribe" => {
                let ch = v["arg"]["channel"].as_str().unwrap_or("").to_string();
                let id = v["arg"]["instId"].as_str().unwrap_or("").to_string();
                let _ = tx.send(WsEvent::Subscribed(ch, id));
            }
            "error" => {
                let code = v["code"].as_str().unwrap_or("").to_string();
                let msg = v["msg"].as_str().unwrap_or("").to_string();
                error!("ws: okx error code={} msg={}", code, msg);
                let _ = tx.send(WsEvent::Error(code, msg));
            }
            _ => {}
        }
        return;
    }

    // 数据消息
    let channel = v["arg"]["channel"].as_str().unwrap_or("");
    let inst_id = v["arg"]["instId"].as_str().unwrap_or("");
    let Some(data) = v.get("data").and_then(|d| d.as_array()) else {
        return;
    };

    match channel {
        "tickers" => {
            for row in data {
                if let Some(t) = parse_ticker(row, inst_id) {
                    let _ = tx.send(WsEvent::Ticker(t));
                }
            }
        }
        "funding-rate" => {
            for row in data {
                if let Some(f) = parse_funding(row, inst_id) {
                    let _ = tx.send(WsEvent::Funding(f));
                }
            }
        }
        other => warn!("ws: unknown channel {}", other),
    }
}

fn parse_ticker(v: &serde_json::Value, inst_id: &str) -> Option<Ticker> {
    let symbol = Symbol::from_swap_inst_id(inst_id)?;
    Some(Ticker {
        symbol,
        last: dec(v, "last")?,
        bid: dec(v, "bidPx")?,
        ask: dec(v, "askPx")?,
        open_24h: dec(v, "open24h")?,
        high_24h: dec(v, "high24h")?,
        low_24h: dec(v, "low24h")?,
        volume_24h: dec(v, "vol24h")?,
        volume_quote_24h: dec(v, "volCcy24h")?,
        ts: v["ts"].as_str()?.parse().ok()?,
    })
}

fn parse_funding(v: &serde_json::Value, inst_id: &str) -> Option<FundingRate> {
    let symbol = Symbol::from_swap_inst_id(inst_id)?;
    Some(FundingRate {
        symbol,
        rate: dec(v, "fundingRate")?,
        next_time: v["nextFundingTime"].as_str().and_then(|s| s.parse().ok()),
        ts: v["ts"].as_str()?.parse().ok()?,
    })
}

/// 从 JSON 里取字符串字段并解析为 Decimal；空字符串返回 None
fn dec(v: &serde_json::Value, key: &str) -> Option<Decimal> {
    let s = v.get(key)?.as_str()?;
    if s.is_empty() {
        return None;
    }
    Decimal::from_str(s).ok()
}
