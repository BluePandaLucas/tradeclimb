# TradeClimb development setup

Use Rust 1.99.1 through rustup. This is the stable patch release published on 15 October 2026; the exact version is pinned in the root `rust-toolchain.toml`. Cargo ships with the toolchain and does not require a separate installation. Source: [Rust release announcement](https://blog.rust-lang.org/2026/10/15/Rust-1.99.1/).

For a fresh Linux installation, use the official installer:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/tradeclimb-rustup-init.sh
sh /tmp/tradeclimb-rustup-init.sh -y --profile minimal --default-toolchain 1.99.1 --component rustfmt --component clippy --target wasm32-unknown-unknown
rm /tmp/tradeclimb-rustup-init.sh
source "$HOME/.cargo/env"
```

If rustup is already installed:

```bash
rustup toolchain install 1.99.1 --profile minimal --component rustfmt --component clippy --target wasm32-unknown-unknown
```

The repository pin selects the toolchain inside this directory without changing an existing user's global default. Verify it from the project root:

```bash
rustc --version
cargo --version
cargo fmt --version
cargo clippy --version
rustup target list --installed
```

The WASM target supports the planned Rust browser UI. Native builds also require a C compiler/linker; dependencies may later require additional system development packages. The current machine has `cc` and `pkg-config` available.

The Cargo workspace and shared checks are established. Run `./scripts/check` for native/WASM formatting, linting, tests, and browser release compilation. Run `cargo deny check` for advisory/license/dependency checks. Install its checker with `cargo install cargo-deny --locked`. Browser packaging additionally requires `cargo install wasm-bindgen-cli --version 0.2.130 --locked`; the version must match Cargo.lock. Source: [official installation guidance](https://rust-lang.org/install).


Optional browser QA uses an isolated Python environment and does not add Node or Python dependencies to the application:

```bash
python3 -m venv .venv-qa
.venv-qa/bin/pip install playwright
.venv-qa/bin/playwright install chromium
.venv-qa/bin/python scripts/browser-smoke.py
```

Build the release CLI and browser assets first. The check runs Chromium headlessly against a temporary synthetic workspace, verifies streaming controls/charts/navigation and mobile width, and stops its server. No exchange orders are sent.


For a large, isolated UI demo (4,800 additional versions, 2,000 navigation-only backtests, and 240 tuning runs):

```bash
./scripts/build-web
cargo build --locked --release --bin tc
python3 scripts/ui-demo.py
./target/release/tc --data-dir var/ui-backlog serve --bind 127.0.0.1:8088 --assets dist
```

The V2 interface requires owner sign-in; locate the private key with `target/release/tc --data-dir var/ui-backlog auth status`. The seed command requires a fresh directory and never overwrites an existing workspace. All data is synthetic; backlog rows have no performance results. The original three demo evaluations run normally and provide inspectable charts. Open the strategy picker to search families and exact versions. New backtests wait behind “Show new runs” so the current page stays in place. Run `scripts/browser-backlog.py` with the optional Playwright environment to check growing backlogs, keyboard focus, old-run search, both themes, and mobile layout.

The browser loads a lightweight run index once and keeps the live feed bounded to 50 runs. DOM pagination bounds rendering; the strategy and run indexes remain in browser memory, so very large workspaces beyond the tested thousands will eventually need server-side search/pagination.

`python scripts/browser-research.py` in the Playwright environment verifies solid translucent references, dense-marker suppression, benchmark diagnostics, explicit owner validation disclosure, unchanged query usage, agent restrictions after disclosure, and archived/empty picker states using temporary synthetic fixtures.
