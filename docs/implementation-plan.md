# TradeClimb v1 implementation plan

Historical V1 planning record. Current behavior and policy are documented in [status](status.md), [operations](operations.md) and [API](api.md); later decisions supersede proposals here.

Recommendations accepted on 3 November 2026, with the model-knowledge policy adjusted below. See [research](research.md) for sources and [self-review](self-review.md) for the original risks.

## Complete v1 workflow

An external agent or human creates a Rust strategy project using the TC SDK, registers an immutable strategy version, submits development backtests or bounded parameter searches, and inspects streamed and saved results. Selected candidates receive limited validation evaluations returning only net CAGR. The user promotes a fixed version and configuration to a separate paper portfolio. The UI tracks its activity alongside historical runs. Every attempt remains in the history book with a description and results.

V1 includes manual submission and bounded numeric tuning, not AI idea generation or agent orchestration. V2 may automate the full pipeline in the separate agent repository. V1 never sends orders or requires a Bitvavo account key.

## Proposed defaults

- Native Rust backend, engine, SDK, CLI, and Leptos browser UI. Allow a small vendored/pinned JavaScript chart bridge rather than building a chart engine.
- BTC-EUR, ETH-EUR, SOL-EUR, XRP-EUR, ADA-EUR; spot, long or cash, no leverage or shorts.
- Each strategy run and paper session has an independent EUR 10,000 virtual portfolio, configurable and immutable per run. No pooling capital across strategies in v1.
- One-minute base data with completed, UTC-aligned 5-minute/hour/day aggregates. Other integral-minute intervals can follow the same contract. No partial higher-timeframe bars in strategy inputs.
- Proposed development `[2022-11-01, 2025-11-01)` and validation `[2025-11-01, 2026-11-01)` UTC, only after coverage audit. Collect preceding warm-up data separately.
- Market intents only; buy/sell sized in quantity or target portfolio weight. No resting limit orders or intrabar stop fills.
- Max drawdown means decline from the running portfolio equity high-water mark, including unrealized P&L and estimated exit costs. Proposed limit: 15%. This interpretation needs review because “15% of capital” could instead mean loss from initial capital.
- Mean completed calendar-month simple return must be positive. Also report geometric monthly return, positive-month fraction, and worst month. Positive arithmetic mean alone is weak evidence.
- Initial target: 15% net annualized growth; target and all risk rules frozen per experiment.
- Two concurrent backtest workers by default, configurable within local memory/CPU limits. No broker, distributed queue, or Kubernetes dependency.
- LAN access without login, interpreted as unauthenticated access on a trusted LAN. Bind address explicit in configuration; no public hosting.

Dates, budgets, execution allowances, capital, and performance targets here are proposed defaults rather than additional user requirements.

## Architecture and boundaries

Use a Cargo workspace with focused crates, not independently deployed microservices. The control service owns scheduling, persistence, public Bitvavo feeds, and serving web assets. It launches a small number of local worker processes. Workers isolate crashes and allow termination; they do not constitute a security sandbox.

| Component | Responsibility |
| --- | --- |
| `tc-core` | Market identities, UTC time, candles, money, intents, fills, portfolio events, metrics definitions |
| `tc-sdk` | Versioned strategy interface, indicators, causal history, parameter schema, resampling, inference requests |
| `tc-engine` | Deterministic simulation, accounting, risk checks, replay, and paper-session state transitions |
| `tc-data` | Bitvavo ingestion/live feed, validation, immutable manifests, Parquet readers and writers |
| `tc-service` | Axum API, SQLite metadata/jobs, worker supervision, inference service, SSE streaming |
| `tc-cli` | Local registration/build/run/export, data management, configuration, owner inspection |
| `tc-web` | Leptos WASM UI and chart bridge; no strategy execution or accounting |

Keep inference and tuning modules inside these crates initially. Split them only when their boundaries justify it. Exclude native-only dependencies from WASM builds. Share API DTOs through a lightweight module/crate if needed, not the whole native engine dependency graph.

Use SQLite in WAL mode for metadata, job state, experiment budgets, and paper events. Use immutable Parquet partitions for market data and large historical output series, with raw source responses compressed on disk. Keep active rolling data separate from frozen datasets. Atomically publish files before committing their manifest reference; clean orphaned files on startup.

Do not write every minute of every run into SQLite. Batch equity/output chunks, retain exact fills and exact risk summaries, and apply configurable retention to verbose logs. Cache normalized source data within a fixed memory budget; never create a full cloned dataset per candidate.

## Strategy delivery and SDK

Strategies live outside this repository. Provide a template that depends on a versioned SDK and compiles into an executable. Register source-tree hash, lockfile, SDK/toolchain versions, target, executable hash, parameter schema, author description, parent version, and optional external-agent metadata. Compile once per code version; parameter sweeps reuse the executable.

Use a versioned length-framed IPC protocol over local pipes, not Rust dynamic libraries with an unstable ABI. Send bounded batches of historical events for speed while requiring decisions to be indexed by event time; the official SDK reveals only the current prefix. Paper mode supplies the same event contract one step at a time. Native strategy code is trusted and could inspect future batch contents; v1 causality is a tested SDK convention, not an adversarial guarantee. If batching complicates correctness, start with buffered single-event framing and benchmark before optimizing.

Strategy inputs contain finalized market events, causal trailing history, portfolio state, and a simulation clock. Strategy outputs are intents, optional checkpoint state, and diagnostics. The engine owns accounting, execution timing, fees, drawdown calculation, and risk rejection. Cross-market events at the same timestamp use a documented deterministic ordering, with all completed bars published before portfolio decisions for that timestamp.

Basic v1 tools: SMA, EMA, RSI, ATR, rolling returns/volatility, position sizing, and completed-bar aggregation. Define warm-up length, initialization, missing-input behavior, and numeric tolerances. Include cash, buy-and-hold, and a simple moving-average example as platform fixtures, not promised profitable strategies.

Inference is an optional logical capability such as `ModelClient::infer(capability, request)`. Configuration resolves capabilities to a provider/model so strategies need not name one. Include structured outputs, timeout, cancellation, call/token budget, provenance, and an explicit failure policy. Start with a fake provider and optional Ollama HTTP provider. Persist input/output hashes and responses for exact replay. Reuse only responses from the same run/provenance scope; do not cross-contaminate periods. Backtests perform no uncached external inference unless explicitly enabled. The user accepts historical model knowledge in v1: record unknown training cutoffs as an informational limitation, without excluding those runs from the primary ranking. Do not describe such model evaluations as point-in-time clean.

## Data acceptance and resampling

The initial importer fetches bounded UTC windows from Bitvavo, observes response limits, retries with backoff, resumes from a manifest, and preserves raw responses. Deduplicate by venue/market/interval/open time; retain revisions as a new snapshot. Normalize prices and quantities without silently rounding source strings.

Produce a coverage report for all five markets: first/last candle, duplicates/conflicts, invalid OHLC/volume, no-candle intervals, outages, timestamp units, and suspicious changes. Check a small recent sample against captured trades and provider semantics. Missing candles cannot always distinguish no trading from a provider outage; retain an `unknown` classification instead of inventing certainty. Large unresolved gaps block a dataset's default acceptance. Freeze quality thresholds before observing strategy performance.

Use sparse real bars and an explicit minute calendar. Valuation can carry the last known price with an age flag; stale marks remain visible and cannot silently count as current risk observations. Strategies receive gap/staleness information. No execution on synthetic zero-volume bars. Missing or stale observations can prevent risk eligibility even when CAGR exists.

Aggregate only observations available by the aggregation boundary. Preserve observed counts, gap flags, volume sums, and completeness status. Do not fabricate a tradable hourly open from an unobserved minute. Warm-up feeds indicators without trading or counting return; reset portfolio and strategy trading state at each evaluation-period boundary. Never carry a development position into validation.

If direct history fails audit, propose Kraken EUR as a separately named research dataset. It must not be presented as validated Bitvavo historical execution. No automatic venue mixing.

## Execution and accounting contract

In historical mode, a decision based on a closed minute bar may fill no earlier than the next observed tradable bar's open, with adverse execution allowance and fees. Expire an intent after a configured short validity window rather than waiting through an outage and filling an old signal. Apply quantity precision, minimum order rules, available cash including fees, and no-negative-position checks. Pin assumed historical market rules when actual historical metadata is unavailable.

Default historical fee assumption: 0.25% taker per side. Proposed adverse allowance: 10 bps per fill; development stress variants use 25/50 bps. Make total costs explicit in fills, curves, exports, and benchmarks. Keep model/provider charges in experiment operating-cost reports; calculate a separate all-in return if those charges are allocated to capital. Tax, deposits/withdrawals, staking, lending yield, and personal tax circumstances are outside simulated trading P&L.

Use decimal/fixed-point money and quantity accounting with explicit rounding; floating-point indicators and return statistics are acceptable with defined tolerances. Never derive exact cash balances from rounded chart values. Mark open holdings at conservative estimated liquidation value and deduct exit fees consistently. Record an explicit terminal liquidation event for annual evaluation and baselines, or report a flagged failed valuation if the final price is stale.

Calculate drawdown from full-resolution portfolio valuations, never the chart samples. Historical close-to-close monitoring cannot certify the absence of intraminute losses. On reaching the configured threshold, reject new risk and request a simulated liquidation at the next executable observation, then keep the session halted until manual reset. Gaps and liquidation costs may take drawdown past 15%; record the breach instead of clamping equity. In paper mode also monitor available live prices between strategy decisions, with stale-data handling.

Paper execution uses the first eligible fresh bid/ask received after the intent, plus an explicit adverse allowance, fees, quantity rules, and a conservative size/quote-availability check. It is still a simple full-fill model for small orders, not a queue or depth simulator. Historical and paper modes share accounting/risk logic but have different, labeled price sources. Persist those observations and event order so paper sessions can be replayed.

## Metrics and validation visibility

For positive starting and ending equity, `CAGR = (ending_equity / starting_equity)^(365.25 / elapsed_days) - 1`. Reject undefined/nonpositive cases with explicit status. Show actual dates/duration, annual calendar returns, and net total return; never silently describe a short annualized sample as a year of evidence. Monthly returns include unrealized P&L and costs and use UTC calendar boundaries; identify partial months separately.

Period 1 reports include net CAGR, maximum drawdown, monthly distribution, total costs, turnover, exposure, trades, duration, data quality, and benchmark curves. Risk eligibility requires max drawdown at most 15%, positive mean monthly return, and acceptable data quality. Rank eligible candidates by net CAGR; show ineligible ones with reasons instead of hiding them. Raise warnings for sparse trades/exposure and insufficient history. No automatic claim of statistical significance.

Period 2's optimizer result contains only `net_cagr` on success, plus opaque run identification and generic operational status. No risk pass/fail bit, per-market breakdown, benchmark-relative value, trades, curves, logs, or intermediate equity is returned. A risk breach is handled within the simulation and reflected in terminal CAGR, not exposed as a separate diagnostic. Keep period 2 progress at queued/running/completed only. Test that errors and debug output do not reveal forbidden details.

The human UI also defaults to scalar validation results so an agent cannot trivially collect detail from a second HTTP route. An explicit local owner inspection can export full validation diagnostics and marks the experiment's validation as revealed. That action ends its claim to restricted validation. Only the owner workflow may use those details to assess paper promotion; do not claim scalar-only feedback proves validation drawdown compliance.

Persist query counts across all candidates/families and prevent new experiment IDs from silently resetting the same campaign budget. Repeated identical validation evaluation returns the cached scalar. Proposed campaign budget: 10 unique validation evaluations, with manual extension recorded as additional contamination. Cap applies before scheduling and is transactionally reserved. This reduces feedback; it does not create an untouched test set.

## Bounded tuning and multiple strategies

Support seeded random search and bounded grids for numeric/enumerated parameters; coordinate mutation around the current best can be a small SDK utility. Code-level strategy invention and agent token management stay in the other repository. Track parent-child relationships and a campaign across several strategy families.

Proposed defaults per family: 100 development evaluations, wall-clock deadline of 30 minutes, and early stop after 20 completed trials without a meaningful feasible improvement. Define improvement as at least 0.25 percentage points of net CAGR, configurable and frozen. Do not discard infeasible trials or mistake them for successful improvement. Queue families fairly; cancel safely; persist the random seed and every trial. These are resource limits, not a definition of an economic optimum.

Only development results drive automatic tuning. Validate selected development winners within the campaign budget. Raising the CAGR target creates a new objective revision while preserving the same validation exposure ledger; it does not reopen unlimited queries. Track strategy-side inference budgets, but external code-generation token use can only be recorded if the external orchestrator reports it.

## Jobs and durability

Persist jobs as queued, preparing, running, completed, failed, cancelled, or interrupted. Use an idempotency key on submissions, bounded queues, worker heartbeats, timeouts, and process termination on cancellation. Kill orphan strategy processes as part of worker cleanup. A failed native strategy must not stop the API or another paper session.

On service restart, mark unfinished historical jobs interrupted and allow an explicit rerun with the same manifest. V1 need not checkpoint arbitrary historical strategy state mid-run. Paper sessions do need durable state: append market observations, intents, fills, and strategy checkpoints with sequence numbers; restore via replay before resuming. Recover missed finalized market bars through REST, but never backdate paper fills into an offline interval. Resume only with fresh observations; show the gap and any halted state.

Use bounded SSE events for progress and sampled development curves, coalesced to at most two UI updates per second per active run. Persist event cursors or force a snapshot on reconnection. Slow browsers may drop intermediate visual updates; jobs and accounting must continue independently. Period 2 uses the restricted event schema.

## Web interface

The overview shows strategy descriptions, active job states, net development growth/drawdown, validation CAGR, paper equity, and small comparison curves. Historical and active runs use the same detail view. Make results' period, venue, cost assumptions, version, and experimental status accessible without cluttering the overview.

Run details show net equity and benchmark curves, drawdown, optional asset-price charts, and fill markers. Use green buys and red sells with distinct shapes/text for accessibility. Markers appear at actual fill time and have trade details. For portfolio equity, identify the market on each marker.

Default chart cap: 1,500 curve points per series, configurable with a server hard cap; use time buckets preserving extrema, endpoints, and risk-breach timestamps. If mandatory points exceed the budget, aggregate and label them explicitly. Cap fill markers separately at 200 and aggregate dense areas with counts. Exact paginated trade data remains available for permitted periods. Zoom requests fetch a more detailed range; downsampling never affects metrics. Provide loading, empty, stale, interrupted, and failure states.

Provide local controls to submit/cancel a run, tune parameters, compare versions, start/stop a paper session, and archive a strategy. Require explicit confirmation for resetting a paper portfolio. Retain old versions and their results when descriptions change.

## LAN and trust boundaries

No login or strategy sandbox in v1, as requested. Native strategies and Cargo build scripts can execute with the local user's permissions. Crash separation and timeouts are reliability features only. Keep registration/building in the CLI against explicitly chosen local paths; do not expose arbitrary code upload or shell commands through the web API.

Any LAN user who reaches the API can view research and invoke permitted run controls. Bind only the intended interface, restrict CORS/Host/Origin for browser mutations, and document the expected LAN firewall boundary. Model credentials stay in service-side environment/configuration and outside saved artifacts. Full protection from generated strategy code and strict holdout secrecy require v2 isolation and access control.

## Milestones and acceptance

### Milestone 0 Repository controls before platform code

Provision Rust 1.99.1 with its bundled Cargo, rustfmt, Clippy, and the WASM target, then create the minimal workspace and check configuration. The root `rust-toolchain.toml` pins these requirements. No domain implementation starts until the check runner and CI exist and pass on the minimal scaffold.

Required native gates are `cargo fmt --all -- --check`, native workspace `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`, and documentation tests. Exclude the browser crate from native-only commands if required and add explicit WASM `cargo check`/Clippy/build commands for it. Use a deliberate feature matrix, not an invalid blanket `--all-features` across native and browser targets.

Commit `Cargo.lock`, a shared check script, lint policy, CI configuration, and development instructions. Deny unsafe code in TC-authored engine/SDK crates unless a justified narrow exception is reviewed. Fail on production `unwrap`/`expect` where recoverable; allow precise test-only exceptions. Add dependency advisory/license checking with documented exceptions and artifact/secret ignores. Demonstrate that an intentional formatting/lint violation fails the check, then remove it. CI provider is chosen when the repository host is known; local checks are mandatory regardless.

### Milestone 1 Contracts and execution choice

Freeze the SDK/IPC, time, money, data-manifest, metric, and fill contracts. Run one native Nautilus reference example and inspect Barter's relevant interfaces, then record the engine decision. Build hand-calculated cash, fee, buy-and-hold, warm-up, and signal-timing fixtures behind the chosen contract. Acceptance: changing future inputs cannot change prior decisions through the official SDK, and no signal fills on its own completed candle.

### Milestone 2 Data ingestion and audited snapshot

Implement resumed imports, Parquet manifests, quality reporting, aggregation, and the live feed recorder. Download/audit the four-year target and needed warm-up. Acceptance: reproducible immutable snapshot, explicit gap policy, consistent UTC aggregation, and a provider decision supported by actual coverage. Use small fixtures in CI; keep multi-year downloads out of source control.

### Milestone 3 Engine and strategy toolkit

Implement indicators, independent portfolios, deterministic historical runs, costs, risk handling, and metrics. Acceptance: exact accounting fixtures; causality and resampling checks; sparse/stale data behavior; identical replay with pinned inputs; benchmark equality against hand-calculated/reference fixtures; and realistic cost stress reports on development data. Integrate the optional inference interface with fake tests and recorded replay.

### Milestone 4 Orchestrator and history book

Implement API/CLI registration, process supervision, scheduling, persistence, bounded tuning, campaign budgets, and validation redaction. Acceptance: two strategies run concurrently; cancel/crash/restart leaves correct durable states; budget reservation is race-safe; all result/export/event/log routes obey period visibility. Source, parameters, dataset, cost model, and result hashes reconstruct a run.

### Milestone 5 Web overview and streaming

Implement the clean overview, detail curves, benchmark toggles, markers, history, and controls. Acceptance: chart bounds hold for dense histories, trade pagination is exact, live updates recover after disconnect, and charts use full-resolution metrics without recomputing them from sampled lines. Verify basic keyboard access, contrast, and readable markers.

### Milestone 6 Paper trading and full workflow

Connect finalized Bitvavo bars and public bid/ask feeds to pinned strategies, independent virtual portfolios, and live UI events. Acceptance: start/stop, stale feeds, quote expiration, restart/replay, missed-data recovery, and drawdown halt behave as specified. Run an automated record/replay smoke test and a manual continuous paper soak across multiple candle boundaries; reserve a longer paper observation period for strategy evidence rather than claiming a short soak proves profitability.

### Milestone 7 Release and performance evidence

Document one-command local startup, strategy authoring, data import, costs, trust assumptions, backups, restore, and the entire develop/validate/paper workflow. Benchmark a simple non-inference strategy over the five-market four-year dataset in release mode. Proposed target: under 60 seconds with one worker and under 2 GiB peak worker RSS on the user's PC, measured separately from compilation/import. Confirm or revise targets after measuring hardware; no promise for arbitrary strategy/model workloads. Confirm two-worker runs remain responsive and within a configured total memory budget.

Back up SQLite consistently with its backup API and copy referenced immutable artifacts; test restore and hash verification. Run all gates, protocol/accounting/recovery tests, and an end-to-end fixture from external strategy registration to paper replay before declaring v1 complete.

## V2 goals

First priority: strategy/build sandboxing and controlled network/filesystem access, including isolation of Cargo build scripts. Add authenticated roles; hidden final-test evaluation and walk-forward methods; historical market universes/delistings; realistic spread/depth, partial fills, latency and limit-order models; broader data providers and point-in-time model tooling; optional distributed workers and Kubernetes packaging. Automated agent idea generation and coordinated improvement belong in the separate orchestrator project using TC's API.

Real orders require a later dedicated execution review, account reconciliation, exchange-state recovery, permissions, operational kill switches, and a deployment decision. They are not unlocked by merely meeting the historical CAGR target.
