> For the complete documentation index, see [llms.txt](https://docs.trade.xyz/llms.txt). Markdown versions of documentation pages are available by appending `.md` to page URLs; this page is available as [Markdown](https://docs.trade.xyz/perpetuals/markets/stocks/us.md).

# US

trade\[XYZ] perpetual markets for U.S. Stocks.

### External Coverage

The Relayer derives external prices for stocks 24/5, from Sunday 8:00 PM ET to Friday 8:00 PM ET.&#x20;

This is achieved by aggregating across the following sessions:

* Pre-Market: 4:00 AM - 9:30 AM ET
* Market: 9:30 AM - 4:00 PM ET
* Post-Market: 4:00 PM - 8:00 PM ET
* Overnight: 8:00 PM - 4:00 AM ET

The overnight trading session is provided by Blue Ocean ATS (BOATS).


---

# Agent Instructions
This documentation is published with GitBook. GitBook is the documentation platform designed so that both humans and AI agents can read, navigate, and reason over technical content effectively. Learn more at gitbook.com.

## Querying This Documentation
If you need additional information that is not directly available in this page, you can query the documentation dynamically by asking a question.

Perform an HTTP GET request on the following URL with the `ask` and `goal` query parameters:

```
GET https://docs.trade.xyz/perpetuals/markets/stocks/us.md?ask=<question>&goal=<user_goal>
```

`ask` is the immediate question: it should be specific, self-contained, and written in natural language.
`goal` is what the user is ultimately trying to achieve, the reason they need the answer. Sharing it helps GitBook give you a better, more relevant answer. A goal is most helpful when it describes the outcome the user wants rather than restating the question. For example, with `ask=how do I create an API token`, a goal like `build a script that syncs our docs to a CMS` lets GitBook tailor the answer to that use case.

The response will contain a direct answer to the question and relevant excerpts and sources from the documentation.

Use this mechanism when the answer is not explicitly present in the current page, you need clarification or additional context, or you want to retrieve related documentation sections.
