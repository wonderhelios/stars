> For the complete documentation index, see [llms.txt](https://docs.trade.xyz/llms.txt). Markdown versions of documentation pages are available by appending `.md` to page URLs; this page is available as [Markdown](https://docs.trade.xyz/perpetuals/changelog/funding-rate-formula-updates.md).

# Funding Rate Formula Updates

Changelog entry for xyz funding-rate formula updates.

### *December 19, 2025 - Scaling Factor for Funding Rate Formula Update*

The default funding rate for crypto perps has remained unchanged at 0.01% / 8 hours since Bitmex first introduced the mechanism in 2016. Borrow rates for traditional asset classes (e.g., equities, commodities) are typically closer to SOFR + 1-2%. We reduced the funding rate across all XYZ markets by applying a scaling factor of 0.5 to Hyperliquid’s funding rate formula. This lowers baseline funding to \~5.5% annualized and results in less aggressive funding during weekend price discovery as well.

The specific formula is `Funding Rate XYZ (F) = 0.5 [Average Premium Index (P) + clamp (interest rate - Premium Index (P), -0.0005, 0.0005)]`.


---

# Agent Instructions
This documentation is published with GitBook. GitBook is the documentation platform designed so that both humans and AI agents can read, navigate, and reason over technical content effectively. Learn more at gitbook.com.

## Querying This Documentation
If you need additional information that is not directly available in this page, you can query the documentation dynamically by asking a question.

Perform an HTTP GET request on the following URL with the `ask` and `goal` query parameters:

```
GET https://docs.trade.xyz/perpetuals/changelog/funding-rate-formula-updates.md?ask=<question>&goal=<user_goal>
```

`ask` is the immediate question: it should be specific, self-contained, and written in natural language.
`goal` is what the user is ultimately trying to achieve, the reason they need the answer. Sharing it helps GitBook give you a better, more relevant answer. A goal is most helpful when it describes the outcome the user wants rather than restating the question. For example, with `ask=how do I create an API token`, a goal like `build a script that syncs our docs to a CMS` lets GitBook tailor the answer to that use case.

The response will contain a direct answer to the question and relevant excerpts and sources from the documentation.

Use this mechanism when the answer is not explicitly present in the current page, you need clarification or additional context, or you want to retrieve related documentation sections.
