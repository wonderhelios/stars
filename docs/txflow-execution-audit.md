# TxFlow execution evidence

The monitor now separates acknowledged orders from verified fills. This is instrumentation, not a strategy or risk-parameter change.

- Future TxFlow order result records retain the exchange order ID (when supplied), submitted quantity and an account fingerprint. Hyperliquid keeps its existing result format.
- Account fills are matched only by exact exchange order ID to records with the same account fingerprint. Historical orders without this evidence remain unassigned; manual trades must not be presumed to be system trades.
- A separate account-scoped `live.audit-<account hash>.json` archive retains raw fills, non-funding ledger rows and linked order evidence. Clearing the UI records does not clear this archive. Keep this file private and include it in backups.
- A read-only collector runs every five minutes with a 45-second request deadline. It never creates a signing client. The monitor's existing low-frequency PNL endpoint also persists retrieved evidence. Collection failures are logged and endpoint failures remain unavailable in the UI.
- Deduplication uses coin plus trade ID, not a timestamp watermark. Same-time and late fills are retained. Rows without IDs use a raw payload hash but are marked incomplete: this cannot establish uniqueness if the exchange rewrites those rows.
- Fees are reported as known USD fees only when currency is explicitly USD/USDC. Missing fields or other currencies remain incomplete. Funding is unknown, not zero.

## Limits before any strategy verdict

The exchange may truncate returned history even when 1,000 records are requested. Five-minute collection cannot guarantee no gaps during high activity or downtime. Archives accumulate observed evidence but do not certify full coverage. Old orders cannot be reliably reconstructed from matching coin/size/time alone.

Known realized PNL minus known fees is **not** account or strategy net profit. The collector now persists its first and latest paired equity/unrealized-PNL samples. The independent reconciliation window begins at the first such sample; the original capital baseline and strategy chart are never reset. Historical `EquityPoint.pnl` has mixed legacy semantics and is not silently accepted as a verified opening unrealized PNL.

For `(first sample, latest sample]`, the API computes equity change less dated registered cashflows, realized PNL, fees, unrealized-PNL change, observed signed funding, and an unexplained residual. Exchange deposit/withdraw rows are compared against registered cashflows but are not silently substituted or double counted. Unknown ledger adjustments and missing effective dates are reported as gaps.

Funding uses an optional read-only `userFunding` request, bounded to ten seconds. This endpoint's availability on the production account has not yet been established. Unsupported/error responses remain explicit gaps and never verify a zero funding value. Returned funding uses signed `delta.usdc`; missing fields are incomplete. Funding rows are persisted and deduplicated by payload hash. This is not a guarantee of full history or uniqueness if the exchange rewrites rows.

The badge now follows evidence: waiting for samples, missing data, unexplained residual, or observed numerical agreement within $0.01. Agreement does not certify completeness: requests are not atomic, the receipt cache may lag by up to 60 seconds, and returned history can be capped. A residual is not automatically classified as missing money, strategy loss, or funding. Exact system-order matching is separate from account-level arithmetic.

No trading parameters, targets, orders or production processes are changed by installing the source patch. Deploying the binary starts only the additional read-only collector alongside the pre-existing application behavior. Existing trading jobs remain governed by their current configuration.

## Verification

Run `cargo test`, `node tools/test-txflow-audit.mjs`, `node tools/test-account-performance.mjs`, `node tools/test-capital-flow.mjs`, `node tools/test-strategy-chart.mjs`, and `node tools/lint-frontend.mjs`.

## Points (independent ledger)

`/api/txflow/points` reads weekly observed activity from the archive and stores user-entered official dashboard values. Weeks start Monday 00:00 UTC; POST requires the current account and upserts that week's trader/referral points. Values are marked estimated or season-final by the user. There is no points API integration or implied eligibility, token conversion, financial return, or points-driven trading policy. The account-scoped `.points-<hash>.json` file uses private atomic persistence. The HTML and script are TxFlow-only.

Additional verification: `node tools/test-txflow-points.mjs`; Rust tests cover epoch boundaries, correction replacement, invalid values, corrupt storage, account changes, route isolation, and deduplicated observed weekly activity.
