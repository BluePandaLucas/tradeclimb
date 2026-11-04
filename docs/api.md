# Local API and strategy integration

Run the server as the filesystem owner on loopback. Every API requires `Authorization: Bearer KEY` or an owner browser session. `POST /login` accepts `{key: OWNER_KEY}` and creates a 12-hour HttpOnly SameSite=Strict cookie; `GET /api/session` reports identity/expiry and `POST /api/logout` revokes the current session. Keys live in private local files; CLI `tc auth status`, `issue-agent`, `revoke`, and `rotate-owner` manage access. There is one owner, with no public account registration. An HTTPS reverse proxy is required for remote access; set `TC_ALLOWED_HOSTS` for its hostname. Host/Origin checks and restrictive CORS remain enforced.

The owner can use all ordinary endpoints. Agents can read development overviews/events, development job details/trades, submit development jobs, tune, and cancel their own development jobs/controllers. They cannot access validation, paper sessions/approvals, campaign creation, archives, or owner inference scopes. Agent overviews and SSE omit validation/paper data and include development jobs/rankings only. Idempotency keys are namespaced per credential; revocation cancels owned work and closes existing streams. The dedicated agent Unix endpoint accepts agent credentials only and rejects owner sessions/keys and login. The network-isolated launcher exposes its path as `TC_API_SOCKET`, alongside `TC_API_TOKEN_FILE`; there is no host TCP or Internet route in that sandbox.

Agents can submit bounded Rust source through the pinned candidate workflow below. Arbitrary local projects and custom dependencies remain owner CLI operations; compile untrusted projects with `tc build`. Source snapshots reject symlinks and concurrent escapes before isolated builds. See the [agent guide](agents.md) for bootstrap, iteration and recovery.

The overview is also streamed as an SSE `snapshot` event every second at `/api/events`. Reconnecting receives a fresh snapshot. Curves are loaded for the selected run, not broadcast for every historic run. Overview snapshots show the latest 50 jobs; the browser fetches `/api/jobs/index` once to search and paginate older summaries, then merges the live feed into that index. SQLite retains the history. The index excludes execution settings, curves, fills, and benchmarks.

The `leaderboards` array compares completed runs across the full retained history, independently of the recent-job limit. Each group contains `id`, `campaign`, `period`, `objective`, the exact `config`, and `entries`. A strategy version contributes its latest completed run by creation time (newest database row breaks timestamp ties). Entries with missing or non-finite objective scores are excluded without falling back to an older run or a different metric. Entries sort by score descending; exact ties share a competition rank (1, 1, 3), ordered by strategy ID. Each entry exposes `rank`, `strategy`, `job`, `created_at`, `score`, the named metric (`sharpe` or legacy `net_cagr`), and `eligibility`. Archived versions remain ranked; eligibility does not filter rank. Validation reports eligibility as `Sharpe only` or legacy `CAGR only`. Different campaigns, objectives, periods, and execution settings never share a ranking.

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/api/agent/capabilities` | Machine-readable platform, tools, limits and policy |
| GET | `/api/agent/kit` | Pinned public SDK, Rust template, client and author guide |
| POST | `/api/agent/candidates` | Submit bounded source for offline build/preflight/immutable registration |
| GET | `/api/agent/candidates/{id}` | Candidate state and bounded compiler/preflight diagnostics; submitting credential or owner |
| POST | `/api/agent/candidates/{id}/cancel` | Cancel candidate; safe to repeat |
| GET | `/api/agent/candles` | Bounded, checksum-verified development candle slice |
| GET | `/api/agent/jobs/{id}/trace` | Requested development frame/decision/fill trace |
| GET | `/api/agent/jobs/{id}/diagnostics` | Development metrics, runtime failure and trace metadata |
| GET | `/api/overview` | Strategies, campaigns, recent jobs, leaderboards, paper sessions, datasets, tuning status, validation ledger |
| GET | `/api/events` | SSE overview snapshots; validation includes no curves or granular progress |
| POST | `/api/jobs` | Submit a registered strategy to a fixed campaign |
| GET | `/api/jobs/index` | All lightweight run summaries, newest first; no chart payloads |
| GET | `/api/jobs/{id}` | Run status and permitted results |
| GET | `/api/jobs/{id}?days=7` | Permitted chart detail, resampled from full equity; also 30 or 90 days |
| GET | `/api/jobs/{id}/trades?offset=0&limit=100` | Exact permitted fills; maximum page size 1,000 |
| POST | `/api/jobs/{id}/reveal` | Owner explicitly opens completed validation metrics, curves and trades; records revelation without another evaluation |
| POST | `/api/jobs/{id}/cancel` | Cancel a queued/running job and terminate its process group; terminal calls are safe to repeat |
| POST | `/api/campaigns` | Create/retrieve a campaign for an accepted immutable dataset |
| POST | `/api/strategies/{id}/archive` | Toggle archive status; preserve old versions and runs |
| POST | `/api/tune` | Bounded development parameter search |
| POST | `/api/tuning/{id}/cancel` | Stop a tuning controller and its active trial |
| POST | `/api/paper` | Owner approves a qualifying development job/version and starts an EUR 10,000 virtual portfolio |
| POST | `/api/paper/{id}/stop` | Request cancellation without placing exchange orders; idempotent for terminal sessions |
| POST | `/api/infer` | Invoke a configured logical model capability with a per-run cache/budget |

Paper start requires `{strategy, job, approve: true}`. The job must be the version's latest qualifying completed Sharpe development evaluation on accepted non-synthetic Bitvavo data, with risk eligibility and maximum drawdown at most 15%. Growth remains aspirational. `paper_candidates` ranks qualifying versions by Sharpe; `paper_approvals` records owner decisions and is owner-only. Session and approval writes are atomic. Old unapproved sessions do not resume automatically.

Paper sessions report `starting`, `running`, `stopping`, `stopped`, or `halted`, plus `activity` and `updated_at`. A stop response may be `stopping`; observe overview/SSE until `stopped` to confirm worker exit. A session stopped before its worker starts goes directly to `stopped`. Starting while this strategy version is still stopping fails; starting after it stops creates a new portfolio.

A campaign creation request defaults to `objective: "sharpe_daily_v1"`. Set `objective: "net_cagr"` explicitly for a legacy-style campaign. Unknown objectives fail. `target_cagr` remains a recorded growth aspiration (a fraction such as `0.20` for 20%), not an additional eligibility gate. Campaigns spanning the reserved `[2026-08-01, 2026-11-01)` final test expose its metadata in `final_test: {start, end, range_covered}` and stop ordinary research before August. `range_covered` refers to manifest date boundaries, not gap-free coverage. Full-range campaigns retain three-year development and use nine-month validation. A reservation is not permission to run/reveal the final test. Owner CLI preparation and explicit one-use execution are documented in operations; no HTTP final-test route exists. Old records lacking this field deserialize with `final_test: null`. All ordinary job submissions, worker execution, cached report/detail routes, and leaderboards enforce the global reservation, including legacy campaigns. Post-holdout runs cannot receive reserved candles as warmup. Existing overlapping job summaries have results/progress suppressed; old campaigns are retained and must be superseded by a current revision. Objectives are immutable campaign revisions; changing one preserves the global validation ledger. Sharpe feedback never reuses a CAGR-only result. Repeated evaluations within the same Sharpe campaign reuse their cached result. Existing persisted campaigns/jobs/worker requests without an objective remain `net_cagr`; existing reports without Sharpe deserialize with null Sharpe and zero sample days. No historical report is rescored automatically.

Development report metrics include `sharpe` and `sharpe_days`. `sharpe_daily_v1` takes simple returns between observed UTC midnight equity marks, after fees and adverse execution costs, including idle days. It excludes partial days at either edge, uses a fixed zero cash benchmark, sample standard deviation (`n-1`), and multiplies daily mean/standard deviation by `sqrt(365.25)`. The last mark at a duplicate timestamp wins, including terminal liquidation costs. Missing daily boundaries, nonpositive equity, fewer than two daily returns, or effectively zero variance produce null rather than infinity. `sharpe_days` counts usable daily returns (zero for an invalid/missing-boundary series). Full-resolution drawdown and existing eligibility rules remain independent. Conventional annualization assumes no serial correlation; report warnings identify that limitation and repeated-selection bias. This score is not autocorrelation-adjusted or Deflated Sharpe, and two days establish computability, not statistical confidence.

Submit a run using IDs returned by the CLI or overview:

```json
{
  "strategy": "REGISTERED_STRATEGY_ID",
  "campaign": "CAMPAIGN_ID",
  "period": "development",
  "idempotency_key": "agent-iteration-0001"
}
```

`period` may be `validation`. Its successful result contains only `sharpe` for a Sharpe campaign or `net_cagr` for a legacy campaign, possibly null when undefined. Ordinary validation summaries remain scalar. Owner `POST /api/jobs/{id}/reveal` records the reveal and returns full detail; subsequent owner detail/window/trade requests are allowed for that job. Agents remain development-only even after a reveal. Owner detail carries `owner_detail: true`; the history index, snapshots and stored ranking summaries keep scalar validation results. Its cost/risk configuration is frozen. A development submission may include `slippage_bps` for cost stress tests. Reusing an idempotency key with different inputs fails; repeated identical validation evaluations return the existing job without consuming another query.

The SQLite validation ledger is global to the data directory, including all campaigns, strategy families, parameter revisions, and objective changes. It starts with ten queries. Failed validation attempts still consume their reserved query. Budget extension remains an audited local CLI action. Owner inspection through the CLI or the authenticated reveal endpoint is audited, sets the global revealed flag, and does not spend another validation query. The final-test reservation is enforced before any reveal.

## Agent source and debugging tools

`POST /api/agent/candidates` requires `platform`, `idempotency_key`, `name`, `markets` and `files` (a map of relative Rust source paths to UTF-8 text). Optional fields are `description`, `parent`, `parameters` and `model_fixtures` (logical capability to synthetic response text). Unknown fields fail. The platform comes from kit/capabilities. Only `src/**/*.rs` is accepted; source is limited to 64 files, 64 KiB each and 256 KiB total. The route accepts at most 2 MiB JSON; other request bodies retain the 64-KiB limit. Custom manifests/build scripts/dependencies are not accepted.

An accepted request returns immediately with a candidate ID. Poll it through `queued` → `building` → `preflight` to `completed`, `failed`, `cancelled` or `interrupted`. Completion supplies the immutable `strategy` ID and synthetic preflight evidence; failures supply `diagnostic.code`, `message` and bounded compiler `log`. The fixture runs regular/gapped causal bars twice and checks decision replay. It uses declared model fixtures only and is not a performance test. Idempotency is credential-scoped and rejects changed inputs under the same key. Revocation/cancellation is rechecked atomically before registration. Active work is interrupted on restart; it is not silently resubmitted.

`GET /api/agent/candles?campaign=ID&market=BTC-EUR&start=UTC_MS&end=UTC_MS&limit=1000` accepts minute-aligned `[start,end)` entirely inside development, at most seven days and 2,000 returned rows. Responses contain dataset/venue, `candles`, `next_start` (or null), observed/missing minute counts, synthetic status and notes. Continue with `start=next_start` and the same end. Decimal prices/volume are strings. Source partitions are pruned by market/month before loading; matching partitions are checksum-verified. The global reserved window is rejected even for legacy campaigns.

Development submissions may add `"trace":{"start":UTC_MS,"end":UTC_MS,"max_frames":256}`. The range must lie inside that job; maximum 512 frames and 2 MiB. The stored trace contains current completed candles, portfolio, intents, fills since the previous frame, pending intents, halt/stale state and errors. `time` is bar close and `bar_start` is bar open; fills precede the current decision. `truncated` signals omitted later frames. Request a narrower window to inspect later behavior. Traces are optional, do not affect scoring and cannot be requested for validation. Both trace and diagnostics endpoints enforce development independently of owner validation revelation.

Candidate diagnostics include `compile_failed`, `build_timeout`, `build_failed`, `preflight_failed` and `worker_interrupted`. Development runtime diagnostics distinguish `strategy_timeout`, `strategy_exit`, `protocol_error`, `invalid_intent` and fallback `evaluation_error`, with bounded stderr where available. Normal HTTP errors retain `{ "error": "message" }` and a status code. Use capabilities for current limits and the [client guide](agents.md#client-and-recovery) for retries, cancellation and polling.

## Numeric tuning

For the built-in moving average, submit `{"strategy":"ID","campaign":"ID","trials":20,"seed":42}` to `/api/tune`. Defaults explore fast windows 5–119 and slow windows above fast through 599. Tuning evaluates development only, uses the campaign objective among eligible development results, stops after 20 trials without a feasible improvement of 0.01 Sharpe units (or 0.25 percentage points legacy CAGR), and has a 30-minute deadline and maximum 100 trials.

For a grid or seeded random search over an external strategy's integer parameters:

```json
{
  "strategy": "ID",
  "campaign": "ID",
  "trials": 12,
  "seed": 42,
  "method": "grid",
  "space": [
    {"name": "lookback", "min": 20, "max": 60, "step": 10},
    {"name": "threshold_bps", "min": 10, "max": 30, "step": 10}
  ]
}
```

The immutable strategy specification is provided to the executable in `TC_STRATEGY_SPEC`; parse its `parameters` object before entering `tc_sdk::serve`. `tc_sdk::propose` provides the same bounded grid/random candidate generation for external agents. The UI exposes a basic built-in tuning action; custom spaces use the API. Tuning state and child job IDs appear in the overview. Tuning records retain `strategy`, `campaign`, `objective`, `trials`, and `jobs` across progress and terminal states; child job membership is published on submission so the UI can group trials before they finish. A service restart leaves unfinished historical runs interrupted; tuning is not automatically resumed.

## SDK contract

The [external example](../examples/strategy/src/main.rs) demonstrates a standalone process using `Strategy::on_frame`. Each frame contains only the current completed minute's real candles, simulated time, portfolio state, and a warm-up flag. Keep trailing indicator state inside the strategy. All markets with a matching timestamp are supplied together. IPC is little-endian length-prefixed JSON, protocol 1, maximum 8 MiB per message. Stdout is exclusively for this protocol.

Weights are fractions of the portfolio, must total at most one per decision, and permit only long/cash positions. Use at most one intent per market per frame. Signals cannot fill on the bar that produced them. External strategies are timed out after 45 seconds without a response; whole historical jobs have a 30-minute limit. Single-event IPC favors clarity over maximum throughput; builtin strategies avoid IPC. External code runs through the mandatory Linux sandbox for both historical and paper execution; missing prerequisites fail closed. Compile untrusted projects with `tc build`, including their build scripts. The service, compiler, SDK, and OS runtime remain trusted. See [isolation setup and limits](operations.md#strategy-isolation).

SMA and volatility use a completed rolling window (population standard deviation). EMA seeds from the first observation and becomes available after its period. RSI currently uses simple rolling gains/losses (Cutler's definition); ATR is the SMA of true range. Window and initialization choices are explicit so later indicator variants do not silently change old results. `Resampler::advance` returns a bar only when its UTC interval is finished; `complete` indicates all base minutes were observed. Gaps remain flagged, not executable zero-volume prices.

## Models

Set `TC_MODEL_MAP='{"quick-analysis":"YOUR_LOCAL_MODEL"}'` and optionally `TC_OLLAMA_URL=http://127.0.0.1:11434` before starting the service. Strategies invoke the logical capability `quick-analysis`; the server selects the model. No model download is automatic. The `fake` capability echoes the prompt for integration tests.

Use `tc_sdk::HttpModelClient::from_env()`; the default server URL is localhost:8080, overridable with `TC_MODEL_URL`. Confined executables receive `TC_MODEL_SOCKET=/broker/model.sock`; `HttpModelClient::from_env()` automatically uses this Unix socket. Its framed request is `{capability, prompt, max_tokens}` and its framed response is `{"status":"ok","value":MODEL_RESPONSE}` or `{"status":"error","value":"MESSAGE"}`. The broker fixes the owning historical job/paper session; request fields cannot change its scope. Requests are at most 64 KiB (prompts still at most 32 KiB), response text at most 128 KiB, and client reads/writes have a shared two-second deadline per frame. Model inference retains a 30-second provider timeout; broker shutdown cancels it. Errors return a generic message and log owner diagnostics. Strategies have no direct host-network access. Legacy HTTP transport requires owner authentication; set `TC_API_TOKEN_FILE` to the private owner-key file for the SDK client. It remains available to trusted local tools using `TC_MODEL_SCOPE` and `{scope, request: {capability, prompt, max_tokens}}`; it is not a sandbox bypass. Each job or paper session allows at most 64 calls, at most 2,048 output tokens each, and 32 KiB prompts. Successful responses are recorded and repeated requests replay them. Failed/pending reservations remain consumed; no unbounded retry loop. Model calls are opt-in strategy behavior and separate scopes prevent cache reuse across development and validation. General model knowledge of historical events is accepted and recorded as an unknown training cutoff.

## Benchmark diagnostics

New development result references include `comparison` with convention `buy_hold_comparison_daily_v1`: benchmark/strategy daily Sharpe, their difference, information ratio, paired-day count, net-return difference and CAGR difference. IR is mean paired daily simple active return divided by its sample standard deviation, annualized by sqrt(365.25). Daily tracking error at or below 1e-12, fewer than two complete days, missing boundaries or invalid equity produces null rather than infinity or a fabricated score. No gaps are forward-filled. Formulae use full equity before chart sampling, with the same UTC-day and terminal-cost convention as Sharpe.

The fixed primary comparison is the five-asset buy-and-hold basket; BTC is secondary. References use the strategy's dates, capital, fees and slippage, with the reference drawdown halt disabled so it remains buy-and-hold. This is not equal drawdown risk. Sharpe remains the campaign objective; diagnostics never divide Sharpe ratios or rewrite old rankings. Chart-window changes affect curves/markers, while the comparison table remains full-run. Legacy development references can be rebuilt from the saved full report and frozen dataset on first detail access, without rerunning the strategy; original stored metrics/objectives are preserved. Revealed validation references are cached privately and never added to its public/scoped summaries.

`GET /api/jobs/index` filters agent history to development and excludes reserved-window runs; agent job counters are similarly scoped. All API responses remain authenticated and non-cacheable.

Native reference reconstruction and full-report processing share one service semaphore across the owner TCP and agent Unix transports. A permit remains owned by the blocking task even if its HTTP request is cancelled. Metadata/history requests do not acquire it; repeated report requests reuse the private cache.
