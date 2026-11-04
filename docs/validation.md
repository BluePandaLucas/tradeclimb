# TradeClimb v1 validation

This document records the V1 baseline. Current V2 strategy/build confinement and its separate tests are described in [operations](operations.md#strategy-isolation) and [V2 progress](v2.md).

The first release is a local research and paper-trading foundation. Validation establishes platform behavior, not a profitable trading strategy. Synthetic fixtures, public live-feed checks, and historical data checks serve different purposes.

## Automated checks

`./scripts/check` passes formatting, native Clippy with warnings denied, 31 native tests, documentation checks, WASM Clippy with warnings denied, and the release browser compilation. Formatting and Clippy were also negative-tested on the minimal workspace before domain implementation, using intentional violations that were then removed.

The native tests cover exact round-trip fee accounting, paper orders whose target lies inside the bid/ask spread, next-bar signal timing, risk breaches without clamping losses, expired intents, unchanged past decisions when future inputs change, sparse/finalized resampling, indicator initialization, bounded protocol frames, deterministic tuning proposals, chart sample bounds/extrema, Parquet round trips and tampering, exclusive-end imports and legacy-cache boundary repair, concurrent validation budgets, persistent evaluation caching, cancellation races, validation redaction through HTTP routes, Host/Origin protections, model-cache scope separation, atomic paper recovery, consistent backups, and single-service ownership.

Paper lifecycle regressions also cover duplicate starts, stopping before worker startup, cancellation during replay and blocked external strategy IPC, immediate wake-up from idle polling, rejection of stale checkpoints after stop, and browser event-stream termination during service shutdown.

`cargo deny check` passes advisory, license, dependency-source, and version checks. The narrow exception for the unmaintained `paste` procedural macro is recorded with a reason in `deny.toml`; it is a transitive Leptos dependency, not a blanket advisory suppression.

## End to end checks

`scripts/smoke.py --live` runs the actual release CLI, service, and child worker processes in a temporary data directory. It verifies synthetic development runs and bounded curves, both cost-matched buy-and-hold references, scalar-only validation, cached repeat evaluations, an external Rust strategy executable, exact trade pagination, cancellation, restoration of the validation ledger, and an external run after the original executable location is removed.

It also kills the service during a blocked external strategy and checks that restart interrupts the run and cleans up its recorded Linux worker group. Its live mode observes actual public Bitvavo bid/ask data, records virtual fills, restarts the service, verifies the paper sequence advances after recovery, and stops the session. No exchange account key or real order is involved. These checks use independent temporary ledgers and do not consume the main research directory's validation budget.

`scripts/browser-smoke.py` passes in headless Chromium at desktop and 390-pixel mobile widths with no page errors. It exercises WASM startup, SSE updates, strategy selections that remain consistent when versions arrive, submission of the visibly selected strategy, chart window changes, benchmark toggles, history/paper navigation, and absence of page-wide mobile overflow. It also verifies system theme changes, saved light/dark overrides, reset to System, visible paper startup/activity, disabled stopped controls, unchanged checkpoints after stop, starting a fresh portfolio, and Ctrl+C with the browser stream connected. Browser QA is optional development tooling, not a production dependency.

`scripts/shutdown-smoke.py` runs the actual start script in a Linux pseudo-terminal with an open SSE connection. Ctrl+C, `q`, and `exit` each produced a successful exit in about one second.

NautilusTrader 0.67.0's native EMA example was run successfully outside the repository and Barter's interfaces were inspected. See the [engine decision](decisions/0001-engine.md). This reference exercise does not establish numerical equivalence between the frameworks and TC's deliberately narrower simulator.

## Supported limits

- Historical fills use next-observed-bar opens and explicit adverse execution assumptions. Paper fills use polled public quotes. Neither models order queues, intraminute stops, or partial fills.
- Public market polling is every ten seconds; UI snapshots stream once per second. Exchange WebSocket ingestion is not implemented yet.
- Current market precision/minimum assumptions and fee settings are pinned per run. Their exact historical evolution is not reconstructed.
- Process groups, leases, and timeouts provide Linux crash handling. CPU/memory/filesystem/network sandboxing remains the first v2 priority. Validation secrecy is cooperative under trusted native execution.
- Model inference supports a fake provider and optional Ollama capability mapping, with recorded responses and finite call/output limits. Historical model knowledge is accepted. Paid-provider billing and automatic code-generation token accounting are not implemented.
- Numeric tuning supports bounded integer grids and seeded random proposals. AI idea generation and code orchestration remain external.
- The overview shows recent runs; full saved reports and older known job IDs remain available locally. Large verbose result series live in compressed report artifacts; exact paper fills live in the SQLite event journal.
- Arbitrary source rebuilds can depend on compiler flags and external dependency sources beyond a source/lockfile snapshot. Exact executable bytes are retained for replay; a general hermetic build system is not part of v1.

A separate three-minute continuous paper soak crossed two active candle boundaries and recorded two virtual fills without errors, then stopped the session and server. Its [record](paper-soak.json) is retained, and the stopped paper session is visible in the prepared demo workspace. The real rejected historical snapshot was also checked in the browser: its coverage-review notice is visible.

Use [operations](operations.md) for startup, data acceptance, costs, owner inspection, and recovery; use [API and SDK](api.md) for agent integration. Real orders remain unavailable.


## Initial data audit and measured throughput

Downloaded 9,489,303 public Bitvavo minute candles across the five EUR markets, from 31 October 2022 through the end of 31 October 2026 UTC. The extra initial day is warm-up. The [audit record](data-audit.json) records the snapshot identity, per-market coverage, and large gaps.

The strict 60-minute unknown-gap gate rejects this snapshot. BTC's largest gap is 23 July 2023 from 01:00 through 07:00 UTC; a fresh direct API request over that interval again returned no candles. Several other multi-hour gaps exist. Their cause has not been established, and no synthetic or cross-venue prices were inserted. Normal research campaigns remain blocked for this snapshot until an explicit data-policy decision. The original main research validation budget remains unused. The isolated `var/demo` workspace is available for the complete UI workflow without accepting those gaps.

The full snapshot was still usable for a throughput exercise that emits no financial feedback. A single native 20/100 moving-average run processed it in **10.8 seconds wall time**, including verified Parquet loading; simulation itself took 8.3 seconds. Peak child RSS was **1.69 GiB** on an AMD Ryzen 7 5800X with warm local filesystem caches. Compilation/import were excluded and the drawdown halt was disabled to exercise the entire window. This met the proposed 60-second and 2-GiB targets for that fixture; external IPC strategies and model calls have different costs. The [machine-readable measurement](performance.json) preserves exact timings and scope.
