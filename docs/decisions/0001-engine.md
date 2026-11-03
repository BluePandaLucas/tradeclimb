# Engine choice for the first release

Use the TC candle engine behind the SDK contracts for v1. Keep framework adoption open for later execution models.

NautilusTrader 0.67.0's packaged `engine-ema-cross` native Rust example was copied to a temporary directory and run successfully with Rust 1.99.1. Native strategies and backtests work without a Python strategy layer. Its richer order, account, instrument, and venue abstractions are valuable when execution realism becomes the primary task. Its crate license is LGPL-3.0-only and would require a separate licensing decision before adoption.

Barter 0.14.0's `AlgoStrategy` and engine interfaces were inspected. They operate on engine state and generate open/cancel order requests, while TC v1 needs a deliberately narrower target-weight contract, controlled validation outputs, local artifact registration, and a dataset ledger. Neither framework removes those responsibilities.

The selected scope is spot/cash, market intents, explicit candle assumptions, and simple paper execution. Owning that small accounting surface allows exact hand-calculated tests without exposing another framework in the external SDK. This is not a claim that TC matches either framework's execution coverage. Intrabar stops, order queues, partial fills, and real orders remain outside this engine's supported contract.

Reference exercise: `cargo run --manifest-path /tmp/tradeclimb-nautilus-reference/Cargo.toml --features examples --example engine-ema-cross`. The upstream example remains outside this repository; its code was not incorporated into TC.

References: [Nautilus native backtest](https://github.com/nautechsystems/nautilus_trader/blob/develop/docs/how_to/run_rust_backtest.md), [Barter](https://github.com/barter-rs/barter-rs).
