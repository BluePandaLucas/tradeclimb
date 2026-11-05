# TradeClimb

TradeClimb is a Rust platform for researching crypto strategies and paper trading against public Bitvavo data. It includes a Rust strategy SDK, reproducible backtests, bounded tuning, a strategy history book, and a Leptos web interface. New research campaigns use daily Sharpe for evaluation; existing campaigns retain their CAGR objective. Strategy-generating agent orchestration belongs in a separate project. No real orders or account keys are supported.

## Run locally

Rust/Cargo 1.99.1, rustfmt, Clippy, and the WASM target are pinned in `rust-toolchain.toml`. Browser packaging uses `wasm-bindgen-cli` matching Cargo.lock (currently 0.2.130).

```bash
source "$HOME/.cargo/env"
# Optional: populate explicitly synthetic examples and queue development runs.
cargo run --locked --release --bin tc -- demo
./scripts/start
```

Open **http://127.0.0.1:8080**. Sign in with the private owner key located by `target/release/tc auth status`. API access requires an owner session or a scoped agent credential. The service binds to loopback; use an HTTPS reverse proxy for remote access. External strategies now require the local Linux Bubblewrap/systemd sandbox. Run `target/release/tc sandbox-check` to verify prerequisites; see [isolation setup and limits](docs/operations.md#strategy-isolation).

The interface follows your system theme. Use the header toggle for a remembered light/dark preference, or **System** to follow the OS again. Stop the service with **Ctrl+C**, or type **q** or **exit** and press Enter in its terminal.

For boot persistence and authenticated LAN HTTPS access, use the [user-service deployment](docs/operations.md#persistent-service-and-lan-access). It runs a prebuilt release independently of this checkout and includes backup and health timers.

For a clean real-data research workflow, skip demo and follow the [operations guide](docs/operations.md). Synthetic results are labeled and are not performance evidence.

## Quality checks

```bash
./scripts/check
cargo deny check
# After the release binary and web assets are built:
./scripts/check-sandbox
```

The [development guide](docs/development.md) covers toolchain installation, packaged workflow/security tests and optional browser QA. Agent setup and the source-to-evaluation loop are documented in [the agent guide](docs/agents.md).

## Documentation

Guides describe the code in this commit; release history lives in Git. Dated research, audits and original planning records retain their historical context.

- [Local operations and complete workflow](docs/operations.md)
- [Agent development workflow and author kit](docs/agents.md)
- [API, model capabilities, and external Rust strategies](docs/api.md)
- [Development toolchain setup](docs/development.md)
- [Engine adoption decision](docs/decisions/0001-engine.md)
- [Validation evidence and supported limits](docs/validation.md)
- [Requested improvements backlog](docs/backlog.md)
- [Status and scope](docs/status.md)
- [Research and sources](docs/research.md)
- [Original V1 implementation plan](docs/implementation-plan.md)
- [Original planning self-review](docs/self-review.md)

Returns include configured trading fees and adverse execution costs. A 15% drawdown criterion and 15% annual growth target are research settings; neither establishes future profitability or guarantees a loss ceiling.
