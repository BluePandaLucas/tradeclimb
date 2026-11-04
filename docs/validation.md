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

The original [Bitvavo data audit](data-audit.json) records 9,489,303 candles and documented multi-hour gaps. The strict 60-minute coverage gate rejected it; the owner later explicitly accepted gaps for research in a distinct immutable [accepted snapshot](v2-data-audit.json). The separate [Kraken audit](v2-kraken-audit.json) records the independently accepted comparison dataset. Neither source was silently filled or shortened to improve returns.

A native 20/100 moving-average throughput fixture processed the full Bitvavo snapshot in 10.8 seconds wall time, including verified loading, with 1.69 GiB peak child RSS on a Ryzen 7 5800X and warm caches. Compilation/import were excluded and risk halting disabled to exercise the full window. It emitted no financial feedback. [Exact measurement](performance.json). External IPC/model strategies have different costs.

The original three-minute [paper soak](paper-soak.json) crossed two candle boundaries and recorded two virtual fills without errors. The [forward-book probe](v2-forward-books-audit.json) demonstrates bounded collection, not calibrated execution costs. The [engine decision](decisions/0001-engine.md) records the NautilusTrader/Barter reference investigation; it does not establish numerical equivalence to TC.

## Fixed backtest performance comparison (2026-11-04)

The frozen demo in `crates/tc-engine/examples/performance.rs` runs a 20/100 moving-average crossover on five markets, over 100,000 synthetic minutes (500,000 candles), three full backtests per process. It includes 100 minutes of indicator warmup and disables drawdown halting so both versions execute the same history. The demo and SDK strategy remain unchanged from baseline commit `b490b17`.

The engine now scans adjacent timestamp groups, validates duplicates without allocating trees, reuses frame/portfolio and pending-order storage, and computes decimal execution costs once per backtest. Decimal accounting, validation, sell-before-buy ordering, observer timing and public simulator configuration updates remain covered by regressions.

The [raw paired measurements](backtest-performance.json) include every sample and executable/workload SHA-256 hashes. One unmeasured warmup process per executable precedes seven pairs of fresh processes, alternating order. Both builds use the same locked Rust 1.99.1 release profile on Linux x86_64 / Intel Core i5-6500T. Compilation and dataset downloads are excluded. Simulation time covers the complete engine call including report metrics; total wall time additionally includes process startup, fixture generation, and full report serialization/hashing. Average process CPU utilization is `(user + system CPU seconds) / wall seconds`, where 100% is one core; memory is GNU time's peak process RSS. This is a native in-process benchmark, without external strategy IPC, inference, storage loading, or UI work.

Every measured run must produce the same SHA-256 of the complete serialized report (config, every equity point and fill, and metrics). The comparison script fails unless median simulation **and** total process wall times fall by at least 25%, and median CPU utilization and peak RSS each stay within 110% of baseline. These measurements establish performance for this fixed synthetic workload, not financial performance or a speed guarantee for every strategy.

Seven-pair medians:

| Measure | Baseline | Optimized | Relative change |
| --- | ---: | ---: | ---: |
| Simulation time | 1.9084 s | 1.2796 s | −32.95% |
| Total process wall time | 2.2351 s | 1.6043 s | −28.22% |
| Process CPU utilization | 99.483% | 99.222% | −0.26% |
| Peak process RSS | 92.508 MiB | 92.594 MiB | +0.09% |
| Process CPU time | 2.23 s | 1.59 s | −28.70% |

Reproduce without changing the active checkout:

```bash
baseline_dir=$(mktemp -d)
git archive b490b17 | tar -x -C "$baseline_dir"
CARGO_TARGET_DIR="$PWD/target/perf-baseline" cargo build --locked --release \
  --manifest-path "$baseline_dir/Cargo.toml" -p tc-engine --example performance
cargo build --locked --release -p tc-engine --example performance
python3 scripts/compare-backtests.py \
  target/perf-baseline/release/examples/performance \
  target/release/examples/performance --output target/backtest-comparison.json
```

Validation: native workspace tests (78 passed, nine explicitly ignored sandbox/helper tests), release engine tests, engine Clippy across all targets with warnings denied, formatting, and Python syntax checks passed.
