# TradeClimb platform research

Research date: 3 November 2026. Scope: a local, single-user Rust platform for strategy development, historical evaluation, and paper trading on Bitvavo. Agent orchestration belongs in a separate repository. This is a platform proposal, not evidence that the requested returns are achievable.

## Recommended direction

Build a small event-driven Rust engine shared by backtesting and paper trading, a Rust strategy SDK, a local job manager, and a Rust web application. Use Bitvavo EUR data as the primary dataset if its full coverage audit passes. Keep downloaded data and every evaluated strategy version immutable. Optimize development performance subject to risk constraints; use the second period sparingly for validation.

Treat 15% net CAGR as an initial research target, not a reason to keep searching indefinitely. A result with 15% maximum drawdown in a simulation is not a guarantee of that loss limit in the future. Keep the target fixed within an experiment so raising it does not silently change the meaning of a leaderboard.

## Existing projects and lessons

### Freqtrade

Freqtrade provides a useful reference for the complete workflow: historical testing, dry running, result inspection, and strategy tuning. Its backtesting documentation describes candle execution assumptions and saved strategy/configuration artifacts. Its lookahead analysis detects changes when strategies run against altered data slices. Borrow the workflow, artifact capture, and causality checks. Do not adopt its Python strategy interface for this Rust requirement. A backtest's candle fill rules are part of the result and must be versioned. Sources: [backtesting](https://www.freqtrade.io/en/stable/backtesting/), [lookahead analysis](https://www.freqtrade.io/en/stable/lookahead-analysis/).

### NautilusTrader

NautilusTrader now documents native Rust strategies and backtests; it should not be dismissed as Python-only. Its architecture shares core components between historical and live execution. It is the strongest candidate if TC needs a mature execution engine soon. Its broader instrument, actor, order, and adapter model also creates integration work for a small candle-only platform. Use it as an architecture reference and evaluate its pinned native Rust example before committing to a bespoke engine. Source: [native Rust guide](https://nautilustrader.io/docs/nightly/concepts/rust/).

### Barter

Barter is a Rust trading ecosystem with interchangeable strategy, risk, data, and execution components, including mock execution and an audit stream for monitoring. Borrow the separation between the engine's hot path and UI monitoring, and its interchangeable historical/live inputs. Adoption still requires validating the exact Bitvavo adapter and accounting semantics; a framework does not provide TC's experiment ledger or validation policy. Source: [Barter repository](https://github.com/barter-rs/barter-rs).

### HftBacktest

HftBacktest explicitly models feed/order latency and queue position using order-book and trade data. This is a useful reference for v2 execution realism. Those inputs cannot be reconstructed faithfully from minute candles. Do not add a superficial limit-order simulator to v1 and describe it as realistic. Source: [HftBacktest documentation](https://hftbacktest.readthedocs.io/en/latest/).

### Build or adopt

Implementation outcome: the [engine decision](decisions/0001-engine.md) records the native reference exercise and selection of the narrow TC engine for v1. The recommendation below describes the original research checkpoint.

Recommendation: own TC's experiment, SDK, UI, and data contracts. Provisionally use a deliberately narrow TC candle simulator; confirm this at milestone 1 against a native Nautilus example and Barter's interfaces. Adopt an existing engine only if it reduces tested code without requiring Python, a large runtime, or exposing framework internals in the public SDK. If adoption wins, replace the simulator implementation behind TC's interface and update the plan before widening scope. Avoid copying project code without inspecting its license and obligations.

## Initial market universe

Proposed research markets are BTC-EUR, ETH-EUR, SOL-EUR, XRP-EUR, and ADA-EUR. BTC and ETH provide core reference assets; SOL adds another ecosystem; XRP and ADA add different asset histories. These are engineering research choices, not a diversified portfolio recommendation or a claim that these five are optimal. Asset unit price is not a useful definition of market diversity.

Read-only requests to Bitvavo's public `GET /v2/markets` on the research date returned all five in `trading` status. Small `1m` candle requests for the first UTC hour of 1 November 2022 and 1 October 2026 returned data for all five. Some requests returned fewer than 60 rows. These probes establish that old samples exist, not continuous four-year coverage. No performance results were calculated or inspected.

Probe URLs used the form `https://api.bitvavo.com/v2/BTC-EUR/candles?interval=1m&start=1667260800000&end=1667264400000&limit=60`, substituting the market and date. A full data audit remains an implementation acceptance gate. Current market metadata is not proof of historical trading rules.

Choosing today's surviving assets introduces selection bias. Freeze this universe in every experiment and do not claim the result generalizes to the entire crypto market. A historical universe including delistings belongs in v2.

## Historical data and price classes

### Free primary data

Prefer direct Bitvavo EUR candles over substituting another exchange's USDT prices. The public candles endpoint supports `1m`, time bounds, up to 1,440 records, and newest-first responses. It omits intervals without trades. The importer must reverse/sort, deduplicate, and request explicit time windows rather than stopping when a page is short. Source: [Bitvavo candles](https://docs.bitvavo.com/docs/rest-api/get-candlestick-data/).

Target the completed interval `[2022-11-01, 2026-11-01)` UTC, subject to audit. That is 1,461 days, 2,103,840 possible minute slots per market, and 10,519,200 across five markets. A one-day request per market is approximately 7,305 requests before retries and warm-up history. These are calculated planning quantities, not measured download time or disk requirements. Rate-limit response headers must govern scheduling. Source: [Bitvavo limits](https://docs.bitvavo.com/docs/rate-limits/).

Bitvavo's FAQ also describes candle prices changing with order-book updates, while the endpoint describes candles through trades. This ambiguity matters: audit a contemporary sample against collected trades before assigning strict trade-bar semantics. Source: [Bitvavo FAQ](https://docs.bitvavo.com/docs/faqs/). Keep raw payloads so later corrections do not silently rewrite past evaluations.

### Free alternatives

Kraken publishes downloadable OHLCVT CSV archives, including minute intervals and EUR markets where listed; files omit no-trade intervals. This is the first alternative if Bitvavo's historical coverage is insufficient. Audit the archive's actual end date and pair coverage instead of relying on the page update date. Source: [Kraken historical downloads](https://support.kraken.com/ca/articles/360047124832-downloadable-historical-ohlcvt-open-high-low-close-volume-trades-data).

Binance publishes daily/monthly candle and trade archives with checksum files. Spot archive timestamps change to microseconds from January 2025. These are useful secondary datasets, but USDT quotes and venue differences make them unsuitable as an unlabeled substitute for Bitvavo EUR. Keep providers separate; never patch holes with another venue invisibly. Source: [official public archive repository](https://github.com/binance/binance-public-data).

### Paid data

For a modest paid budget, CoinAPI currently lists a $79/month Startup plan with 1,000 REST credits/day; larger plans and metered access are also offered. A subscription price is not a quote for downloading four years across five markets. Obtain symbol coverage, first/last timestamps, credit estimates, and redistribution terms before purchasing. Source: [CoinAPI pricing](https://www.coinapi.io/products/market-data-api/pricing).

For detailed trade/quote/book research, Tardis provides tick datasets and replay products. Its spot Solo pricing currently displays $900/month; its historical-access policy gives four years with yearly billing, versus shorter access under monthly or quarterly billing. Confirm the exact plan, venue coverage, and billing total. Bitvavo coverage was not established by this research; do not buy a subscription on that assumption. Sources: [Tardis pricing](https://tardis.dev/#pricing), [history and billing](https://docs.tardis.dev/faq/billing-and-subscriptions).

Recommendation: spend nothing on v1 data until the direct Bitvavo audit fails or a strategy demonstrates that minute candles are insufficient. Paid book data is an execution-research upgrade, not an automatic improvement in strategy quality. Historical spread, depth, and executable size remain unknown in a candle-only dataset.

## Costs and benchmark design

Bitvavo currently lists entry-tier EUR taker fees of 0.25% per trade and maker fees of 0.15%. Market-only simulation should start with the taker rate. Pin an explicitly assumed fee schedule per experiment; applying today's rate to history is not a reconstruction of historical account fees. Source: [Bitvavo fees](https://bitvavo.com/en/fees).

A buy and subsequent sell therefore cost approximately 0.5% in fees alone at the entry tier, before spread/slippage. This makes turnover a first-class diagnostic. Use a configurable adverse execution allowance, initially proposed at 10 basis points per fill, and run development stress checks at 25 and 50 basis points. These allowances are sensitivity assumptions, not measured Bitvavo estimates. Avoid double-counting spread in paper trading when using observed bid/ask quotes.

Use BTC buy-and-hold and an equal-weight, initially allocated five-asset buy-and-hold basket as reference curves; show individual-asset references when relevant. Use the same venue, EUR denomination, dates, starting cash, fees, and end liquidation convention. The basket has no periodic rebalancing in v1. Show risk alongside return because an unconstrained buy-and-hold portfolio may fail TC's drawdown objective.

## Evaluation and optimization pitfalls

Repeated strategy selection can overfit even when each backtest is technically correct. Bailey and coauthors discuss why conventional holdouts are unreliable under investment backtest selection. TC's exact policy below is a proposed mitigation, not a claim of statistical validity. Source: [The probability of backtest overfitting](https://escholarship.org/uc/item/4w1110bb).

Expose complete period 1 results and only period 2 net CAGR to the optimizer, as requested. Scalar feedback still enables adaptive fitting. Count validation queries across the whole experiment and all strategy families, cache repeated queries, freeze dates, and limit new validation evaluations. Future paper trading then provides genuinely new observations. Keep rolling/walk-forward testing in v2, and recommend a one-time final historical holdout there as well.

Do not leak validation trades through charts, logs, model caches, partial progress, benchmark ratios, combined-period statistics, error messages, or strategy-produced stdout. Without sandboxing and access control, this remains a cooperative interface rule; native code and agents sharing the filesystem can bypass it.

Model calls add a separate bias: today's model may know events from historical evaluation periods. Recorded inference responses make replay reproducible but do not erase that knowledge. The user accepts this limitation for v1, including historical ranking. Record model provenance and any unknown training cutoff without excluding the result. Historical performance from such calls is not evidence of point-in-time predictive ability.

## Foundation choices

Axum/Tokio fit the proposed local API and asynchronous market feed; CPU backtests must run outside the async request executor. Leptos supports a Rust browser UI, with a small pinned chart-library bridge if needed. References: [Axum](https://docs.rs/axum/latest/axum/), [Leptos](https://book.leptos.dev/).

Use a provider-independent inference service with a fake implementation for tests and an optional local Ollama adapter. The adapter is an interface demonstration, not a requirement to download or run a model. Source: [Ollama generation API](https://docs.ollama.com/api/generate).

Run Clippy with warnings denied on the same pinned toolchain as compilation, in local checks and CI. Source: [Clippy CI guidance](https://doc.rust-lang.org/clippy/continuous_integration/index.html). Rust was initially absent; following user approval, Rust/Cargo 1.99.1 and the required components were installed and pinned. See [development setup](development.md).


## Cost accuracy audit — 4 November 2026

The 25 bps commission assumption is appropriate for current entry-tier EUR market orders. The complete cost estimate is **not calibrated to realized execution**. A short live-book sample suggests the 10 bps adverse allowance is conservative for displayed liquidity in this window, but does not establish historical accuracy or stressed-market coverage. No simulator defaults were changed by this audit.

### Commission and accounting

All five supported markets are currently Category A. Bitvavo lists 25 bps taker fees below EUR 100,000 of rolling 30-day volume, 20 bps from EUR 100,000, and 18 bps from EUR 250,000. Our fixed 25 bps is therefore correct for a current entry-tier taker account and can overstate commissions at higher volumes. It does not reconstruct historical schedules or account tiers. Maker pricing is inapplicable to our market-only execution model. Sources: [official fee schedule](https://www.bitvavo.com/en/fees), [account fee API](https://docs.bitvavo.com/docs/rest-api/get-market-fees/).

`RunConfig::new` pins 25 bps fees and 10 bps adverse price movement per fill. `Simulator::execute` debits/credits the adjusted fill notional, then applies fees to that notional; both sides and terminal liquidation incur costs. At an unchanged reference price, a fully invested buy/sell retains `(1-0.001)*(1-0.0025)/((1+0.001)*(1+0.0025))` of starting cash: a **69.76 bps (0.6976%) loss**, before quantity rounding. Simply summing 35 + 35 bps gives the useful approximation of 0.70%.

There is a precision mismatch to resolve. The engine rounds each fee upward at eight decimal places. Bitvavo describes quote-asset fees rounded up at asset precision, with fee averaging across fills of an order; its help article describes fiat rounding at two decimals. Our one-simulated-fill accounting does not replicate that reconciliation. For illustration, EUR 5 × 0.25% = EUR 0.0125; rounding that fee up to cents would charge EUR 0.02 rather than EUR 0.0125. Exact account-ledger treatment should be reconciled before calling fees exchange-exact. Sources: [trading rules §17.4](https://www.bitvavo.com/en/trading-rules), [fee calculation help](https://support.bitvavo.com/hc/en-us/articles/4405175148689-What-is-the-trading-fee-when-I-buy-or-sell-crypto).

### Live public-book sanity check

The read-only [audit script](../scripts/cost-audit.py) captured three snapshots per market from 13:17:58 to 13:18:05 UTC. It walked up to 1,000 levels for EUR 1,000, 2,000 and 10,000 orders. Buy calculations spend the stated quote notional before fees; sells use base quantity worth that notional at the contemporaneous midpoint. Every hypothetical side had sufficient returned depth. Metadata, request timestamps, response hashes, summaries and individual calculations are in [cost-audit.json](cost-audit.json). The [interactive chart](cost-audit.html) compares these static-book costs with the assumed allowance. Endpoint definition: [Bitvavo order book API](https://docs.bitvavo.com/docs/rest-api/get-order-book/).

| Market | EUR 2,000 median cost, bps/side | EUR 10,000 median | EUR 10,000 largest sampled side |
| --- | ---: | ---: | ---: |
| BTC-EUR | 0.07 | 0.07 | 0.16 |
| ETH-EUR | 0.24 | 0.51 | 0.68 |
| SOL-EUR | 0.51 | 1.11 | 1.67 |
| XRP-EUR | 0.79 | 0.92 | 1.45 |
| ADA-EUR | 1.26 | 2.34 | 2.89 |

These are **spread plus displayed-depth cost relative to midpoint, excluding commissions**. The median pools buy/sell costs from three snapshots (six hypothetical sides), not six independent market observations. The largest sampled side was 2.89 bps. The fixed 10 bps allowance exceeded every displayed-book estimate in this short window. That supports retaining a conservative sensitivity assumption; it does **not** justify reducing it or interpreting it as a measured percentile.

A static book walk assumes visible orders remain available throughout execution. It excludes cancellations, latency, adverse selection, subsequent impact and volatile periods. The backtest reference is the next minute candle's opening trade price, not midpoint; transaction-price spread effects may already be embedded. Consequently this sample cannot estimate historical prediction error for the 10 bps parameter. Bitvavo confirms that market orders may consume multiple price levels and that order size and liquidity affect execution: [official explanation](https://support.bitvavo.com/hc/en-us/articles/4405191470353-How-are-cryptocurrency-prices-determined).

### Remaining model limitations and recommended work

- **Historical fills:** a positive candle volume is sufficient for the engine to fill the full intended quantity at one adjusted opening price. There is no participation cap, book-depth walk, partial fill or latency-dependent price. This can be optimistic during thin or stressed markets even when the usual fixed allowance is conservative.
- **Paper fills:** `execute_quotes` obtains executable bid/ask quotes, then calls the same engine with its default 10 bps adverse adjustment still enabled. Thus paper costs include observed spread **plus** a further 10 bps, not a measured 10 bps total. It waits if required quantity exceeds displayed top-level size rather than walking deeper liquidity. That is a conservative policy, not a reproduction of an exchange market order.
- **Market rules:** the sample confirms the EUR 5 minimum, but BTC/ETH/SOL allow eight quantity decimals while our default rules use six for every market. ADA/XRP match six. The API also exposes per-market tick size, base minimums and quote precision that are not fully reconstructed historically. Source: [market metadata API](https://docs.bitvavo.com/docs/rest-api/get-markets/).
- **Calibration priority:** first reconcile EUR fee rounding and use a pinned, explicit account tier; then capture timestamped spread/depth and simulated order size over representative sessions, including volatile periods. Estimate costs by market, size and volatility, and separate observed spread from residual latency/impact. Keep development stress scenarios at 10/25/50 bps per fill and disclose them as assumptions. Preserve validation's frozen configuration; research changes should be new explicit experiments, not silent retroactive rescoring.

The available evidence supports an entry-tier commission assumption and a conservative normal-liquidity allowance. It does not support a numerical claim such as “costs are 95% accurate,” or a guarantee that the allowance covers tail execution costs.

## V2 data-source shortlist — checked 4 November 2026

The owner accepted the Bitvavo gaps and reserved August–October 2026 as the untouched final test. Ranking prioritizes a usable historical comparison now, cost, then venue fidelity. No option is a verified clean, venue-matched replacement for the whole four years. No vendor was contacted or paid subscription started. Research inspected documentation and coverage metadata, not final-test prices or strategy performance.

| Rank | Way to improve the data | Evidence and cost | Limitation / next step |
| --- | --- | --- | --- |
| 1 | Build a separate EUR-market research dataset from **Kraken's official historical trades/OHLCVT** | Public bulk downloads cover each market from inception; the currently published complete bundles stop at **30 September 2026**. One-minute OHLCVT documents no-trade omissions. | Different venue/liquidity/costs. Audit the five EUR pair files/listing dates and use only pre-August research data. Keep this separate from Bitvavo. Current delivery is five roughly 2-GB OHLCVT parts or fourteen trade parts. |
| 2 | Obtain native Bitvavo trades/quotes from **Kaiko**, then rebuild audited candles | Public instrument metadata for BTC, ETH, SOL, XRP and ADA/EUR starts their **trade history on 1 January 2024**. Trades, quotes and books are offered; pricing requires a custom quote. | Useful native upgrade for 2024 onward, not a documented repair for the July 2023 gap. Earlier history and quote/book start dates need separate confirmation. Require a pre-holdout sample and coverage/quality audit before buying. |
| 3 | Use **Tardis Bitvavo** trades/books and preserve forward raw trade/book events | Documentation and exchange metadata agree on **23 September 2026** as the start for all five markets. First-day monthly CSV samples require no Data API key; full access uses subscriptions (applicable price not verified). | Cannot repair older gaps or supply the complete final quarter. Useful for future execution calibration. Do not inspect October samples while the final test stays sealed. |

Primary sources: [Kraken OHLCVT](https://support.kraken.com/articles/360047124832-downloadable-historical-ohlcvt-open-high-low-close-volume-trades-data), [Kraken trades](https://support.kraken.com/articles/360047543791-downloadable-historical-market-data-time-and-sales-), [Kaiko instrument metadata](https://reference-data-api.kaiko.io/v1/instruments?exchange_code=bvav&base_asset=btc&quote_asset=eur&class=spot&limit=10&offset=0), [Kaiko products](https://www.kaiko.com/news/kaiko-enhances-market-data-platform-by-integrating-bitvavo), [Kaiko pricing](https://www.kaiko.com/about-kaiko/pricing-and-contracts), [Tardis coverage](https://docs.tardis.dev/historical-data-details/bitvavo), [Tardis exchange metadata](https://api.tardis.dev/v1/exchanges/bitvavo). The five-market metadata extracts and per-pair query URLs are retained in [the audit](v2-data-source-audit.json). Start dates describe published metadata, not verified minute-by-minute completeness. Kaiko's December 2024 partnership announcement alone would have been insufficient to establish coverage.

A fresh public Bitvavo trades query for BTC-EUR `[2023-07-23T01:00Z, 2023-07-23T02:00Z)` returned zero rows. This does not establish the cause of missing candles or prove another archive complete. The [trade API](https://docs.bitvavo.com/docs/rest-api/get-trades/) limits each request to a 24-hour interval; that is not proof of a 24-hour historical retention limit. Repeating candle downloads is not an established repair. The exchange documents [trade subscriptions](https://docs.bitvavo.com/docs/websocket-api/trades-subscription/) and [book updates](https://docs.bitvavo.com/docs/websocket-api/book-subscription/); future recording must retain sequences/reconnect gaps and immutable events.

Recommendation: keep the accepted Bitvavo snapshot as the primary dataset and use Kraken as an independently labeled robustness check. Consider Kaiko for newer native history only after a sample/quote; use Tardis and forward collection for future execution evidence. Do not patch native prices with another venue or inspect final-test prices during source selection.
