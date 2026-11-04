# TradeClimb plan self review

Historical V1 planning record. Current behavior and policy are documented in [status](status.md), [operations](operations.md) and [API](api.md); later decisions supersede proposals here.

This review challenges the proposed v1 against the user's objectives. Corrections described as adopted are already incorporated in the implementation plan. Remaining limitations are explicit review decisions, not work silently deferred past v1 completion.

## Most consequential findings

### Scalar validation still participates in optimization

Severity: high. Returning only period 2 CAGR reduces detail but still lets a search select on that period. Ten queries is a resource/feedback limit, not a statistical guarantee. Adopted: persistent campaign-wide budgets, immutable dates, cached duplicate evaluations, no combined charts/logs, and an explicit revealed state for owner inspection. Residual: validation cannot honestly be called an untouched final test. Recommend a one-time hidden test and walk-forward testing in v2; new paper observations are essential in v1.

### Strict secrecy conflicts with trusted native execution

Severity: high. Strategies, Cargo build scripts, and agents on the same PC can read local files or use the network. IPC batching also makes future bytes accessible to code bypassing the SDK. Adopted: no claim of adversarial secrecy or sandboxing, causal SDK, prefix-invariance tests, no code-upload API, and process boundaries for crash recovery. Residual: generated code must be reviewed/trusted in v1. If strong secrecy is a requirement rather than a cooperative rule, move isolation into v1 before implementation.

### Validation CAGR cannot demonstrate the drawdown condition

Severity: high. A scalar return does not reveal validation drawdown or monthly behavior. Exposing a pass/fail risk bit would violate the requested scalar-only rule. Adopted: risk handling runs internally, but the automated response remains CAGR-only; the owner can deliberately reveal a full report for paper-promotion review. Residual: doing so contaminates validation. The UI must not imply all risk constraints passed in period 2 merely because its CAGR is positive.

### Fifteen percent is a risk criterion and halt trigger

Severity: high. Minute observations miss intraminute troughs, gaps can exceed a threshold, and a liquidation incurs additional costs. Adopted: full-resolution marks, conservative valuation, explicit breach recording, next-executable liquidation, and no artificial cap on losses. Residual: the requested maximum cannot be guaranteed, including during paper trading. Confirm whether drawdown is from peak equity or initial capital; the proposal uses peak equity.

### Historical model inference can import future knowledge

Severity: high for interpretation; accepted by the user for v1 on 3 November 2026. Cached responses and fixed seeds improve repeatability but do not establish historical validity. Adopted: provenance tracking, explicit opt-in external inference, fake tests, and an informational record of unknown training cutoffs. Such runs remain eligible for ranking. Residual: v1 cannot prove a general language model lacks knowledge of historical events. The optional abstraction should not block the non-model core workflow.

## Architecture challenges

A custom engine is a correctness commitment, not automatically the simplest choice because it is small. Nautilus now supports native Rust; a stale assumption here would lead to needless reinvention. Adopted: a bounded adoption checkpoint before domain implementation, a TC-owned facade, and hand-calculated accounting fixtures regardless of engine. Outcome: the native reference exercise is complete and the narrow TC engine was selected for v1; see the engine decision. Neither Rust performance nor framework adoption proves financial correctness.

A fully Rust web UI adds WASM/build complexity. Adopted: Leptos with a small pinned chart bridge, separate target checks, and no server rendering/hydration requirement. A conventional TypeScript frontend is a reasonable lower-friction alternative if the user later prioritizes frontend delivery over all-Rust application code. No switch is assumed.

Worker subprocesses alone do not protect the API if SQLite writes or UI broadcasts block. Adopted: bounded queues, batched output, separate CPU workers, supervised children, and snapshot-based SSE recovery. Two simultaneous backtests plus paper sessions must be tested under load. Arbitrary strategy CPU/memory behavior remains a trusted-code limitation until isolation.

Paper recovery is easy to under-scope. Persisting portfolio balances without strategy state can change decisions after restart. Adopted: ordered market/intent/fill events, strategy checkpoints or replay, no historical fills during downtime, and deterministic restoration. Residual: external model failure and unavailable recorded data must halt rather than invent a recovered state.

## Data challenges

The API probes establish sample availability, not completeness. Sparse candles, ambiguous candle semantics, and historical market-rule changes can dominate differences between backtests and paper outcomes. Adopted: audit gate, raw snapshots, flagged gaps, current-versus-historical metadata distinction, and no silent cross-venue patching. Residual: full coverage, exact historical fees/rules, import duration, and disk usage remain unmeasured.

Five current assets are convenient but selected after survival. Four years include only the regimes present in that window. Adopted: frozen experiment universe and dates, explicit selection-bias warning, and asset-specific/basket benchmarks. Residual: neither many correlated assets nor many tuned candidates create independent evidence. No dataset-extension decision should depend on which extension makes returns look better.

Free external-venue data may be clean and still mismatch Bitvavo execution. Paid data can also lack the required venue. Adopted: direct Bitvavo first, separately labeled fallback datasets, and coverage/cost checks before purchase. Residual: the paid provider list is a procurement shortlist, not a verified four-year Bitvavo quote.

## Objective challenges

Positive mean monthly return can coexist with losing compounded capital. Adopted: geometric return, distribution, exposure, turnover, and drawdown alongside the arithmetic mean; optimize net CAGR only among risk-eligible development candidates. Residual: the user has not required a minimum fraction of profitable months or minimum trading activity. Show sparse-evidence warnings rather than inventing an economic success threshold.

An ever-rising CAGR goal can reward increasingly brittle winners. Adopted: fixed objective revisions, bounded development tuning, recorded failed trials, and validation budgets spanning revisions. Residual: an algorithm hitting a search budget has not found a “reasonable maximum” in a mathematical sense. Report “budget exhausted” or “stalled,” not “optimal.”

Model costs can make a small simulated capital base misleading. Adopted: distinguish execution-net return from experiment operating costs and provide an optional all-in calculation. Residual: code-generation token spend is outside TC's direct control and needs external-agent reporting.

## Scope and repository controls

V1 is already substantial: it includes reproducible data, SDK, engine, history, tuning, web UI, and durable paper operation. Adopted: acceptance-gated milestones, no distributed infrastructure, no real orders, market-only fills, and a small indicator set. A partial dashboard without durable paper operation would not fulfill the requested complete workflow.

Clippy and formatting alone do not catch incorrect chronology, hidden validation leaks, fee rounding, or recovery errors. Adopted: repository gates before domain code and targeted accounting, causality, protocol, redaction, crash, and restore tests. No coverage percentage target is proposed because it would not establish those properties.

At the initial planning pass, Rust was absent from PATH. Following user approval, Rust/Cargo 1.99.1, rustfmt, Clippy, and the WASM target were installed and pinned. Native execution and WASM compilation passed smoke checks. Workspace lint/test/CI gates were subsequently established and negative-tested before domain implementation; current checks and evidence are in the validation guide.

## Accepted decisions

1. Accept the proposed five EUR markets, per-session EUR 10,000 capital, and three-year development/one-year validation split, subject to coverage.
2. Accept peak-equity drawdown and positive mean monthly simple return as the initial rule definitions, with the additional diagnostics described above.
3. Accept cooperative validation secrecy in v1, ten campaign-wide scalar validation queries, and explicitly revealed owner inspection.
4. Accept Leptos plus a small chart bridge and a provisional narrow simulator subject to the engine checkpoint. Historical model knowledge is accepted in v1; model provenance remains recorded, with no ranking exclusion solely for unknown training cutoffs.
5. Confirm that LAN access without authentication was intended; the phrase “LAN unauthorized access” was interpreted that way.

The user accepted the recommendations on 3 November 2026, accepting cooperative validation mitigation, drawdown eligibility, and trusted native code, while permitting historical model knowledge. Strategy/build sandboxing is explicitly the first v2 priority. The user subsequently authorized v1 implementation. Repository controls were established and negative-tested before domain code. See the operations and API guides for current behavior.
