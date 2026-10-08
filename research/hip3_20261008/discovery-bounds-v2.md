> For the complete documentation index, see [llms.txt](https://docs.trade.xyz/llms.txt). Markdown versions of documentation pages are available by appending `.md` to page URLs; this page is available as [Markdown](https://docs.trade.xyz/perpetuals/changelog/discovery-bounds-v2.md).

# Discovery Bounds v2

Changelog entry for the Discovery Bounds v2 methodology.

### March 13, 2026 - Updated Discovery Bounds Methodology

> We upgraded the discovery bounds methodology, starting with CL. The v2 methodology is described in the [External Price](/perpetuals/mechanics/external-price.md) section of the documentation; the following is an archive of the v1 discovery bounds design with which CL was launched.

## How it works

Discovery Bounds are leverage-based limits that restrict how far the mark price may deviate from the external oracle price. Enabled by default on all markets, these bounds provide clearly defined price levels within which market makers and other market participants can confidently participate, without risk of liquidation. Their purpose is to prevent price movements that exceed what may reasonably constitute true price discovery.

#### Bound Range

The mark price is restricted to move within `1 / max leverage` of the last externally derived oracle price. Discovery Bounds are enforced 24/7, but are particularly relevant during internal pricing sessions (e.g., weekends or holidays), in which the external reference price may remain unchanged for extended periods. Market participants can use this information to manage their risk and margin accordingly.

Refer to the [Specification Index](/perpetuals/specifications-and-schedules/specification-index.md)for the max leverage and discovery bounds per market.&#x20;

#### Example

If the Silver oracle price is $75 at Friday’s 17:00 ET close, given 25× max leverage, the applicable Discovery Bound is ±4%. As a result, both the mark price and oracle price are restricted to the range $72 – $78 until external pricing resumes on Sunday at 18:00 ET.

If Silver is trading above Friday’s last externally-derived fair price and a trader believes the move does not reflect fair value, they may enter a short position. Once external pricing resumes, the market may reprice toward the externally-derived reference price depending on prevailing market conditions. Importantly, if a trader’s liquidation price lies outside the active price bounds, their position cannot be liquidated while those bounds are in effect.


---

# Agent Instructions
This documentation is published with GitBook. GitBook is the documentation platform designed so that both humans and AI agents can read, navigate, and reason over technical content effectively. Learn more at gitbook.com.

## Querying This Documentation
If you need additional information that is not directly available in this page, you can query the documentation dynamically by asking a question.

Perform an HTTP GET request on the following URL with the `ask` and `goal` query parameters:

```
GET https://docs.trade.xyz/perpetuals/changelog/discovery-bounds-v2.md?ask=<question>&goal=<user_goal>
```

`ask` is the immediate question: it should be specific, self-contained, and written in natural language.
`goal` is what the user is ultimately trying to achieve, the reason they need the answer. Sharing it helps GitBook give you a better, more relevant answer. A goal is most helpful when it describes the outcome the user wants rather than restating the question. For example, with `ask=how do I create an API token`, a goal like `build a script that syncs our docs to a CMS` lets GitBook tailor the answer to that use case.

The response will contain a direct answer to the question and relevant excerpts and sources from the documentation.

Use this mechanism when the answer is not explicitly present in the current page, you need clarification or additional context, or you want to retrieve related documentation sections.
