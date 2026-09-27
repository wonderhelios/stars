use std::collections::HashMap;
use std::sync::Arc;

use rust_decimal::Decimal;
use tokio::sync::RwLock;

use crate::types::MarketSnapshot;

pub struct AppState {
    pub snapshots: RwLock<HashMap<String, MarketSnapshot>>,
}

pub type SharedState = Arc<AppState>;

impl AppState {
    pub fn new() -> SharedState {
        Arc::new(Self {
            snapshots: RwLock::new(HashMap::new()),
        })
    }

    /// 更新 ticker 数据。若 inst_id 尚不存在则创建。
    pub async fn update_ticker(
        &self,
        inst_id: &str,
        last: Decimal,
        bid: Decimal,
        ask: Decimal,
        open_24h: Decimal,
        high_24h: Decimal,
        low_24h: Decimal,
        volume_quote_24h: Decimal,
        ts: i64,
    ) {
        let mut map = self.snapshots.write().await;
        let entry = map
            .entry(inst_id.to_string())
            .or_insert_with(|| MarketSnapshot::empty_for(inst_id));

        entry.last = last;
        entry.bid = bid;
        entry.ask = ask;
        entry.open_24h = open_24h;
        entry.high_24h = high_24h;
        entry.low_24h = low_24h;
        entry.volume_quote_24h = volume_quote_24h;
        entry.ts = ts;
    }

    /// 更新资金费率。
    pub async fn update_funding(
        &self,
        inst_id: &str,
        rate: Decimal,
        next_funding_time: Option<i64>,
    ) {
        let mut map = self.snapshots.write().await;
        let entry = map
            .entry(inst_id.to_string())
            .or_insert_with(|| MarketSnapshot::empty_for(inst_id));

        entry.funding_rate = Some(rate);
        if next_funding_time.is_some() {
            entry.next_funding_time = next_funding_time;
        }
    }
}
