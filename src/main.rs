mod error;
mod okx;
mod types;

use rust_decimal::Decimal;
use tracing::info;

use okx::{Channel, RestClient, WsClient, WsEvent};
use types::{Interval, Symbol};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let rest = RestClient::new();

    // ========== 1. 全部 SWAP 合约 ==========
    let instruments = rest.instruments().await?;
    println!("\n=== {} SWAP instruments ===", instruments.len());
    for i in instruments.iter().take(5) {
        println!(
            "  {:<22} tick={:<8} ctVal={:<8} maxLev={}",
            i.symbol.inst_id, i.tick_size, i.ct_val, i.max_leverage
        );
    }

    // ========== 2. 全市场行情，按 24h 成交额排序 ==========
    let mut tickers = rest.all_tickers().await?;
    tickers.retain(|t| t.volume_quote_24h >= Decimal::from(5_000_000));
    tickers.sort_by(|a, b| b.volume_quote_24h.cmp(&a.volume_quote_24h));

    println!("\n=== Top 15 SWAP by 24h quote volume ===");
    println!(
        "{:<22} {:>12} {:>18} {:>9} {:>9}",
        "Symbol", "Last", "Vol(USDT)", "Chg%", "Spread%"
    );
    println!("{}", "-".repeat(72));
    for t in tickers.iter().take(15) {
        println!(
            "{:<22} {:>12} {:>18} {:>9} {:>9}",
            t.symbol.inst_id,
            t.last,
            t.volume_quote_24h,
            t.change_pct().round_dp(2),
            t.spread_pct().round_dp(4),
        );
    }

    // ========== 3. BTC-USDT-SWAP 最近 1m K线 ==========
    let btc = Symbol::from_swap_inst_id("BTC-USDT-SWAP").unwrap();
    let candles = rest.candles(&btc, Interval::M1, 5).await?;
    println!("\n=== BTC-USDT-SWAP 1m candles ===");
    for c in &candles {
        println!(
            "  ts={} O={} H={} L={} C={} V={}",
            c.open_time, c.open, c.high, c.low, c.close, c.volume
        );
    }

    // ========== 4. WebSocket 跑 15 秒：tickers + funding-rate ==========
    println!("\n=== WebSocket (15s, tickers + funding-rate) ===");
    let subs: Vec<String> = tickers
        .iter()
        .take(10)
        .map(|t| t.symbol.inst_id.clone())
        .collect();

    let ws = WsClient::new(subs, vec![Channel::Tickers, Channel::FundingRate]);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        if let Err(e) = ws.run(tx).await {
            eprintln!("ws error: {}", e);
        }
    });

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut count = 0u32;

    loop {
        tokio::select! {
            Some(ev) = rx.recv() => {
                count += 1;
                if count <= 15 {
                    match ev {
                        WsEvent::Ticker(t) => {
                            println!("  tick {:<22} {}", t.symbol.inst_id, t.last);
                        }
                        WsEvent::Funding(f) => {
                            println!("  fund {:<22} {}", f.symbol.inst_id, f.rate);
                        }
                        WsEvent::Subscribed(c, i) => {
                            println!("  sub  {:<22} {}", c, i);
                        }
                        WsEvent::Error(c, m) => {
                            eprintln!("  err  {} {}", c, m);
                        }
                    }
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }
    info!("received {} events in 15s", count);

    Ok(())
}
