> For the complete documentation index, see [llms.txt](https://docs.trade.xyz/llms.txt). Markdown versions of documentation pages are available by appending `.md` to page URLs; this page is available as [Markdown](https://docs.trade.xyz/perpetuals/mechanics/discovery-bounds.md).

# Discovery Bounds

Discovery-bound controls for trade\[XYZ] perpetuals.

A fundamental unlock of XYZ markets is the facilitation of 24/7 price discovery. When external markets close, XYZ traders continue to buy, sell, or otherwise speculate on the asset's fair price. As a result, XYZ is the primary venue for price discovery over weekends, holidays, and other extended sessions.

To provide a fair and safe venue for 24/7 price discovery, mark price movements are limited within a defined range around a reference price. By restricting the extent to which price moves can occur, market makers and traders can more confidently participate in extended-hours price discovery. If a trader’s liquidation price lies outside the active price bounds, their position cannot be liquidated while those bounds are in effect.

### How Discovery Bounds Work

#### Instantaneous Bounds

Discovery bounds restrict the mark price within ±(1/max leverage) of a reference price during both external and internal pricing sessions. For example, with WTIOIL at 20× leverage, the instantaneous bound is ±5% around the reference price.

During external sessions, the reference price is based on the last externally-derived fair price. During internal sessions, the reference price for markets is initially based on the last externally derived oracle price (e.g., the Friday close for assets that follow weekday market hours). For Pre-IPO Perpetual markets without external prices, the initial reference price is the XYZ-defined initial reference price for that market.

#### Re-Anchoring

When the oracle price approaches the edge of the current bound and reaches a configured trigger threshold (e.g., 90% of the distance from the reference price to the upper bound for WTIOIL), the reference price for the discovery bounds re-anchors to that bound, and a fresh bound is established around the new reference. This ratcheting mechanism allows the market to reflect genuine moves during extended periods without external data, rather than freezing at a static limit.

Each market has a configured number of permitted re-anchors per direction (referred to as "resets"), specified independently for upward and downward movement. The counter for each direction increments by one whenever a new bound and trigger is recalculated from a reference price. Once the configured maximum is reached, the bound becomes a hard cap until external pricing resumes. When external pricing resumes, the reference price reverts to the live externally derived price and all reset counters return to zero.

The number of resets per market is set on a per-asset basis and described in [Specification Index](/perpetuals/specifications-and-schedules/specification-index.md) and [Specification Index (IPOPs)](/perpetuals/specifications-and-schedules/pre-ipo-specification-index.md). A market with zero resets behaves as a static bound around the external close.

#### Trigger Formulas

```
upper_trigger = reference_price × (1 + discovery_bound × oracle_threshold)
```

```
lower_trigger = reference_price × (1 - discovery_bound × oracle_threshold)
```

### Example: WTIOIL

| Parameters                                        | Value                       |
| ------------------------------------------------- | --------------------------- |
| Discovery Bound                                   | ±5% (from 20× max leverage) |
| Oracle Threshold                                  | 90%                         |
| Friday Closing External Price (initial reference) | $100                        |

#### Upward Path:

| Level        | Reference Price | Lower Bound | Upper Bound | Tradeable Range | Upper Trigger | Lower Trigger |
| ------------ | --------------- | ----------- | ----------- | --------------- | ------------- | ------------- |
| 0            | 100.00          | 95.00       | 105.00      | 95.00 – 105.00  | 104.50        | 95.50         |
| 1            | 105.00          | 99.75       | 110.25      | 99.75 – 110.25  | 109.73        | 100.28        |
| 2 (hard cap) | 110.25          | 104.74      | 115.76      | 104.74 – 115.76 | N/A           | N/A           |

**Step-by-step:**

1. The mark price trades within the Level 0 range of 95.00–105.00.
2. Over Saturday, the oracle price rises on genuine buying pressure and reaches 104.50 (the upper trigger at 100.00 × 1.045).
3. Re-anchor: the reference price moves to 105.00 (the upper bound). Upward level counter = 1.
4. New tradeable range: 99.75–110.25. The oracle price continues from where it was (\~104.50) with room to rise further.
5. Buying continues. The oracle price reaches 109.73 (the new upper trigger at 105.00 × 1.045).
6. Re-anchor: the reference price moves to 110.25. Upward level counter = 2.
7. New tradeable range: 104.74–115.76. This is the final level upward.
8. If the oracle price reaches 115.76, it hits the hard cap. No further upward movement until external pricing resumes.

$$
\text{Maximum price from origin} = 100.00 \times 1.05^3 \approx 115.76 , (+15.76%)
$$

$$
\text{Maximum price below origin} = 100.00 \times 0.95^3 \approx 85.74 , (-14.26%)
$$

**Reversal:** The mechanism is fully bidirectional. If the oracle price reverses and hits the lower trigger, the reference price re-anchors downward using the same logic. Upward and downward level counters are independent. Ratcheting up does not consume downward levels.

### Effect of Discovery Bounds in Practice

* **Broader price discovery during genuine events:** The discoverable range expands beyond the instantaneous bound to a wider total range that depends on the configured number of resets (e.g., from ±5% to approximately ±15.8% for WTIOIL), allowing the market to reflect large moves rather than freezing at the static limit.
* **The instantaneous range is static:** The tiers shift the window, they do not widen it. At any given moment, the mark price can only move within the instantaneous range (e.g. ±5% for WTIOIL) around the current reference price.
* **The window re-centers as it moves:** After an upward ratchet, the floor rises. The system commits to the move by shifting the entire window upward. This constrains reversal amplitude and reduces re-open gap risk.
* **Levels are independent per direction:** Upward ratchets do not consume downward budget and vice versa.
* **Risk management:** Traders should manage risk based on the maximum possible range (in the above WTIOIL example, approximately ±15.8% from the external close at the start of the internal session), not the instantaneous range.


---

# Agent Instructions
This documentation is published with GitBook. GitBook is the documentation platform designed so that both humans and AI agents can read, navigate, and reason over technical content effectively. Learn more at gitbook.com.

## Querying This Documentation
If you need additional information that is not directly available in this page, you can query the documentation dynamically by asking a question.

Perform an HTTP GET request on the following URL with the `ask` and `goal` query parameters:

```
GET https://docs.trade.xyz/perpetuals/mechanics/discovery-bounds.md?ask=<question>&goal=<user_goal>
```

`ask` is the immediate question: it should be specific, self-contained, and written in natural language.
`goal` is what the user is ultimately trying to achieve, the reason they need the answer. Sharing it helps GitBook give you a better, more relevant answer. A goal is most helpful when it describes the outcome the user wants rather than restating the question. For example, with `ask=how do I create an API token`, a goal like `build a script that syncs our docs to a CMS` lets GitBook tailor the answer to that use case.

The response will contain a direct answer to the question and relevant excerpts and sources from the documentation.

Use this mechanism when the answer is not explicitly present in the current page, you need clarification or additional context, or you want to retrieve related documentation sections.
