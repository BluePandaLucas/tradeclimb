# Running TradeClimb locally

TradeClimb v1 runs as a native Rust service and browser WASM application. It never sends orders to Bitvavo. Account keys are not needed. Native strategy code, Cargo build scripts, the local filesystem, and LAN users are trusted in this release.

## Start the interface

From the repository root:

```bash
source "$HOME/.cargo/env"
./scripts/start
```

Open `http://127.0.0.1:8080`. The start script builds browser assets and the release binary, then starts the local service. To allow LAN access explicitly:

```bash
./scripts/start --bind 0.0.0.0:8080
```

Use the PC's LAN IP address in the browser. Set `TC_ALLOWED_HOSTS` for a custom DNS hostname. Keep the service on a trusted LAN; this release has no login. One service owns a data directory at a time. Data defaults to `var/`, which is ignored by Git.

The header theme toggle follows the system setting by default. An explicit light/dark preference is remembered in this browser; **System** restores automatic detection. To exit the running service, press **Ctrl+C**, or type **q**, **quit**, or **exit** and press Enter in its terminal. SIGTERM also requests a clean shutdown.

## Try the complete workflow with synthetic data

```bash
cargo run --locked --release --bin tc -- demo
./scripts/start
```

Demo creates a deterministic synthetic dataset, three example strategies, a fixed development/validation split, and three development jobs. The web service processes those queued jobs. All synthetic data is labeled; these returns are not evidence about the crypto market. The overview automatically opens a completed development curve. Select another run to compare it, toggle benchmarks, or request the last 7/30/90 days at higher resolution.

An independent demo directory can be used without mixing fixtures into research. A prepared one is available in this development checkout:

```bash
target/release/tc --data-dir var/demo demo
TC_DATA_DIR=var/demo ./scripts/start
```

Strategy evaluations open from any cell in a row; the strategy-name button also supports keyboard navigation. A tuning experiment appears as a compact progress/best-objective summary. Individual trials are hidden until **Show individual tuning trials** is checked. The evaluation list shows ten rows per page, and the selected chart remains independent of arriving trials. **Stop tuning** cancels further trials; starting another experiment is disabled while tuning is pending/running.

## Import real Bitvavo data

```bash
target/release/tc import > /tmp/tc-import.json
```

The default range is `[2022-10-31, 2026-11-01)` UTC, including one day for indicator warm-up before the accepted four-year evaluation range. Downloading can take several minutes. Progress goes to stderr; the manifest goes to stdout. Imports resume from compressed caches, preserve decimal values, and publish immutable Parquet partitions with hashes. Bitvavo's candle `end` is exclusive. The importer has a regression test for this boundary and preserves legacy responses separately when repairing old caches.

Inspect the manifest's coverage before creating a campaign. Missing candle intervals are unknown/no-trade intervals; they are not synthesized for execution. By default every market must contain data and its longest unknown interval must be at most 60 minutes. A failing snapshot remains on disk for inspection and cannot be used by a default campaign. If you deliberately accept its gaps after inspection, rerun with `--accept-gaps`; it creates a differently identified snapshot with that override recorded. The platform never silently selects a shorter, more profitable time range.

```bash
target/release/tc list
target/release/tc campaign DATASET_ID
```

A full-range campaign evaluates development `[2022-11-01, 2025-11-01)` and validation `[2025-11-01, 2026-11-01)`. Smaller explicitly imported datasets use a 75/25 chronological split, which is useful for platform smoke tests and is not a replacement for four-year research. New campaigns default to daily Sharpe (`sharpe_daily_v1`); the UI, tuning, and validation feedback follow that campaign objective. Existing campaigns remain on CAGR. Create a legacy-style campaign with an explicit growth aspiration:

```bash
target/release/tc campaign DATASET_ID --objective net_cagr --target-cagr 0.20
```

The target is a fraction: `0.20` means 20%. A new objective does not reset the global validation budget or change the simulation's risk/cost rules.

## Register and evaluate strategies

To register a built-in fixture:

```bash
target/release/tc builtin --name "MA baseline" --kind ma --fast 20 --slow 100
```

To build and register the external Rust example:

```bash
cargo build --manifest-path examples/strategy/Cargo.toml --release --locked
target/release/tc register \
  --name "Half BTC" \
  --description "Hold half the virtual portfolio in BTC and the rest in cash" \
  --project examples/strategy \
  --executable examples/strategy/target/release/tc-example-strategy
```

Registration retains the executable, source files, and Cargo.lock and records hashes and optional lineage. The executable is rechecked when preparing a run and resolved within the selected data directory after restore. For other strategies, `--parameters '{"lookback":30}'` supplies immutable inputs through `TC_STRATEGY_SPEC`. Strategy sources stay in their own project; the example here is an integration template.

Submit in the UI or CLI:

```bash
target/release/tc run --strategy STRATEGY_ID --campaign CAMPAIGN_ID --key trial-001
target/release/tc run --strategy STRATEGY_ID --campaign CAMPAIGN_ID --key stress-025 --slippage-bps 25
target/release/tc run --strategy STRATEGY_ID --campaign CAMPAIGN_ID --key validation-001 --validation
```

The service must be running to execute queued jobs. It defaults to two workers, with a bounded queue and process deadlines. Cost stress tests use development only. Fees default to 25 basis points per fill and historical adverse execution allowance to 10 basis points per fill. These are explicit assumptions, not reconstructed historical fees or measured historical spreads. Current minimum notional and quantity precision are conservative assumed rules in the run configuration; historical rule changes are not reconstructed.

Validation exposes only Sharpe for a new Sharpe campaign, or net CAGR for a legacy campaign. Ten unique queries are allowed globally per data directory, including all campaigns and strategies. Repeated identical queries reuse the original evaluation. Extending the budget and inspecting details are explicit local owner actions:

```bash
target/release/tc budget
target/release/tc budget --extend 5
target/release/tc inspect JOB_ID --reveal-validation --output /tmp/validation-report.json
```

Owner inspection marks the ledger revealed. Trusted native code can bypass these cooperative rules; v2 isolation is the first priority. Development reports can be exported without the reveal flag. The exported report contains full equity, exact fills, monthly returns, and the pinned simulation configuration.

## Paper trading and recovery

Select Paper trading in the UI and start a fixed strategy version. Each session has independent EUR 10,000 virtual capital. Public closed candles are polled every ten seconds, and fresh REST bid/ask observations are used for small simulated full fills. Paper quote polling is a deliberate v1 implementation choice; exchange WebSocket ingestion and depth-aware execution are later improvements. The UI receives SSE snapshots.

A new session shows **starting**, connects to the feed, warms up its indicators, then becomes **running**. Its activity message, virtual equity, and update time distinguish a live session awaiting a signal from a failed session. There is at most one active session per strategy version; another start returns that existing session. A new independent session can be started after the previous one stops.

Paper decisions do not execute retrospectively after downtime. Startup replays recorded causal frames to restore strategy state and resumes with fresh observations. Ordered frames, exact fills, and the latest accounting snapshot commit atomically in SQLite. Portfolio curves and fill markers in the live view are bounded; the journal retains exact historical fills. If offline for over a day, or a feed/model/strategy fails, the session halts visibly for inspection. It does not quietly trade on incomplete recovery data.

The drawdown criterion is 15% from portfolio peak equity, including unrealized value and modeled exit costs. A breach stops new risk and attempts liquidation at the next executable observation. Gaps, absent depth, and costs can exceed 15%. Historical minute data cannot certify intraminute drawdown. A paper session's stop control stops evaluation; it does not place exchange orders or erase its journal.

Stop changes an active worker to **stopping**, cancels warm-up/replay and polling, and becomes **stopped** when the worker exits. An in-flight feed request can take up to its five-second timeout. Stopped, halted, and stopping sessions have disabled stop controls. Portfolio checkpoints and the event journal remain available after stopping.

Service shutdown closes browser event streams, cancels workers, and leaves active paper sessions recoverable on the next start. Explicitly stopped sessions stay stopped, including when shutdown happens while a stop is pending. Linux process leases and groups clean up worker and external paper processes after cancellation or a server restart. Process separation is crash handling, not a security or resource sandbox.

## Backup and restore

```bash
target/release/tc backup /absolute/path/to/new-backup
```

The backup uses SQLite's consistent backup API and copies datasets, registered source/executable artifacts, and historical job reports. It excludes raw download caches, which are not necessary to reproduce a published snapshot. Runtime PID ownership/leases are removed from the backup. Use a new destination. Restore by starting against that directory:

```bash
target/release/tc --data-dir /absolute/path/to/new-backup serve --bind 127.0.0.1:8081
```

Interrupted historical jobs need a new submission/idempotency key. Published dataset files are verified by checksum when loaded. Preserve the backup as a unit: a database without its referenced immutable files is incomplete. Confirm a restored external strategy can be loaded before relying on a backup.


## Reproduce throughput measurements

```bash
cargo build --locked --release -p tc-cli --example benchmark
python3 scripts/benchmark.py var DATASET_ID
```

The benchmark runs a 20/100 moving-average fixture across the complete snapshot, with the drawdown halt disabled so it exercises the whole window. It emits timing/row counts only, withholding financial metrics and curves. It does not create a development/validation result or consume validation feedback. Compilation and initial import are excluded. Report results with the dataset, hardware, and whether data is synthetic; a measured fixture is not a latency guarantee for arbitrary external strategies or model calls.
