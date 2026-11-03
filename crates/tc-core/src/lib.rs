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
pub fn number(value: Decimal) -> f64 {
    value.to_f64().unwrap_or(0.0)
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Candle {
    pub market: String,
    pub time: i64,
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub volume: Decimal,
}
impl Candle {
    pub fn validate(&self) -> Result<(), ContractError> {
        require(
            MARKETS.contains(&self.market.as_str()),
            "unsupported market",
        )?;
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
        )
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
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Metrics {
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
