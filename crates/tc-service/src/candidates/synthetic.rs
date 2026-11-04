//! Bounded, declarative behavior checks using the production engine and sandbox.
use super::*;
use rust_decimal::Decimal;
use tc_sdk::Strategy;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Suite {
    pub version: u32,
    pub scenarios: Vec<Scenario>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub name: String,
    pub config: RunConfig,
    pub candles: Vec<Candle>,
    pub assertions: Vec<Assertion>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Assertion {
    NoIntents {
        start: i64,
        end: i64,
    },
    Intent {
        time: i64,
        market: String,
        target_weight: Option<Decimal>,
    },
    FramePresent {
        time: i64,
        present: bool,
    },
    FillCount {
        start: i64,
        end: i64,
        market: String,
        side: Option<Side>,
        count: usize,
    },
    PortfolioWeight {
        time: i64,
        market: String,
        weight: Decimal,
        tolerance: Decimal,
    },
}
pub fn capabilities() -> Value {
    json!({
        "version":1,"max_scenarios":8,"max_candles_per_scenario":512,"max_candles_total":2048,
        "max_assertions_per_scenario":128,"max_assertions_total":256,"max_bytes":524288,
        "max_span_ms":86400000,"max_name_bytes":100,"max_fill_count":2560,
        "numeric_bounds":{"price_min":"0.000001","price_max":"1000000000","volume_max":"1000000000","capital_max":"1000000000","weights_tolerances":"0..1"},
        "schema":{
            "suite":{"version":"integer: 1","scenarios":"array of scenario; nonempty"},
            "scenario":{"name":"unique UTF-8 string","config":"complete RunConfig from kit fixtures","candles":"array of Candle from kit fixtures; sorted by (time,market)","assertions":"nonempty array of tagged assertions"},
            "assertions":{
                "no_intents":{"start":"UTC ms inclusive","end":"UTC ms exclusive; requires at least one frame"},
                "intent":{"time":"frame UTC ms","market":"strategy-declared market","target_weight":"decimal string or null for no intent"},
                "frame_present":{"time":"frame UTC ms","present":"boolean"},
                "fill_count":{"start":"fill UTC ms inclusive","end":"fill UTC ms exclusive","market":"strategy-declared market","side":"buy, sell, or null","count":"nonnegative integer"},
                "portfolio_weight":{"time":"frame UTC ms","market":"strategy-declared market","weight":"decimal string","tolerance":"absolute decimal string"}
            },"assertion_tag":"kind","unknown_suite_scenario_assertion_fields":"rejected"
        },
        "timing":"decision/portfolio assertions use frame.time (bar open + 60000); fill windows use fill.time; windows are [start,end); excludes terminal liquidation at config.end",
        "replay_runs":2,"guide":"synthetic-tests.md","performance_evidence":false
    })
}
impl Suite {
    pub fn validate(&self, markets: &[String]) -> Result<()> {
        ensure!(self.version == 1, "unsupported synthetic suite version");
        ensure!(
            !self.scenarios.is_empty() && self.scenarios.len() <= 8,
            "require 1..8 synthetic scenarios"
        );
        ensure!(
            serde_json::to_vec(self)?.len() <= 524288,
            "synthetic suite exceeds 512 KiB"
        );
        let mut names = std::collections::BTreeSet::new();
        let mut candles = 0;
        let mut assertions = 0;
        for s in &self.scenarios {
            ensure!(
                !s.name.is_empty() && s.name.len() <= 100 && names.insert(&s.name),
                "scenario names must be unique and contain 1..100 bytes"
            );
            s.config.validate()?;
            ensure!(
                s.config.capital <= Decimal::from(1_000_000_000u64),
                "synthetic capital exceeds 1 billion"
            );
            ensure!(
                s.config.rules.keys().all(|m| MARKETS.contains(&m.as_str())),
                "unknown scenario market rule"
            );
            ensure!(
                s.candles.len() >= 2 && s.candles.len() <= 512,
                "require 2..512 candles per scenario"
            );
            ensure!(
                !s.assertions.is_empty() && s.assertions.len() <= 128,
                "require 1..128 assertions per scenario"
            );
            let first = s.candles[0].time;
            ensure!(
                first >= 0 && first <= s.config.start && s.config.end - first <= 86400000,
                "scenario including warmup must span at most one day"
            );
            let mut previous = None;
            for c in &s.candles {
                c.validate()?;
                ensure!(
                    c.low >= Decimal::new(1, 6)
                        && c.high <= Decimal::from(1_000_000_000u64)
                        && c.volume <= Decimal::from(1_000_000_000u64),
                    "synthetic price/volume exceeds numeric bounds"
                );
                ensure!(
                    markets.contains(&c.market) && s.config.rules.contains_key(&c.market),
                    "candle market must be declared by strategy and config"
                );
                ensure!(
                    c.time >= first && c.time < s.config.end,
                    "candle outside scenario"
                );
                let key = (c.time, c.market.as_str());
                ensure!(
                    previous.is_none_or(|p| p < key),
                    "candles must be ordered by time then market without duplicates"
                );
                previous = Some(key);
            }
            ensure!(
                s.candles.iter().any(|c| c.time >= s.config.start),
                "scenario needs evaluation bars"
            );
            for a in &s.assertions {
                let (start, end, market) = match a {
                    Assertion::NoIntents { start, end } => (*start, *end, None),
                    Assertion::FillCount {
                        start,
                        end,
                        market,
                        count,
                        ..
                    } => {
                        ensure!(*count <= 2560, "fill count exceeds scenario limit");
                        (*start, *end, Some(market))
                    }
                    Assertion::Intent {
                        time,
                        market,
                        target_weight,
                    } => {
                        ensure!(
                            target_weight
                                .is_none_or(|w| (Decimal::ZERO..=Decimal::ONE).contains(&w)),
                            "invalid target weight"
                        );
                        (*time, *time, Some(market))
                    }
                    Assertion::FramePresent { time, .. } => (*time, *time, None),
                    Assertion::PortfolioWeight {
                        time,
                        market,
                        weight,
                        tolerance,
                    } => {
                        ensure!(
                            (Decimal::ZERO..=Decimal::ONE).contains(weight)
                                && (Decimal::ZERO..=Decimal::ONE).contains(tolerance),
                            "invalid portfolio weight/tolerance"
                        );
                        (*time, *time, Some(market))
                    }
                };
                ensure!(
                    start >= first && end >= start && end <= s.config.end,
                    "assertion outside scenario"
                );
                if matches!(a, Assertion::NoIntents { .. } | Assertion::FillCount { .. }) {
                    ensure!(end > start, "assertion window must be nonempty");
                }
                ensure!(
                    market.is_none_or(|m| markets.contains(m)),
                    "assertion market not declared by strategy"
                );
            }
            candles += s.candles.len();
            assertions += s.assertions.len();
        }
        ensure!(
            candles <= 2048 && assertions <= 256,
            "synthetic suite exceeds total candle/assertion limits"
        );
        Ok(())
    }
}
#[derive(Serialize)]
struct Observation {
    time: i64,
    warmup: bool,
    intents: Vec<Intent>,
    portfolio: Portfolio,
    fills: Vec<Fill>,
    error: Option<String>,
}
fn observe(s: &Scenario, strategy: &mut dyn Strategy) -> (Vec<Observation>, Option<String>) {
    let mut observations = vec![];
    let mut seen = 0;
    // Untrusted synthetic price paths can overflow decimal accounting after compounding.
    // Keep that failure in the candidate evidence rather than losing the worker.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tc_engine::backtest_with_observer(
            &s.candles,
            s.config.clone(),
            strategy,
            |_, _, _| Ok(()),
            |f, intents, sim, error| {
                observations.push(Observation {
                    time: f.time,
                    warmup: f.warmup,
                    intents: intents.to_vec(),
                    portfolio: sim.portfolio.clone(),
                    fills: sim.fills[seen..].to_vec(),
                    error: error.map(|e| e.chars().take(2000).collect()),
                });
                seen = sim.fills.len();
                Ok(())
            },
        )
    }))
    .unwrap_or_else(|_| {
        Err(anyhow::anyhow!(
            "engine panicked while evaluating synthetic scenario"
        ))
    });
    (
        observations,
        result
            .err()
            .map(|e| e.to_string().chars().take(2000).collect()),
    )
}
fn check(a: &Assertion, observations: &[Observation]) -> Value {
    let frame = |time| observations.iter().find(|o| o.time == time);
    let (expected, observed, passed) = match a {
        Assertion::NoIntents { start, end } => {
            let frames: Vec<_> = observations
                .iter()
                .filter(|o| o.time >= *start && o.time < *end)
                .collect();
            let count: usize = frames.iter().map(|o| o.intents.len()).sum();
            (
                json!({"intents":0,"requires_frame":true}),
                json!({"intents":count,"frames":frames.len()}),
                !frames.is_empty() && count == 0,
            )
        }
        Assertion::FramePresent { time, present } => (
            json!(present),
            json!(frame(*time).is_some()),
            frame(*time).is_some() == *present,
        ),
        Assertion::Intent {
            time,
            market,
            target_weight,
        } => {
            let found = frame(*time);
            let actual = found
                .and_then(|o| o.intents.iter().find(|i| i.market == *market))
                .map(|i| i.target_weight);
            (
                json!(target_weight),
                json!(actual),
                found.is_some() && actual == *target_weight,
            )
        }
        Assertion::FillCount {
            start,
            end,
            market,
            side,
            count,
        } => {
            let actual = observations
                .iter()
                .flat_map(|o| &o.fills)
                .filter(|f| {
                    f.time >= *start
                        && f.time < *end
                        && f.market == *market
                        && side.as_ref().is_none_or(|s| *s == f.side)
                })
                .count();
            (json!(count), json!(actual), actual == *count)
        }
        Assertion::PortfolioWeight {
            time,
            market,
            weight,
            tolerance,
        } => {
            let actual = frame(*time).and_then(|o| {
                let total = o
                    .portfolio
                    .positions
                    .values()
                    .try_fold(o.portfolio.cash, |sum, p| {
                        sum.checked_add(p.quantity.checked_mul(p.price)?)
                    })?;
                let value = match o.portfolio.positions.get(market) {
                    Some(p) => p.quantity.checked_mul(p.price)?,
                    None => Decimal::ZERO,
                };
                if total > Decimal::ZERO {
                    value.checked_div(total)
                } else {
                    None
                }
            });
            (
                json!({"weight":weight,"tolerance":tolerance}),
                json!(actual),
                actual.is_some_and(|v| (v - weight).abs() <= *tolerance),
            )
        }
    };
    json!({"assertion":a,"passed":passed,"expected":expected,"observed":observed})
}
pub fn run(
    store: &Store,
    spec: &StrategySpec,
    fixtures: &BTreeMap<String, String>,
    suite: &Suite,
) -> Result<Value> {
    let mut cases = vec![];
    for s in &suite.scenarios {
        let mut runs = vec![];
        for _ in 0..2 {
            let run = (|| -> Result<_> {
                let broker = broker::ModelBroker::fixtures(store.clone(), fixtures.clone())?;
                let StrategyKind::Executable { path } = &spec.kind else {
                    anyhow::bail!("external candidate required")
                };
                let mut strategy =
                    tc_engine::ProcessStrategy::spawn_with_broker(path, spec, Some(broker.path()))?;
                Ok(observe(s, &mut strategy))
            })();
            runs.push(
                run.unwrap_or_else(|e| (vec![], Some(e.to_string().chars().take(2000).collect()))),
            );
        }
        let assertions: Vec<_> = s.assertions.iter().map(|a| check(a, &runs[0].0)).collect();
        let hashes: Vec<_> = runs
            .iter()
            .map(|r| serde_json::to_vec(r).map(|bytes| digest(&bytes)))
            .collect::<std::result::Result<_, _>>()?;
        let deterministic = hashes[0] == hashes[1];
        let passed = deterministic
            && runs.iter().all(|r| r.1.is_none())
            && assertions.iter().all(|a| a["passed"] == true);
        cases.push(json!({"name":s.name,"passed":passed,"deterministic":deterministic,"observation_hash":hashes[0],"assertions":assertions,"errors":[runs[0].1,runs[1].1]}));
    }
    Ok(json!({"passed":cases.iter().all(|c| c["passed"] == true),"cases":cases}))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn scenario() -> Scenario {
        Scenario {
            name: "momentum".into(),
            config: RunConfig::new(MINUTE, 6 * MINUTE),
            candles: [(0, 100), (1, 100), (2, 110), (4, 90), (5, 90)]
                .into_iter()
                .map(|(t, price)| Candle {
                    time: t * MINUTE,
                    market: "BTC-EUR".into(),
                    open: price.into(),
                    high: price.into(),
                    low: price.into(),
                    close: price.into(),
                    volume: 1000.into(),
                })
                .collect(),
            assertions: vec![
                Assertion::NoIntents {
                    start: 0,
                    end: 2 * MINUTE,
                },
                Assertion::Intent {
                    time: 3 * MINUTE,
                    market: "BTC-EUR".into(),
                    target_weight: Some(Decimal::new(5, 1)),
                },
                Assertion::FramePresent {
                    time: 4 * MINUTE,
                    present: false,
                },
                Assertion::Intent {
                    time: 5 * MINUTE,
                    market: "BTC-EUR".into(),
                    target_weight: Some(Decimal::ZERO),
                },
                Assertion::FillCount {
                    start: 4 * MINUTE,
                    end: 5 * MINUTE,
                    market: "BTC-EUR".into(),
                    side: Some(Side::Buy),
                    count: 1,
                },
                Assertion::FillCount {
                    start: 5 * MINUTE,
                    end: 6 * MINUTE,
                    market: "BTC-EUR".into(),
                    side: Some(Side::Sell),
                    count: 1,
                },
                Assertion::PortfolioWeight {
                    time: 5 * MINUTE,
                    market: "BTC-EUR".into(),
                    weight: Decimal::new(5, 1),
                    tolerance: Decimal::new(1, 2),
                },
            ],
        }
    }
    struct Momentum(Option<Decimal>);
    impl Strategy for Momentum {
        fn on_frame(&mut self, f: &Frame) -> Result<Vec<Intent>> {
            let price = f.candles[0].close;
            let previous = self.0.replace(price);
            if f.warmup {
                return Ok(vec![]);
            }
            Ok(match previous {
                Some(p) if price != p => vec![Intent {
                    market: "BTC-EUR".into(),
                    target_weight: if price > p {
                        Decimal::new(5, 1)
                    } else {
                        Decimal::ZERO
                    },
                }],
                _ => vec![],
            })
        }
    }
    #[test]
    fn momentum_entry_exit_warmup_gaps_and_execution() -> Result<()> {
        let s = scenario();
        Suite {
            version: 1,
            scenarios: vec![s.clone()],
        }
        .validate(&["BTC-EUR".into()])?;
        let (observations, error) = observe(&s, &mut Momentum(None));
        assert!(error.is_none(), "{error:?}");
        for a in &s.assertions {
            assert_eq!(check(a, &observations)["passed"], true, "{a:?}");
        }
        // The entry intent precedes any fill; the missing frame is not an empty decision.
        assert_eq!(
            check(
                &Assertion::FillCount {
                    start: 2 * MINUTE,
                    end: 4 * MINUTE,
                    market: "BTC-EUR".into(),
                    side: None,
                    count: 0
                },
                &observations
            )["passed"],
            true
        );
        assert_eq!(
            check(
                &Assertion::Intent {
                    time: 4 * MINUTE,
                    market: "BTC-EUR".into(),
                    target_weight: None
                },
                &observations
            )["passed"],
            false
        );
        assert_eq!(
            check(
                &Assertion::NoIntents {
                    start: 4 * MINUTE,
                    end: 5 * MINUTE
                },
                &observations
            )["passed"],
            false
        );
        assert_eq!(
            check(
                &Assertion::Intent {
                    time: 3 * MINUTE,
                    market: "BTC-EUR".into(),
                    target_weight: Some(Decimal::ONE)
                },
                &observations
            )["observed"],
            "0.5"
        );
        Ok(())
    }
    #[test]
    fn warmup_emissions_and_engine_errors_are_visible() {
        struct Bad;
        impl Strategy for Bad {
            fn on_frame(&mut self, _: &Frame) -> Result<Vec<Intent>> {
                Ok(vec![Intent {
                    market: "BTC-EUR".into(),
                    target_weight: Decimal::from(2),
                }])
            }
        }
        let s = scenario();
        let (observations, error) = observe(&s, &mut Bad);
        assert!(error.is_some());
        assert_eq!(check(&s.assertions[0], &observations)["passed"], false);
        assert!(observations.last().unwrap().error.is_some());
    }
    #[test]
    fn engine_panics_retain_partial_observations() {
        struct Panics;
        impl Strategy for Panics {
            fn on_frame(&mut self, f: &Frame) -> Result<Vec<Intent>> {
                assert!(f.warmup, "simulated arithmetic failure");
                Ok(vec![])
            }
        }
        let (observations, error) = observe(&scenario(), &mut Panics);
        assert_eq!(observations.len(), 1);
        assert!(error.unwrap().contains("engine panicked"));
    }
    #[test]
    fn schema_and_resource_limits() -> Result<()> {
        let valid = Suite {
            version: 1,
            scenarios: vec![scenario()],
        };
        let markets = ["BTC-EUR".into()];
        valid.validate(&markets)?;
        let mut value = serde_json::to_value(&valid)?;
        value["scenarios"][0]["assertions"][0]["typo"] = json!(true);
        assert!(serde_json::from_value::<Suite>(value).is_err());
        for mutation in 0..10 {
            let mut suite = valid.clone();
            match mutation {
                0 => suite.version = 2,
                1 => suite.scenarios = vec![scenario(); 9],
                2 => suite.scenarios[0].candles.reverse(),
                3 => suite.scenarios[0].candles[1].time = 1,
                4 => suite.scenarios[0].assertions.clear(),
                5 => suite.scenarios[0].config.end = 2 * 86400000,
                6 => suite.scenarios[0].candles[0].volume = Decimal::MAX,
                7 => {
                    suite.scenarios[0].assertions = vec![
                        Assertion::NoIntents {
                            start: 0,
                            end: MINUTE
                        };
                        129
                    ]
                }
                8 => {
                    suite.scenarios[0].assertions = vec![Assertion::Intent {
                        time: MINUTE,
                        market: "ETH-EUR".into(),
                        target_weight: None,
                    }]
                }
                _ => {
                    suite.scenarios[0].assertions = vec![Assertion::NoIntents {
                        start: MINUTE,
                        end: MINUTE,
                    }]
                }
            }
            assert!(suite.validate(&markets).is_err(), "mutation {mutation}");
        }
        let mut total = valid.clone();
        total.scenarios = (0..3)
            .map(|i| {
                let mut s = scenario();
                s.name = i.to_string();
                s.assertions = vec![s.assertions[0].clone(); 128];
                s
            })
            .collect();
        assert!(total.validate(&markets).is_err());
        Ok(())
    }
}
