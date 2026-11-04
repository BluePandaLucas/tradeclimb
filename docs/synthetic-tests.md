# Synthetic behavior tests

Submit an optional `synthetic_suite` with a candidate, or use `tc_agent.py submit . --name Example --synthetic-suite suite.json --wait`. The Python client also accepts `synthetic_suite=...`. The suite participates in credential-scoped idempotency. Changed scenarios or assertions require a new key, just like changed source. Standard preflight remains mandatory; any custom assertion, engine error, or replay mismatch rejects registration. Candidate status, cancellation, confinement, fixture-only model responses, and the existing two-minute total preflight deadline apply to the entire suite. No dataset is read and no validation queries are consumed.

These are author-specified correctness checks, not performance evidence, independent approval, or proof of causal correctness. Use fabricated candles only; the service never fetches historical bars for these tests. A schema cannot verify that user-supplied numbers were invented.

## Schema version 1

A suite is `{ "version": 1, "scenarios": [...] }`. Each scenario requires a unique `name`, `config` (the engine's complete `RunConfig`, as included in kit fixtures), `candles` (OHLCV objects matching kit fixtures), and nonempty `assertions`. Decimal values are strings. Unknown suite, scenario, and assertion fields fail. Config/candle objects use the existing engine contracts.

Each assertion has a `kind` and these fields:

| kind | Fields | Pass condition |
| --- | --- | --- |
| `no_intents` | `start`, `end` | At least one frame in the window and zero emitted intents across those frames, including warmup. |
| `intent` | `time`, `market`, `target_weight` | Frame exists and emits the exact target weight for this market. `null` means no intent for this market; zero means an explicit cash/exit intent. |
| `frame_present` | `time`, `present` | A strategy invocation exists (or does not exist) at this time. |
| `fill_count` | `start`, `end`, `market`, `side`, `count` | Exactly this many executed fills in the window. `side` is `buy`, `sell`, or `null` for either. |
| `portfolio_weight` | `time`, `market`, `weight`, `tolerance` | Marked market value / total portfolio value differs from `weight` by at most the absolute `tolerance`. A missing frame or nonpositive total fails. |

Times are UTC milliseconds. Windows are `[start,end)`. Decision and portfolio assertions use `frame.time`, which is bar-open time + 60000. Fill windows use engine `fill.time` (execution bar open). Portfolio observations include fills from prior decisions and current closing marks, before the current decision executes. Intents can fill only on a subsequent available bar, subject to fees, slippage, volume, minimum size, stale/risk state and intent expiry. A target weight does not guarantee that exact portfolio weight.

Missing candles stay missing. An entirely missing timestamp produces no frame; a timestamp with another market's bar still produces a frame without the missing market. An absent frame fails `intent` even when its expected weight is null. Use `frame_present` for missing invocations. `no_intents` requires a frame to avoid a vacuous warmup pass. A zero fill count can pass over an empty window of observations, but engine errors still fail the scenario.

## Limits and evidence

At most 8 scenarios; 2–512 candles and 1–128 assertions per scenario; 2,048 candles and 256 assertions total; serialized suite at most 512 KiB. Names are 1–100 UTF-8 bytes. Candles must be sorted by `(time,market)` without duplicates, minute aligned, and use strategy-declared markets with config rules. Evaluation and warmup together span at most one day. There must be evaluation bars. Assertions stay within the scenario's first bar-open through config end; range assertions require a nonempty window. Prices are between 0.000001 and 1 billion, volume between zero and 1 billion, initial capital at most 1 billion. Other config validity and market rules follow the engine contract. Weights/tolerances are in `[0,1]`; fill count is at most 2,560. The existing candidate request body limit remains 2 MiB.

Each scenario runs twice with fresh strategy processes and fixture brokers. Replay hashes cover frames' times, warmup flags, intents, portfolio observations, executed fills, and bounded errors. Results report each assertion's expected/observed values and pass/fail, replay status, observation hash, and at most two 2,000-character runtime errors per case. Full traces and performance reports are not returned. Fill assertions cover executions observed during frames; terminal liquidation at `config.end` is outside assertion windows.

`candidate.preflight` includes synthetic/non-performance labels, source/parameters/model-fixture/suite hashes, declared markets, pinned platform, an executable hash once built, and an execution-code hash. The immutable strategy ID is added only after successful registration. Before registration, the candidate ID and hashes identify the attempted strategy. Build failures retain `status: not_run`; completed checks retain their evidence even when admission fails. Cancellation, interruption and deadline termination remain visible in candidate state/diagnostics; they do not certify a passed suite. Standard results are under `standard` (and the legacy top-level `cases`); custom results are under `custom`.

## Example

This suite tests the kit's half-BTC strategy, including warmup, a missing bar, the requested allocation, next-bar execution and an approximate achieved weight. Start with a copy of `fixtures/regular.json` from the kit and generate a complete suite:

```python
import json
fixture = json.load(open("fixtures/regular.json"))
config = fixture["config"]
config.update(start=60000, end=300000)
candles = [dict(market="BTC-EUR", time=t, open="100", high="100",
                low="100", close="100", volume="1000")
           for t in [0, 60000, 180000, 240000]]
suite = {"version": 1, "scenarios": [{
    "name": "half-btc-with-gap", "config": config, "candles": candles,
    "assertions": [
        {"kind": "no_intents", "start": 0, "end": 120000},
        {"kind": "intent", "time": 120000, "market": "BTC-EUR", "target_weight": "0.5"},
        {"kind": "frame_present", "time": 180000, "present": False},
        {"kind": "fill_count", "start": 120000, "end": 240000,
         "market": "BTC-EUR", "side": "buy", "count": 1},
        {"kind": "portfolio_weight", "time": 240000, "market": "BTC-EUR",
         "weight": "0.5", "tolerance": "0.01"}
    ]}]}
json.dump(suite, open("suite.json", "w"), indent=2)
```

For a momentum strategy, supply an explicit rising-then-falling price path and assert its entry and zero-weight exit at the specified completed-frame times. Choose the path, lookback parameters, and warmup to exercise the intended rule; separately assert subsequent fills if execution is part of the test.
