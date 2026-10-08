> For the complete documentation index, see [llms.txt](https://docs.trade.xyz/llms.txt). Markdown versions of documentation pages are available by appending `.md` to page URLs; this page is available as [Markdown](https://docs.trade.xyz/perpetuals/mechanics/oracle-price.md).

# Oracle Price

How oracle prices are constructed and used for trade\[XYZ] perpetuals.

The oracle serves two critical functions:&#x20;

1. as the reference price for funding.
2. as a direct input to mark price calculation.

The key unlock for equity perpetuals is extending the oracle to operate 24/7, beyond external trading sessions of the underlying asset. To achieve this, the system prioritizes external pricing and employs a robust internal mechanism when external data is unavailable.

### External Pricing

The Relayer consumes price data for the underlying assets from a robust set of venues, markets, and institutional data providers. When external markets are open, the externally-derived fair price is transmitted as the oracle price in relayer updates.

Instrument-specific details for external price derivation can be found in their respective section.

### Internal Pricing

When external inputs are unavailable, the oracle advances via a continuous-time exponentially weighted moving average that incrementally adjusts the previous oracle price by a fraction of the impact price difference.

With oracle price $$S$$ the impact price difference ($$IPD$$) is defined as:

$$
IPD = max(P\_{impactBid} - S, 0) - max(S-P\_{impactAsk}, 0)
$$

where the impact bid price ($$P\_{impactBid}$$) and impact ask price $$P\_{impactAsk}$$ are the average execution prices to trade a configured impact notional amount on the bid and ask sides of the orderbook, respectively. If insufficient depth exists on a side, that side's contribution is set to zero.

The oracle is robust to irregular updates and market halts. It uses a time constant $$\tau = 30$$ minutes and updates as:

$$
S\_t = \beta\_t, S\_{t^-} + (1-\beta\_t), x\_t,
\qquad
\beta\_t = \exp!\left(-\frac{\Delta t^\ast}{\tau}\right),
\qquad
x\_t = S\_{t^-} + \mathrm{IPD}\_t
$$

where $$\Delta t^\ast = \min!\bigl(\Delta t,, c,\tau\bigr)$$, $$\Delta t = t - t\_{\mathrm{prev}}$$, and $$c = 0.1$$ (so $$1-\beta\_t \le 1 - e^{-0.1} \approx 9.5%$$).

> **Note:** This provides the more general form of the continuous-time EMA. Component (2) of the mark price samples the basis ($$x\_t = S\_t - P\_{mid,t}$$) with a time constant  $$\tau = 150$$ seconds.

When external data becomes unavailable, the internal mechanism initializes from the last available external price. When external inputs resume, the oracle reverts to the externally derived price on the next tick.


---

# Agent Instructions
This documentation is published with GitBook. GitBook is the documentation platform designed so that both humans and AI agents can read, navigate, and reason over technical content effectively. Learn more at gitbook.com.

## Querying This Documentation
If you need additional information that is not directly available in this page, you can query the documentation dynamically by asking a question.

Perform an HTTP GET request on the following URL with the `ask` and `goal` query parameters:

```
GET https://docs.trade.xyz/perpetuals/mechanics/oracle-price.md?ask=<question>&goal=<user_goal>
```

`ask` is the immediate question: it should be specific, self-contained, and written in natural language.
`goal` is what the user is ultimately trying to achieve, the reason they need the answer. Sharing it helps GitBook give you a better, more relevant answer. A goal is most helpful when it describes the outcome the user wants rather than restating the question. For example, with `ask=how do I create an API token`, a goal like `build a script that syncs our docs to a CMS` lets GitBook tailor the answer to that use case.

The response will contain a direct answer to the question and relevant excerpts and sources from the documentation.

Use this mechanism when the answer is not explicitly present in the current page, you need clarification or additional context, or you want to retrieve related documentation sections.
