use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use std::time::{Duration, Instant};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

use crate::error::{Error, Result};
use crate::okx::rest::parse_dec;
use crate::state::SharedState;

const WS_URL: &str = "wss://ws.okx.com:8443/ws/v5/public";
const PING_INTERVAL: Duration = Duration::from_secs(20);
const READ_TIMEOUT: Duration = Duration::from_secs(35);
const TICKER_STALE_TIMEOUT: Duration = Duration::from_secs(90);
const BATCH_SIZE: usize = 50;
const BATCH_INTERVAL: Duration = Duration::from_millis(200);

/// 每个连接负责的 instId 数量。OKX 单连接订阅数有隐性上限，拆组更稳。
const GROUP_SIZE: usize = 200;

/// 启动 WebSocket 采集：拉取全部 SWAP instId → 分组 → 每组一个独立连接（自动重连）
pub async fn run_forever(state: SharedState) -> Result<()> {
    // 1. 拉取全部 SWAP instId
    let rest = crate::okx::RestClient::new();
    let inst_ids = loop {
        match rest.all_swap_inst_ids().await {
            Ok(ids) if !ids.is_empty() => break ids,
            Ok(_) => {
                warn!("empty instrument list, retry in 5s");
            }
            Err(e) => {
                error!("fetch instruments failed: {}, retry in 5s", e);
            }
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    };

    info!("loaded {} SWAP instruments", inst_ids.len());

    // 2. 分组，每组一个常驻 task（内部自动重连）
    let groups: Vec<Vec<String>> = inst_ids.chunks(GROUP_SIZE).map(|c| c.to_vec()).collect();

    let mut handles = Vec::with_capacity(groups.len());
    for (idx, group) in groups.into_iter().enumerate() {
        let state = state.clone();
        let handle = tokio::spawn(async move {
            run_group_forever(state, group, idx).await;
        });
        handles.push(handle);
    }

    // 3. join 全部（实际上不会自然退出）
    for h in handles {
        let _ = h.await;
    }
    Ok(())
}

async fn run_group_forever(state: SharedState, inst_ids: Vec<String>, group_id: usize) {
    loop {
        info!(
            "[group {}] connecting with {} instruments",
            group_id,
            inst_ids.len()
        );
        match run_group_once(state.clone(), &inst_ids).await {
            Ok(()) => warn!("[group {}] closed, reconnecting in 3s", group_id),
            Err(e) => error!("[group {}] error: {}, reconnecting in 3s", group_id, e),
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn run_group_once(state: SharedState, inst_ids: &[String]) -> Result<()> {
    let (stream, _) = connect_async(WS_URL).await?;
    let (mut write, mut read) = stream.split();

    // 订阅 tickers + funding-rate
    let args: Vec<serde_json::Value> = inst_ids
        .iter()
        .flat_map(|id| {
            ["tickers", "funding-rate"].iter().map(move |ch| {
                serde_json::json!({
                    "channel": ch,
                    "instType": "SWAP",
                    "instId": id
                })
            })
        })
        .collect();

    for chunk in args.chunks(BATCH_SIZE) {
        let msg = serde_json::json!({"op": "subscribe", "args": chunk});
        write
            .send(Message::Text(msg.to_string()))
            .await
            .map_err(|e| Error::Msg(format!("ws send: {}", e)))?;
        tokio::time::sleep(BATCH_INTERVAL).await;
    }

    // 心跳
    let ping_task = tokio::spawn(async move {
        let mut tick = tokio::time::interval(PING_INTERVAL);
        tick.tick().await;
        loop {
            tick.tick().await;
            if write.send(Message::Text("ping".into())).await.is_err() {
                break;
            }
        }
    });

    // 收到 pong 只能说明连接还在；若行情订阅长期没有推送，也必须重连。
    let mut last_ticker = Instant::now();
    loop {
        let msg = match tokio::time::timeout(READ_TIMEOUT, read.next()).await {
            Ok(Some(msg)) => msg,
            Ok(None) => break,
            Err(_) => {
                ping_task.abort();
                return Err(Error::Msg("okx ws read timeout".into()));
            }
        };
        match msg {
            Ok(Message::Text(text)) => {
                if text != "pong" && handle_text(&text, &state).await {
                    last_ticker = Instant::now();
                }
            }
            Ok(Message::Close(_)) => break,
            Err(e) => {
                ping_task.abort();
                return Err(Error::Ws(e));
            }
            _ => {}
        }
        if last_ticker.elapsed() > TICKER_STALE_TIMEOUT {
            ping_task.abort();
            return Err(Error::Msg("okx ws ticker feed stale".into()));
        }
    }

    ping_task.abort();
    Ok(())
}

async fn handle_text(text: &str, state: &SharedState) -> bool {
    let v: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            warn!("ws bad json: {}", e);
            return false;
        }
    };

    // 事件消息（订阅确认 / 错误）—— 忽略
    if v.get("event").is_some() {
        if let Some(err) = v.get("event").and_then(|e| e.as_str()) {
            if err == "error" {
                let code = v["code"].as_str().unwrap_or("");
                let msg = v["msg"].as_str().unwrap_or("");
                error!("okx ws error code={} msg={}", code, msg);
            }
        }
        return false;
    }

    let channel = v["arg"]["channel"].as_str().unwrap_or("");
    let inst_id = v["arg"]["instId"].as_str().unwrap_or("");
    if inst_id.is_empty() {
        return false;
    }

    let Some(data) = v.get("data").and_then(|d| d.as_array()) else {
        return false;
    };

    match channel {
        "tickers" => {
            let mut updated = false;
            for row in data {
                updated |= apply_ticker(state, inst_id, row).await;
            }
            updated
        }
        "funding-rate" => {
            for row in data {
                apply_funding(state, inst_id, row).await;
            }
            false
        }
        _ => false,
    }
}

async fn apply_ticker(state: &SharedState, inst_id: &str, v: &serde_json::Value) -> bool {
    let Some(last) = parse_dec(v, "last") else {
        return false;
    };
    let bid = parse_dec(v, "bidPx").unwrap_or(last);
    let ask = parse_dec(v, "askPx").unwrap_or(last);
    let open_24h = parse_dec(v, "open24h").unwrap_or(Decimal::ZERO);
    let high_24h = parse_dec(v, "high24h").unwrap_or(Decimal::ZERO);
    let low_24h = parse_dec(v, "low24h").unwrap_or(Decimal::ZERO);
    let vol_quote = parse_dec(v, "volCcy24h").unwrap_or(Decimal::ZERO) * last;
    let ts = v["ts"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0);

    state
        .update_ticker(
            inst_id, last, bid, ask, open_24h, high_24h, low_24h, vol_quote, ts,
        )
        .await;
    true
}

async fn apply_funding(state: &SharedState, inst_id: &str, v: &serde_json::Value) {
    let Some(rate) = parse_dec(v, "fundingRate") else {
        return;
    };
    let next_time = v["nextFundingTime"].as_str().and_then(|s| s.parse().ok());

    state.update_funding(inst_id, rate, next_time).await;
}
