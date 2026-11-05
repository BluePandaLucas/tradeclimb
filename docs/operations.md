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

## Persistent service and LAN access

`scripts/install-service` installs prebuilt native/browser assets, a reviewed SDK
snapshot and user units. The installed service runs independently of the checkout;
it never compiles at boot. Keep the pinned Rust toolchain and reviewed Cargo cache
installed for candidate builds. Bubblewrap and the user's systemd manager must work
without an interactive session. Installation requires `loginctl` linger:

```bash
sudo loginctl enable-linger "$USER"
# Install nginx from the distribution. If this is a new nginx installation,
# mask its default system unit first so it does not open an unrelated port 80.
sudo systemctl mask nginx.service
sudo apt-get install --no-install-recommends nginx-light
cargo build --locked --release --bin tc
./scripts/build-web
./scripts/install-service --lan-ip 192.168.0.223 --lan-cidr 192.168.0.0/24 --hostname buntubox1
```

Do not mask an existing proxy used by other applications. This deployment runs its
own unprivileged nginx process with a separate configuration. Buntubox1 uses
**https://192.168.0.223:8443** (or `https://buntubox1:8443` if the client resolves
that name). Port 8080 stays on loopback; port 8443 permits only loopback and
`192.168.0.0/24`, plus explicitly configured client addresses/subnets. No router port forwarding is needed. Reserve this LAN IP in DHCP;
if it changes, regenerate the certificate for the new address and rerun installation.

Clients arriving from another routed subnet need an explicit allow entry.
Buntubox1 also allows the owner's `10.0.10.0/24` subnet:

```bash
./scripts/install-service --lan-ip 192.168.0.223 --lan-cidr 192.168.0.0/24 \
  --hostname buntubox1 --allow-client 10.0.10.0/24 --proxy-only
```

This validates and reloads only the proxy; TradeClimb and active evaluations keep
running. Extra addresses/subnets are saved in `~/.config/tradeclimb/access.json` and
preserved when `--allow-client` is omitted on later deployments. Supplying the flag
replaces that saved list; repeat it to allow multiple addresses or subnets.
Authentication is still required.

The first installation creates a one-year self-signed certificate in
`~/.config/tradeclimb/lan.crt`, with the private key next to it. Verify its
fingerprint on the host (`openssl x509 -in ~/.config/tradeclimb/lan.crt -noout
-fingerprint -sha256`) and trust the public certificate on the client. Until then,
the browser reports an untrusted issuer. Never copy `lan.key`. Renewal requires
replacing the certificate/key pair, restarting `tradeclimb-lan.service` and
updating client trust. Health checks warn thirty days before expiry.

Persistent paths:

| Path | Contents |
| --- | --- |
| `~/.local/lib/tradeclimb/releases/` | Installed release snapshots |
| `~/.local/lib/tradeclimb/current` | Active release symlink |
| `~/.local/share/tradeclimb/` | Private research database, artifacts and owner key |
| `~/.local/share/tradeclimb-backups/` | Last seven successful daily local backups |
| `~/.local/state/tradeclimb/` | Latest health result and bounded soak reports |
| `~/.config/tradeclimb/` | LAN proxy configuration and certificate/key |

Installation preserves the data directory; it starts empty unless research state
was explicitly restored there. Never run `demo` against the research directory to
populate an empty UI. Restore an existing backup as a complete directory with the
service stopped, using the backup/restore guidance below. Old agent credentials
remain recorded in a restored database; provision or rotate agent token files
explicitly. The new owner key is generated on first open.

```bash
systemctl --user status tradeclimb tradeclimb-lan --no-pager
journalctl --user -u tradeclimb -u tradeclimb-lan -n 100 --no-pager
~/.local/lib/tradeclimb/current/tc --data-dir ~/.local/share/tradeclimb auth status
systemctl --user list-timers 'tradeclimb-*'
systemctl --user start tradeclimb-backup.service tradeclimb-health.service
cat ~/.local/state/tradeclimb/health.json
```

Both servers restart after failure and start at boot via the lingering user manager.
Ten rapid failures in five minutes stop restart loops; inspect the journal and use
`systemctl --user reset-failed` after fixing the cause. Clean shutdown has a
40-second deadline. No interactive terminal is needed. Existing worker recovery
rules still apply: interrupted historical/build attempts require a new attempt;
queued work survives. The sandbox prerequisite probe runs before every start.

Backups use TC's SQLite backup API and immutable artifact copying, check SQLite
integrity and publish a completed directory before pruning old backups. Failed
`.partial-*` directories remain for inspection and are never treated as successful
backups. Copies reserve an estimated full backup plus 5 GiB free space. Raw download
caches are excluded. These are **same-disk recovery copies**, not protection against
host/disk loss; arrange an off-host copy separately. No credentials are written to
the journal. The public TLS certificate and service configuration are outside TC's
research backup and need separate host recovery provisions.

Health runs every five minutes and verifies both units, authenticated HTTPS, the
UI document and at least 5 GiB disk headroom. It records warnings for old/missing
backups, expiring agent credentials and the LAN certificate. Failures appear as
failed systemd units; this does not send external notifications. Journals use host
retention and the service rate-limits log bursts.

To update, build and validate a new revision, rerun the installer, and inspect the
health result. The installer refuses full deployment while any evaluation or candidate
build is nonterminal (including unknown states), or a final evaluation is running.
It reads one SQLite snapshot before staging and rechecks before switching releases;
unreadable state fails closed. Wait for terminal work and rerun; there is no force
bypass that silently interrupts it. `--proxy-only` remains available without a TC
restart. This preflight is not an admission lock: avoid starting work concurrently
with deployment. Direct `systemctl` restarts bypass the installer check.
`python3 scripts/deployment_guard.py ~/.local/share/tradeclimb` checks idle state
without staging assets or touching services. Installed releases are retained for manual rollback: stop TC, point
`current` to the previous release and restart both services. Check database/schema
compatibility before rolling back a future schema-changing release; binaries alone
cannot roll back database changes. Do not delete a release still used by a worker.

### Persistent-service acceptance and overnight observation

```bash
python3 scripts/service-smoke.py
python3 scripts/smoke.py
./scripts/check-sandbox
# A bounded health-only observation; does not launch an orchestrator or spend queries.
systemd-run --user --unit=tradeclimb-soak --property=RuntimeMaxSec=13h \
  --property=UMask=0077 /usr/bin/python3 \
  "$HOME/.local/lib/tradeclimb/current/scripts/service-ops.py" soak --hours 12
```

`service-smoke.py` uses the deployed service template, isolated temporary synthetic
state and another loopback port. It exercises terminal-free startup, graceful stop,
SIGKILL/automatic restart during a real isolated worker, worker cleanup, retained
owner identity/validation count, and restoration with the original tree hidden.
It verifies a retained report and executes a new evaluation after restoration.
The existing workflow smoke additionally restores and executes a registered
external SDK artifact. No real validation/final observations are consumed by QA.

Soak samples are flushed once per minute under `~/.local/state/tradeclimb/`.
Success writes a final `complete` record; a missing completion record is not a pass.
Inspect `systemctl --user status tradeclimb-soak` and its journal. The soak checks
service/HTTP/storage health, not research quality, sustained evaluation load or live
paper-feed performance. A real cold host reboot is a separate acceptance check;
enabled units and linger establish configuration, not evidence of a completed reboot.

## Development-agent access

The tool has one owner; there is no multi-user registration or third-party identity service. Future strategy orchestration remains a separate project. Provision its locked Unix identity and scoped credential as the owner:

```bash
./scripts/setup-agent var
```

This uses noninteractive sudo to create/reuse `tc-agent`, with a locked password, no login shell or supplementary groups, and no sudo access. It issues a 30-day development credential into that account's private home. Re-running revokes its preceding credential. `tc auth status` lists credential IDs/expiry/revocation without secret values; `tc auth revoke ID` revokes one, closes its streams, and cancels its queued/running candidates, jobs and tuning controllers.

Start TC first, then run an explicitly selected program through the prepared launcher:

```bash
./scripts/run-agent /usr/bin/python3 /tools/tc_agent.py capabilities
./scripts/run-agent /usr/bin/python3 /tools/tc_agent.py init my-strategy
```

The launcher uses a separate UID, private user/PID/network/IPC namespaces, a read-only system image, a writable `/workspace` mapped to the agent home, 256-MiB temporary space, no owner environment, and a cgroup capped at 4 GiB/no swap, 128 tasks, two CPU cores and six hours. The only host service it can reach is the mounted `TC_API_SOCKET`, speaking ordinary HTTP over a Unix socket. Read the agent key from `TC_API_TOKEN_FILE` and send `Authorization: Bearer KEY`. The socket rejects owner credentials/cookies and login entirely. This prevents an agent from reaching owner browser sessions, local debug ports, other local services, or the Internet. Do not run untrusted agents directly with `sudo -u`; the launcher is the supported boundary.

Agents can obtain the pinned SDK/client kit, submit bounded Rust source for sandboxed build/preflight/registration, read development candle slices and request bounded decision traces, as well as submit/tune development evaluations. See the [agent workflow](agents.md). Idempotency and cancellation are credential-scoped. They cannot query validation, create campaigns, archive strategies, invoke owner model scopes, promote/stop paper sessions, or see owner paper/validation snapshots. Token revocation/expiry also closes long-lived streams. The separate orchestrator remains responsible for strategy invention and its model-provider credentials; setup launches no agent/model process. The service prepares its trusted SDK/compiler platform on startup; when starting outside this checkout, pass `serve --sdk-root /path/to/tradeclimb`. Provision the reviewed Cargo cache before candidate builds; agents cannot fetch dependencies.

Local CLI operations still belong to the filesystem owner. Raw owner credentials must never be given to agents. Candidate source copying uses Linux openat2 with beneath/no-symlink resolution to reject symlink/directory-swap escapes, special files and private TC directories before isolated builds.

To exit, press **Ctrl+C**, or type **q**, **quit**, or **exit** and press Enter. SIGTERM also requests a clean shutdown.

## Try the complete workflow with synthetic data

```bash
cargo run --locked --release --bin tc -- demo
./scripts/start
```

Demo creates a deterministic synthetic dataset, three example strategies, a fixed development/validation split, and three development jobs. The web service processes those queued jobs. All synthetic data is labeled; these returns are not evidence about the crypto market. Select a run to open its curve; incoming evaluations preserve the selected page and chart. Choose another run to compare it, toggle benchmarks, or request the last 7/30/90 days at higher resolution.

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

Inspect the manifest's coverage before creating a campaign. Missing candle intervals are unknown/no-trade intervals; they are not synthesized for execution. By default every market must contain data and its longest unknown interval must be at most 60 minutes. A failing snapshot remains on disk for inspection and cannot be used by a default campaign. If you deliberately accept its gaps after inspection, rerun with `--accept-gaps`; it creates a differently identified snapshot with that override recorded. The owner explicitly accepted the documented gaps on 4 November 2026 for research. That override is specific and recorded; defaults still apply the coverage gate. The platform never silently selects a shorter, more profitable time range. The reserved final test is a separately approved calendar policy, not a data-gap repair.

```bash
target/release/tc list
target/release/tc campaign DATASET_ID
```

A new full-range campaign evaluates development `[2022-11-01, 2025-11-01)` and validation `[2025-11-01, 2026-08-01)`. The owner reserved `[2026-08-01, 2026-11-01)` as the untouched final test. Ordinary evaluation, cached detail, rankings, and warmup inputs cannot expose it; only the separate explicit owner final-test workflow can consume it. Old campaigns are retained, but any overlapping job is blocked: create a current campaign revision. Smaller explicitly imported datasets use a 75/25 chronological split, which is useful for platform smoke tests and is not a replacement for four-year research. New campaigns default to daily Sharpe (`sharpe_daily_v1`); the UI, tuning, and validation feedback follow that campaign objective. Existing campaigns remain on CAGR. Create a legacy-style campaign with an explicit growth aspiration:

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
target/release/tc build --project examples/strategy --sdk-root . \
  --bin tc-example-strategy --output var/builds/strategy
target/release/tc register \
  --name "Half BTC" \
  --description "Hold half the virtual portfolio in BTC and the rest in cash" \
  --project examples/strategy \
  --executable var/builds/strategy
```

Registration retains the executable, source files, and Cargo.lock and records hashes and optional lineage. The executable is rechecked when preparing a run and resolved within the selected data directory after restore. For other strategies, `--parameters '{"lookback":30}'` supplies immutable inputs through `TC_STRATEGY_SPEC`. Strategy sources stay in their own project; the example here is an integration template.

Submit in the UI or CLI:

```bash
target/release/tc run --strategy STRATEGY_ID --campaign CAMPAIGN_ID --key trial-001
target/release/tc run --strategy STRATEGY_ID --campaign CAMPAIGN_ID --key stress-025 --slippage-bps 25
target/release/tc run --strategy STRATEGY_ID --campaign CAMPAIGN_ID --key validation-001 --validation
```

The service must be running to execute queued jobs. It defaults to two workers, with a bounded queue and process deadlines. Cost stress tests use development only. Fees default to 25 basis points per fill and historical adverse execution allowance to 10 basis points per fill. These are explicit assumptions, not reconstructed historical fees or measured historical spreads. Current minimum notional and quantity precision are conservative assumed rules in the run configuration; historical rule changes are not reconstructed.

Validation summaries expose Sharpe for a new Sharpe campaign, or net CAGR for a legacy campaign. In Run details, the owner can select **View validation details** to open full metrics, curves, benchmark diagnostics and exact trades. This is an audited reveal; development agents remain blocked. Ten unique queries are allowed globally per data directory, including all campaigns and strategies. Repeated identical queries reuse the original evaluation. Budget extensions remain local owner actions; the CLI also supports explicit owner inspection:

```bash
target/release/tc budget
target/release/tc budget --extend 5
target/release/tc inspect JOB_ID --reveal-validation --output /tmp/validation-report.json
```

Owner inspection marks the ledger revealed and does not run the strategy again or spend an additional validation query. The private owner account remains trusted; development agents use the isolated account and scoped API. Development reports can be exported without the reveal flag. The exported report contains full equity, exact fills, monthly returns, and the pinned simulation configuration.

## Paper trading and recovery

Select Paper trading in the UI and start a fixed strategy version. Each session has independent EUR 10,000 virtual capital. Public closed candles are polled every ten seconds, and fresh REST bid/ask observations are used for small simulated full fills. Paper quote polling is the current implementation; exchange WebSocket ingestion and depth-aware execution are later improvements. The UI receives SSE snapshots.

A new session shows **starting**, connects to the feed, warms up its indicators, then becomes **running**. Its activity message, virtual equity, and update time distinguish a live session awaiting a signal from a failed session. There is at most one active session per strategy version; another start returns that existing session. A new independent session can be started after the previous one stops.

Paper decisions do not execute retrospectively after downtime. Startup replays recorded causal frames to restore strategy state and resumes with fresh observations. Ordered frames, exact fills, and the latest accounting snapshot commit atomically in SQLite. Portfolio curves and fill markers in the live view are bounded; the journal retains exact historical fills. If offline for over a day, or a feed/model/strategy fails, the session halts visibly for inspection. It does not quietly trade on incomplete recovery data.

The drawdown criterion is 15% from portfolio peak equity, including unrealized value and modeled exit costs. A breach stops new risk and attempts liquidation at the next executable observation. Gaps, absent depth, and costs can exceed 15%. Historical minute data cannot certify intraminute drawdown. A paper session's stop control stops evaluation; it does not place exchange orders or erase its journal.

Stop changes an active worker to **stopping**, cancels warm-up/replay and polling, and becomes **stopped** when the worker exits. An in-flight feed request can take up to its five-second timeout. Stopped, halted, and stopping sessions have disabled stop controls. Portfolio checkpoints and the event journal remain available after stopping.

Service shutdown closes browser event streams, cancels workers, and leaves active paper sessions recoverable on the next start. Explicitly stopped sessions stay stopped, including when shutdown happens while a stop is pending. Linux process leases and groups clean up worker and external paper processes after cancellation or a server restart. Process separation is crash handling, not a security or resource sandbox.

## Backup and restore

```bash
target/release/tc backup /absolute/path/to/new-backup
```

The backup uses SQLite's consistent backup API and copies datasets, registered source/executable artifacts, historical reports, candidate requests and pinned SDK platforms. It excludes raw download caches, which are not necessary to reproduce a published snapshot. Runtime PID ownership/leases are removed from the backup. Use a new destination. Restore by starting against that directory:

```bash
target/release/tc --data-dir /absolute/path/to/new-backup serve --bind 127.0.0.1:8081
```

Interrupted historical jobs and active candidate builds need a new submission/idempotency key. Queued candidates remain queued. Use the reviewed SDK root when starting a restored service. Published dataset files are verified by checksum when loaded. Preserve the backup as a unit: a database without its referenced immutable files is incomplete. Confirm a restored external strategy can be loaded before relying on a backup.


## Reproduce throughput measurements

```bash
cargo build --locked --release -p tc-cli --example benchmark
python3 scripts/benchmark.py var DATASET_ID
```

The benchmark runs a 20/100 moving-average fixture across the complete snapshot, with the drawdown halt disabled so it exercises the whole window. It emits timing/row counts only, withholding financial metrics and curves. It does not create a development/validation result or consume validation feedback. Compilation and initial import are excluded. Report results with the dataset, hardware, and whether data is synthetic; a measured fixture is not a latency guarantee for arbitrary external strategies or model calls.

## Owner-approved paper promotion

The Paper trading page uses the searchable strategy/version picker and selects the best qualifying Sharpe candidate initially. **Approve paper session** explicitly approves the selected immutable version and starts an independent EUR 10,000 paper session. The backend requires accepted, non-synthetic Bitvavo data, development risk eligibility, a defined Sharpe/CAGR, and observed drawdown at most 15%. The 15% growth objective is an aspiration, not a rejection threshold. A scalar validation score does not certify validation risk.

API owners send `{"strategy":"ID","job":"DEVELOPMENT_JOB_ID","approve":true}` to `/api/paper`. Approval, session creation and its audit entry are committed together; repeated starts retain the existing active portfolio. The `paper_approvals` overview entries retain the basis, Sharpe/drawdown/CAGR, growth-target status, approved job/version and owner time. Old unapproved sessions stop on service startup; historical journals remain. Existing simulation risk halts still apply and are not a guaranteed loss ceiling.

`python3 scripts/security-smoke.py --agent-account tc-agent` verifies real owner login, scoped agent requests/revocation, an actual separate UID, host file denial, and network isolation with the development Unix socket. Browser and live-paper smoke fixtures explicitly mock native provenance/eligibility to exercise approval; their data is not market evidence.

## Chronological evaluation and sealed final test

Owner CLI `tc walk-forward --campaign CAMPAIGN --candidates VERSION_A VERSION_B --folds 3` runs expanding training selection entirely inside development. The first half trains; the remainder is split into adjacent forward folds. Each fold selects the highest eligible training Sharpe from the frozen candidate list, breaking ties by version ID, then evaluates that version on the next interval with fresh capital and strategy state. No qualifying training candidate means a skipped test, not a substituted winner. Run configuration uses the documented frozen defaults; no validation feedback is spent. Results and scope/model-cache identities are durable; retrieve an earlier report with `tc walk-forward-report RUN_ID`. This is exploratory robustness analysis because authors may already know the development dates; it does not establish out-of-sample predictive ability. Do not average fold Sharpe values.

`tc diagnostics` reports all retained development job attempts, distinct evaluated versions, walk-forward attempts, the validation ledger and final-test state. Counts include failures/repeats, do not observe external research, and are not independent-trial estimates or adjusted significance.

The final test remains sealed until the owner deliberately executes a prepared plan:

```bash
# Metadata only: freeze the latest qualifying native development candidate.
target/release/tc final-test prepare DEVELOPMENT_JOB_ID
# Explicit, irreversible consumption: run only when strategy selection is finished.
target/release/tc final-test execute PLAN_ID --consume-final-test
target/release/tc final-test status
target/release/tc final-test report --output final-report.json
```

Preparation freezes the immutable version/executable, campaign/dataset, August–October window, costs/risk settings, evaluation binary hash and inference-configuration hash. It reads no reserved candles and consumes no feedback. Model configuration hashes do not pin mutable external model weights or establish training cutoffs. Execution verifies frozen identities, records consumption atomically before loading data, and permits one run globally per retained database, including failures and interrupted runs. There is no reset/retry endpoint. An interrupted `running` record is consumed; inspect owner diagnostics rather than rerunning. Backups retain consumption and final reports; rolling back an older backup cannot restore scientific untouchedness. The owner can export the final report; agents, normal jobs, overviews and leaderboards never receive it. There is no HTTP execution route.

No real final evaluation has been executed as part of implementing or testing these controls. Test fixtures use generated prices and metadata-only failure plans, never actual final-test observations.

## Free Kraken comparison data

`python3 scripts/kraken-download.py var/kraken-research-csv` fetches only the five EUR one-minute entries from Kraken's official split archive using byte ranges. It checks stable archive ETags, exact ranges and ZIP CRC, saves CSV hashes and source metadata, and retains dates before August 2026 only. It does not download or verify the full multi-gigabyte archive checksum. Failed downloads leave a partial directory for inspection; use a new destination when retrying. `--quarter 2026Q1` or `2026Q2` can fetch a smaller research subset with matching `--start`/`--end`.

Import using `target/release/tc import-kraken var/kraken-research-csv`. The manifest includes the distinct Kraken venue, provenance hashes, coverage and warnings. Accepted snapshots spanning the complete research interval use the same November 2022/November 2025/August 2026 split as the native campaign, without claiming final-test coverage. The downloaded subset begins in November 2022 and has no preceding warmup day. One-minute alignment, seven numeric columns, positive traded volume, monotonic timestamps and all five files are required. No zero filling or Bitvavo price replacement occurs. Default execution assumptions remain Bitvavo assumptions; comparative price robustness is not calibrated Kraken execution performance. Kraken data cannot qualify a native Bitvavo paper/final-test promotion. Campaign/comparison menus display the venue; the initial selection prefers the primary Bitvavo Sharpe campaign while preserving later manual choices.

The imported 2022-11-01–2026-08-01 snapshot contains 7,919,484 candles. It fails strict coverage with 411–419-minute gaps on 14 April 2024 and was explicitly accepted by the owner for research-only comparison on 4 November 2026. The strict failed snapshot and a new approved immutable snapshot are both retained. The [metadata-only audit](v2-kraken-audit.json) records campaign creation before any Kraken evaluations. The separate Kraken approval does not alter Bitvavo data or waive valuation/risk eligibility.

### Kraken campaign in the persistent service

On 5 November 2026, the original accepted Kraken snapshot was copied from the retained research worktree (`/home/lucas/.t3/worktrees/tradeclimb/t3-42665a60/var/`) into `~/.local/share/tradeclimb/datasets/`. It had been absent from the persistent installation. The complete manifest matched `accepted_manifest` in the audit, and all 225 partition SHA-256 checks passed (7,919,484 candles). The copy was staged outside `datasets/` and renamed into place only after verification. The campaign was then recreated through the installed CLI and both objects were verified through the running service's authenticated `/api/overview`. No service restart or evaluation was needed.

Select **Kraken 3a1a4d25** in the **Evaluation campaign** dropdown. The primary Bitvavo campaign remains the initial default.

| Field | Value |
| --- | --- |
| Dataset | `3a1a4d25916723b1604533a1227a7372363131739d132b38834aa4e603a243f1` |
| Campaign | `9a9e4cb9d8ea68e845705a5cccdb29e05b8c54c0e07f00ce986f3383008aed51` |
| Objective | `sharpe_daily_v1` |
| Development | `[2022-11-01, 2025-11-01)` UTC |
| Validation | `[2025-11-01, 2026-08-01)` UTC; existing global owner-controlled budget |
| Final test | None for this snapshot |

To recreate this campaign after restoring the same verified dataset into the persistent data directory:

```bash
~/.local/lib/tradeclimb/current/tc --data-dir ~/.local/share/tradeclimb \
  campaign 3a1a4d25916723b1604533a1227a7372363131739d132b38834aa4e603a243f1 \
  --objective sharpe_daily_v1 --target-cagr 0.15
```

This command is idempotent for these inputs and reproduces the audited campaign identity. It does not copy dataset files or run evaluations. The committed audit contains metadata only; preserve the persistent dataset through the normal complete-directory backups rather than relying on the old worktree.

## Forward execution evidence

`python3 scripts/record-books.py var/books-SESSION --seconds 3600 --interval 5` records bounded, read-only Bitvavo depth snapshots for all five markets. Each JSONL event retains original response JSON/hash, server nanosecond timestamp/nonce, local send/receive times, monotonic request duration and explicit errors/rate-limit pauses. The closed recording has a checksum manifest and defaults to a 1-GiB/one-day maximum. A fresh destination prevents accidental overwrite. Ctrl+C seals the captured recording; a hard crash leaves a partial JSONL file without a completed manifest.

These are independent REST snapshots; gaps between polls cannot reconstruct a continuous book or queue position. The recorder makes no fills or cost estimates. Use representative sessions before calibrating participation, partial fills, latency and residual impact. The existing short `cost-audit.py` remains a displayed-depth sanity check. Current engine fee rounding and market precision, candle full-fill assumptions, paper top-level liquidity, limit-order/queue simulation and historical universes remain explicit follow-up work; those financial semantics have not been changed by adding data collection.

## Research charts and comparisons

Buy-and-hold overlays are solid, partially transparent lines with stable reference colors. Trade markers are omitted for runs with more than 100 visible-window fills or dense clusters; the chart explains what is hidden. A shorter chart window can restore sparse markers. Exact trades remain available through the authenticated link. Maximum drawdown and eligibility are visible in run details.

The full-run comparison table shows each reference's Sharpe, the strategy-minus-reference Sharpe difference, information ratio and return difference in percentage points. Keep Sharpe as the main risk-adjusted ranking and use IR to assess consistent performance above the fixed five-asset basket; BTC is a secondary reference. An undefined comparison remains —, especially when complete daily observations are missing. Hiding a reference curve or changing chart windows does not change scoring. Existing recorded results are not reranked.

Verification commands and temporary-fixture suites are listed in [development](development.md).
