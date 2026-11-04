# Local API and strategy integration

Run the server on the same machine as the CLI. The HTTP API has no login and is intended for a trusted LAN. Code registration is a CLI operation; the HTTP API cannot upload or compile executable code. Browser mutations require an accepted Host and matching Origin. IP literals and localhost are accepted; configure `TC_ALLOWED_HOSTS=tradeclimb.home,tc.home` for explicit LAN hostnames. No permissive CORS headers are sent.

The overview is also streamed as an SSE `snapshot` event every second at `/api/events`. Reconnecting receives a fresh snapshot. Curves are loaded for the selected run, not broadcast for every historic run. Overview snapshots show the latest 50 jobs; the browser fetches `/api/jobs/index` once to search and paginate older summaries, then merges the live feed into that index. SQLite retains the history. The index excludes execution settings, curves, fills, and benchmarks.

The `leaderboards` array compares completed runs across the full retained history, independently of the recent-job limit. Each group contains `id`, `campaign`, `period`, the exact `config`, and `entries`. A strategy version contributes its latest completed run by creation time (newest database row breaks timestamp ties). Entries with missing or non-finite net CAGR are excluded without falling back to an older run. Entries sort by net CAGR descending; exact ties share a competition rank (1, 1, 3), ordered by strategy ID. Each entry exposes `rank`, `strategy`, `job`, `created_at`, `net_cagr`, and `eligibility`. Archived versions remain ranked; eligibility does not filter rank. Validation uses only its permitted scalar CAGR and reports eligibility as `CAGR only`. Development and validation, different campaigns, and different execution settings never share a ranking.

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/api/overview` | Strategies, campaigns, recent jobs, leaderboards, paper sessions, datasets, tuning status, validation ledger |
| GET | `/api/events` | SSE overview snapshots; validation includes no curves or granular progress |
| POST | `/api/jobs` | Submit a registered strategy to a fixed campaign |
| GET | `/api/jobs/index` | All lightweight run summaries, newest first; no chart payloads |
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

A campaign creation request may include `target_cagr`, a fraction such as `0.20` for 20%. Objectives are immutable campaign revisions; changing one preserves validation exposure and can reuse an identical cached validation evaluation.

Submit a run using IDs returned by the CLI or overview:

```json
{
  "strategy": "REGISTERED_STRATEGY_ID",
  "campaign": "CAMPAIGN_ID",
  "period": "development",
  "idempotency_key": "agent-iteration-0001"
}
```

`period` may be `validation`. Its successful result contains only `net_cagr`, possibly null when undefined. Validation fills and chart routes are restricted. Its cost/risk configuration is frozen. A development submission may include `slippage_bps` for cost stress tests. Reusing an idempotency key with different inputs fails; repeated identical validation evaluations return the existing job without consuming another query.

The SQLite validation ledger is global to the data directory, including all campaigns, strategy families, parameter revisions, and objective changes. It starts with ten queries. Failed validation attempts still consume their reserved query. CLI budget extension and owner inspection are audited and never available as web mutations.

## Numeric tuning

For the built-in moving average, submit `{"strategy":"ID","campaign":"ID","trials":20,"seed":42}` to `/api/tune`. Defaults explore fast windows 5–119 and slow windows above fast through 599. Tuning evaluates development only, stops after 20 trials without a feasible improvement of 0.25 percentage points CAGR, and has a 30-minute deadline and maximum 100 trials.

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

The immutable strategy specification is provided to the executable in `TC_STRATEGY_SPEC`; parse its `parameters` object before entering `tc_sdk::serve`. `tc_sdk::propose` provides the same bounded grid/random candidate generation for external agents. The UI exposes a basic built-in tuning action; custom spaces use the API. Tuning state and child job IDs appear in the overview. Tuning records retain `strategy`, `campaign`, `trials`, and `jobs` across progress and terminal states; child job membership is published on submission so the UI can group trials before they finish. A service restart leaves unfinished historical runs interrupted; tuning is not automatically resumed.

## SDK contract

The [external example](../examples/strategy/src/main.rs) demonstrates a standalone process using `Strategy::on_frame`. Each frame contains only the current completed minute's real candles, simulated time, portfolio state, and a warm-up flag. Keep trailing indicator state inside the strategy. All markets with a matching timestamp are supplied together. IPC is little-endian length-prefixed JSON, protocol 1, maximum 8 MiB per message. Stdout is exclusively for this protocol.

Weights are fractions of the portfolio, must total at most one per decision, and permit only long/cash positions. Use at most one intent per market per frame. Signals cannot fill on the bar that produced them. External strategies are timed out after 45 seconds without a response; whole historical jobs have a 30-minute limit. Single-event IPC favors clarity over maximum throughput; builtin strategies avoid IPC. Native code and Cargo build scripts remain trusted.

SMA and volatility use a completed rolling window (population standard deviation). EMA seeds from the first observation and becomes available after its period. RSI currently uses simple rolling gains/losses (Cutler's definition); ATR is the SMA of true range. Window and initialization choices are explicit so later indicator variants do not silently change old results. `Resampler::advance` returns a bar only when its UTC interval is finished; `complete` indicates all base minutes were observed. Gaps remain flagged, not executable zero-volume prices.

## Models

Set `TC_MODEL_MAP='{"quick-analysis":"YOUR_LOCAL_MODEL"}'` and optionally `TC_OLLAMA_URL=http://127.0.0.1:11434` before starting the service. Strategies invoke the logical capability `quick-analysis`; the server selects the model. No model download is automatic. The `fake` capability echoes the prompt for integration tests.

Use `tc_sdk::HttpModelClient::from_env()`; the default server URL is localhost:8080, overridable with `TC_MODEL_URL`. TC supplies a separate `TC_MODEL_SCOPE` to each external historical worker or paper session. The request contains `scope` and `request: {capability, prompt, max_tokens}`. Each job or paper session allows at most 64 calls, at most 2,048 output tokens each, and 32 KiB prompts. Successful responses are recorded and repeated requests replay them. Failed/pending reservations remain consumed; no unbounded retry loop. Model calls are opt-in strategy behavior and separate scopes prevent cache reuse across development and validation. General model knowledge of historical events is accepted for v1 and recorded as an unknown training cutoff.
