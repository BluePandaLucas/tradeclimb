//! Versioned explanations of retained development evidence. Never rewrite scores.
use super::*;

pub(crate) fn audit(store: &Store, job: &Job) -> Result<Value> {
    ensure!(job.period == Period::Development, "development jobs only");
    ensure_research_window(job.config.start, job.config.end)?;
    let directory = store.root.join("jobs").join(&job.id);
    let report_path = directory.join("report.json.gz");
    if job.state != "completed" || !report_path.exists() {
        return Ok(json!({"available":false,"reason":"completed full-resolution report required"}));
    }
    let request_bytes = fs::read(directory.join("request.json"))?;
    let request: WorkerRequest = serde_json::from_slice(&request_bytes)?;
    ensure!(
        request.period == Period::Development
            && serde_json::to_value(&request.config)? == serde_json::to_value(&job.config)?,
        "retained request scope/config mismatch"
    );
    let report_hash = digest(&fs::read(&report_path)?);
    let manifest_hash = evaluation::manifest_hash(store, &request.dataset)?;
    let origin: Option<Value> = read_json(&directory.join("measurement-origin-v1.json")).ok();
    let recorded_bytes = fs::read(directory.join("result.json")).ok();
    let trace_bytes = fs::read(directory.join("trace.json")).ok();
    let provenance = json!({"job":job.id,"strategy":job.strategy,"campaign":job.campaign,
        "dataset":request.dataset,"dataset_manifest_hash":manifest_hash,
        "report_gzip_sha256":report_hash,"request_sha256":digest(&request_bytes),
        "recorded_result_sha256":recorded_bytes.as_ref().map(|b|digest(b)),
        "trace_sha256":trace_bytes.as_ref().map(|b|digest(b)),
        "config":job.config,"metric_version":"sharpe_daily_v1",
        "comparison_version":"buy_hold_comparison_daily_v1",
        "audit_engine_build_hash":evaluation::engine_build_hash()?,
        "original_execution":origin,
        "original_engine_note":"original_execution is null for legacy jobs; audit engine is identified separately; strategy is never reexecuted"});
    let key = digest(&serde_json::to_vec(&provenance)?);
    let cache = directory.join(format!("measurement-audit-v1-{key}.json"));
    if let Ok(value) = read_json::<Value>(&cache) {
        return Ok(value);
    }
    let report = read_report(&report_path)?;
    ensure!(
        serde_json::to_value(&report.config)? == serde_json::to_value(&job.config)?,
        "retained report config mismatch"
    );
    // Native, deterministic passive references only; never execute the submitted strategy.
    // load verifies every partition checksum against the identified manifest.
    let (_, candles) = tc_data::load(&store.root, &request.dataset)?;
    let relevant: Vec<_> = candles
        .into_iter()
        .filter(|c| research_observation(c.time, &report.config))
        .collect();
    let mut benchmarks = comparison::benchmarks_with_audit(&relevant, &report, true)?;
    let recorded: Option<Value> = recorded_bytes
        .as_ref()
        .and_then(|b| serde_json::from_slice(b).ok());
    for benchmark in &mut benchmarks {
        let old = recorded
            .as_ref()
            .and_then(|r| r["benchmarks"].as_array())
            .and_then(|rows| rows.iter().find(|r| r["id"] == benchmark["id"]));
        benchmark["recorded_comparison"] = old.map_or(Value::Null, |r| r["comparison"].clone());
        benchmark["recorded_comparison_match"] = old.map_or(Value::Null, |r| {
            json!(r["comparison"] == benchmark["comparison"])
        });
    }
    let trace: Option<Value> = trace_bytes
        .as_ref()
        .and_then(|b| serde_json::from_slice(b).ok());
    let first_stale = trace
        .as_ref()
        .and_then(|v| v["events"].as_array())
        .and_then(|events| {
            events
                .iter()
                .find_map(|e| e.get("first_stale").filter(|v| !v.is_null()))
        })
        .cloned();
    let value = json!({"available":true,"version":"measurement-diagnostics-v1",
        "provenance":provenance,
        "strategy":{"daily":tc_engine::daily_diagnostics(&report),
            "historical_stale":report.stale,"stale_scope":"cumulative_since_start_including_before_open_refresh",
            "first_stale":first_stale,"first_stale_evidence":"available only when retained trace includes a new-format historical witness; null means unknown, not never stale",
            "stale_policy":"any positive holding older than stale_after_ms before open refresh, after close refresh or at terminal latches historical stale; recovery does not clear it; eligibility fails for any historical violation; daily Sharpe has no staleness mask"},
        "benchmarks":benchmarks,
        "effects":{"strategy_executed":false,"evaluation_created":false,"metrics_overwritten":false,
            "native_references_replayed":true},
        "interpretation":"A boundary-valid day can be excluded by another day's missing boundary. Audit establishes computability under existing v1 rules, not freshness certification or profitability. If the series is rejected, an explicit owner-approved measurement/campaign decision is needed; do not silently change the objective or insert prices."});
    save_json(&cache, &value)?;
    Ok(value)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn retained_diagnostics_explain_series_rejection_without_spending_or_rewriting() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path())?;
        let day = 1440 * MINUTE;
        let rows = (0..4 * 1440)
            .filter(|i| *i != 2 * 1440 - 1)
            .flat_map(|i| {
                MARKETS.iter().map(move |market| Candle {
                    market: (*market).into(),
                    time: i * MINUTE,
                    open: 100.into(),
                    high: 100.into(),
                    low: 100.into(),
                    close: 100.into(),
                    volume: 100.into(),
                })
            })
            .collect();
        let dataset = tc_data::snapshot(&store.root, rows, 0, 4 * day, true, true)?;
        let campaign = store.campaign(&dataset.id)?;
        let strategy = store.register(
            StrategySpec {
                name: "Cash".into(),
                description: "Synthetic fixture".into(),
                kind: StrategyKind::Cash,
                markets: vec!["BTC-EUR".into()],
                parameters: Default::default(),
            },
            None,
            None,
        )?;
        let mut job = store.submit(Submission {
            strategy: strategy.id,
            campaign: campaign.id,
            period: Period::Development,
            idempotency_key: "audit-fixture".into(),
            slippage_bps: None,
            trace: None,
            timing: None,
        })?;
        worker(&store.root, &job.id)?;
        let dir = store.root.join("jobs").join(&job.id);
        job.state = "completed".into();
        job.result = Some(read_json(&dir.join("result.json"))?);
        store.update_job(&job)?;
        let originals = ["report.json.gz", "result.json", "request.json"]
            .map(|f| fs::read(dir.join(f)).unwrap());
        let ledger = store.ledger()?;
        let jobs = serde_json::to_value(store.jobs()?)?;
        let result = agent_api::diagnostics(&store, &job.id)?;
        let measurement = &result["measurement"];
        assert_eq!(measurement["strategy"]["daily"]["expected_days"], 3);
        assert_eq!(measurement["strategy"]["daily"]["boundary_invalid_days"], 2);
        assert_eq!(
            measurement["strategy"]["daily"]["days"][0]["excluded_by_series_policy"],
            true
        );
        assert_eq!(
            measurement["strategy"]["daily"]["recorded_metrics_match"],
            true
        );
        for reference in measurement["benchmarks"].as_array().unwrap() {
            assert_eq!(reference["daily"]["included_days"], 0);
            assert_eq!(reference["comparison"]["paired_days"], 0);
            assert_eq!(reference["recorded_comparison_match"], true);
        }
        assert_eq!(agent_api::diagnostics(&store, &job.id)?, result);
        assert_eq!(store.ledger()?, ledger);
        assert_eq!(serde_json::to_value(store.jobs()?)?, jobs);
        for (i, name) in ["report.json.gz", "result.json", "request.json"]
            .iter()
            .enumerate()
        {
            assert_eq!(fs::read(dir.join(name))?, originals[i]);
        }
        assert!(measurement["provenance"]["original_execution"]["engine_build_hash"].is_string());
        fs::remove_file(dir.join("measurement-origin-v1.json"))?;
        let legacy = audit(&store, &job)?;
        assert!(legacy["provenance"]["original_execution"].is_null());
        assert!(legacy["strategy"]["first_stale"].is_null());
        let mut forbidden = job.clone();
        forbidden.period = Period::Validation;
        assert!(audit(&store, &forbidden).is_err());
        forbidden.period = Period::Development;
        forbidden.config.start = FINAL_TEST_START;
        forbidden.config.end = FINAL_TEST_END;
        assert!(audit(&store, &forbidden).is_err());
        Ok(())
    }
}
