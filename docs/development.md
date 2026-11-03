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
