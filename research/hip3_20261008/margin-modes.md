> For the complete documentation index, see [llms.txt](https://docs.trade.xyz/llms.txt). Markdown versions of documentation pages are available by appending `.md` to page URLs; this page is available as [Markdown](https://docs.trade.xyz/perpetuals/risk-and-margining/margin-modes.md).

# Margin Modes

Cross and isolated margin modes for trade\[XYZ] perpetuals.

XYZ inherits its margining mechanisms from Hypercore.

#### Margin Mode

When opening a position, a margin mode is selected. Refer to the 'Margin Mode' column on [Specification Index](/perpetuals/specifications-and-schedules/specification-index.md) to see the default margin mode for each market.

*Cross Margin:* is the default for markets where cross margin is enabled. Cross margin allows for maximal capital efficiency by sharing collateral between all other cross margin positions.&#x20;

*Normal Isolated Margin:* allows an asset's collateral to be constrained to that asset. Liquidations in that asset do not affect other isolated positions or cross positions. Similarly, cross liquidations or other isolated liquidations do not affect the original isolated position.&#x20;

*Strict Isolated Margin*: functions the same as isolated margin, with the additional constraint that margin cannot be removed. Margin is proportionally removed as the position is closed.&#x20;

#### Initial Margin and Leverage <a href="#initial-margin-and-leverage" id="initial-margin-and-leverage"></a>

Leverage can be set by a user to any integer between 1 and the max leverage. Max leverage depends on the asset.

The margin required to open a position is `position_size * mark_price / leverage`. *Normal Isolated Margin* positions support adding and removing margin after opening the position. Isolated positions will apply unrealized pnl as additional margin for the open position. The leverage of an existing position can be increased without closing the position. Leverage is only checked upon opening a position. Afterwards, the user is responsible for monitoring the leverage usage to avoid liquidation. Possible actions to take on positions with negative unrealized pnl include partially or fully closing the position or adding margin (if isolated).&#x20;

**Maintenance Margin:** the minimum collateral you must keep to avoid liquidation. If your account equity (collateral + PnL) falls below the maintenance level, you'll be liquidated. The maintenance level is currently determined by multiplying the maintenance margin by the total open notional position. The maintenance margin is set to half of the initial margin at max leverage for that asset, which varies from 3-40x. In other words, the maintenance margin ranges from 1.25% (at 40x max leverage) to 16.7 (at 3x max leverage).


---

# Agent Instructions
This documentation is published with GitBook. GitBook is the documentation platform designed so that both humans and AI agents can read, navigate, and reason over technical content effectively. Learn more at gitbook.com.

## Querying This Documentation
If you need additional information that is not directly available in this page, you can query the documentation dynamically by asking a question.

Perform an HTTP GET request on the following URL with the `ask` and `goal` query parameters:

```
GET https://docs.trade.xyz/perpetuals/risk-and-margining/margin-modes.md?ask=<question>&goal=<user_goal>
```

`ask` is the immediate question: it should be specific, self-contained, and written in natural language.
`goal` is what the user is ultimately trying to achieve, the reason they need the answer. Sharing it helps GitBook give you a better, more relevant answer. A goal is most helpful when it describes the outcome the user wants rather than restating the question. For example, with `ask=how do I create an API token`, a goal like `build a script that syncs our docs to a CMS` lets GitBook tailor the answer to that use case.

The response will contain a direct answer to the question and relevant excerpts and sources from the documentation.

Use this mechanism when the answer is not explicitly present in the current page, you need clarification or additional context, or you want to retrieve related documentation sections.
