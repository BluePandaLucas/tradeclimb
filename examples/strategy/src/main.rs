//! External strategy: keep half the portfolio in BTC and the rest in cash.
use anyhow::Result;
use rust_decimal::Decimal;
use tc_sdk::{Frame,Intent,Strategy};
struct HalfBitcoin;
impl Strategy for HalfBitcoin {
    fn on_frame(&mut self, frame: &Frame) -> Result<Vec<Intent>> {
        if frame.warmup || !frame.candles.iter().any(|c| c.market == "BTC-EUR") {
            return Ok(vec![]);
        }
        Ok(vec![Intent { market: "BTC-EUR".into(), target_weight: Decimal::new(5, 1) }])
    }
}
fn main() -> Result<()> { tc_sdk::serve(HalfBitcoin) }
