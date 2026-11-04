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

Open **http://127.0.0.1:8080**. This development checkout also has an isolated prepared demo: run `TC_DATA_DIR=var/demo ./scripts/start` to inspect it without mixing synthetic runs into the main research directory. Sign in with the private owner key located by `target/release/tc auth status`. API access requires an owner session or a scoped agent credential. The service binds to loopback; use an HTTPS reverse proxy for remote access. External strategies now require the local Linux Bubblewrap/systemd sandbox. Run `target/release/tc sandbox-check` to verify prerequisites; see [isolation setup and limits](docs/operations.md#strategy-isolation).

The interface follows your system theme. Use the header toggle for a remembered light/dark preference, or **System** to follow the OS again. Stop the service with **Ctrl+C**, or type **q** or **exit** and press Enter in its terminal.

For a clean real-data research workflow, skip demo and follow the [operations guide](docs/operations.md). Synthetic results are labeled and are not performance evidence.

## Quality checks

```bash
./scripts/check
cargo deny check
# After the release binary and web assets are built:
./scripts/check-sandbox
```

Checks cover formatting, native and WASM Clippy with warnings denied, domain/protocol/persistence tests, documentation tests, and the release browser build. Dependency checks run separately in CI. The one narrow advisory exception is Leptos's transitive, unmaintained `paste` procedural macro; its reason is recorded in `deny.toml`.

Build the example and release executable, then exercise the real CLI/API/worker workflow:

```bash
cargo build --locked --release --bin tc
cargo build --manifest-path examples/strategy/Cargo.toml --release --locked
./scripts/build-web
python3 scripts/smoke.py
# Also observe public market data, paper fills, and recovery across a restart:
python3 scripts/smoke.py --live
# Check terminal exit controls with a live event stream connected (Linux):
python3 scripts/shutdown-smoke.py
```

## Documentation

- [Local operations and complete workflow](docs/operations.md)
- [API, model capabilities, and external Rust strategies](docs/api.md)
- [Development toolchain setup](docs/development.md)
- [Engine adoption decision](docs/decisions/0001-engine.md)
- [Validation evidence and supported limits](docs/validation.md)
- [Requested improvements backlog](docs/backlog.md)
- [V2 priorities](docs/v2.md)
- [Research and sources](docs/research.md)
- [Accepted implementation plan](docs/implementation-plan.md)
- [Critical planning self-review](docs/self-review.md)

Returns include configured trading fees and adverse execution costs. A 15% drawdown criterion and 15% annual growth target are research settings; neither establishes future profitability or guarantees a loss ceiling.
