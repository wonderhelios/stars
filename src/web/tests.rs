use super::*;
use super::live::{live_config, valid_flow_time};
use super::ml::plan_ml_execution_phases;

#[cfg(test)]
mod capital_time_tests {
    use super::*;
    #[test]
    fn capital_time_requires_a_known_baseline_and_past_time() {
        let mut st = crate::live::LiveState::default();
        assert!(!valid_flow_time(&st, 1000));
        st.config.account = "a".into();
        st.capital_baseline = Some(crate::live::CapitalBaseline { account:"a".into(), equity:500.0, ts:Some(1000), source:"test".into() });
        assert!(!valid_flow_time(&st, 1000));
        assert!(valid_flow_time(&st, 1001));
        assert!(!valid_flow_time(&st, crate::live::now_ms_pub()+60_000));
    }
    #[test]
    fn legacy_capital_flow_does_not_invent_an_effective_time() {
        let f: crate::live::CapitalFlow = serde_json::from_value(json!({"ts":2000,"amount":500,"note":"legacy"})).unwrap();
        assert!(f.effective_ts.is_none());
        let mut updated = f; updated.effective_ts=Some(1500);
        let restored: crate::live::CapitalFlow = serde_json::from_value(serde_json::to_value(updated).unwrap()).unwrap();
        assert_eq!(restored.ts,2000);
        assert_eq!(restored.effective_ts,Some(1500));
    }
}

#[cfg(test)]
mod live_routes_tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[tokio::test]
    async fn live_page_and_configuration_preserve_save_and_execution_gates() {
        let root = std::env::temp_dir().join(format!("stars-routes-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let base = AppState {
            store: Arc::new(Store::open(&root.join("hl.sqlite")).unwrap()),
            live: Arc::new(Mutex::new(crate::live::LiveState::default())),
            live_path: Arc::new(root.join("hl.json")),
            meta: Arc::new(Mutex::new(MetaCache::default())),
            refresh: Arc::new(Mutex::new(RefreshStatus::default())),
            http: reqwest::Client::new(), exec_gate: Arc::new(Mutex::new(())),
            run_gate: Arc::new(Mutex::new(())),
            hlcarry: Arc::new(Mutex::new(crate::hlcarry::HlCarryState::default())),
            hlcarry_path: Arc::new(root.join("hlcarry.json")),
            auth_token: None,
            intraday_path: Arc::new(root.join("intraday.json")),
            tradfi_path: Arc::new(root.join("tradfi.json")),
            fastswing_path: Arc::new(root.join("fastswing.json")),
            limitoco: Arc::new(Mutex::new(crate::limitoco::LimitOcoState::default())),
            limitoco_path: Arc::new(root.join("limitoco.json")),
            limitoco_gate: Arc::new(Mutex::new(())),
        };
        let app = router(base.clone());
        let response = app.clone().oneshot(Request::builder().uri("/").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(app.clone().oneshot(Request::builder().uri("/api/points").body(Body::empty()).unwrap()).await.unwrap().status(), StatusCode::NOT_FOUND);
        let request = Request::builder().method("POST").uri("/api/live/config")
            .header("content-type", "application/json").body(Body::from(r#"{"lookback":21,"target_positions":4,"ema_alpha":0.4,"rebal_band":0.08,"hard_gross_cap":2.6}"#)).unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(base.live.lock().await.config.lookback, 21);
        assert!((base.live.lock().await.config.ema_alpha - 0.4).abs() < 1e-9);
        assert!((base.live.lock().await.config.rebal_band - 0.08).abs() < 1e-9);
        assert!((base.live.lock().await.config.hard_gross_cap - 2.6).abs() < 1e-9);
        // 超界参数应安全截断
        let clamp_req = Request::builder().method("POST").uri("/api/live/config")
            .header("content-type", "application/json").body(Body::from(r#"{"ema_alpha":0.01,"rebal_band":0.99,"hard_gross_cap":99.0}"#)).unwrap();
        assert_eq!(app.clone().oneshot(clamp_req).await.unwrap().status(), StatusCode::OK);
        assert_eq!(base.live.lock().await.config.ema_alpha, 0.1);
        assert_eq!(base.live.lock().await.config.rebal_band, 0.5);
        assert_eq!(base.live.lock().await.config.hard_gross_cap, 5.0);
        assert!(!base.live.lock().await.config.armed);
        assert!(base.live_path.exists());
        let failing = AppState { live_path: Arc::new(root.clone()), ..base.clone() };
        let response = live_config(State(failing), Json(serde_json::from_value(json!({"armed":true})).unwrap())).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!base.live.lock().await.config.armed);
        let planning = base.run_gate.lock().await;
        let request = Request::builder().method("POST").uri("/api/live/config")
            .header("content-type", "application/json").body(Body::from(r#"{"armed":false,"auto_run":false,"min_vol_usd":1000000}"#)).unwrap();
        assert_eq!(app.clone().oneshot(request).await.unwrap().status(), StatusCode::OK);
        drop(planning);
        let executing = base.exec_gate.lock().await;
        let request = Request::builder().method("POST").uri("/api/live/config")
            .header("content-type", "application/json").body(Body::from(r#"{"armed":true}"#)).unwrap();
        assert_eq!(app.clone().oneshot(request).await.unwrap().status(), StatusCode::CONFLICT);
        assert!(!base.live.lock().await.config.armed);
        drop(executing);
        drop(app); drop(base);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn capital_routes_round_trip() {
        let root = std::env::temp_dir().join(format!("stars-funding-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let base = AppState {
            store: Arc::new(Store::open(&root.join("hl.sqlite")).unwrap()),
            live: Arc::new(Mutex::new(crate::live::LiveState::default())),
            live_path: Arc::new(root.join("hl.json")),
            meta: Arc::new(Mutex::new(MetaCache::default())),
            refresh: Arc::new(Mutex::new(RefreshStatus::default())),
            http: reqwest::Client::new(), exec_gate: Arc::new(Mutex::new(())),
            run_gate: Arc::new(Mutex::new(())),
            hlcarry: Arc::new(Mutex::new(crate::hlcarry::HlCarryState::default())),
            hlcarry_path: Arc::new(root.join("hlcarry.json")),
            auth_token: None,
            intraday_path: Arc::new(root.join("intraday.json")),
            tradfi_path: Arc::new(root.join("tradfi.json")),
            fastswing_path: Arc::new(root.join("fastswing.json")),
            limitoco: Arc::new(Mutex::new(crate::limitoco::LimitOcoState::default())),
            limitoco_path: Arc::new(root.join("limitoco.json")),
            limitoco_gate: Arc::new(Mutex::new(())),
        };
        let app = router(base.clone());
        // 资金矩阵：保存 → 持久化 → 拒绝对负数。
        let req = Request::builder().method("POST").uri("/api/capital")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"momentum_usd":3500,"carry_usd":1000,"hunting_usd":500}"#)).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        {
            let st = base.live.lock().await;
            assert_eq!(st.capital_alloc.momentum_usd, 3500.0);
            assert_eq!(st.capital_alloc.carry_usd, 1000.0);
            assert_eq!(st.capital_alloc.hunting_usd, 500.0);
        }
        assert!(base.live_path.exists());
        // 负数被拒。
        let req = Request::builder().method("POST").uri("/api/capital")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"momentum_usd":-1,"carry_usd":0,"hunting_usd":0}"#)).unwrap();
        assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::BAD_REQUEST);
        // 落盘后重新加载：字段必须保留（serde 持久化回归）。
        let reloaded = crate::live::LiveState::load(&root.join("hl.json"));
        assert_eq!(reloaded.capital_alloc.momentum_usd, 3500.0);
        // 经 /api/live 读回：快照必须包含 capital_alloc（漏掉会静默丢配置）。
        let resp = app.clone().oneshot(Request::builder().uri("/api/live").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["capital_alloc"]["momentum_usd"], 3500.0);
        assert_eq!(v["capital_alloc"]["carry_usd"], 1000.0);
        assert_eq!(v["capital_alloc"]["hunting_usd"], 500.0);
        drop(app); drop(base);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn limitoco_config_and_control_routes_round_trip() {
        let root = std::env::temp_dir().join(format!("stars-limitoco-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let base = AppState {
            store: Arc::new(Store::open(&root.join("hl.sqlite")).unwrap()),
            live: Arc::new(Mutex::new(crate::live::LiveState::default())),
            live_path: Arc::new(root.join("hl.json")),
            meta: Arc::new(Mutex::new(MetaCache::default())),
            refresh: Arc::new(Mutex::new(RefreshStatus::default())),
            http: reqwest::Client::new(),
            exec_gate: Arc::new(Mutex::new(())),
            run_gate: Arc::new(Mutex::new(())),
            hlcarry: Arc::new(Mutex::new(crate::hlcarry::HlCarryState::default())),
            hlcarry_path: Arc::new(root.join("hlcarry.json")),
            auth_token: None,
            intraday_path: Arc::new(root.join("intraday.json")),
            tradfi_path: Arc::new(root.join("tradfi.json")),
            fastswing_path: Arc::new(root.join("fastswing.json")),
            limitoco: Arc::new(Mutex::new(crate::limitoco::LimitOcoState::default())),
            limitoco_path: Arc::new(root.join("limitoco.json")),
            limitoco_gate: Arc::new(Mutex::new(())),
        };
        let app = router(base.clone());

        // Default config is 1.5x (1.2%R, max_sl=14%)
        let resp = app.clone().oneshot(Request::builder().uri("/api/limitoco").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["config"]["risk_tier"], "1.5x");
        assert_eq!(v["config"]["risk_per_trade"], 0.012);
        assert_eq!(v["config"]["max_sl_pct"], 0.14);

        // Save updated capital + 2.0x tier
        let req = Request::builder().method("POST").uri("/api/limitoco/config")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"capital":25000,"risk_tier":"2.0x","compounding":true,"proxy_url":"http://127.0.0.1:7897"}"#)).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        {
            let st = base.limitoco.lock().await;
            assert_eq!(st.capital, 25000.0);
            assert_eq!(st.equity, 25000.0);
            assert_eq!(st.config.risk_tier, "2.0x");
            assert_eq!(st.config.risk_per_trade, 0.016);
            assert_eq!(st.config.max_pos_lev, 3.0);
            assert_eq!(st.config.proxy_url, "http://127.0.0.1:7897");
        }
        assert!(base.limitoco_path.exists());

        // Stop & Reset routes
        let resp = app.clone().oneshot(Request::builder().method("POST").uri("/api/limitoco/stop").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = app.clone().oneshot(Request::builder().method("POST").uri("/api/limitoco/reset").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Serve artistic frontend assets
        for uri in [
            "/css/tokens.css",
            "/css/base.css",
            "/css/components.css",
            "/js/main.js",
            "/js/core/api.js",
            "/js/core/fmt.js",
            "/js/core/chart.js",
            "/js/core/router.js",
            "/js/views/overview.js",
            "/js/views/live.js",
            "/js/views/limitoco.js",
            "/js/views/archive.js",
        ] {
            assert_eq!(
                app.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap().status(),
                StatusCode::OK,
                "static route failed: {uri}"
            );
        }

        drop(app); drop(base);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ml_execution_phases_put_closes_first_and_split_flips() {
        let mut acct = crate::exchange::Acct { equity: 2000.0, positions: Default::default() };
        acct.positions.insert("CLOSE_ME".into(), crate::exchange::Pos { size: 5.0, entry_px: 100.0, ..Default::default() });
        acct.positions.insert("REDUCE_SHORT".into(), crate::exchange::Pos { size: -10.0, entry_px: 50.0, ..Default::default() });
        acct.positions.insert("FLIP_L2S".into(), crate::exchange::Pos { size: 4.0, entry_px: 20.0, ..Default::default() });
        let markets: std::collections::HashMap<String, crate::exchange::MarketInfo> =
            ["CLOSE_ME", "REDUCE_SHORT", "FLIP_L2S", "NEW_LONG"]
                .iter()
                .map(|c| (c.to_string(), crate::exchange::MarketInfo { sz_decimals: 2, max_leverage: 10 }))
                .collect();
        let mids: std::collections::HashMap<String, f64> = [
            ("CLOSE_ME", 101.0),
            ("REDUCE_SHORT", 49.0),
            ("FLIP_L2S", 21.0),
            ("NEW_LONG", 15.0),
        ]
        .iter()
        .map(|(c, p)| (c.to_string(), *p))
        .collect();

        // 即使输入顺序先开后平，plan_ml_execution_phases 也必须先平/减仓再开仓，且反手拆成 reduce_only 平旧仓 + 开新仓
        let raw_orders = vec![
            json!({"coin": "NEW_LONG", "buy": true, "size": "10", "price": "15.1"}),
            json!({"coin": "FLIP_L2S", "buy": false, "size": "9", "price": "20.9"}),
            json!({"coin": "CLOSE_ME", "buy": false, "size": "5", "price": "100.5", "reduce_only": true, "action": "close"}),
            json!({"coin": "REDUCE_SHORT", "buy": true, "size": "4", "price": "49.2"}),
        ];
        let (closes, opens, invalid) = plan_ml_execution_phases(&raw_orders, &acct, &markets, &mids);
        assert!(invalid.is_empty());
        assert_eq!(closes.len(), 3, "应有 3 笔只减仓单（平仓+减空+反手平旧仓）: {closes:?}");
        assert!(closes.iter().all(|c| c.reduce_only), "阶段 1 必须全部 reduce_only=true");
        assert_eq!(opens.len(), 2, "应有 2 笔开仓单（新开+反手开新仓）: {opens:?}");
        assert!(opens.iter().all(|o| !o.reduce_only), "阶段 2 必须全部 reduce_only=false");

        let flip_close = closes.iter().find(|c| c.coin == "FLIP_L2S").unwrap();
        let flip_open = opens.iter().find(|o| o.coin == "FLIP_L2S").unwrap();
        assert!((flip_close.size - 4.0).abs() < 1e-9);
        assert!((flip_open.size - 5.0).abs() < 1e-9);
        // 必须使用实时 mid，而非 place.py 的含滑点 price
        assert_eq!(flip_close.mid, 21.0);
        assert_eq!(opens.iter().find(|o| o.coin == "NEW_LONG").unwrap().mid, 15.0);
    }
}
