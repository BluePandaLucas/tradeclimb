//! Fixed synthetic demo: five-market 20/100 moving-average crossover.
//! Keep this workload unchanged when comparing engine revisions.
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::time::Instant;
use tc_core::*;

fn main() -> Result<()> {
    let minutes: usize = std::env::args().nth(1).unwrap_or("100000".into()).parse()?;
    let repetitions: usize = std::env::args().nth(2).unwrap_or("3".into()).parse()?;
    anyhow::ensure!(minutes > 100 && repetitions > 0, "workload too small");
    let mut candles = Vec::with_capacity(minutes * MARKETS.len());
    for minute in 0..minutes {
        for (market_index, market) in MARKETS.iter().enumerate() {
            // Deterministic triangular prices exercise buys, sells and cash periods.
            let phase = (minute + market_index * 137) % 2000;
            let price = rust_decimal::Decimal::new(
                10000 + (1000_i64 - (phase as i64 - 1000).abs()) + market_index as i64 * 100,
                2,
            );
            candles.push(Candle {
                market: (*market).into(),
                time: minute as i64 * MINUTE,
                open: price,
                high: price,
                low: price,
                close: price,
                volume: 100.into(),
            });
        }
    }
    let spec = StrategySpec {
        parameters: Default::default(),
        name: "Fixed demo MA 20/100".into(),
        description: "Synthetic performance fixture; not investment evidence".into(),
        kind: StrategyKind::MovingAverage {
            fast: 20,
            slow: 100,
        },
        markets: MARKETS.iter().map(|m| (*m).into()).collect(),
    };
    let mut config = RunConfig::new(100 * MINUTE, minutes as i64 * MINUTE);
    config.max_drawdown = 1.0;
    let mut seconds = 0.0;
    let mut checksum = String::new();
    for _ in 0..repetitions {
        let mut strategy = tc_sdk::Builtin::new(&spec)?;
        let started = Instant::now();
        let report =
            tc_engine::backtest(&candles, config.clone(), &mut strategy, |_, _, _| Ok(()))?;
        seconds += started.elapsed().as_secs_f64();
        let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&report)?));
        anyhow::ensure!(
            checksum.is_empty() || checksum == digest,
            "nondeterministic report"
        );
        checksum = digest;
    }
    println!(
        "{}",
        serde_json::json!({"minutes":minutes,"rows":candles.len(),"repetitions":repetitions,"simulation_seconds":seconds,"report_sha256":checksum,"synthetic":true})
    );
    Ok(())
}
