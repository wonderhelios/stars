//! Incremental 1h kline/funding cache and async Binance USD-M market data fetcher.

use super::{
    classify_binance_contract, compute_indicators, now_ms, Bar1h, BtcRegime, LimitOcoState,
    COLD_FETCH_LIMIT, COMMODITY_SYMBOLS, DAY_MS, FAPI_BASE_DEFAULT, HOUR_MS, MAX_BARS_CACHE,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

// ---------------------------------------------------------------------------
// 增量 K 线缓存 + 币安 USD-M 异步行情拉取（纯 Rust reqwest + tokio）
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct MarketDataCache {
    exchange_updated_ms: i64,
    eligible: HashMap<String, String>,
    total_perps: usize,
    excluded_stocks: usize,
    bars: HashMap<String, Vec<Bar1h>>,
    funding: HashMap<String, Vec<(i64, f64)>>,
}

impl MarketDataCache {
    fn load(path: &Path) -> Self {
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(c) = serde_json::from_slice::<Self>(&bytes) {
                return c;
            }
        }
        Self::default()
    }

    fn save(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
        if let Ok(bytes) = serde_json::to_vec(self) {
            if std::fs::write(&tmp, bytes).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
    }

    fn merge_bars(&mut self, sym: &str, new_bars: Vec<Bar1h>) -> Vec<Bar1h> {
        let mut by_ts: BTreeMap<i64, Bar1h> = BTreeMap::new();
        if let Some(old) = self.bars.get(sym) {
            for b in old {
                by_ts.insert(b.open_time_ms, b.clone());
            }
        }
        for b in new_bars {
            by_ts.insert(b.open_time_ms, b);
        }
        let all: Vec<Bar1h> = by_ts.into_values().collect();
        let trimmed = if all.len() > MAX_BARS_CACHE {
            all[all.len() - MAX_BARS_CACHE..].to_vec()
        } else {
            all
        };
        self.bars.insert(sym.to_string(), trimmed.clone());
        trimmed
    }

    fn merge_funding(&mut self, sym: &str, events: Vec<(i64, f64)>) -> Vec<(i64, f64)> {
        let mut by_ts: BTreeMap<i64, f64> = BTreeMap::new();
        if let Some(old) = self.funding.get(sym) {
            for &(t, r) in old {
                by_ts.insert(t, r);
            }
        }
        for (t, r) in events {
            by_ts.insert(t, r);
        }
        let all: Vec<(i64, f64)> = by_ts.into_iter().collect();
        let trimmed = if all.len() > 120 {
            all[all.len() - 120..].to_vec()
        } else {
            all
        };
        self.funding.insert(sym.to_string(), trimmed.clone());
        trimmed
    }
}

fn build_binance_http(proxy_url: &str) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .pool_max_idle_per_host(16)
        .user_agent("stars-limitoco-rust/1.0");
    let proxy = if !proxy_url.trim().is_empty() {
        proxy_url.trim().to_string()
    } else {
        std::env::var("BINANCE_PROXY")
            .or_else(|_| std::env::var("HTTPS_PROXY"))
            .or_else(|_| std::env::var("https_proxy"))
            .unwrap_or_default()
    };
    if !proxy.is_empty() {
        builder = builder.proxy(reqwest::Proxy::all(&proxy).context("无效的代理地址")?);
    }
    builder.build().context("创建币安 HTTP 客户端失败")
}

fn fapi_base() -> String {
    std::env::var("BINANCE_FAPI_BASE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| FAPI_BASE_DEFAULT.into())
        .trim_end_matches('/')
        .to_string()
}

async fn get_json_retry(client: &reqwest::Client, url: &str) -> Result<Value> {
    let mut last_err = None;
    for attempt in 0..3 {
        match client.get(url).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    return resp.json::<Value>().await.context("解析 JSON 失败");
                }
                last_err = Some(anyhow::anyhow!("HTTP {} {}", resp.status(), url));
            }
            Err(e) => {
                last_err = Some(anyhow::anyhow!("{e}"));
            }
        }
        if attempt < 2 {
            tokio::time::sleep(Duration::from_millis(450 * (attempt + 1))).await;
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("请求失败: {url}")))
}

async fn fetch_premium_marks(client: &reqwest::Client, base: &str) -> Result<HashMap<String, f64>> {
    let v = get_json_retry(client, &format!("{base}/fapi/v1/premiumIndex")).await?;
    let mut out = HashMap::new();
    if let Some(arr) = v.as_array() {
        for item in arr {
            let sym = item.get("symbol").and_then(|x| x.as_str()).unwrap_or("").to_uppercase();
            let mp = item
                .get("markPrice")
                .and_then(|x| x.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            if !sym.is_empty() && mp > 0.0 {
                out.insert(sym, mp);
            }
        }
    }
    Ok(out)
}

async fn fetch_exchange_universe(client: &reqwest::Client, base: &str) -> Result<(HashMap<String, String>, usize, usize)> {
    let v = get_json_retry(client, &format!("{base}/fapi/v1/exchangeInfo")).await?;
    let mut eligible = HashMap::new();
    let mut total_perps = 0usize;
    let mut excluded_stocks = 0usize;
    if let Some(arr) = v.get("symbols").and_then(|x| x.as_array()) {
        for s in arr {
            if s.get("contractType").and_then(|x| x.as_str()) == Some("PERPETUAL")
                && s.get("quoteAsset").and_then(|x| x.as_str()) == Some("USDT")
                && s.get("status").and_then(|x| x.as_str()) == Some("TRADING")
            {
                total_perps += 1;
            }
            let cls = classify_binance_contract(s);
            if cls == "crypto" || cls == "commodity" {
                if let Some(sym) = s.get("symbol").and_then(|x| x.as_str()) {
                    eligible.insert(sym.to_uppercase(), cls.into());
                }
            } else if cls == "stock" {
                excluded_stocks += 1;
            }
        }
    }
    Ok((eligible, total_perps, excluded_stocks))
}

async fn fetch_24h_qv(client: &reqwest::Client, base: &str) -> Result<HashMap<String, f64>> {
    let v = get_json_retry(client, &format!("{base}/fapi/v1/ticker/24hr")).await?;
    let mut out = HashMap::new();
    if let Some(arr) = v.as_array() {
        for item in arr {
            let sym = item.get("symbol").and_then(|x| x.as_str()).unwrap_or("").to_uppercase();
            let qv = item
                .get("quoteVolume")
                .and_then(|x| x.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            if !sym.is_empty() {
                out.insert(sym, qv);
            }
        }
    }
    Ok(out)
}

async fn fetch_closed_1h_bars(
    client: &reqwest::Client,
    base: &str,
    sym: &str,
    cur_hour_open: i64,
    limit: usize,
) -> Result<Vec<Bar1h>> {
    let url = format!("{base}/fapi/v1/klines?symbol={sym}&interval=1h&limit={limit}");
    let v = get_json_retry(client, &url).await?;
    let mut bars = Vec::new();
    if let Some(arr) = v.as_array() {
        for row in arr {
            let Some(r) = row.as_array() else { continue };
            if r.len() < 8 {
                continue;
            }
            let ot = r[0].as_i64().unwrap_or(0);
            if ot <= 0 || ot >= cur_hour_open {
                continue;
            }
            let p = |i: usize| -> f64 {
                r[i].as_str()
                    .and_then(|s| s.parse::<f64>().ok())
                    .or_else(|| r[i].as_f64())
                    .unwrap_or(0.0)
            };
            bars.push(Bar1h {
                open_time_ms: ot,
                open: p(1),
                high: p(2),
                low: p(3),
                close: p(4),
                volume: p(5),
                quote_volume: p(7),
            });
        }
    }
    bars.sort_by_key(|b| b.open_time_ms);
    Ok(bars)
}

async fn fetch_funding_history(
    client: &reqwest::Client,
    base: &str,
    sym: &str,
    start_ms: i64,
    end_ms: i64,
    limit: usize,
) -> Result<Vec<(i64, f64)>> {
    let url = format!(
        "{base}/fapi/v1/fundingRate?symbol={sym}&startTime={start_ms}&endTime={end_ms}&limit={limit}"
    );
    let v = get_json_retry(client, &url).await?;
    let mut out = Vec::new();
    if let Some(arr) = v.as_array() {
        for item in arr {
            let ts = item.get("fundingTime").and_then(|x| x.as_i64()).unwrap_or(0);
            let rate = item
                .get("fundingRate")
                .and_then(|x| x.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            if ts > 0 {
                out.push((ts, rate));
            }
        }
    }
    out.sort_by_key(|x| x.0);
    Ok(out)
}

/// 推进一步影子引擎（由后台 tokio 定时任务或前端手动点击「启动/立即扫描」调用）。
pub async fn step_once(
    state: &mut LimitOcoState,
    state_path: &Path,
    force_scan: bool,
) {
    let started = Instant::now();
    let now = now_ms();
    if !state.config.running && !force_scan {
        state.running = false;
        state.status_text = "已暂停".into();
        let _ = state.save(state_path);
        return;
    }

    let client = match build_binance_http(&state.config.proxy_url) {
        Ok(c) => c,
        Err(e) => {
            state.last_error = Some(format!("{e}"));
            state.status_text = "代理配置异常".into();
            state.log_event(now, "ERROR", "", "", format!("HTTP 客户端初始化失败: {e}"));
            let _ = state.save(state_path);
            return;
        }
    };
    let base = fapi_base();
    let cache_path = state_path.with_file_name("limitoco_kline_cache.json");

    let step_res: Result<()> = async {
        let mark_prices = fetch_premium_marks(&client, &base).await?;
        let cur_hour_open = now.div_euclid(HOUR_MS) * HOUR_MS;
        let latest_closed_bar_ts = cur_hour_open - HOUR_MS;
        let need_hourly_scan = force_scan || latest_closed_bar_ts > state.last_bar_ms;

        if need_hourly_scan {
            let mut cache = MarketDataCache::load(&cache_path);
            if now - cache.exchange_updated_ms > 6 * HOUR_MS || cache.eligible.is_empty() {
                let (el, tot, ex_st) = fetch_exchange_universe(&client, &base).await?;
                cache.eligible = el;
                cache.total_perps = tot;
                cache.excluded_stocks = ex_st;
                cache.exchange_updated_ms = now;
            }
            state.universe.total_perps = cache.total_perps;
            state.universe.excluded_stocks = cache.excluded_stocks;
            state.universe.eligible_count = cache.eligible.len();

            let qv24 = fetch_24h_qv(&client, &base).await?;
            let mut ranked: Vec<(f64, String)> = cache
                .eligible
                .keys()
                .map(|s| (qv24.get(s).copied().unwrap_or(0.0), s.clone()))
                .collect();
            ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.1.cmp(&b.1)));

            let n_fetch = ((state.config.top_n_liquid as f64 * 1.45).round() as usize).max(95);
            let mut targets: HashSet<String> = ranked.into_iter().take(n_fetch).map(|x| x.1).collect();
            targets.insert("BTCUSDT".into());
            for &c in COMMODITY_SYMBOLS {
                if cache.eligible.contains_key(c) {
                    targets.insert(c.into());
                }
            }
            for s in state.positions.keys() {
                targets.insert(s.clone());
            }
            for s in state.pending.keys() {
                targets.insert(s.clone());
            }

            let sem = Arc::new(Semaphore::new(12));
            let mut join_set = JoinSet::new();
            for sym in targets {
                let c = client.clone();
                let b_url = base.clone();
                let permit_sem = sem.clone();
                let cached_bars_len = cache.bars.get(&sym).map(|v| v.len()).unwrap_or(0);
                let cached_last_bar = cache.bars.get(&sym).and_then(|v| v.last()).map(|b| b.open_time_ms).unwrap_or(0);
                let cached_last_fund = cache.funding.get(&sym).and_then(|v| v.last()).map(|x| x.0).unwrap_or(0);
                let k_lim = if cached_bars_len >= 360 && cached_last_bar >= latest_closed_bar_ts - 6 * HOUR_MS {
                    8usize
                } else {
                    COLD_FETCH_LIMIT
                };
                let (f_start, f_lim) = if cached_last_fund >= cur_hour_open - 9 * HOUR_MS {
                    (cached_last_fund + 1, 12usize)
                } else {
                    (cur_hour_open - 7 * DAY_MS, 30usize)
                };

                join_set.spawn(async move {
                    let _permit = permit_sem.acquire_owned().await.ok()?;
                    let nb = fetch_closed_1h_bars(&c, &b_url, &sym, cur_hour_open, k_lim).await.ok()?;
                    let nf = fetch_funding_history(&c, &b_url, &sym, f_start, cur_hour_open, f_lim)
                        .await
                        .unwrap_or_default();
                    Some((sym, nb, nf))
                });
            }

            let mut bars_by_sym: HashMap<String, Vec<Bar1h>> = HashMap::new();
            let mut funding_by_sym: HashMap<String, Vec<(i64, f64)>> = HashMap::new();
            while let Some(res) = join_set.join_next().await {
                if let Ok(Some((sym, nb, nf))) = res {
                    let mb = cache.merge_bars(&sym, nb);
                    let mf = cache.merge_funding(&sym, nf);
                    if !mb.is_empty() {
                        bars_by_sym.insert(sym.clone(), mb);
                        funding_by_sym.insert(sym, mf);
                    }
                }
            }
            cache.save(&cache_path);

            let bars_to_run: Vec<i64> = if state.last_bar_ms == 0 {
                vec![latest_closed_bar_ts]
            } else {
                let start_bar = (state.last_bar_ms + HOUR_MS).max(latest_closed_bar_ts - 11 * HOUR_MS);
                let mut v = Vec::new();
                let mut t = start_bar;
                while t <= latest_closed_bar_ts {
                    v.push(t);
                    t += HOUR_MS;
                }
                v
            };

            if bars_to_run.is_empty() {
                // Already processed current closed hour; refresh BTC regime + live marks
                if let Some(btc_b) = bars_by_sym.get("BTCUSDT") {
                    let fev = funding_by_sym.get("BTCUSDT").map(|v| v.as_slice()).unwrap_or(&[]);
                    let ind = compute_indicators(btc_b, fev, state.config.split_jump_pct, state.config.split_block_hours);
                    let bi = btc_b.len() - 1;
                    let bp = btc_b[bi].close;
                    let e168 = ind.ema168[bi];
                    let e336 = ind.ema336[bi];
                    let bull_ok = bp > e168 && bp > e336;
                    let short_ok = bp < e168;
                    state.btc_regime = BtcRegime {
                        bar_ts: btc_b[bi].open_time_ms,
                        price: bp,
                        ema168: e168,
                        ema336: e336,
                        bull_ok,
                        short_ok,
                        funding_rate: ind.last_funding[bi],
                        label: if bull_ok {
                            "多头趋势 (A多允许)".into()
                        } else if short_ok {
                            "空头趋势 (A空+B空活跃)".into()
                        } else {
                            "震荡过渡 (仅B空/观望)".into()
                        },
                    };
                }
                state.process_intrabar_marks(&mark_prices, &cache.eligible, now);
            } else {
                const LIVE_SIGNAL_WINDOW_MS: i64 = 10 * 60_000;
                let was_cold = state.last_bar_ms == 0;
                for &bts in &bars_to_run {
                    let age_since_close = now - (bts + HOUR_MS);
                    let is_latest = bts == latest_closed_bar_ts;
                    let allow_new = is_latest && age_since_close <= LIVE_SIGNAL_WINDOW_MS;
                    state.process_closed_bar_with_opts(
                        bts,
                        &bars_by_sym,
                        &funding_by_sym,
                        &cache.eligible,
                        Some(&mark_prices),
                        allow_new,
                    );
                    if was_cold && is_latest && !allow_new {
                        let mins = (age_since_close / 60_000).max(1);
                        state.log_event(
                            now,
                            "INIT",
                            "",
                            "",
                            format!(
                                "冷启动行情与 Top-{} 标的池已同步（距上根 1h 收盘已 {} 分钟，跳过陈旧信号，等待下根整点收盘后挂新单）",
                                state.universe.liquid_count, mins
                            ),
                        );
                    }
                }
                state.process_intrabar_marks(&mark_prices, &cache.eligible, now);
            }
        } else {
            let cache = MarketDataCache::load(&cache_path);
            state.process_intrabar_marks(&mark_prices, &cache.eligible, now);
        }

        state.last_error = None;
        state.running = state.config.running;
        state.status_text = if state.config.running {
            "运行中".into()
        } else {
            "已暂停（手动扫描完成）".into()
        };
        Ok(())
    }
    .await;

    if let Err(e) = step_res {
        state.last_error = Some(format!("{e}"));
        state.status_text = "行情连接异常".into();
        state.log_event(now, "ERROR", "", "", format!("币安行情拉取失败：{e}"));
    }

    state.scan_duration_ms = started.elapsed().as_millis() as u64;
    let _ = state.save(state_path);
}

