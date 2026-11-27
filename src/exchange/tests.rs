use super::*;
use serde_json::json;

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
