//! Shared, serializable trading contracts. All times are UTC milliseconds.
use chrono::{DateTime, Utc};
use rust_decimal::{Decimal, prelude::ToPrimitive};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MINUTE: i64 = 60_000;
pub const MARKETS: [&str; 5] = ["BTC-EUR", "ETH-EUR", "SOL-EUR", "XRP-EUR", "ADA-EUR"];
pub const PROTOCOL: u32 = 1;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ContractError(pub String);
pub fn require(ok: bool, message: &str) -> Result<(), ContractError> {
    if ok {
        Ok(())
    } else {
        Err(ContractError(message.into()))
    }
}
#[inline]
pub fn number(value: Decimal) -> f64 {
    // Match Decimal::to_f64's integral/fractional conversion and rounding, but
    // avoid wide integer division and dynamic powers for small market prices.
    const POWERS: [u32; 10] = [
        1,
        10,
        100,
        1_000,
        10_000,
        100_000,
        1_000_000,
        10_000_000,
        100_000_000,
        1_000_000_000,
    ];
    let parts = value.unpack();
    if parts.hi == 0
        && parts.mid == 0
        && parts.scale > 0
        && let Some(&power) = POWERS.get(parts.scale as usize)
    {
        let integral = (parts.lo / power) as f64;
        let fraction = (parts.lo % power) as f64 / power as f64;
        let result = if fraction == 0.0 {
            integral
        } else {
            ((integral + fraction) * power as f64).round() / power as f64
        };
        if parts.negative { -result } else { result }
    } else {
        value.to_f64().unwrap_or(0.0)
    }
}
pub fn timestamp(text: &str) -> Result<i64, ContractError> {
    let value = if text.len() == 10 {
        format!("{text}T00:00:00Z")
    } else {
        text.into()
    };
    DateTime::parse_from_rfc3339(&value)
        .map(|d| d.timestamp_millis())
        .map_err(|e| ContractError(e.to_string()))
}
pub fn now() -> i64 {
    Utc::now().timestamp_millis()
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Candle {
    pub market: String,
    pub time: i64,
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub volume: Decimal,
}
impl Clone for Candle {
    fn clone(&self) -> Self {
        Self {
            market: self.market.clone(),
            time: self.time,
            open: self.open,
            high: self.high,
            low: self.low,
            close: self.close,
            volume: self.volume,
        }
    }
    fn clone_from(&mut self, source: &Self) {
        // Vec::clone_from can retain candle and market-string storage per frame.
        self.market.clone_from(&source.market);
        self.time = source.time;
        self.open = source.open;
        self.high = source.high;
        self.low = source.low;
        self.close = source.close;
        self.volume = source.volume;
    }
}
impl Candle {
    pub fn validate(&self) -> Result<(), ContractError> {
        self.validated_market_index().map(|_| ())
    }
    /// Validate all fields and return the index in MARKETS for frame grouping.
    pub fn validated_market_index(&self) -> Result<usize, ContractError> {
        let market = MARKETS
            .iter()
            .position(|m| *m == self.market)
            .ok_or_else(|| ContractError("unsupported market".into()))?;
        require(
            self.time >= 0 && self.time % MINUTE == 0,
            "unaligned candle timestamp",
        )?;
        require(
            self.low > Decimal::ZERO
                && self.high >= self.low
                && self.open >= self.low
                && self.open <= self.high
                && self.close >= self.low
                && self.close <= self.high
                && self.volume >= Decimal::ZERO,
            "invalid OHLCV",
        )?;
        Ok(market)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarketRules {
    pub quantity_decimals: u32,
    pub minimum_eur: Decimal,
}
impl Default for MarketRules {
    fn default() -> Self {
        Self {
            quantity_decimals: 6,
            minimum_eur: Decimal::from(5),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunConfig {
    pub start: i64,
    pub end: i64,
    pub capital: Decimal,
    pub fee_bps: u32,
    pub slippage_bps: u32,
    pub max_drawdown: f64,
    pub intent_ttl_ms: i64,
    pub stale_after_ms: i64,
    pub rules: BTreeMap<String, MarketRules>,
}
impl RunConfig {
    pub fn new(start: i64, end: i64) -> Self {
        Self {
            start,
            end,
            capital: Decimal::from(10_000),
            fee_bps: 25,
            slippage_bps: 10,
            max_drawdown: 0.15,
            intent_ttl_ms: 2 * MINUTE,
            stale_after_ms: 5 * MINUTE,
            rules: MARKETS
                .into_iter()
                .map(|m| (m.into(), MarketRules::default()))
                .collect(),
        }
    }
    pub fn validate(&self) -> Result<(), ContractError> {
        require(
            self.start >= 0
                && self.end > self.start
                && self.start % MINUTE == 0
                && self.end % MINUTE == 0,
            "invalid time range",
        )?;
        require(
            self.capital > Decimal::ZERO && self.fee_bps < 10_000 && self.slippage_bps < 10_000,
            "invalid capital or costs",
        )?;
        require(
            self.max_drawdown.is_finite()
                && self.max_drawdown > 0.0
                && self.max_drawdown <= 1.0
                && self.intent_ttl_ms > 0
                && self.stale_after_ms >= MINUTE,
            "invalid risk configuration",
        )?;
        for rule in self.rules.values() {
            require(
                rule.quantity_decimals <= 12 && rule.minimum_eur > Decimal::ZERO,
                "invalid market rules",
            )?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StrategyKind {
    Cash,
    BuyHold,
    MovingAverage { fast: usize, slow: usize },
    Executable { path: String },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StrategySpec {
    #[serde(default)]
    pub parameters: BTreeMap<String, serde_json::Value>,
    pub name: String,
    pub description: String,
    pub kind: StrategyKind,
    pub markets: Vec<String>,
}
impl StrategySpec {
    pub fn validate(&self) -> Result<(), ContractError> {
        require(
            self.parameters.len() <= 64
                && serde_json::to_vec(&self.parameters).is_ok_and(|v| v.len() <= 8192),
            "strategy parameter limit exceeded",
        )?;
        require(
            !self.name.trim().is_empty()
                && self.name.len() <= 120
                && self.description.len() <= 8_192,
            "invalid strategy description",
        )?;
        require(
            !self.markets.is_empty()
                && self.markets.len() <= 5
                && self.markets.iter().all(|m| MARKETS.contains(&m.as_str())),
            "invalid strategy markets",
        )?;
        let unique: std::collections::BTreeSet<_> = self.markets.iter().collect();
        require(
            unique.len() == self.markets.len(),
            "duplicate strategy markets",
        )?;
        if let StrategyKind::MovingAverage { fast, slow } = self.kind {
            require(
                fast > 0 && slow > fast && slow <= 100_000,
                "require 0 < fast < slow <= 100000",
            )?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Intent {
    pub market: String,
    pub target_weight: Decimal,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Position {
    pub quantity: Decimal,
    pub price: Decimal,
    pub observed_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Portfolio {
    pub cash: Decimal,
    pub positions: BTreeMap<String, Position>,
}
impl Portfolio {
    pub fn new(capital: Decimal) -> Self {
        Self {
            cash: capital,
            positions: BTreeMap::new(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Frame {
    pub protocol: u32,
    pub time: i64,
    pub candles: Vec<Candle>,
    pub portfolio: Portfolio,
    pub warmup: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Decision {
    pub protocol: u32,
    pub intents: Vec<Intent>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Buy,
    Sell,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fill {
    pub time: i64,
    pub market: String,
    pub side: Side,
    pub quantity: Decimal,
    pub price: Decimal,
    pub fee: Decimal,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EquityPoint {
    pub time: i64,
    pub equity: f64,
    pub drawdown: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MonthlyReturn {
    pub month: String,
    pub net_return: f64,
    pub complete: bool,
}
/// Immutable campaign scoring convention. Missing fields in V1 records remain CAGR.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Objective {
    #[default]
    NetCagr,
    SharpeDailyV1,
}
impl Objective {
    pub fn metric(self) -> &'static str {
        match self {
            Self::NetCagr => "net_cagr",
            Self::SharpeDailyV1 => "sharpe",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Metrics {
    /// Annualized daily UTC simple-return Sharpe, zero cash benchmark, sample variance.
    #[serde(default)]
    pub sharpe: Option<f64>,
    #[serde(default)]
    pub sharpe_days: usize,
    pub net_cagr: Option<f64>,
    pub net_return: f64,
    pub max_drawdown: f64,
    pub mean_monthly_return: Option<f64>,
    pub geometric_monthly_return: Option<f64>,
    pub positive_month_fraction: Option<f64>,
    pub worst_month: Option<f64>,
    pub total_fees: Decimal,
    pub turnover: f64,
    pub trades: usize,
    pub eligible: bool,
    pub reasons: Vec<String>,
    pub warnings: Vec<String>,
    pub months: Vec<MonthlyReturn>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub config: RunConfig,
    pub metrics: Metrics,
    pub equity: Vec<EquityPoint>,
    pub fills: Vec<Fill>,
    pub halted: bool,
    pub stale: bool,
}
/// Bounded extrema sampling, independent of accounting and metric calculations.
pub fn sample(points: &[EquityPoint], limit: usize) -> Vec<EquityPoint> {
    if limit < 4 {
        return points.iter().take(limit).cloned().collect();
    }
    if points.len() <= limit {
        return points.to_vec();
    }
    let buckets = (limit - 4) / 2;
    if buckets == 0 {
        return vec![points[0].clone(), points[points.len() - 1].clone()];
    }
    let mut indices = vec![0];
    for bucket in 0..buckets {
        let start = 1 + bucket * (points.len() - 2) / buckets;
        let end = 1 + (bucket + 1) * (points.len() - 2) / buckets;
        let slice = &points[start..end];
        let lo = slice
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.equity.total_cmp(&b.1.equity))
            .map(|(i, _)| start + i);
        let hi = slice
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.equity.total_cmp(&b.1.equity))
            .map(|(i, _)| start + i);
        if let Some(i) = lo {
            indices.push(i);
        }
        if let Some(i) = hi {
            indices.push(i);
        }
    }
    indices.push(points.len() - 1);
    if let Some((i, _)) = points
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.drawdown.total_cmp(&b.1.drawdown))
    {
        indices.push(i);
    }
    if let Some(i) = points.iter().position(|p| p.drawdown >= 0.15) {
        indices.push(i);
    }
    indices.sort_unstable();
    indices.dedup();
    indices.into_iter().map(|i| points[i].clone()).collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn persisted_financial_scores_retain_their_exact_float_values() {
        // Regression values from a generated chronological evaluation fixture.
        for score in [
            11.429298040082259_f64,
            0.21713038747733338,
            13.885058049104599,
            0.09487235395479421,
        ] {
            let value = serde_json::to_string(&score).unwrap();
            let restored: f64 = serde_json::from_str(&value).unwrap();
            assert_eq!(score.to_bits(), restored.to_bits());
        }
    }
    #[test]
    fn samples_keep_extreme_and_obey_cap() {
        let points: Vec<_> = (0..10_000)
            .map(|i| EquityPoint {
                time: i,
                equity: if i == 4321 { 0.0 } else { 10_000.0 + i as f64 },
                drawdown: 0.0,
            })
            .collect();
        let sampled = sample(&points, 1500);
        assert!(sampled.len() <= 1500);
        assert_eq!(sampled[0].time, 0);
        assert_eq!(sampled.last().unwrap().time, 9999);
        assert!(sampled.iter().any(|p| p.time == 4321));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod conversion_regressions {
    use super::*;

    #[test]
    fn small_decimal_conversion_matches_library_bit_for_bit() {
        let mut state = 123456789_u32;
        for scale in 0..=28 {
            for negative in [false, true] {
                for lo in [0, 1, 9, 10, 99, 100, 999_999_999, u32::MAX] {
                    for (mid, hi) in [(0, 0), (1, 0), (u32::MAX, u32::MAX)] {
                        let value = Decimal::from_parts(lo, mid, hi, negative, scale);
                        assert_eq!(
                            number(value).to_bits(),
                            value.to_f64().unwrap().to_bits(),
                            "{value}"
                        );
                    }
                }
                for _ in 0..2000 {
                    state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                    let value = Decimal::from_parts(state, 0, 0, negative, scale);
                    assert_eq!(
                        number(value).to_bits(),
                        value.to_f64().unwrap().to_bits(),
                        "{value}"
                    );
                }
            }
        }
    }
    #[test]
    fn candle_clone_from_preserves_all_fields_and_reuses_market_storage() {
        let source = Candle {
            market: "BTC-EUR".into(),
            time: MINUTE,
            open: 10.into(),
            high: 12.into(),
            low: 9.into(),
            close: 11.into(),
            volume: 3.into(),
        };
        let mut destination = source.clone();
        destination.market = String::with_capacity(32);
        destination.market.push_str("SOL-EUR");
        destination.time = 0;
        destination.close = 1.into();
        let storage = destination.market.as_ptr();
        destination.clone_from(&source);
        assert_eq!(destination, source);
        assert_eq!(storage, destination.market.as_ptr());
    }
}
