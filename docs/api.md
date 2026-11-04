# Local API and strategy integration

Run the server on the same machine as the CLI. The HTTP API has no login and is intended for a trusted LAN. Code registration is a CLI operation; the HTTP API cannot upload or compile executable code. Browser mutations require an accepted Host and matching Origin. IP literals and localhost are accepted; configure `TC_ALLOWED_HOSTS=tradeclimb.home,tc.home` for explicit LAN hostnames. No permissive CORS headers are sent.

The overview is also streamed as an SSE `snapshot` event every second at `/api/events`. Reconnecting receives a fresh snapshot. Curves are loaded for the selected run, not broadcast for every historic run. Overview snapshots show the latest 50 jobs; the CLI and job endpoint can retrieve an older known ID. SQLite retains the history.

The `leaderboards` array compares completed runs across the full retained history, independently of the recent-job limit. Each group contains `id`, `campaign`, `period`, `objective`, the exact `config`, and `entries`. A strategy version contributes its latest completed run by creation time (newest database row breaks timestamp ties). Entries with missing or non-finite objective scores are excluded without falling back to an older run or a different metric. Entries sort by score descending; exact ties share a competition rank (1, 1, 3), ordered by strategy ID. Each entry exposes `rank`, `strategy`, `job`, `created_at`, `score`, the named metric (`sharpe` or legacy `net_cagr`), and `eligibility`. Archived versions remain ranked; eligibility does not filter rank. Validation reports eligibility as `Sharpe only` or legacy `CAGR only`. Different campaigns, objectives, periods, and execution settings never share a ranking.

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/api/overview` | Strategies, campaigns, recent jobs, leaderboards, paper sessions, datasets, tuning status, validation ledger |
| GET | `/api/events` | SSE overview snapshots; validation includes no curves or granular progress |
| POST | `/api/jobs` | Submit a registered strategy to a fixed campaign |
| GET | `/api/jobs/{id}` | Run status and permitted results |
| GET | `/api/jobs/{id}?days=7` | Development chart detail, resampled from full equity; also 30 or 90 days |
| GET | `/api/jobs/{id}/trades?offset=0&limit=100` | Exact development fills; maximum page size 1,000 |
| POST | `/api/jobs/{id}/cancel` | Cancel a queued/running job and terminate its process group |
| POST | `/api/campaigns` | Create/retrieve a campaign for an accepted immutable dataset |
| POST | `/api/strategies/{id}/archive` | Toggle archive status; preserve old versions and runs |
| POST | `/api/tune` | Bounded development parameter search |
| POST | `/api/tuning/{id}/cancel` | Stop a tuning controller and its active trial |
| POST | `/api/paper` | Start an EUR 10,000 virtual portfolio, or return this strategy version's existing active session |
| POST | `/api/paper/{id}/stop` | Request cancellation without placing exchange orders; idempotent for terminal sessions |
| POST | `/api/infer` | Invoke a configured logical model capability with a per-run cache/budget |

Paper sessions report `starting`, `running`, `stopping`, `stopped`, or `halted`, plus `activity` and `updated_at`. A stop response may be `stopping`; observe overview/SSE until `stopped` to confirm worker exit. A session stopped before its worker starts goes directly to `stopped`. Starting while this strategy version is still stopping fails; starting after it stops creates a new portfolio.

A campaign creation request defaults to `objective: "sharpe_daily_v1"`. Set `objective: "net_cagr"` explicitly for a legacy-style campaign. Unknown objectives fail. `target_cagr` remains a recorded growth aspiration (a fraction such as `0.20` for 20%), not an additional eligibility gate. Campaigns spanning the reserved `[2026-08-01, 2026-11-01)` final test expose its metadata in `final_test: {start, end, range_covered}` and stop ordinary research before August. `range_covered` refers to manifest date boundaries, not gap-free coverage. Full-range campaigns retain three-year development and use nine-month validation. A reservation is not permission to run/reveal the final test; no such endpoint exists yet. Old records lacking this field deserialize with `final_test: null`. All ordinary job submissions, worker execution, cached report/detail routes, and leaderboards enforce the global reservation, including legacy campaigns. Post-holdout runs cannot receive reserved candles as warmup. Existing overlapping job summaries have results/progress suppressed; old campaigns are retained and must be superseded by a current revision. Objectives are immutable campaign revisions; changing one preserves the global validation ledger. Sharpe feedback never reuses a CAGR-only result. Repeated evaluations within the same Sharpe campaign reuse their cached result. Existing persisted campaigns/jobs/worker requests without an objective remain `net_cagr`; existing reports without Sharpe deserialize with null Sharpe and zero sample days. No historical report is rescored automatically.

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

`period` may be `validation`. Its successful result contains only `sharpe` for a Sharpe campaign or `net_cagr` for a legacy campaign, possibly null when undefined. Validation fills and chart routes are restricted. Its cost/risk configuration is frozen. A development submission may include `slippage_bps` for cost stress tests. Reusing an idempotency key with different inputs fails; repeated identical validation evaluations return the existing job without consuming another query.

The SQLite validation ledger is global to the data directory, including all campaigns, strategy families, parameter revisions, and objective changes. It starts with ten queries. Failed validation attempts still consume their reserved query. CLI budget extension and owner inspection are audited and never available as web mutations.

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

Use `tc_sdk::HttpModelClient::from_env()`; the default server URL is localhost:8080, overridable with `TC_MODEL_URL`. Confined executables receive `TC_MODEL_SOCKET=/broker/model.sock`; `HttpModelClient::from_env()` automatically uses this Unix socket. Its framed request is `{capability, prompt, max_tokens}` and its framed response is `{"status":"ok","value":MODEL_RESPONSE}` or `{"status":"error","value":"MESSAGE"}`. The broker fixes the owning historical job/paper session; request fields cannot change its scope. Requests are at most 64 KiB (prompts still at most 32 KiB), response text at most 128 KiB, and client reads/writes have a shared two-second deadline per frame. Model inference retains a 30-second provider timeout; broker shutdown cancels it. Errors return a generic message and log owner diagnostics. Strategies have no direct host-network access. Legacy HTTP transport remains available to trusted local tools using `TC_MODEL_SCOPE` and `{scope, request: {capability, prompt, max_tokens}}`; it is not a sandbox bypass. Each job or paper session allows at most 64 calls, at most 2,048 output tokens each, and 32 KiB prompts. Successful responses are recorded and repeated requests replay them. Failed/pending reservations remain consumed; no unbounded retry loop. Model calls are opt-in strategy behavior and separate scopes prevent cache reuse across development and validation. General model knowledge of historical events is accepted for v1 and recorded as an unknown training cutoff.
