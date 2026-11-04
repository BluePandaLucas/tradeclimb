//! Full-resolution, cost-matched diagnostic references. Never optimize on a sampled curve.
use super::*;
pub(crate) fn benchmarks(candles: &[Candle], report: &Report) -> Result<Vec<Value>> {
    ensure_research_window(report.config.start, report.config.end)?;
    let mut values = vec![];
    for (id, name, markets) in [
        (
            "btc_buy_hold_v1",
            "BTC buy & hold",
            vec!["BTC-EUR".to_string()],
        ),
        (
            "five_asset_buy_hold_v1",
            "Five-asset buy & hold",
            MARKETS.iter().map(|m| (*m).to_string()).collect(),
        ),
    ] {
        let spec = StrategySpec {
            parameters: Default::default(),
            name: name.into(),
            description: "Cost-matched unlevered buy-and-hold reference".into(),
            kind: StrategyKind::BuyHold,
            markets,
        };
        let mut config = report.config.clone();
        config.max_drawdown = 1.0;
        let benchmark = tc_engine::backtest(
            candles,
            config,
            &mut tc_sdk::Builtin::new(&spec)?,
            |_, _, _| Ok(()),
        )?;
        let comparison = tc_engine::compare_benchmark(report, &benchmark)?;
        values.push(json!({"id":id,"name":name,"equity":sample(&benchmark.equity,1500),"net_cagr":benchmark.metrics.net_cagr,"max_drawdown":benchmark.metrics.max_drawdown,"comparison":comparison,"primary_reference":id=="five_asset_buy_hold_v1"}));
    }
    Ok(values)
}
impl Store {
    pub(crate) fn detail_view(&self, job: &Job) -> Result<Value> {
        ensure_research_window(job.config.start, job.config.end)?;
        ensure!(job.state == "completed", "completed evaluation required");
        let directory = self.root.join("jobs").join(&job.id);
        let cache = directory.join("detail-v3.json");
        if let Ok(value) = read_json(&cache) {
            return Ok(value);
        }
        let report = read_report(&directory.join("report.json.gz"))?;
        let campaign: Campaign = self.get("campaign", &job.campaign)?;
        let (_, candles) = tc_data::load(&self.root, &campaign.dataset)?;
        let relevant: Vec<_> = candles
            .into_iter()
            .filter(|c| research_observation(c.time, &report.config))
            .collect();
        let mut value = serde_json::to_value(RunView::from_report(&report))?;
        value["benchmarks"] = json!(benchmarks(&relevant, &report)?);
        save_json(&cache, &value)?;
        Ok(value)
    }
}
