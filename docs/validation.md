# Verification and supported limits

Tests establish platform behavior, not a profitable trading strategy. Synthetic fixtures, public live-feed checks and historical data audits serve different purposes. Use [development](development.md#checks) for reproducible commands and [operations](operations.md) for current controls.

## Automated coverage

The portable Rust suite covers accounting/costs, next-bar decisions, risk breaches, causal indicators/resampling, bounded IPC, immutable data checksums, persistence/cancellation races, shared validation budgets, objective compatibility, Sharpe/benchmark formulae and undefined cases, owner disclosure, reserved-window filtering, inference scope limits and atomic paper recovery.

Agent-tool regressions additionally cover source/path limits, credential-scoped idempotency and revocation, fixture-only preflight inference, development-only candle/trace boundaries, pruning unrelated data partitions, trace truncation and invalid decisions, and unchanged reports with tracing enabled.

`./scripts/check` runs formatting, native/WASM Clippy with warnings denied, portable tests and release browser compilation. `cargo deny check` separately checks advisories, licenses, sources and dependency versions. Its narrow transitive `paste` exception is documented in `deny.toml`.

## End-to-end suites

| Suite | Behavior checked |
| --- | --- |
| `scripts/smoke.py` | CLI/API/worker, both objectives, cached runs, references/trades, external SDK execution, backup/restore and crash recovery |
| `scripts/smoke.py --live` | Also public paper quotes/fills, restart and stop; requires exchange availability |
| `scripts/evaluation-smoke.py` | Development walk-forward, explicit one-use final controls with synthetic/metadata fixtures, unchanged validation budget |
| `scripts/kraken-smoke.py` | Separate-venue import, provenance/coverage, download bounds and integrity |
| `scripts/check-sandbox` | Actual isolation/resource limits, malicious offline build scripts and scoped model sockets |
| `scripts/security-smoke.py` | Owner sessions/rotation, scoped agent API, revocation and optional separate-account/network isolation |
| `scripts/agent-smoke.py` | Pinned kit, real source builds/preflight, immutable lineage, candle pagination, decision traces, useful compiler/runtime failures and candidate cancellation/revocation/restart |
| `scripts/browser-smoke.py` | Desktop/mobile controls, streaming, selection, themes, paper lifecycle and shutdown |
| `scripts/browser-backlog.py` | Thousands of versions/runs, keyboard search, pagination and stable selection |
| `scripts/browser-research.py` | Translucent references, dense markers, diagnostics, explicit validation reveal and continued agent denial |
| `scripts/shutdown-smoke.py` | Terminal exit controls while SSE is connected |

Smoke suites use temporary synthetic workspaces and independent ledgers. Test-only eligibility/provenance fixtures exercise control flow; they do not qualify real strategies. Implementation QA does not run or reveal real validation/final observations. Published audits record status at their timestamp, not the current owner's query usage.

## Practical limits

- Historical fills use next-observed-bar opens with pinned adverse execution assumptions. Paper fills use polled public quotes. Partial fills, queues, latency and intraminute stops are not reconstructed.
- Public paper polling is every ten seconds; UI snapshots stream once per second. Exchange WebSocket ingestion is pending.
- Fees, quantity precision and minimum notional are assumptions frozen per run; historical rule changes are not reconstructed. Gaps and modeled exit costs can exceed a drawdown threshold.
- Mandatory Linux namespaces/cgroups confine external strategies/builds. Separate-account agent tooling has only the development API. The owner, trusted service/compiler/SDK/cache and OS runtime remain trusted. Programs run directly as the owner are outside that boundary.
- Synthetic preflight checks protocol/replay behavior on two fixtures. It cannot prove all causal behavior, statistical soundness or future profitability.
- Successful model calls are recorded with bounded scope budgets. Historical model knowledge is accepted; a configuration hash does not pin external model weights or establish training cutoffs. Strategy-generation orchestration and paid-provider accounting remain external.
- Retained executable bytes support exact artifact replay. Offline locked builds do not claim bit-for-bit compiler reproducibility.
- Daily Sharpe and information ratio use conventional annualization. Short samples, serial correlation, gaps and repeated selection limit inference; two returns establish computability, not confidence. Validation feedback participates in selection; only the separately sealed final window is reserved as untouched.
- Browser indexes live in memory while rendering is paginated. Larger workspaces than the tested thousands need server-side search/pagination.

## Historical evidence

The original [Bitvavo data audit](data-audit.json) records 9,489,303 candles and documented multi-hour gaps. The strict 60-minute coverage gate rejected it; the owner later explicitly accepted gaps for research in a distinct immutable [V2 snapshot](v2-data-audit.json). The separate [Kraken audit](v2-kraken-audit.json) records the independently accepted comparison dataset. Neither source was silently filled or shortened to improve returns.

A native 20/100 moving-average throughput fixture processed the full Bitvavo snapshot in 10.8 seconds wall time, including verified loading, with 1.69 GiB peak child RSS on a Ryzen 7 5800X and warm caches. Compilation/import were excluded and risk halting disabled to exercise the full window. It emitted no financial feedback. [Exact measurement](performance.json). External IPC/model strategies have different costs.

The original three-minute [paper soak](paper-soak.json) crossed two candle boundaries and recorded two virtual fills without errors. The [forward-book probe](v2-forward-books-audit.json) demonstrates bounded collection, not calibrated execution costs. The [engine decision](decisions/0001-engine.md) records the NautilusTrader/Barter reference investigation; it does not establish numerical equivalence to TC.
