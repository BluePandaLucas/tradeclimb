# Running TradeClimb locally

TradeClimb runs as a native Linux service and browser WASM application. It never sends orders to Bitvavo. Account keys are not needed. External strategies execute inside a Linux sandbox. Use the isolated build command for untrusted strategy projects; manually running Cargo still executes build scripts on the host. The service, trusted SDK/compiler/cache, and local owner account remain trusted. The owner browser authenticates separately from development-only agent credentials; use the prepared agent launcher for untrusted agent programs.

## Start the interface

From the repository root:

```bash
source "$HOME/.cargo/env"
./scripts/start
```

Open `http://127.0.0.1:8080` and sign in with the private owner key. Locate it using `target/release/tc auth status`; the key itself is only in the reported local file, never command output or a URL. Sign-in creates a 12-hour HttpOnly, SameSite=Strict session; it is not stored in browser local storage. **Sign out** invalidates it. `tc auth rotate-owner` replaces the key and invalidates existing owner sessions; it does not reset research/validation budgets.

The service accepts loopback IP listeners only. For another device, use an HTTPS reverse proxy, configure `TC_ALLOWED_HOSTS` for its hostname, and forward the browser's Host/Origin. HTTPS-origin sign-in sets Secure cookies. The static assets directory must be separate from private data. Anonymous APIs fail with 401; agent access outside development fails with 403. One service owns a data directory at a time. Data defaults to `var/` and is owner-only (0700), including all raw data, model caches and evaluation artifacts. Backups are also private. Restoring a backup generates a new owner key because the key file is not included; old owner sessions stop working.

## Development-agent access

The tool has one owner; there is no multi-user registration or third-party identity service. Future strategy orchestration remains a separate project. Provision its locked Unix identity and scoped credential as the owner:

```bash
./scripts/setup-agent var
```

This uses noninteractive sudo to create/reuse `tc-agent`, with a locked password, no login shell or supplementary groups, and no sudo access. It issues a 30-day development credential into that account's private home. Re-running revokes its preceding credential. `tc auth status` lists credential IDs/expiry/revocation without secret values; `tc auth revoke ID` revokes one, closes its streams, and cancels its queued/running jobs and tuning controllers.

Start TC first, then run an explicitly selected program through the prepared launcher:

```bash
./scripts/run-agent /usr/bin/python3 -c 'import os; print(os.environ["TC_API_SOCKET"])'
```

The launcher uses a separate UID, private user/PID/network/IPC namespaces, a read-only system image, a writable `/workspace` mapped to the agent home, 256-MiB temporary space, no owner environment, and a cgroup capped at 4 GiB/no swap, 128 tasks, two CPU cores and six hours. The only host service it can reach is the mounted `TC_API_SOCKET`, speaking ordinary HTTP over a Unix socket. Read the agent key from `TC_API_TOKEN_FILE` and send `Authorization: Bearer KEY`. The socket rejects owner credentials/cookies and login entirely. This prevents an agent from reaching owner browser sessions, local debug ports, other local services, or the Internet. Do not run untrusted agents directly with `sudo -u`; the launcher is the supported boundary.

Agents can read development results and submit/tune development evaluations. Idempotency and cancellation are credential-scoped. They cannot query validation, create campaigns, archive strategies, invoke owner model scopes, promote/stop paper sessions, or see owner paper/validation snapshots. Token revocation/expiry also closes long-lived streams. Actual agent/model tooling, bounded inference for strategy invention, reviewed SDK distribution and candidate handoff will be connected when the separate orchestrator project starts; no agent/model process is launched by setup.

Local CLI operations still belong to the filesystem owner. Raw owner credentials must never be given to agents. Candidate source copying uses Linux openat2 with beneath/no-symlink resolution to reject symlink/directory-swap escapes, special files and private TC directories before isolated builds.

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

Inspect the manifest's coverage before creating a campaign. Missing candle intervals are unknown/no-trade intervals; they are not synthesized for execution. By default every market must contain data and its longest unknown interval must be at most 60 minutes. A failing snapshot remains on disk for inspection and cannot be used by a default campaign. If you deliberately accept its gaps after inspection, rerun with `--accept-gaps`; it creates a differently identified snapshot with that override recorded. The owner explicitly accepted the documented gaps on 4 November 2026 for V2 research. That override is specific and recorded; defaults still apply the coverage gate. The platform never silently selects a shorter, more profitable time range. The reserved final test is a separately approved calendar policy, not a data-gap repair.

```bash
target/release/tc list
target/release/tc campaign DATASET_ID
```

A new full-range campaign evaluates development `[2022-11-01, 2025-11-01)` and validation `[2025-11-01, 2026-08-01)`. The owner reserved `[2026-08-01, 2026-11-01)` as the untouched final test. Ordinary evaluation, cached detail, rankings, and warmup inputs cannot expose it; final-test execution remains unavailable pending its owner-controlled workflow. Old campaigns are retained, but any overlapping job is blocked: create a current campaign revision. Smaller explicitly imported datasets use a 75/25 chronological split, which is useful for platform smoke tests and is not a replacement for four-year research. New campaigns default to daily Sharpe (`sharpe_daily_v1`); the UI, tuning, and validation feedback follow that campaign objective. Existing campaigns remain on CAGR. Create a legacy-style campaign with an explicit growth aspiration:

```bash
target/release/tc campaign DATASET_ID --objective net_cagr --target-cagr 0.20
```

The target is a fraction: `0.20` means 20%. A new objective does not reset the global validation budget or change the simulation's risk/cost rules.

## Strategy isolation

External historical and paper strategies require Linux, `/usr/bin/bwrap`, util-linux `prlimit`, and a running systemd user manager with delegated memory/CPU/pids controllers. Install the distribution's `bubblewrap` and `dbus-user-session` packages and run TC from a normal user session with `XDG_RUNTIME_DIR` set. On this machine the existing `bwrap-userns-restrict` AppArmor profile permits Bubblewrap without changing global namespace restrictions. Headless services also need their user manager/bus available; run this probe as the same user/session as the service:

```bash
target/release/tc sandbox-check
```

A failed prerequisite or confinement launch fails the external run; there is no trusted-code fallback. Built-in strategies are part of the trusted evaluator and do not require an external sandbox. The sandbox gets a new user/PID/network/IPC namespace, no capabilities or nested user namespaces, sanitized launcher/strategy environments, a read-only root, system executables/libraries, private `/proc` and `/dev`, the one strategy executable, and bounded scratch space. The data directory, owner home, account credentials, and host network are not mounted. OS executable/library directories are a trusted runtime image and must not contain private research data.

| Limit | External strategy | Isolated build |
| --- | --- | --- |
| Aggregate memory, including tmpfs | 512 MiB | 4 GiB |
| Swap | Disabled | Disabled |
| Aggregate tasks/threads | 32 | 128 |
| Aggregate CPU quota | One core | Two cores |
| `/tmp` | 16 MiB | 256 MiB |
| Build output tmpfs | None | 2 GiB |
| Per-file size | 16 MiB | 512 MiB |
| Open descriptors per process | 128 | 128 |
| Deadline | 45 seconds per strategy frame; historical supervisor 30 minutes | 30 minutes |

Cgroup cleanup kills all descendants, including processes that create another session. Bubblewrap also kills its PID namespace on abrupt owner death. Paper strategies have no overall 30-minute lifetime; their frame timeout and resource limits still apply. The model broker uses a separate, per-process socket bound to the owning run/session. It retains existing provider mapping, cache, and 64-call scope budgets, accepts no caller-selected scope or provider URL, limits request/response sizes, and bounds slow-client reads/writes and shutdown. Recompile SDK users to use this socket transport; legacy binaries that attempt direct HTTP cannot reach the host network.

Build an untrusted strategy without running Cargo or build scripts directly on the host:

```bash
target/release/tc build --project examples/strategy --sdk-root . \
  --bin tc-example-strategy --output var/builds/strategy
```

Builds snapshot regular source files (no symlinks or special files; at most 10,000 files, 20,000 entries, 100 MiB, 10 MiB per file, and 64 nested directories). `target`, `.git`, `.env`, `.env.*`, and `var` are excluded. Keep private data outside source trees, compiler directories, and Cargo caches. The trusted SDK mounts contain only this workspace's manifest, tc-core and tc-sdk snapshots; the strategy is mounted at `/workspace/examples/strategy`, preserving the example's relative SDK paths. Only Cargo's registry `src`, `cache`, and `index` are mounted, not Cargo credentials/config or Git caches. The selected compiler sysroot is read-only. Builds run `--offline --frozen`; provision reviewed dependencies in the trusted cache first. There is no automatic dependency fetching or retry with network access. Lockfiles are mandatory; other host path dependencies are unavailable.

Cargo output remains on bounded tmpfs. Only a successful Linux executable (at most 64 MiB) is copied to the selected host output path. Build metadata reports its SHA-256; registration retains those executable bytes as before. Isolated builds are not a claim of compiler reproducibility. The trusted platform/SDK itself is built with the normal repository workflow.

`./scripts/check-sandbox` runs the real prerequisite probe, adversarial resource/confinement tests, an offline malicious-build-script fixture, and scoped model calls through historical workers. These Linux tests are explicitly ignored in the portable Rust suite and run separately on a configured host and in CI. An agent running directly as the owner remains outside this boundary; use the separate account and mandatory agent launcher described above. It intentionally has no model-provider or other external-network access until the orchestrator integration is designed.

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

## Owner-approved paper promotion

The Paper trading page lists each version's latest completed development evaluation, ranked by Sharpe when it qualifies. **Approve NAME** explicitly approves that immutable version and starts an independent EUR 10,000 paper session. The backend requires accepted, non-synthetic Bitvavo data, development risk eligibility, a defined Sharpe/CAGR, and observed drawdown at most 15%. The 15% growth objective is an aspiration, not a rejection threshold. A scalar validation score does not certify validation risk.

API owners send `{"strategy":"ID","job":"DEVELOPMENT_JOB_ID","approve":true}` to `/api/paper`. Approval, session creation and its audit entry are committed together; repeated starts retain the existing active portfolio. The `paper_approvals` overview entries retain the basis, Sharpe/drawdown/CAGR, growth-target status, approved job/version and owner time. Old unapproved sessions stop on service startup; historical journals remain. Existing simulation risk halts still apply and are not a guaranteed loss ceiling.

`python3 scripts/security-smoke.py --agent-account tc-agent` verifies real owner login, scoped agent requests/revocation, an actual separate UID, host file denial, and network isolation with the development Unix socket. Browser and live-paper smoke fixtures explicitly mock native provenance/eligibility to exercise approval; their data is not market evidence.
