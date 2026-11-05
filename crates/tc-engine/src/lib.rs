//! Deterministic accounting and historical execution. No network or storage here.
use anyhow::{Result, ensure};
use chrono::{DateTime, Datelike, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, process::Stdio, sync::mpsc, time::Duration};
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
// Per-run constants; public Simulator methods still honor mutable configuration.
#[derive(Clone, Copy)]
struct ExecutionCosts {
    buy_multiplier: Decimal,
    sell_multiplier: Decimal,
    fee_multiplier: Decimal,
    fee: Decimal,
    haircut: Decimal,
}
impl ExecutionCosts {
    fn new(config: &RunConfig) -> Self {
        let denominator = Decimal::from(10_000);
        let slip = Decimal::from(config.slippage_bps) / denominator;
        let fee = Decimal::from(config.fee_bps) / denominator;
        Self {
            buy_multiplier: Decimal::ONE + slip,
            sell_multiplier: Decimal::ONE - slip,
            fee_multiplier: Decimal::ONE + fee,
            fee,
            haircut: Decimal::ONE
                - Decimal::from(config.fee_bps + config.slippage_bps) / denominator,
        }
    }
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
        self.liquidation_equity_with_haircut(ExecutionCosts::new(&self.config).haircut)
    }
    fn liquidation_equity_with_haircut(&self, haircut: Decimal) -> Decimal {
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
        self.validate_intents(&intents)?;
        self.queue_validated(intents, time);
        Ok(())
    }
    fn queue_borrowed(&mut self, intents: &[Intent], time: i64) -> Result<()> {
        self.validate_intents(intents)?;
        self.queue_validated(intents.iter().cloned(), time);
        Ok(())
    }
    fn validate_intents(&self, intents: &[Intent]) -> Result<()> {
        ensure!(intents.len() <= 5, "too many intents");
        for (index, intent) in intents.iter().enumerate() {
            ensure!(
                self.config.rules.contains_key(&intent.market),
                "unknown intent market"
            );
            ensure!(
                intent.target_weight >= Decimal::ZERO && intent.target_weight <= Decimal::ONE,
                "invalid target weight"
            );
            ensure!(
                !intents[..index]
                    .iter()
                    .any(|other| other.market == intent.market),
                "duplicate market intent"
            );
        }
        ensure!(
            intents
                .iter()
                .map(|intent| intent.target_weight)
                .sum::<Decimal>()
                <= Decimal::ONE,
            "weights exceed portfolio capital"
        );
        Ok(())
    }
    fn queue_validated(&mut self, intents: impl IntoIterator<Item = Intent>, time: i64) {
        for intent in intents {
            self.pending.retain(|p| p.intent.market != intent.market);
            if !self.halted || intent.target_weight == Decimal::ZERO {
                self.pending.push(Pending {
                    intent,
                    created: time,
                });
            }
        }
    }
    pub fn execute(
        &mut self,
        market: &str,
        price: Decimal,
        time: i64,
        target: Decimal,
        reason: &str,
    ) -> Result<()> {
        self.execute_with_costs(
            market,
            price,
            time,
            target,
            reason,
            ExecutionCosts::new(&self.config),
            &mut None,
        )
    }
    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn execute_with_costs(
        &mut self,
        market: &str,
        price: Decimal,
        time: i64,
        target: Decimal,
        reason: &str,
        costs: ExecutionCosts,
        gross_equity: &mut Option<Decimal>,
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
        if (self.halted || target == Decimal::ZERO) && held == Decimal::ZERO {
            return Ok(());
        }
        let desired = if self.halted || target == Decimal::ZERO {
            Decimal::ZERO
        } else {
            *gross_equity.get_or_insert_with(|| self.gross_equity()) * target / price
        };
        let delta = desired - held;
        let fee_rate = costs.fee;
        let (side, fill_price, quantity) = if delta > Decimal::ZERO {
            let p = price * costs.buy_multiplier;
            // Cash limits and rounding toward zero only reduce the requested
            // quantity. Its unrounded notional is a conservative upper bound.
            if reason != "terminal"
                && reason != "risk"
                && delta
                    .checked_mul(p)
                    .is_some_and(|notional| notional < rule.minimum_eur)
            {
                return Ok(());
            }
            (
                Side::Buy,
                p,
                delta
                    .min(self.portfolio.cash / (p * costs.fee_multiplier))
                    .round_dp_with_strategy(
                        rule.quantity_decimals,
                        rust_decimal::RoundingStrategy::ToZero,
                    ),
            )
        } else {
            let p = price * costs.sell_multiplier;
            let requested = (-delta).min(held);
            if reason != "terminal"
                && reason != "risk"
                && requested
                    .checked_mul(p)
                    .is_some_and(|notional| notional < rule.minimum_eur)
            {
                return Ok(());
            }
            (
                Side::Sell,
                p,
                requested.round_dp_with_strategy(
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
        // Portfolio changed: later orders must value the updated cash/holdings.
        *gross_equity = None;
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
        self.open_with_costs(candles, time, ExecutionCosts::new(&self.config))
    }
    fn open_with_costs(
        &mut self,
        candles: &[Candle],
        time: i64,
        costs: ExecutionCosts,
    ) -> Result<()> {
        self.stale |= self.portfolio.positions.values().any(|p| {
            p.quantity > Decimal::ZERO && time - p.observed_at > self.config.stale_after_ms
        });
        for c in candles {
            if let Some(p) = self.portfolio.positions.get_mut(&c.market) {
                p.price = c.open;
                p.observed_at = time;
            }
        }
        // Keep the allocation for the next frame. Stable ordering still executes
        // sells before buys and preserves the order of unready intents.
        let mut pending = std::mem::take(&mut self.pending);
        pending.retain(|p| time - p.created <= self.config.intent_ttl_ms);
        let mut sells = 0;
        let mut ready = 0;
        for index in 0..pending.len() {
            let p = &pending[index];
            if time >= p.created
                && candles
                    .iter()
                    .any(|c| c.market == p.intent.market && c.volume > Decimal::ZERO)
            {
                let selling = p.intent.target_weight == Decimal::ZERO;
                let insert = if selling { sells } else { ready };
                // Stable insertion partitions the small intent list with one
                // readiness lookup per intent, rather than per sort comparison.
                pending[insert..=index].rotate_right(1);
                sells += usize::from(selling);
                ready += 1;
            }
        }
        let mut gross_equity = None;
        let result = (|| {
            for p in &pending[..ready] {
                if let Some(c) = candles.iter().find(|c| c.market == p.intent.market) {
                    self.execute_with_costs(
                        &p.intent.market,
                        c.open,
                        time,
                        p.intent.target_weight,
                        if self.halted { "risk" } else { "signal" },
                        costs,
                        &mut gross_equity,
                    )?;
                }
            }
            Ok(())
        })();
        pending.drain(..ready);
        self.pending = pending;
        result
    }
    pub fn mark(&mut self, candles: &[Candle], time: i64) -> Result<()> {
        self.mark_with_haircut(candles, time, ExecutionCosts::new(&self.config).haircut)
    }
    fn mark_with_haircut(&mut self, candles: &[Candle], time: i64, haircut: Decimal) -> Result<()> {
        for c in candles {
            if let Some(observed) = self.observed.get_mut(&c.market) {
                *observed = time;
            } else {
                self.observed.insert(c.market.clone(), time);
            }
            if let Some(p) = self.portfolio.positions.get_mut(&c.market) {
                p.price = c.close;
                p.observed_at = time;
            }
        }
        let stale = self.portfolio.positions.values().any(|p| {
            p.quantity > Decimal::ZERO && time - p.observed_at > self.config.stale_after_ms
        });
        self.stale |= stale;
        let value = self.liquidation_equity_with_haircut(haircut);
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
    backtest_impl::<false>(
        candles,
        config,
        strategy,
        &mut progress,
        |_, _, _, _| Ok(()),
        || Ok(()),
    )
}
#[derive(Debug)]
pub struct ExecutionFailure {
    pub code: &'static str,
    pub message: String,
}
impl std::fmt::Display for ExecutionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for ExecutionFailure {}
/// Observer sees the completed frame, current decision, and fills from earlier decisions.
pub fn backtest_with_observer(
    candles: &[Candle],
    config: RunConfig,
    strategy: &mut dyn Strategy,
    mut progress: impl FnMut(usize, usize, &Simulator) -> Result<()>,
    mut observer: impl FnMut(&Frame, &[Intent], &Simulator, Option<&str>) -> Result<()>,
) -> Result<Report> {
    backtest_impl::<true>(
        candles,
        config,
        strategy,
        &mut progress,
        &mut observer,
        || Ok(()),
    )
}
/// Phase boundary before terminal liquidation and metric calculation.
/// Existing backtest entry points retain their behavior.
pub fn backtest_with_phases(
    candles: &[Candle],
    config: RunConfig,
    strategy: &mut dyn Strategy,
    progress: impl FnMut(usize, usize, &Simulator) -> Result<()>,
    observer: impl FnMut(&Frame, &[Intent], &Simulator, Option<&str>) -> Result<()>,
    before_metrics: impl FnMut() -> Result<()>,
    observe: bool,
) -> Result<Report> {
    if observe {
        backtest_impl::<true>(
            candles,
            config,
            strategy,
            progress,
            observer,
            before_metrics,
        )
    } else {
        backtest_impl::<false>(
            candles,
            config,
            strategy,
            progress,
            observer,
            before_metrics,
        )
    }
}
fn backtest_impl<const OBSERVE: bool>(
    candles: &[Candle],
    config: RunConfig,
    strategy: &mut dyn Strategy,
    mut progress: impl FnMut(usize, usize, &Simulator) -> Result<()>,
    mut observer: impl FnMut(&Frame, &[Intent], &Simulator, Option<&str>) -> Result<()>,
    mut before_metrics: impl FnMut() -> Result<()>,
) -> Result<Report> {
    let mut sim = Simulator::new(config)?;
    let costs = ExecutionCosts::new(&sim.config);
    // A conservative five-market estimate avoids repeated equity-buffer growth.
    let estimated_frames = candles
        .len()
        .div_ceil(MARKETS.len())
        .min(((sim.config.end - sim.config.start) / MINUTE) as usize);
    sim.equity.reserve(estimated_frames);
    let mut index = 0;
    let mut previous = None;
    let mut frame = Frame {
        protocol: PROTOCOL,
        time: 0,
        candles: Vec::new(),
        portfolio: sim.portfolio.clone(),
        warmup: true,
    };
    while index < candles.len() {
        let time = candles[index].time;
        ensure!(previous.is_none_or(|p| p <= time), "candles not ordered");
        previous = Some(time);
        // Timestamp groups are small; scan adjacent rows rather than binary-searching
        // the whole remaining history (and touching distant cache lines per frame).
        let end = index
            + candles[index..]
                .iter()
                .take_while(|c| c.time == time)
                .count();
        let group = &candles[index..end];
        let mut seen = 0_u8;
        let mut duplicate = false;
        for c in group {
            let market = c.validated_market_index()?;
            let bit = 1 << market;
            duplicate |= seen & bit != 0;
            seen |= bit;
        }
        // Complete field validation precedes duplicate errors, as in the public API.
        ensure!(!duplicate, "duplicate candle");
        if time >= sim.config.end {
            break;
        }
        let warmup = time < sim.config.start;
        if !warmup {
            sim.open_with_costs(group, time, costs)?;
            sim.mark_with_haircut(group, time + MINUTE, costs.haircut)?;
        }
        frame.time = time + MINUTE;
        group.clone_into(&mut frame.candles);
        if OBSERVE {
            frame.portfolio.cash = sim.portfolio.cash;
            // Simulator keeps zero-quantity positions; keys only accumulate during
            // a backtest, so cached frame positions never need to be removed.
            for (market, position) in &sim.portfolio.positions {
                if let Some(cached) = frame.portfolio.positions.get_mut(market) {
                    cached.clone_from(position);
                } else {
                    frame
                        .portfolio
                        .positions
                        .insert(market.clone(), position.clone());
                }
            }
        } else {
            // With no observer, lend the portfolio to the strategy's frame and
            // restore it immediately after the callback, avoiding a snapshot.
            std::mem::swap(&mut frame.portfolio, &mut sim.portfolio);
        }
        frame.warmup = warmup;
        let decision = strategy.on_frame(&frame);
        if !OBSERVE {
            std::mem::swap(&mut frame.portfolio, &mut sim.portfolio);
        }
        let intents = match decision {
            Ok(v) => v,
            Err(error) => {
                let message = error.to_string();
                observer(&frame, &[], &sim, Some(&message))?;
                return Err(error);
            }
        };
        if !OBSERVE && !warmup {
            // No observation needs the decision after admission: move its owned
            // strings into the pending queue instead of cloning them.
            sim.queue(intents, time + MINUTE)
                .map_err(|error| ExecutionFailure {
                    code: "invalid_intent",
                    message: error.to_string(),
                })?;
        } else {
            if !warmup && let Err(error) = sim.queue_borrowed(&intents, time + MINUTE) {
                let message = error.to_string();
                observer(&frame, &intents, &sim, Some(&message))?;
                return Err(ExecutionFailure {
                    code: "invalid_intent",
                    message,
                }
                .into());
            }
            observer(&frame, &intents, &sim, None)?;
        }
        if index % 2_000 < group.len() {
            progress(index, candles.len(), &sim)?;
        }
        index = end;
    }
    ensure!(sim.equity.len() > 1, "no observations in evaluation period");
    progress(candles.len(), candles.len(), &sim)?;
    before_metrics()?;
    sim.finish()
}
/// Use complete UTC days only. Missing day boundaries invalidate the series;
/// never fabricate valuations across data gaps. Keep idle/cash days in the sample.
fn daily_returns(points: &[EquityPoint], start: i64, end: i64) -> Option<Vec<f64>> {
    const DAY: i64 = 86_400_000;
    let first = start.div_euclid(DAY) * DAY + if start.rem_euclid(DAY) == 0 { 0 } else { DAY };
    let last = end.div_euclid(DAY) * DAY;
    if last <= first {
        return None;
    }
    let mut boundaries = BTreeMap::new();
    for p in points {
        if p.time >= first && p.time <= last && p.time.rem_euclid(DAY) == 0 {
            // Terminal liquidation may replace a mark at the same timestamp.
            boundaries.insert(p.time, p.equity);
        }
    }
    let mut returns = vec![];
    let mut time = first;
    while time < last {
        let (Some(&before), Some(&after)) = (boundaries.get(&time), boundaries.get(&(time + DAY)))
        else {
            return None;
        };
        if !before.is_finite() || !after.is_finite() || before <= 0.0 || after <= 0.0 {
            return None;
        }
        returns.push(after / before - 1.0);
        time += DAY;
    }
    Some(returns)
}
fn daily_sharpe(points: &[EquityPoint], start: i64, end: i64) -> (Option<f64>, usize) {
    let Some(returns) = daily_returns(points, start, end) else {
        return (None, 0);
    };
    (daily_ratio(&returns), returns.len())
}
fn daily_ratio(returns: &[f64]) -> Option<f64> {
    ratio_with_variance_floor(returns, f64::EPSILON * f64::EPSILON)
}
fn ratio_with_variance_floor(returns: &[f64], floor: f64) -> Option<f64> {
    let n = returns.len();
    if n < 2 {
        return None;
    }
    let mean = returns.iter().sum::<f64>() / n as f64;
    let variance = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
    if variance <= floor {
        return None;
    }
    let sharpe = mean / variance.sqrt() * 365.25_f64.sqrt();
    sharpe.is_finite().then_some(sharpe)
}
/// Benchmark-relative diagnostics; never divide two Sharpe ratios.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchmarkComparison {
    pub version: String,
    pub strategy_sharpe: Option<f64>,
    pub benchmark_sharpe: Option<f64>,
    pub sharpe_difference: Option<f64>,
    pub information_ratio: Option<f64>,
    pub paired_days: usize,
    pub net_return_difference: f64,
    pub cagr_difference: Option<f64>,
}
pub fn compare_benchmark(strategy: &Report, benchmark: &Report) -> Result<BenchmarkComparison> {
    ensure!(
        strategy.config.start == benchmark.config.start
            && strategy.config.end == benchmark.config.end,
        "benchmark comparison requires identical evaluation dates"
    );
    let (start, end) = (strategy.config.start, strategy.config.end);
    let p = daily_returns(&strategy.equity, start, end);
    let b = daily_returns(&benchmark.equity, start, end);
    let (ps, _) = daily_sharpe(&strategy.equity, start, end);
    let (bs, _) = daily_sharpe(&benchmark.equity, start, end);
    let (information_ratio, paired_days) = match (p, b) {
        (Some(p), Some(b)) if p.len() == b.len() => {
            let active: Vec<_> = p.iter().zip(&b).map(|(p, b)| p - b).collect();
            (ratio_with_variance_floor(&active, 1e-24), active.len())
        }
        _ => (None, 0),
    };
    Ok(BenchmarkComparison {
        version: "buy_hold_comparison_daily_v1".into(),
        strategy_sharpe: ps,
        benchmark_sharpe: bs,
        sharpe_difference: ps.zip(bs).map(|(p, b)| p - b),
        information_ratio,
        paired_days,
        net_return_difference: strategy.metrics.net_return - benchmark.metrics.net_return,
        cagr_difference: strategy
            .metrics
            .net_cagr
            .zip(benchmark.metrics.net_cagr)
            .map(|(p, b)| p - b),
    })
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
    let (sharpe, sharpe_days) = daily_sharpe(&sim.equity, sim.config.start, sim.config.end);
    if sharpe.is_none() {
        warnings.push("daily Sharpe undefined: need at least two complete UTC days, observed boundaries, positive equity, and nonzero return variance".into());
    }
    warnings.push("Sharpe uses a zero cash benchmark and conventional sqrt(365.25) annualization; serial correlation and repeated selection can inflate it".into());
    Ok(Metrics {
        sharpe,
        sharpe_days,
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
/// External code runs in a confined child; timeout tears down its cgroup.
pub struct ProcessStrategy {
    sandbox: tc_sandbox::SandboxedChild,
    request: mpsc::Sender<Frame>,
    response: mpsc::Receiver<Result<Decision>>,
    diagnostics: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    diagnostics_done: mpsc::Receiver<()>,
}
impl ProcessStrategy {
    fn failure_message(&self, message: &str) -> String {
        // On exit, let the bounded stderr drain finish before reporting the error.
        // A live/stalled process cannot hold failure handling open indefinitely.
        let _ = self
            .diagnostics_done
            .recv_timeout(Duration::from_millis(100));
        let log = self
            .diagnostics
            .lock()
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .unwrap_or_default();
        if log.is_empty() {
            message.into()
        } else {
            format!("{message}; stderr: {log}")
        }
    }
    pub fn process_id(&self) -> u32 {
        self.sandbox.child.id()
    }
    pub fn spawn(path: &str, spec: &StrategySpec) -> Result<Self> {
        Self::spawn_with_broker(path, spec, None)
    }
    pub fn spawn_with_broker(
        path: &str,
        spec: &StrategySpec,
        broker: Option<&std::path::Path>,
    ) -> Result<Self> {
        let mut sandbox = tc_sandbox::Sandbox::strategy(
            std::path::Path::new(path),
            &serde_json::to_string(spec)?,
            broker,
        )?;
        sandbox
            .command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = sandbox.spawn()?;
        let diagnostics = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (diagnostics_end, diagnostics_done) = mpsc::channel();
        if let Some(mut stderr) = child.child.stderr.take() {
            let kept = diagnostics.clone();
            std::thread::spawn(move || {
                use std::io::Read;
                let mut bytes = [0; 4096];
                loop {
                    match stderr.read(&mut bytes) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if let Ok(mut log) = kept.lock() {
                                log.extend_from_slice(&bytes[..n]);
                                let remove = log.len().saturating_sub(8192);
                                log.drain(..remove);
                            }
                        }
                    }
                }
                let _ = diagnostics_end.send(());
            });
        }
        let mut input = child
            .child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("missing stdin"))?;
        let mut output = child
            .child
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
            sandbox: child,
            request: tx,
            response,
            diagnostics,
            diagnostics_done,
        })
    }
}
impl Strategy for ProcessStrategy {
    fn on_frame(&mut self, frame: &Frame) -> Result<Vec<Intent>> {
        self.request
            .send(frame.clone())
            .map_err(|e| ExecutionFailure {
                code: "strategy_exit",
                message: self.failure_message(&e.to_string()),
            })?;
        let reply = self
            .response
            .recv_timeout(Duration::from_secs(45))
            .map_err(|e| ExecutionFailure {
                code: if e == std::sync::mpsc::RecvTimeoutError::Timeout {
                    "strategy_timeout"
                } else {
                    "strategy_exit"
                },
                message: self.failure_message(&e.to_string()),
            })?
            .map_err(|e| ExecutionFailure {
                code: if e.downcast_ref::<std::io::Error>().is_some_and(|e| {
                    matches!(
                        e.kind(),
                        std::io::ErrorKind::UnexpectedEof
                            | std::io::ErrorKind::BrokenPipe
                            | std::io::ErrorKind::ConnectionReset
                    )
                }) {
                    "strategy_exit"
                } else {
                    "protocol_error"
                },
                message: self.failure_message(&e.to_string()),
            })?;
        if reply.protocol != PROTOCOL {
            return Err(ExecutionFailure {
                code: "protocol_error",
                message: "strategy protocol mismatch".into(),
            }
            .into());
        }
        Ok(reply.intents)
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
    fn points(values: &[f64]) -> Vec<EquityPoint> {
        values
            .iter()
            .enumerate()
            .map(|(i, &equity)| EquityPoint {
                time: i as i64 * 86_400_000,
                equity,
                drawdown: 0.0,
            })
            .collect()
    }
    #[test]
    fn sharpe_uses_daily_sample_variance_and_includes_cash_days() {
        // Daily returns +10%, -10%, 0%, +10%: mean .025, sample variance .0091666…
        let p = points(&[100.0, 110.0, 99.0, 99.0, 108.9]);
        let (score, days) = daily_sharpe(&p, 0, 4 * 86_400_000);
        let expected = 0.025 / (0.0275_f64 / 3.0).sqrt() * 365.25_f64.sqrt();
        assert!((score.unwrap() - expected).abs() < 1e-10);
        assert_eq!(days, 4);
    }
    #[test]
    fn sharpe_rejects_missing_boundaries_and_degenerate_returns() {
        let day = 86_400_000;
        assert_eq!(
            daily_sharpe(&points(&[100.0, 100.0, 100.0]), 0, 2 * day),
            (None, 2)
        );
        assert_eq!(daily_sharpe(&points(&[100.0, 110.0]), 0, day), (None, 1));
        assert_eq!(
            daily_sharpe(&points(&[100.0, 110.0, 0.0]), 0, 2 * day),
            (None, 0)
        );
        let mut p = points(&[100.0, 110.0, 99.0, 108.0]);
        p.remove(1);
        assert_eq!(daily_sharpe(&p, 0, 3 * day), (None, 0));
    }
    #[test]
    fn sharpe_excludes_partial_days_and_uses_terminal_liquidation() {
        let day = 86_400_000;
        let mut p = points(&[100.0, 110.0, 99.0, 108.0]);
        let expected = daily_sharpe(&p, day, 3 * day);
        assert_eq!(daily_sharpe(&p, 1, 3 * day + MINUTE), expected);
        p.push(EquityPoint {
            time: 3 * day,
            equity: 99.0,
            drawdown: 0.0,
        });
        assert_ne!(daily_sharpe(&p, day, 3 * day), expected);
        assert_eq!(
            daily_sharpe(&p, day, 3 * day),
            daily_sharpe(&points(&[100.0, 110.0, 99.0, 99.0]), day, 3 * day)
        );
    }
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
    fn cached_backtest_matches_public_simulator_with_fresh_frames() {
        let mut data = Vec::new();
        for minute in 0..40 {
            for (index, market) in MARKETS.iter().enumerate() {
                if minute % 7 == 0 && index == 2 {
                    continue;
                }
                let mut c = candle(minute * MINUTE, 100 + (minute % 9 - 4).abs());
                c.market = (*market).into();
                if minute % 11 == 0 {
                    c.volume = Decimal::ZERO;
                }
                data.push(c);
            }
        }
        let mut spec = spec();
        spec.markets = MARKETS.iter().map(|m| (*m).into()).collect();
        spec.kind = StrategyKind::MovingAverage { fast: 2, slow: 3 };
        let mut config = RunConfig::new(4 * MINUTE, 40 * MINUTE);
        config.fee_bps = 37;
        config.slippage_bps = 19;
        config.max_drawdown = 1.0;
        let mut reference = Simulator::new(config.clone()).unwrap();
        let mut strategy = tc_sdk::Builtin::new(&spec).unwrap();
        let mut expected_frames = Vec::new();
        let mut start = 0;
        while start < data.len() {
            let time = data[start].time;
            let end = start + data[start..].iter().take_while(|c| c.time == time).count();
            let group = &data[start..end];
            let warmup = time < config.start;
            if !warmup {
                reference.open(group, time).unwrap();
                reference.mark(group, time + MINUTE).unwrap();
            }
            let frame = Frame {
                protocol: PROTOCOL,
                time: time + MINUTE,
                candles: group.to_vec(),
                portfolio: reference.portfolio.clone(),
                warmup,
            };
            let intents = strategy.on_frame(&frame).unwrap();
            if !warmup {
                reference.queue(intents.clone(), time + MINUTE).unwrap();
            }
            expected_frames.push(serde_json::to_value((&frame, &intents, &reference)).unwrap());
            start = end;
        }
        let expected = reference.finish().unwrap();
        let mut observed = Vec::new();
        let actual = backtest_with_observer(
            &data,
            config,
            &mut tc_sdk::Builtin::new(&spec).unwrap(),
            |_, _, _| Ok(()),
            |frame, intents, sim, error| {
                assert!(error.is_none());
                observed.push(serde_json::to_value((frame, intents, sim)).unwrap());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(observed, expected_frames);
        let plain = backtest(
            &data,
            expected.config.clone(),
            &mut tc_sdk::Builtin::new(&spec).unwrap(),
            |_, _, _| Ok(()),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(plain).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }
    #[test]
    fn grouping_rejects_duplicates_unsupported_and_unordered_candles() {
        for (data, message) in [
            (vec![candle(0, 100), candle(0, 100)], "duplicate candle"),
            (
                vec![candle(MINUTE, 100), candle(0, 100)],
                "candles not ordered",
            ),
            (
                {
                    let mut c = candle(0, 100);
                    c.market = "UNKNOWN".into();
                    vec![c]
                },
                "unsupported market",
            ),
        ] {
            let error = backtest(
                &data,
                RunConfig::new(0, 3 * MINUTE),
                &mut tc_sdk::Builtin::new(&spec()).unwrap(),
                |_, _, _| Ok(()),
            )
            .unwrap_err();
            assert_eq!(error.to_string(), message);
        }
    }
    #[test]
    fn pending_orders_preserve_sell_priority_expiry_and_unready_order() {
        let mut config = RunConfig::new(0, 5 * MINUTE);
        config.fee_bps = 0;
        config.slippage_bps = 0;
        config.intent_ttl_ms = MINUTE;
        let mut sim = Simulator::new(config).unwrap();
        sim.execute("BTC-EUR", 100.into(), 0, Decimal::ONE, "signal")
            .unwrap();
        let pending = |market: &str, target, created| Pending {
            intent: Intent {
                market: market.into(),
                target_weight: target,
            },
            created,
        };
        sim.pending = vec![
            pending("ETH-EUR", Decimal::ONE, MINUTE),
            pending("SOL-EUR", Decimal::ZERO, MINUTE),
            pending("BTC-EUR", Decimal::ZERO, MINUTE),
            pending("XRP-EUR", Decimal::ZERO, 0),
            pending("ADA-EUR", Decimal::ZERO, 3 * MINUTE),
        ];
        let mut eth = candle(2 * MINUTE, 100);
        eth.market = "ETH-EUR".into();
        sim.open(&[candle(2 * MINUTE, 100), eth], 2 * MINUTE)
            .unwrap();
        assert_eq!(sim.fills[1].market, "BTC-EUR");
        assert_eq!(sim.fills[1].side, Side::Sell);
        assert_eq!(sim.fills[2].market, "ETH-EUR");
        assert_eq!(
            sim.pending
                .iter()
                .map(|p| p.intent.market.as_str())
                .collect::<Vec<_>>(),
            vec!["SOL-EUR", "ADA-EUR"]
        );
        // Public methods must compute costs from the current configuration.
        sim.config.fee_bps = 100;
        let equity = sim.liquidation_equity();
        assert_eq!(equity, Decimal::from(9900));
    }
    #[test]
    fn batch_valuation_cache_matches_independent_order_execution() {
        let mut config = RunConfig::new(0, 40 * MINUTE);
        config.fee_bps = 37;
        config.slippage_bps = 19;
        config.max_drawdown = 1.0;
        let mut batch = Simulator::new(config).unwrap();
        let mut independent = batch.clone();
        for minute in 0..30 {
            let time = minute * MINUTE;
            let group: Vec<_> = MARKETS
                .iter()
                .enumerate()
                .map(|(i, market)| {
                    let mut c = candle(time, 100 + minute % (i as i64 + 3));
                    c.market = (*market).into();
                    c
                })
                .collect();
            batch.open(&group, time).unwrap();
            for c in &group {
                if let Some(p) = independent.portfolio.positions.get_mut(&c.market) {
                    p.price = c.open;
                    p.observed_at = time;
                }
            }
            let mut pending = std::mem::take(&mut independent.pending);
            pending.sort_by_key(|p| p.intent.target_weight > Decimal::ZERO);
            for p in pending {
                let c = group.iter().find(|c| c.market == p.intent.market).unwrap();
                independent
                    .execute(
                        &p.intent.market,
                        c.open,
                        time,
                        p.intent.target_weight,
                        "signal",
                    )
                    .unwrap();
            }
            batch.mark(&group, time + MINUTE).unwrap();
            independent.mark(&group, time + MINUTE).unwrap();
            let intents: Vec<_> = MARKETS
                .iter()
                .enumerate()
                .map(|(i, market)| Intent {
                    market: (*market).into(),
                    target_weight: if (minute + i as i64) % 11 < 3 {
                        Decimal::ZERO
                    } else {
                        Decimal::new(2, 1)
                    },
                })
                .collect();
            batch.queue(intents.clone(), time + MINUTE).unwrap();
            independent.queue(intents, time + MINUTE).unwrap();
            assert_eq!(
                serde_json::to_value(&batch).unwrap(),
                serde_json::to_value(&independent).unwrap()
            );
        }
    }
    #[test]
    fn minimum_notional_shortcuts_preserve_boundary_and_forced_fills() {
        for (weight, expected_fills) in [
            (Decimal::new(49999, 7), 0),
            (Decimal::new(5, 3), 1),
            (Decimal::new(50001, 7), 1),
        ] {
            let mut config = RunConfig::new(0, MINUTE);
            config.capital = 1000.into();
            config.fee_bps = 0;
            config.slippage_bps = 0;
            let mut sim = Simulator::new(config).unwrap();
            sim.execute("BTC-EUR", 100.into(), 0, weight, "signal")
                .unwrap();
            assert_eq!(sim.fills.len(), expected_fills);
        }
        for reason in ["signal", "terminal", "risk"] {
            let mut config = RunConfig::new(0, MINUTE);
            config.fee_bps = 0;
            config.slippage_bps = 0;
            let mut sim = Simulator::new(config).unwrap();
            sim.portfolio.positions.insert(
                "BTC-EUR".into(),
                Position {
                    quantity: Decimal::new(1, 2),
                    price: 100.into(),
                    observed_at: 0,
                },
            );
            sim.execute("BTC-EUR", 100.into(), 0, Decimal::ZERO, reason)
                .unwrap();
            assert_eq!(sim.fills.len(), usize::from(reason != "signal"));
        }
    }
    #[test]
    fn overflowing_notional_bound_falls_back_to_cash_limited_execution() {
        let mut config = RunConfig::new(0, MINUTE);
        config.capital = Decimal::MAX;
        let mut sim = Simulator::new(config).unwrap();
        assert!(
            sim.execute("BTC-EUR", Decimal::ONE, 0, Decimal::ONE, "signal")
                .is_ok()
        );
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
        // V1 compressed reports lack these fields and must remain readable.
        let mut old = serde_json::to_value(&report).unwrap();
        old["metrics"].as_object_mut().unwrap().remove("sharpe");
        old["metrics"]
            .as_object_mut()
            .unwrap()
            .remove("sharpe_days");
        let restored: Report = serde_json::from_value(old).unwrap();
        assert_eq!(restored.metrics.sharpe, None);
        assert_eq!(restored.metrics.sharpe_days, 0);
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod benchmark_tests {
    use super::*;
    fn report(returns: &[f64]) -> Report {
        let day = 1440 * MINUTE;
        let mut report = Simulator::new(RunConfig::new(0, returns.len() as i64 * day))
            .unwrap()
            .finish()
            .unwrap();
        let mut equity = 10_000.0;
        report.equity = vec![EquityPoint {
            time: 0,
            equity,
            drawdown: 0.0,
        }];
        for (i, r) in returns.iter().enumerate() {
            equity *= 1.0 + r;
            report.equity.push(EquityPoint {
                time: (i + 1) as i64 * day,
                equity,
                drawdown: 0.0,
            });
        }
        report.metrics.net_return = equity / 10_000.0 - 1.0;
        report
    }
    #[test]
    fn information_ratio_uses_paired_daily_active_returns() {
        let p = report(&[0.02, 0.01, 0.06]);
        let b = report(&[0.01, 0.02, 0.03]);
        let c = compare_benchmark(&p, &b).unwrap();
        assert_eq!(c.paired_days, 3);
        assert!((c.information_ratio.unwrap() - 0.5 * 365.25_f64.sqrt()).abs() < 1e-10);
        assert!(
            (c.sharpe_difference.unwrap()
                - (c.strategy_sharpe.unwrap() - c.benchmark_sharpe.unwrap()))
            .abs()
                < 1e-12
        );
        assert!(
            (c.net_return_difference - (1.02 * 1.01 * 1.06 - 1.01 * 1.02 * 1.03)).abs() < 1e-12
        );
    }
    #[test]
    fn missing_boundaries_and_zero_tracking_error_never_fabricate_scores() {
        let constant = compare_benchmark(&report(&[0.02; 3]), &report(&[0.01; 3])).unwrap();
        assert!(constant.information_ratio.is_none());
        let p = report(&[0.02, 0.01, 0.06]);
        let mut b = p.clone();
        let c = compare_benchmark(&p, &b).unwrap();
        assert!(c.information_ratio.is_none());
        assert_eq!(c.paired_days, 3);
        b.equity.remove(1);
        let c = compare_benchmark(&p, &b).unwrap();
        assert!(c.information_ratio.is_none());
        assert_eq!(c.paired_days, 0);
        b.config.end += MINUTE;
        assert!(compare_benchmark(&p, &b).is_err());
    }
}
