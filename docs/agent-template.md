# Strategy author kit

Read `tc_agent.py capabilities` before changing code. This kit pins the public SDK, compiler version and dependency lock. Edit only `src/**/*.rs`; the service supplies its own trusted manifest, SDK and lockfile. Custom dependencies, manifests, build scripts and arbitrary shell commands are not accepted by candidate admission.

`fixtures/regular.json` and `fixtures/gapped.json` contain the exact synthetic candle/config inputs used by server preflight. They are debugging examples, not market data or performance evidence. Optional `--synthetic-suite suite.json` adds declarative behavior assertions; see `synthetic-tests.md` in this kit for the versioned schema, timing, limits and example. Standard preflight always runs.

## Development loop

1. Read the campaign's development dates and dataset notes from `overview`.
2. Explore bounded development candles with `candles`; timestamps are UTC milliseconds at bar open, available at `time + 60000`.
3. Edit `src/main.rs`, using the SDK source under `sdk/` and the included half-BTC example.
4. `submit . --name "My strategy" --wait` builds offline, tests synthetic regular/gapped input twice, and returns an immutable strategy ID. Use `--parent ID` on a revision. Fix errors from `diagnostic.code`, `message` and bounded compiler `log`.
5. `run --strategy ID --campaign ID --trace-start DATE --trace-end DATE --wait` evaluates development. `diagnostics JOB_ID` explains metrics/failure; `trace JOB_ID` shows the bounded frame/decision/fill sequence.

Run commands as `python3 /tools/tc_agent.py ...` inside the prepared agent launcher, or `python3 tc_agent.py ...` from this project. The client reads `TC_API_SOCKET` and `TC_API_TOKEN_FILE`; provider and owner credentials never belong in source. Outside the launcher, a trusted local user can set `TC_API_URL` and an explicit agent credential file.

The client derives stable idempotency keys from inputs. Repeating the same submit/run returns the same attempt. A source or parameter change creates a new attempt; use an explicit `--key` to deliberately retry unchanged inputs after an infrastructure failure. Interrupted work is not silently resumed; `wait-candidate ID` and `wait-job ID` resume observation. Candidate results are saved in `.tc-agent-last.json`.

## Strategy contract

- Use `tc_sdk::serve(YourStrategy)`; stdout is reserved for framed protocol messages. Stderr is for bounded diagnostics.
- Read parameters from `TC_STRATEGY_SPEC`; use explicit seeds. Synthetic preflight requires reproducible decisions.
- Frames contain completed bars and portfolio state. Do not trade during `frame.warmup`. A signal can fill only on a subsequent bar.
- Return at most one intent per market, long/cash weights in `[0,1]`, with total weight at most one. Gaps stay unknown; they are not synthetic executable prices.
- For model-using strategies, supply `--model-fixtures '{"capability":"fixture response"}'` during submission. Preflight never calls a real provider. The `fake` capability echoes its prompt by default. Real historical model knowledge retains the platform's accepted limitations.
- Preflight is a protocol/replay check, not performance evidence or proof against all lookahead. A rejected order or absent fill may be legitimate: inspect cash, minimum size, warmup, pending intent expiry, stale flags and risk halts.
- Sharpe ranking, drawdown eligibility and benchmark diagnostics are distinct. Undefined Sharpe is not zero. Two daily returns establish computability, not confidence.

Validation, final testing and paper promotion belong to the owner. Development data, traces and diagnostics cannot request those windows. Review dataset coverage and the explicit cost assumptions before interpreting results.
