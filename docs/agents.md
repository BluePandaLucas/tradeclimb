# Strategy-development agents

TradeClimb provides evaluation tools; the separate orchestrator supplies strategy ideas and model-provider access. Agents use scoped development credentials and the isolated launcher. Owner setup, host requirements and resource limits are in [operations](operations.md#development-agent-access).

## Bootstrap and iterate

Start the service from the repository root, then initialize a project in the agent's private workspace:

```bash
./scripts/run-agent /usr/bin/python3 /tools/tc_agent.py capabilities
./scripts/run-agent /usr/bin/python3 /tools/tc_agent.py overview
./scripts/run-agent /usr/bin/python3 /tools/tc_agent.py init my-strategy
```

The kit includes a working Rust example, public SDK source, fixed manifest/lockfile, exact synthetic preflight inputs under `fixtures/`, the standard-library Python client, platform identity and [author instructions](agent-template.md). The service pins its reviewed SDK/compiler inputs at startup; pass `tc serve --sdk-root /path/to/tradeclimb` when starting outside the repository. After a platform change, initialize a fresh kit and copy your Rust source into it.

The selected agent program edits `/workspace/my-strategy/src/**/*.rs`. It then uses:

```bash
./scripts/run-agent /usr/bin/python3 /tools/tc_agent.py submit my-strategy \
  --name 'My strategy' --parameters '{"lookback":30}' --wait
./scripts/run-agent /usr/bin/python3 /tools/tc_agent.py run \
  --strategy STRATEGY_ID --campaign CAMPAIGN_ID \
  --trace-start 2024-01-01 --trace-end 2024-01-02 --trace-frames 100 --wait
./scripts/run-agent /usr/bin/python3 /tools/tc_agent.py diagnostics JOB_ID
./scripts/run-agent /usr/bin/python3 /tools/tc_agent.py trace JOB_ID
```

Use the actual campaign's development dates from `overview`. A revision can pass `submit --parent STRATEGY_ID`. Successful admission retains immutable source, executable, parameters and lineage; it does not consume validation queries. Failed candidates never become registered versions.

## Admission and preflight

Only UTF-8 Rust files under `src/` are admitted: at most 64 files, 64 KiB each and 256 KiB total. Paths cannot escape the source tree. Arbitrary manifests, dependencies, build scripts and shell commands are not accepted. The service supplies the pinned SDK, manifest and lockfile and builds offline inside the existing compiler sandbox. Missing cached dependencies fail visibly; there is no network fallback.

Preflight runs the executable twice on each of two synthetic fixtures, including warmup and missing bars. Protocol errors, invalid intents and differing decision hashes reject admission. Model calls use declared `--model-fixtures '{"capability":"synthetic response"}'`; `fake` echoes prompts by default. Preflight cannot contact a provider and is not performance evidence or a complete proof against lookahead. Optional [custom synthetic behavior tests](synthetic-tests.md) add bounded declarative assertions with `submit --synthetic-suite suite.json`. These run through the same sandbox/engine, twice per scenario, under the existing total preflight deadline. Failed assertions reject admission and retain expected/observed evidence on the candidate.

One candidate builds at a time, with at most eight outstanding candidates across the service. Build and preflight deadlines are 30 minutes and two minutes. These safety bounds are separate from the deferred per-agent budgets/fair scheduling in [the backlog](backlog.md#deferred-agent-research-features).

Candidate states are `queued`, `building`, `preflight`, `completed`, `failed`, `cancelled`, or `interrupted`. Results carry `strategy`, synthetic `preflight` evidence, and bounded `diagnostic` code/message/compiler log. Only the submitting credential and owner can read/cancel a candidate. Revocation cancels active work and prevents publication. Service restarts interrupt active candidates; queued candidates remain queued.

## Explore and debug development

```bash
./scripts/run-agent /usr/bin/python3 /tools/tc_agent.py candles \
  --campaign CAMPAIGN_ID --market BTC-EUR \
  --start 2024-01-01 --end 2024-01-02 --limit 1000
```

Candle pages cover one market and at most seven days, with at most 2,000 rows. Follow `next_start` with the original end time. Timestamps are UTC bar-open milliseconds; a candle becomes available at `time + 60000`. Responses include dataset/venue identity, coverage counts, synthetic status and notes. Missing bars remain missing. Reads verify only matching market/month partitions and their checksums.

A development submission can request a trace window inside its run. Traces retain completed frames, portfolio state, decisions, next-bar fills, pending intents, stale/risk state and errors. They stop at the first requested frame/byte limit (maximum 512 frames/2 MiB) and report truncation. Narrow the window to inspect later decisions. Traces do not change evaluation metrics. Compiler logs and runtime stderr are bounded; job diagnostics explain undefined Sharpe and common no-fill conditions. The `measurement` diagnostic includes a date-level ledger from retained full-resolution equity, boundary-reason counts, and both cost-matched native references under the same `sharpe_daily_v1` / `buy_hold_comparison_daily_v1` rules. A single missing/nonpositive boundary rejects the entire v1 series, including otherwise complete days; intraday completeness and stale flags are not Sharpe masks. The run-wide stale flag separately disqualifies eligibility. The audit never executes strategy code, creates an evaluation, spends budgets, or overwrites recorded metrics. Native reference replays and their agreement with original comparisons are labeled explicitly. Provenance identifies the frozen request/config/dataset manifest, compressed report hash and audit binary; original engine/source/executable provenance is available for newly executed jobs and unknown for legacy jobs. Hash-keyed caches are derived artifacts; expensive work uses the bounded report-work queue. No trace is retained unless requested. In new traces, `stale` remains cumulative since run start, including pre-open mark-age checks. `current_stale` and `stale_marks` describe positive holdings after the current close, and `first_stale` records the first violation time, phase and affected markets. Legacy traces cannot recover an unrecorded first violation; null means unknown. Equity boundary ledgers do not certify historical mark freshness because Report does not retain per-point mark ages. Initial capital and terminal valuation are explicitly labeled boundary sources.

The SDK contract and metric conventions are in [the API reference](api.md). Validation, final-test and paper controls remain owner-only; revealing validation to the owner does not expose it to agents. Raw candle and trace tools cannot request those windows, including through old campaigns.

## Client and recovery

The client works as a CLI or an importable `Client`. It needs only Python's standard library. It reads `TC_API_SOCKET` and `TC_API_TOKEN_FILE`; trusted local callers may instead set `TC_API_URL` with an explicit scoped agent token file. It never falls back to owner credentials. The launcher exposes a read-only copy at `/tools/tc_agent.py` and each kit includes its own copy.

Identical input derives a stable idempotency key; repeated submission returns the same attempt. Changed source, parameters or run settings create a new attempt. An explicit new `--key` retries unchanged inputs after an infrastructure failure. Reusing a key for different inputs fails. Source submission records the last candidate in `.tc-agent-last.json`.

`wait-candidate ID` / `wait-job ID` resume observation; `candidate ID` / `job ID` return current state. `cancel-candidate ID` / `cancel-job ID` are safe to repeat. Transport retries apply only to reads and idempotent requests. A wait timeout leaves the work running; interrupted work requires a new attempt. CLI results are JSON, with nonzero exit status for failure/interruption or client errors.
