# Binance spot panel for TxFlow

See REPORT.md for full coverage and risk exclusions. Data cut-off: 2026-10-08 00:00 UTC (last closed day 2026-10-07).

CSV: data/<BinanceSymbol>.csv, columns ts,open,high,low,close,volume. ts is UTC opening epoch milliseconds. OHLC in native Binance spot USDT/base quote units; volume in native Binance base asset units. Never concatenate native volume without conversion.

JSON adapter: data/<BinanceSymbol>.json is an array with t,o,h,l,c,v (same values, native units), compatible with prior /tmp/hl-daily-full pandas readers. File names are Binance symbols, not TxFlow names: use explicit mapping.

mapping.json/mapping.csv contain ALL markets, including failures; approved_mapping.json contains only validated candidates. Unit fields count underlying tokens per quoted asset unit. Binance->TxFlow multiplier M=binance_unit/txflow_unit. Convert OHLC by dividing by M and volume by multiplying by M; quote notional is invariant. This conversion is NOT applied to saved data. price_ratio and volume_ratio compare normalized Binance/HL underlying units. raw_* preserve unnormalized comparisons.

Do not feed full mapping or all downloaded files into strategy. Noneligible files are retained solely as evidence. Re-run offline audit: python3 finalize.py. Network collector: python3 build_panel.py (fixed cutoff and cached raw inputs; does not refresh metadata). An intentional new batch needs new metadata and cutoff; do not silently reuse this dated snapshot.

Source responses raw/, HTTP request evidence request_log.json (collector only), integrity manifest manifest_sha256.json. No credentials or private account endpoints are used. No TxFlow explorer requests.
