# Development

The repository pins Rust 1.99.1 in `rust-toolchain.toml`. Install rustup using the [official instructions](https://rust-lang.org/install), then install the pinned components:

```bash
rustup toolchain install 1.99.1 --profile minimal \
  --component rustfmt --component clippy --target wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130 --locked
cargo install cargo-deny --locked
```

Native builds need a C compiler/linker. Browser packaging requires the wasm-bindgen CLI version in Cargo.lock. External strategy tests additionally require the [Linux sandbox setup](operations.md#strategy-isolation).

## Checks

```bash
./scripts/check
cargo deny check
cargo build --locked --release --bin tc
cargo build --manifest-path examples/strategy/Cargo.toml --release --locked
./scripts/build-web
python3 scripts/smoke.py
python3 scripts/evaluation-smoke.py
python3 scripts/kraken-smoke.py
./scripts/check-sandbox
python3 scripts/security-smoke.py --agent-account tc-agent
python3 scripts/agent-smoke.py --agent-account tc-agent
python3 scripts/shutdown-smoke.py
```

`check` covers formatting, native/WASM linting, portable tests and release browser compilation. `check-sandbox` runs adversarial confinement/resource tests. Provision the locked agent account with `scripts/setup-agent TEMP_DATA_DIRECTORY` before separate-account checks; do not rotate a live research credential for QA. Without `--agent-account`, the two Python suites still test scoped API behavior but omit the separate-UID launcher check.

The agent suite uses a temporary synthetic workspace to test real offline candidate builds, deterministic preflight, development data/traces, diagnostics, lineage, cancellation, revocation and restart. All smoke suites use disposable ledgers. `smoke.py --live` additionally observes public paper quotes and recovery without real orders. Live checks require network access and can depend on exchange availability.

## Browser QA

Optional Playwright tooling is isolated from application dependencies:

```bash
python3 -m venv .venv-qa
.venv-qa/bin/pip install playwright
.venv-qa/bin/playwright install chromium
.venv-qa/bin/python scripts/browser-smoke.py
.venv-qa/bin/python scripts/browser-backlog.py
.venv-qa/bin/python scripts/browser-research.py
```

Build the release CLI and browser assets first. The suites cover desktop/mobile controls, theme/streaming, strategy selection, thousands of history rows, benchmark charts and explicit owner validation disclosure. Each starts and stops its own synthetic service.

For manual backlog inspection, `python3 scripts/ui-demo.py` creates a fresh synthetic `var/ui-backlog` directory. Start it with `target/release/tc --data-dir var/ui-backlog serve --bind 127.0.0.1:8088 --assets dist`. Sign in using that directory's owner key (`tc --data-dir var/ui-backlog auth status`). Seeded extra rows have no fabricated performance results.

The browser keeps strategy and lightweight run indexes in memory and bounds DOM rendering with pagination. Workspaces far larger than the tested thousands will need server-side search/pagination.
