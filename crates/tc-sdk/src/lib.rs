//! Causal strategy API and portable, bounded length-framed protocol.
use anyhow::{Result, bail, ensure};
use rust_decimal::Decimal;
use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Write},
};
pub use tc_core::*;

pub trait Strategy {
    fn on_frame(&mut self, frame: &Frame) -> Result<Vec<Intent>>;
}
const MAX_FRAME: usize = 8 * 1024 * 1024;
pub fn write_message<T: serde::Serialize>(writer: &mut impl Write, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(bytes.len() <= MAX_FRAME, "frame exceeds protocol limit");
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}
pub fn read_message<T: serde::de::DeserializeOwned>(reader: &mut impl Read) -> Result<T> {
    let mut length = [0; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    ensure!(length <= MAX_FRAME, "frame exceeds protocol limit");
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}
/// Run a trusted external strategy. Stdout is reserved for the protocol.
pub fn serve(mut strategy: impl Strategy) -> Result<()> {
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    loop {
        let frame: Frame = match read_message(&mut input) {
            Ok(v) => v,
            Err(e) => {
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::UnexpectedEof)
                {
                    return Ok(());
                }
                return Err(e);
            }
        };
        ensure!(frame.protocol == PROTOCOL, "unsupported protocol");
        let intents = strategy.on_frame(&frame)?;
        write_message(
            &mut output,
            &Decision {
                protocol: PROTOCOL,
                intents,
            },
        )?;
    }
}
#[derive(Clone, Debug)]
pub struct Rolling {
    period: usize,
    values: VecDeque<f64>,
    sum: f64,
}
impl Rolling {
    pub fn new(period: usize) -> Result<Self> {
        ensure!(period > 0 && period <= 100_000, "invalid window");
        Ok(Self {
            period,
            values: VecDeque::new(),
            sum: 0.0,
        })
    }
    pub fn push(&mut self, value: f64) -> Option<f64> {
        if !value.is_finite() {
            return None;
        }
        self.values.push_back(value);
        self.sum += value;
        if self.values.len() > self.period
            && let Some(v) = self.values.pop_front()
        {
            self.sum -= v;
        }
        (self.values.len() == self.period).then_some(self.sum / self.period as f64)
    }
    pub fn volatility(&self) -> Option<f64> {
        if self.values.len() != self.period {
            return None;
        }
        let mean = self.sum / self.period as f64;
        Some(
            (self.values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / self.period as f64)
                .sqrt(),
        )
    }
}
pub struct Ema {
    period: usize,
    count: usize,
    value: Option<f64>,
}
impl Ema {
    pub fn new(period: usize) -> Result<Self> {
        ensure!(period > 0, "invalid period");
        Ok(Self {
            period,
            count: 0,
            value: None,
        })
    }
    pub fn push(&mut self, value: f64) -> Option<f64> {
        let alpha = 2.0 / (self.period + 1) as f64;
        let next = self.value.map_or(value, |old| old + alpha * (value - old));
        self.value = Some(next);
        self.count += 1;
        (self.count >= self.period).then_some(next)
    }
}
pub struct Rsi {
    previous: Option<f64>,
    gains: Rolling,
    losses: Rolling,
}
impl Rsi {
    pub fn new(period: usize) -> Result<Self> {
        Ok(Self {
            previous: None,
            gains: Rolling::new(period)?,
            losses: Rolling::new(period)?,
        })
    }
    pub fn push(&mut self, value: f64) -> Option<f64> {
        let old = self.previous.replace(value)?;
        let gain = self.gains.push((value - old).max(0.0));
        let loss = self.losses.push((old - value).max(0.0));
        match (gain, loss) {
            (Some(g), Some(l)) => Some(if l == 0.0 {
                if g == 0.0 { 50.0 } else { 100.0 }
            } else {
                100.0 - 100.0 / (1.0 + g / l)
            }),
            _ => None,
        }
    }
}
pub struct Atr {
    previous: Option<f64>,
    range: Rolling,
}
impl Atr {
    pub fn new(period: usize) -> Result<Self> {
        Ok(Self {
            previous: None,
            range: Rolling::new(period)?,
        })
    }
    pub fn push(&mut self, high: f64, low: f64, close: f64) -> Option<f64> {
        let range = self.previous.map_or(high - low, |old| {
            (high - low).max((high - old).abs()).max((low - old).abs())
        });
        self.previous = Some(close);
        self.range.push(range)
    }
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Aggregate {
    pub candle: Candle,
    pub observed: usize,
    pub expected: usize,
    pub complete: bool,
}
pub struct Resampler {
    minutes: i64,
    current: Option<Aggregate>,
    previous: Option<(String, i64)>,
}
impl Resampler {
    pub fn new(minutes: i64) -> Result<Self> {
        ensure!((1..=1440).contains(&minutes), "invalid resampling interval");
        Ok(Self {
            minutes,
            current: None,
            previous: None,
        })
    }
    pub fn push(&mut self, candle: &Candle) -> Result<Option<Aggregate>> {
        candle.validate()?;
        ensure!(
            self.previous
                .as_ref()
                .is_none_or(|(market, time)| market == &candle.market && candle.time > *time),
            "resampler requires one market in strictly increasing time order"
        );
        self.previous = Some((candle.market.clone(), candle.time));
        let boundary = candle.time / (self.minutes * MINUTE) * (self.minutes * MINUTE);
        let mut emitted = None;
        if self
            .current
            .as_ref()
            .is_some_and(|x| x.candle.time != boundary)
        {
            emitted = self.current.take();
        }
        match &mut self.current {
            Some(x) => {
                ensure!(x.candle.market == candle.market, "one resampler per market");
                x.candle.high = x.candle.high.max(candle.high);
                x.candle.low = x.candle.low.min(candle.low);
                x.candle.close = candle.close;
                x.candle.volume += candle.volume;
                x.observed += 1;
                x.complete = x.observed == x.expected;
            }
            None => {
                let mut c = candle.clone();
                c.time = boundary;
                self.current = Some(Aggregate {
                    candle: c,
                    observed: 1,
                    expected: self.minutes as usize,
                    complete: self.minutes == 1,
                });
            }
        }
        Ok(emitted)
    }
    /// Call at a known UTC clock boundary; an unfinished bar is never returned.
    pub fn advance(&mut self, closed_through: i64) -> Option<Aggregate> {
        if self
            .current
            .as_ref()
            .is_some_and(|a| a.candle.time + self.minutes * MINUTE <= closed_through)
        {
            self.current.take()
        } else {
            None
        }
    }
}
struct Averages {
    fast: Rolling,
    slow: Rolling,
}
pub struct Builtin {
    spec: StrategySpec,
    averages: BTreeMap<String, Averages>,
}
impl Builtin {
    pub fn new(spec: &StrategySpec) -> Result<Self> {
        spec.validate()?;
        ensure!(
            !matches!(spec.kind, StrategyKind::Executable { .. }),
            "external strategy needs a process"
        );
        let mut averages = BTreeMap::new();
        if let StrategyKind::MovingAverage { fast, slow } = spec.kind {
            for market in &spec.markets {
                averages.insert(
                    market.clone(),
                    Averages {
                        fast: Rolling::new(fast)?,
                        slow: Rolling::new(slow)?,
                    },
                );
            }
        }
        Ok(Self {
            spec: spec.clone(),
            averages,
        })
    }
}
impl Strategy for Builtin {
    fn on_frame(&mut self, frame: &Frame) -> Result<Vec<Intent>> {
        let weight = Decimal::ONE / Decimal::from(self.spec.markets.len());
        let mut intents = Vec::new();
        for candle in &frame.candles {
            if !self.spec.markets.contains(&candle.market) {
                continue;
            }
            let desired = match self.spec.kind {
                StrategyKind::Cash => None,
                StrategyKind::BuyHold => frame
                    .portfolio
                    .positions
                    .get(&candle.market)
                    .is_none_or(|p| p.quantity == Decimal::ZERO)
                    .then_some(weight),
                StrategyKind::MovingAverage { .. } => {
                    let a = self
                        .averages
                        .get_mut(&candle.market)
                        .ok_or_else(|| anyhow::anyhow!("missing averages"))?;
                    match (
                        a.fast.push(number(candle.close)),
                        a.slow.push(number(candle.close)),
                    ) {
                        (Some(f), Some(s)) => Some(if f > s { weight } else { Decimal::ZERO }),
                        _ => None,
                    }
                }
                StrategyKind::Executable { .. } => bail!("external strategy"),
            };
            if !frame.warmup
                && let Some(target_weight) = desired
            {
                intents.push(Intent {
                    market: candle.market.clone(),
                    target_weight,
                });
            }
        }
        Ok(intents)
    }
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ModelRequest {
    pub capability: String,
    pub prompt: String,
    pub max_tokens: u32,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ModelResponse {
    pub text: String,
    pub provider: String,
    pub model: String,
    pub training_cutoff: Option<String>,
}
pub trait ModelClient {
    fn infer(&mut self, request: &ModelRequest) -> Result<ModelResponse>;
}
pub struct FakeModel;
impl ModelClient for FakeModel {
    fn infer(&mut self, request: &ModelRequest) -> Result<ModelResponse> {
        Ok(ModelResponse {
            text: request.prompt.clone(),
            provider: "fake".into(),
            model: "echo".into(),
            training_cutoff: None,
        })
    }
}

/// Blocking connector for native strategy executables. Scope is supplied by TC.
pub struct HttpModelClient {
    pub base_url: String,
    pub scope: String,
}
impl ModelClient for HttpModelClient {
    fn infer(&mut self, request: &ModelRequest) -> Result<ModelResponse> {
        let response = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(35))
            .build()?
            .post(format!("{}/api/infer", self.base_url))
            .json(&serde_json::json!({"scope": self.scope, "request": request}))
            .send()?
            .error_for_status()?
            .json()?;
        Ok(response)
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn windows_and_indicators() {
        let mut sma = Rolling::new(3).unwrap();
        assert_eq!(sma.push(1.0), None);
        sma.push(2.0);
        assert_eq!(sma.push(3.0), Some(2.0));
        assert_eq!(sma.push(4.0), Some(3.0));
        let mut rsi = Rsi::new(2).unwrap();
        rsi.push(1.0);
        rsi.push(2.0);
        assert_eq!(rsi.push(3.0), Some(100.0));
    }
    #[test]
    fn oversized_protocol_is_rejected() {
        let binding = (u32::MAX).to_le_bytes();
        let mut bytes = binding.as_slice();
        assert!(read_message::<Frame>(&mut bytes).is_err());
    }
}

/// Bounded integer parameter spaces, suitable for periods and threshold steps.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ParameterRange {
    pub name: String,
    pub min: i64,
    pub max: i64,
    pub step: u32,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SearchMethod {
    Grid,
    #[default]
    Random,
}
/// A trial number and seed reproduce a candidate without any model/token calls.
pub fn propose(
    space: &[ParameterRange],
    method: &SearchMethod,
    seed: u64,
    trial: usize,
) -> Result<BTreeMap<String, serde_json::Value>> {
    ensure!(
        !space.is_empty() && space.len() <= 16,
        "parameter space must have 1..16 dimensions"
    );
    let mut state = seed
        .wrapping_add(trial as u64)
        .wrapping_add(0x9e3779b97f4a7c15);
    let mut grid = trial as u64;
    let mut parameters = BTreeMap::new();
    for range in space {
        ensure!(
            range.step > 0
                && range.max >= range.min
                && range.max.saturating_sub(range.min) <= 1_000_000,
            "invalid parameter range"
        );
        let count = (range.max - range.min) as u64 / u64::from(range.step) + 1;
        let index = match method {
            SearchMethod::Grid => {
                let index = grid % count;
                grid /= count;
                index
            }
            SearchMethod::Random => {
                state ^= state >> 12;
                state ^= state << 25;
                state ^= state >> 27;
                state.wrapping_mul(0x2545f4914f6cdd1d) % count
            }
        };
        ensure!(
            !range.name.is_empty()
                && parameters
                    .insert(
                        range.name.clone(),
                        serde_json::json!(range.min + index as i64 * i64::from(range.step))
                    )
                    .is_none(),
            "invalid/duplicate parameter name"
        );
    }
    Ok(parameters)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod aggregation_regressions {
    use super::*;
    fn bar(time: i64) -> Candle {
        Candle {
            market: "BTC-EUR".into(),
            time,
            open: 100.into(),
            high: 110.into(),
            low: 90.into(),
            close: 105.into(),
            volume: 1.into(),
        }
    }
    #[test]
    fn only_finalized_aggregates_are_available() {
        let mut r = Resampler::new(5).unwrap();
        for t in 0..5 {
            assert!(r.push(&bar(t * MINUTE)).unwrap().is_none());
            assert!(r.advance((t + 1) * MINUTE - 1).is_none());
        }
        let result = r.advance(5 * MINUTE).unwrap();
        assert!(result.complete);
        assert_eq!(result.candle.volume, 5.into());
        assert!(r.push(&bar(4 * MINUTE)).is_err());
    }
    #[test]
    fn sparse_aggregate_is_flagged() {
        let mut r = Resampler::new(5).unwrap();
        r.push(&bar(0)).unwrap();
        r.push(&bar(4 * MINUTE)).unwrap();
        let result = r.advance(5 * MINUTE).unwrap();
        assert!(!result.complete);
        assert_eq!(result.observed, 2);
    }
    #[test]
    fn grid_and_random_search_are_reproducible() {
        let space = [
            ParameterRange {
                name: "fast".into(),
                min: 5,
                max: 15,
                step: 5,
            },
            ParameterRange {
                name: "slow".into(),
                min: 30,
                max: 40,
                step: 10,
            },
        ];
        let a = propose(&space, &SearchMethod::Grid, 0, 4).unwrap();
        assert_eq!(a["fast"], 10);
        assert_eq!(a["slow"], 40);
        assert_eq!(
            propose(&space, &SearchMethod::Random, 42, 10).unwrap(),
            propose(&space, &SearchMethod::Random, 42, 10).unwrap()
        );
    }
}
impl HttpModelClient {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            base_url: std::env::var("TC_MODEL_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8080".into()),
            scope: std::env::var("TC_MODEL_SCOPE").map_err(|_| {
                anyhow::anyhow!("model client requires a TC-managed run/session scope")
            })?,
        })
    }
}
