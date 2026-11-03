//! Deterministic accounting and historical execution. No network or storage here.
use anyhow::{Result, ensure};
use chrono::{DateTime, Datelike, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::{
    collections::BTreeMap,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};
use tc_core::*;
use tc_sdk::Strategy;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pending {
    pub intent: Intent,
    pub created: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Simulator {
    pub config: RunConfig,
    pub portfolio: Portfolio,
    pub pending: Vec<Pending>,
    pub halted: bool,
    pub stale: bool,
    pub peak: Decimal,
    pub max_drawdown: f64,
    pub equity: Vec<EquityPoint>,
    pub fills: Vec<Fill>,
    pub observed: BTreeMap<String, i64>,
}
impl Simulator {
    pub fn new(config: RunConfig) -> Result<Self> {
        config.validate()?;
        let capital = config.capital;
        Ok(Self {
            portfolio: Portfolio::new(capital),
            peak: capital,
            equity: vec![EquityPoint {
                time: config.start,
                equity: number(capital),
                drawdown: 0.0,
            }],
            config,
            pending: vec![],
            halted: false,
            stale: false,
            max_drawdown: 0.0,
            fills: vec![],
            observed: BTreeMap::new(),
        })
    }
    pub fn liquidation_equity(&self) -> Decimal {
        let haircut = Decimal::ONE
            - Decimal::from(self.config.fee_bps + self.config.slippage_bps) / Decimal::from(10_000);
        self.portfolio.cash
            + self
                .portfolio
                .positions
                .values()
                .map(|p| p.quantity * p.price * haircut)
                .sum::<Decimal>()
    }
    fn gross_equity(&self) -> Decimal {
        self.portfolio.cash
            + self
                .portfolio
                .positions
                .values()
                .map(|p| p.quantity * p.price)
                .sum::<Decimal>()
    }
    pub fn queue(&mut self, intents: Vec<Intent>, time: i64) -> Result<()> {
        ensure!(intents.len() <= 5, "too many intents");
        let mut weights = BTreeMap::new();
        for intent in &intents {
            ensure!(
                self.config.rules.contains_key(&intent.market),
                "unknown intent market"
            );
            ensure!(
                intent.target_weight >= Decimal::ZERO && intent.target_weight <= Decimal::ONE,
                "invalid target weight"
            );
            ensure!(
                weights
                    .insert(intent.market.clone(), intent.target_weight)
                    .is_none(),
                "duplicate market intent"
            );
        }
        ensure!(
            weights.values().copied().sum::<Decimal>() <= Decimal::ONE,
            "weights exceed portfolio capital"
        );
        for intent in intents {
            self.pending.retain(|p| p.intent.market != intent.market);
            if !self.halted || intent.target_weight == Decimal::ZERO {
                self.pending.push(Pending {
                    intent,
                    created: time,
                });
            }
        }
        Ok(())
    }
    pub fn execute(
        &mut self,
        market: &str,
        price: Decimal,
        time: i64,
        target: Decimal,
        reason: &str,
    ) -> Result<()> {
        ensure!(price > Decimal::ZERO, "nonpositive execution price");
        let rule = self
            .config
            .rules
            .get(market)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown market rules"))?;
        let held = self
            .portfolio
            .positions
            .get(market)
            .map_or(Decimal::ZERO, |p| p.quantity);
        let desired = if self.halted {
            Decimal::ZERO
        } else {
            self.gross_equity() * target / price
        };
        let delta = desired - held;
        let slip = Decimal::from(self.config.slippage_bps) / Decimal::from(10_000);
        let fee_rate = Decimal::from(self.config.fee_bps) / Decimal::from(10_000);
        let (side, fill_price, quantity) = if delta > Decimal::ZERO {
            let p = price * (Decimal::ONE + slip);
            (
                Side::Buy,
                p,
                delta
                    .min(self.portfolio.cash / (p * (Decimal::ONE + fee_rate)))
                    .round_dp_with_strategy(
                        rule.quantity_decimals,
                        rust_decimal::RoundingStrategy::ToZero,
                    ),
            )
        } else {
            (
                Side::Sell,
                price * (Decimal::ONE - slip),
                (-delta).min(held).round_dp_with_strategy(
                    rule.quantity_decimals,
                    rust_decimal::RoundingStrategy::ToZero,
                ),
            )
        };
        let notional = quantity * fill_price;
        if quantity <= Decimal::ZERO
            || (notional < rule.minimum_eur && reason != "terminal" && reason != "risk")
        {
            return Ok(());
        }
        let fee = (notional * fee_rate)
            .round_dp_with_strategy(8, rust_decimal::RoundingStrategy::AwayFromZero);
        match side {
            Side::Buy => {
                if notional + fee > self.portfolio.cash {
                    return Ok(());
                }
                self.portfolio.cash -= notional + fee;
                let p = self
                    .portfolio
                    .positions
                    .entry(market.into())
                    .or_insert(Position {
                        quantity: Decimal::ZERO,
                        price,
                        observed_at: time,
                    });
                p.quantity += quantity;
                p.price = price;
                p.observed_at = time;
            }
            Side::Sell => {
                self.portfolio.cash += notional - fee;
                if let Some(p) = self.portfolio.positions.get_mut(market) {
                    p.quantity -= quantity;
                    p.price = price;
                    p.observed_at = time;
                }
            }
        }
        self.fills.push(Fill {
            time,
            market: market.into(),
            side,
            quantity,
            price: fill_price,
            fee,
            reason: reason.into(),
        });
        Ok(())
    }
    pub fn open(&mut self, candles: &[Candle], time: i64) -> Result<()> {
        self.stale |= self.portfolio.positions.values().any(|p| {
            p.quantity > Decimal::ZERO && time - p.observed_at > self.config.stale_after_ms
        });
        for c in candles {
            if let Some(p) = self.portfolio.positions.get_mut(&c.market) {
                p.price = c.open;
                p.observed_at = time;
            }
        }
        let mut ready = Vec::new();
        let pending = std::mem::take(&mut self.pending);
        for p in pending {
            if time - p.created > self.config.intent_ttl_ms {
                continue;
            }
            if time >= p.created
                && candles
                    .iter()
                    .any(|c| c.market == p.intent.market && c.volume > Decimal::ZERO)
            {
                ready.push(p);
            } else {
                self.pending.push(p);
            }
        }
        ready.sort_by_key(|p| p.intent.target_weight > Decimal::ZERO);
        for p in ready {
            if let Some(c) = candles.iter().find(|c| c.market == p.intent.market) {
                self.execute(
                    &p.intent.market,
                    c.open,
                    time,
                    p.intent.target_weight,
                    if self.halted { "risk" } else { "signal" },
                )?;
            }
        }
        Ok(())
    }
    pub fn mark(&mut self, candles: &[Candle], time: i64) -> Result<()> {
        for c in candles {
            self.observed.insert(c.market.clone(), time);
            if let Some(p) = self.portfolio.positions.get_mut(&c.market) {
                p.price = c.close;
                p.observed_at = time;
            }
        }
        let stale = self.portfolio.positions.values().any(|p| {
            p.quantity > Decimal::ZERO && time - p.observed_at > self.config.stale_after_ms
        });
        self.stale |= stale;
        let value = self.liquidation_equity();
        self.peak = self.peak.max(value);
        let dd = if self.peak > Decimal::ZERO {
            number((self.peak - value) / self.peak)
        } else {
            1.0
        };
        self.max_drawdown = self.max_drawdown.max(dd);
        self.equity.push(EquityPoint {
            time,
            equity: number(value),
            drawdown: dd,
        });
        if dd >= self.config.max_drawdown && !self.halted {
            self.halted = true;
            self.pending.clear();
            for (m, p) in &self.portfolio.positions {
                if p.quantity > Decimal::ZERO {
                    self.pending.push(Pending {
                        created: time,
                        intent: Intent {
                            market: m.clone(),
                            target_weight: Decimal::ZERO,
                        },
                    });
                }
            }
        }
        // Risk liquidation remains active until a fresh executable observation exists.
        if self.halted {
            for (m, p) in &self.portfolio.positions {
                if p.quantity > Decimal::ZERO && !self.pending.iter().any(|x| x.intent.market == *m)
                {
                    self.pending.push(Pending {
                        created: time,
                        intent: Intent {
                            market: m.clone(),
                            target_weight: Decimal::ZERO,
                        },
                    });
                }
            }
        }
        Ok(())
    }
    pub fn finish(mut self) -> Result<Report> {
        let time = self.config.end;
        let holdings: Vec<_> = self
            .portfolio
            .positions
            .iter()
            .filter(|(_, p)| p.quantity > Decimal::ZERO)
            .map(|(m, p)| (m.clone(), p.price, p.observed_at))
            .collect();
        for (m, price, observed) in holdings {
            if time - observed > self.config.stale_after_ms {
                self.stale = true;
            } else {
                self.execute(&m, price, time, Decimal::ZERO, "terminal")?;
            }
        }
        self.mark(&[], time)?;
        let metrics = metrics(&self)?;
        Ok(Report {
            config: self.config,
            metrics,
            equity: self.equity,
            fills: self.fills,
            halted: self.halted,
            stale: self.stale,
        })
    }
}
pub fn backtest(
    candles: &[Candle],
    config: RunConfig,
    strategy: &mut dyn Strategy,
    mut progress: impl FnMut(usize, usize, &Simulator) -> Result<()>,
) -> Result<Report> {
    let mut sim = Simulator::new(config)?;
    let mut index = 0;
    let mut previous = None;
    while index < candles.len() {
        let time = candles[index].time;
        ensure!(previous.is_none_or(|p| p <= time), "candles not ordered");
        previous = Some(time);
        let end = index + candles[index..].partition_point(|c| c.time == time);
        let group = &candles[index..end];
        for c in group {
            c.validate()?;
        }
        let mut unique = std::collections::BTreeSet::new();
        ensure!(
            group.iter().all(|c| unique.insert(&c.market)),
            "duplicate candle"
        );
        if time >= sim.config.end {
            break;
        }
        let warmup = time < sim.config.start;
        if !warmup {
            sim.open(group, time)?;
            sim.mark(group, time + MINUTE)?;
        }
        let frame = Frame {
            protocol: PROTOCOL,
            time: time + MINUTE,
            candles: group.to_vec(),
            portfolio: sim.portfolio.clone(),
            warmup,
        };
        let intents = strategy.on_frame(&frame)?;
        if !warmup {
            sim.queue(intents, time + MINUTE)?;
        }
        if index % 2_000 < group.len() {
            progress(index, candles.len(), &sim)?;
        }
        index = end;
    }
    ensure!(sim.equity.len() > 1, "no observations in evaluation period");
    progress(candles.len(), candles.len(), &sim)?;
    sim.finish()
}
fn metrics(sim: &Simulator) -> Result<Metrics> {
    let start = number(sim.config.capital);
    let end = sim.equity.last().map_or(start, |p| p.equity);
    let years = (sim.config.end - sim.config.start) as f64 / (365.25 * 86_400_000.0);
    let growth = (end / start).powf(1.0 / years) - 1.0;
    let net_cagr = (end > 0.0 && growth.is_finite()).then_some(growth);
    let mut by_month: BTreeMap<(i32, u32), f64> = BTreeMap::new();
    for p in &sim.equity {
        let dt = DateTime::<Utc>::from_timestamp_millis((p.time - 1).max(sim.config.start))
            .ok_or_else(|| anyhow::anyhow!("invalid equity timestamp"))?;
        by_month.insert((dt.year(), dt.month()), p.equity);
    }
    let mut before = start;
    let mut months = vec![];
    for ((year, month), equity) in by_month {
        let date = chrono::NaiveDate::from_ymd_opt(year, month, 1)
            .ok_or_else(|| anyhow::anyhow!("invalid month"))?;
        let next = if month == 12 {
            chrono::NaiveDate::from_ymd_opt(year + 1, 1, 1)
        } else {
            chrono::NaiveDate::from_ymd_opt(year, month + 1, 1)
        }
        .ok_or_else(|| anyhow::anyhow!("invalid month"))?;
        let begin = date
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| anyhow::anyhow!("invalid date"))?
            .and_utc()
            .timestamp_millis();
        let finish = next
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| anyhow::anyhow!("invalid date"))?
            .and_utc()
            .timestamp_millis();
        months.push(MonthlyReturn {
            month: format!("{year}-{month:02}"),
            net_return: equity / before - 1.0,
            complete: sim.config.start <= begin && sim.config.end >= finish,
        });
        before = equity;
    }
    let complete: Vec<_> = months
        .iter()
        .filter(|m| m.complete)
        .map(|m| m.net_return)
        .collect();
    let mean = (!complete.is_empty()).then(|| complete.iter().sum::<f64>() / complete.len() as f64);
    let geometric = (!complete.is_empty()).then(|| {
        complete
            .iter()
            .map(|v| 1.0 + v)
            .product::<f64>()
            .powf(1.0 / complete.len() as f64)
            - 1.0
    });
    let mut reasons = vec![];
    if sim.max_drawdown > sim.config.max_drawdown {
        reasons.push("drawdown exceeds limit".into());
    }
    if mean.is_none_or(|v| v <= 0.0) {
        reasons.push("positive mean completed-month return not established".into());
    }
    if sim.stale {
        reasons.push("stale valuation observations".into());
    }
    if net_cagr.is_none() {
        reasons.push("CAGR undefined".into());
    }
    let mut warnings = vec![];
    if sim.fills.len() < 20 {
        warnings.push("fewer than 20 fills: sparse evidence".into());
    }
    if years < 1.0 {
        warnings.push("less than one year: CAGR is annualized, not a full observed year".into());
    }
    Ok(Metrics {
        net_cagr,
        net_return: end / start - 1.0,
        max_drawdown: sim.max_drawdown,
        mean_monthly_return: mean,
        geometric_monthly_return: geometric,
        positive_month_fraction: (!complete.is_empty())
            .then(|| complete.iter().filter(|v| **v > 0.0).count() as f64 / complete.len() as f64),
        worst_month: complete.iter().copied().reduce(f64::min),
        total_fees: sim.fills.iter().map(|f| f.fee).sum(),
        turnover: sim
            .fills
            .iter()
            .map(|f| number(f.quantity * f.price))
            .sum::<f64>()
            / start,
        trades: sim.fills.len(),
        eligible: reasons.is_empty(),
        reasons,
        warnings,
        months,
    })
}
/// External trusted code runs in a child; timeout kills the child, never the API.
pub struct ProcessStrategy {
    child: Child,
    request: mpsc::Sender<Frame>,
    response: mpsc::Receiver<Result<Decision>>,
}
impl ProcessStrategy {
    pub fn process_id(&self) -> u32 {
        self.child.id()
    }
    pub fn spawn(path: &str, spec: &StrategySpec) -> Result<Self> {
        Self::spawn_scoped(path, spec, None)
    }
    pub fn spawn_scoped(path: &str, spec: &StrategySpec, scope: Option<&str>) -> Result<Self> {
        let mut command = Command::new(path);
        if let Some(scope) = scope {
            command.env("TC_MODEL_SCOPE", scope);
            #[cfg(unix)]
            command.process_group(0);
        }
        let mut child = command
            .env("TC_STRATEGY_SPEC", serde_json::to_string(spec)?)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut input = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("missing stdin"))?;
        let mut output = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("missing stdout"))?;
        let (tx, rx) = mpsc::channel::<Frame>();
        let (reply, response) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(frame) = rx.recv() {
                let result = tc_sdk::write_message(&mut input, &frame)
                    .and_then(|()| tc_sdk::read_message(&mut output));
                let failed = result.is_err();
                if reply.send(result).is_err() || failed {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            request: tx,
            response,
        })
    }
}
impl Strategy for ProcessStrategy {
    fn on_frame(&mut self, frame: &Frame) -> Result<Vec<Intent>> {
        self.request.send(frame.clone())?;
        let reply = self.response.recv_timeout(Duration::from_secs(45))??;
        ensure!(reply.protocol == PROTOCOL, "strategy protocol mismatch");
        Ok(reply.intents)
    }
}
impl Drop for ProcessStrategy {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
pub fn strategy(spec: &StrategySpec) -> Result<Box<dyn Strategy>> {
    match &spec.kind {
        StrategyKind::Executable { path } => Ok(Box::new(ProcessStrategy::spawn(path, spec)?)),
        _ => Ok(Box::new(tc_sdk::Builtin::new(spec)?)),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn candle(time: i64, price: i64) -> Candle {
        Candle {
            market: "BTC-EUR".into(),
            time,
            open: price.into(),
            high: price.into(),
            low: price.into(),
            close: price.into(),
            volume: 100.into(),
        }
    }
    fn spec() -> StrategySpec {
        StrategySpec {
            parameters: Default::default(),
            name: "hold".into(),
            description: "test".into(),
            kind: StrategyKind::BuyHold,
            markets: vec!["BTC-EUR".into()],
        }
    }
    #[test]
    fn fees_are_exact_and_signal_fills_next_bar() {
        let config = RunConfig::new(0, 3 * MINUTE);
        let candles = vec![candle(0, 100), candle(MINUTE, 100), candle(2 * MINUTE, 100)];
        let mut strategy = tc_sdk::Builtin::new(&spec()).unwrap();
        let report = backtest(&candles, config, &mut strategy, |_, _, _| Ok(())).unwrap();
        assert_eq!(report.fills[0].time, MINUTE);
        assert!(report.fills[0].fee > Decimal::ZERO);
        assert!(report.metrics.net_return < 0.0);
        let bought = report
            .fills
            .iter()
            .filter(|f| f.side == Side::Buy)
            .map(|f| f.quantity * f.price + f.fee)
            .sum::<Decimal>();
        let sold = report
            .fills
            .iter()
            .filter(|f| f.side == Side::Sell)
            .map(|f| f.quantity * f.price - f.fee)
            .sum::<Decimal>();
        assert!(
            (report.equity.last().unwrap().equity - number(Decimal::from(10_000) - bought + sold))
                .abs()
                < 0.001
        );
    }
    #[test]
    fn cash_is_unchanged() {
        let config = RunConfig::new(0, 2 * MINUTE);
        let mut s = spec();
        s.kind = StrategyKind::Cash;
        let report = backtest(
            &[candle(0, 100), candle(MINUTE, 10)],
            config,
            &mut tc_sdk::Builtin::new(&s).unwrap(),
            |_, _, _| Ok(()),
        )
        .unwrap();
        assert_eq!(report.metrics.net_return, 0.0);
        assert!(report.fills.is_empty());
    }
    #[test]
    fn risk_gap_records_breach_without_clamping() {
        let mut config = RunConfig::new(0, 4 * MINUTE);
        config.fee_bps = 0;
        config.slippage_bps = 0;
        let data = [
            candle(0, 100),
            candle(MINUTE, 100),
            candle(2 * MINUTE, 70),
            candle(3 * MINUTE, 60),
        ];
        let report = backtest(
            &data,
            config,
            &mut tc_sdk::Builtin::new(&spec()).unwrap(),
            |_, _, _| Ok(()),
        )
        .unwrap();
        assert!(report.halted);
        assert!(report.metrics.max_drawdown > 0.39);
        assert!(report.fills.iter().any(|f| f.reason == "risk"));
    }
    #[test]
    fn future_changes_do_not_change_prior_fills() {
        let config = RunConfig::new(0, 4 * MINUTE);
        let a = vec![
            candle(0, 100),
            candle(MINUTE, 100),
            candle(2 * MINUTE, 110),
            candle(3 * MINUTE, 120),
        ];
        let mut b = a.clone();
        b[3] = candle(3 * MINUTE, 1000);
        let run = |d: &[Candle]| {
            backtest(
                d,
                config.clone(),
                &mut tc_sdk::Builtin::new(&spec()).unwrap(),
                |_, _, _| Ok(()),
            )
            .unwrap()
        };
        let aa = run(&a);
        let bb = run(&b);
        let early = |r: &Report| {
            serde_json::to_string(
                &r.fills
                    .iter()
                    .filter(|f| f.time < 3 * MINUTE)
                    .collect::<Vec<_>>(),
            )
            .unwrap()
        };
        assert_eq!(early(&aa), early(&bb));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod accounting_regressions {
    use super::*;
    #[test]
    fn hand_calculated_round_trip_includes_both_fees() {
        let mut config = RunConfig::new(0, MINUTE);
        config.capital = 1000.into();
        config.fee_bps = 100;
        config.slippage_bps = 0;
        let mut sim = Simulator::new(config).unwrap();
        sim.execute("BTC-EUR", 100.into(), 0, Decimal::new(5, 1), "signal")
            .unwrap();
        assert_eq!(sim.portfolio.cash, Decimal::from(495));
        assert_eq!(
            sim.portfolio.positions["BTC-EUR"].quantity,
            Decimal::from(5)
        );
        assert_eq!(sim.liquidation_equity(), Decimal::from(990));
        let report = sim.finish().unwrap();
        assert_eq!(report.metrics.total_fees, Decimal::from(10));
        assert!((report.metrics.net_return + 0.01).abs() < 1e-12);
    }
    #[test]
    fn stale_gap_cannot_fill_expired_signal() {
        let mut config = RunConfig::new(0, 20 * MINUTE);
        config.slippage_bps = 0;
        let mut sim = Simulator::new(config).unwrap();
        sim.queue(
            vec![Intent {
                market: "BTC-EUR".into(),
                target_weight: Decimal::ONE,
            }],
            MINUTE,
        )
        .unwrap();
        let c = Candle {
            market: "BTC-EUR".into(),
            time: 10 * MINUTE,
            open: 100.into(),
            high: 100.into(),
            low: 100.into(),
            close: 100.into(),
            volume: 1.into(),
        };
        sim.open(&[c], 10 * MINUTE).unwrap();
        assert!(sim.fills.is_empty());
    }
}
