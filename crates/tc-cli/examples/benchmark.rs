//! Throughput measurement only: no financial metrics or curves are emitted.
use anyhow::{Context, Result};
use std::{path::Path, time::Instant};
use tc_core::*;
fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let root = args
        .next()
        .context("usage: benchmark DATA_DIRECTORY DATASET_ID")?;
    let id = args.next().context("dataset id required")?;
    let started = Instant::now();
    let (manifest, rows) = tc_data::load(Path::new(&root), &id)?;
    let load = started.elapsed();
    let mut config = RunConfig::new(manifest.start, manifest.end);
    // Run through the entire window rather than stop at the research risk limit.
    config.max_drawdown = 1.0;
    let spec = StrategySpec {
        parameters: Default::default(),
        name: "Throughput fixture".into(),
        description: "Performance exercise, no financial feedback".into(),
        kind: StrategyKind::MovingAverage {
            fast: 20,
            slow: 100,
        },
        markets: MARKETS.iter().map(|m| (*m).into()).collect(),
    };
    let mut strategy = tc_sdk::Builtin::new(&spec)?;
    let simulation = Instant::now();
    let _report = tc_engine::backtest(&rows, config, &mut strategy, |_, _, _| Ok(()))?;
    let elapsed = simulation.elapsed();
    println!(
        "{}",
        serde_json::json!({"dataset":id,"synthetic":manifest.synthetic,"rows":rows.len(),"load_seconds":load.as_secs_f64(),"simulation_seconds":elapsed.as_secs_f64(),"total_seconds":started.elapsed().as_secs_f64(),"candles_per_second":rows.len()as f64/elapsed.as_secs_f64(),"financial_feedback":"withheld"})
    );
    Ok(())
}
