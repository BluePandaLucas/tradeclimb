//! Small, development-only tools for strategy authors.
use super::*;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceRequest {
    pub start: i64,
    pub end: i64,
    #[serde(default = "trace_limit")]
    pub max_frames: usize,
}
fn trace_limit() -> usize {
    256
}
impl TraceRequest {
    pub fn validate(&self, config: &RunConfig) -> Result<()> {
        ensure!(
            self.start >= config.start && self.end <= config.end && self.end > self.start,
            "trace must lie inside development"
        );
        ensure!(
            (1..=512).contains(&self.max_frames),
            "trace max_frames must be 1–512"
        );
        ensure_research_window(self.start, self.end)
    }
}
pub(crate) struct TraceCollector {
    request: Option<TraceRequest>,
    events: Vec<Value>,
    bytes: usize,
    seen: usize,
    truncated: bool,
}
impl TraceCollector {
    pub fn new(request: Option<TraceRequest>) -> Self {
        Self {
            request,
            events: vec![],
            bytes: 0,
            seen: 0,
            truncated: false,
        }
    }
    pub fn observe(
        &mut self,
        frame: &Frame,
        intents: &[Intent],
        sim: &tc_engine::Simulator,
        error: Option<&str>,
    ) -> Result<()> {
        let new = &sim.fills[self.seen.min(sim.fills.len())..];
        self.seen = sim.fills.len();
        let Some(r) = &self.request else {
            return Ok(());
        };
        let bar = frame.time - MINUTE;
        if bar < r.start || bar >= r.end {
            return Ok(());
        }
        if self.events.len() >= r.max_frames {
            self.truncated = true;
            return Ok(());
        }
        let event = json!({"time":frame.time,"bar_start":bar,"warmup":frame.warmup,"candles":frame.candles,"portfolio":frame.portfolio,"intents":intents,"fills":new,"pending":sim.pending,"halted":sim.halted,"stale":sim.stale,"error":error.map(|e|e.chars().take(2000).collect::<String>())});
        let bytes = serde_json::to_vec(&event)?.len();
        if self.bytes + bytes > 2_097_152 {
            self.truncated = true;
            return Ok(());
        }
        self.bytes += bytes;
        self.events.push(event);
        Ok(())
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        if self.request.is_some() {
            save_json(
                path,
                &json!({"version":"decision-trace-v1","request":self.request,"events":self.events,"truncated":self.truncated,"bytes":self.bytes,"semantics":"frame time is bar close; fills precede this frame's decision"}),
            )?;
        }
        Ok(())
    }
}
pub fn capabilities(store: &Store) -> Result<Value> {
    let current: Value = store.get("agent_platform", "current")?;
    let mapping: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&std::env::var("TC_MODEL_MAP").unwrap_or_else(|_| "{}".into()))?;
    let mut models: std::collections::BTreeSet<_> = mapping.keys().cloned().collect();
    models.insert("fake".into());
    Ok(
        json!({"version":"agent-tools-v1","platform":current["id"],"strategy_protocol":PROTOCOL,
        "source":{"language":"rust","files":"src/**/*.rs","max_files":64,"max_file_bytes":65536,"max_total_bytes":262144,"dependencies":"fixed by kit Cargo.lock; no custom manifests or build scripts"},
        "tools":["kit","submit_candidate","candidate_status","cancel_candidate","development_candles","submit_development","job_diagnostics","decision_trace","cancel_job"],
        "transport":{"socket_env":"TC_API_SOCKET","credential_env":"TC_API_TOKEN_FILE","authentication":"Bearer; no owner credential required"},
        "objective":"sharpe_daily_v1","benchmark":"five-asset buy-and-hold diagnostics; ranking stays Sharpe",
        "constraints":{"long_only":true,"max_total_weight":"1","markets":MARKETS,"capital":"10000 EUR","fee_bps":25,"slippage_bps":10,"max_drawdown":0.15,"target_cagr_aspiration":0.15,"warmup_minutes":1440},
        "models":{"capabilities":models,"max_unique_calls_per_run":64,"max_output_tokens":2048,"max_prompt_bytes":32768,"preflight":"declared fixtures only; no provider calls"},
        "limits":{"outstanding_candidates":8,"concurrent_builds":1,"build_seconds":1800,"preflight_seconds":120,"trace_frames":512,"trace_bytes":2097152,"candle_rows":2000,"candle_days":7},
        "schemas":{"candidate":{"required":["platform","idempotency_key","name","markets","files"],"optional":["description","parent","parameters","model_fixtures"]},"trace":{"start":"inclusive bar-open UTC milliseconds","end":"exclusive UTC milliseconds","max_frames":"1..512"},"candle":{"time":"bar-open UTC milliseconds","available_at":"time+60000","prices_volume":"decimal strings"}},
        "diagnostic_codes":["compile_failed","build_timeout","build_failed","preflight_failed","worker_interrupted","strategy_timeout","strategy_exit","protocol_error","invalid_intent","evaluation_error"],
        "policy":{"research":"development only","validation":"owner-controlled","final_test":{"start":FINAL_TEST_START,"end":FINAL_TEST_END,"access":"owner one-use workflow only"},"paper":"explicit owner approval","generation":"external orchestrator owns model credentials; agent tool sandbox has no Internet","preflight":"synthetic correctness checks; model responses are declared fixtures; not performance evidence"}}),
    )
}
#[derive(Deserialize)]
pub struct CandleQuery {
    pub campaign: String,
    pub market: String,
    pub start: i64,
    pub end: i64,
    #[serde(default = "candle_limit")]
    pub limit: usize,
}
fn candle_limit() -> usize {
    1000
}
pub fn candles(store: &Store, q: CandleQuery) -> Result<Value> {
    let c: Campaign = store.get("campaign", &q.campaign)?;
    ensure!(
        q.start >= c.development_start
            && q.end <= c.development_end
            && q.start % MINUTE == 0
            && q.end % MINUTE == 0,
        "candle slice must lie inside development on minute boundaries"
    );
    ensure_research_window(q.start, q.end)?;
    ensure!((1..=2000).contains(&q.limit), "candle limit must be 1–2000");
    let (m, rows) = tc_data::load_slice(&store.root, &c.dataset, &q.market, q.start, q.end)?;
    ensure!(m.accepted, "dataset is not accepted");
    let observed = rows.len();
    let page: Vec<_> = rows.into_iter().take(q.limit).collect();
    let next = if observed > page.len() {
        page.last().map(|r| r.time + MINUTE)
    } else {
        None
    };
    Ok(
        json!({"dataset":m.id,"venue":m.venue,"market":q.market,"start":q.start,"end":q.end,"candles":page,"next_start":next,"observed_minutes":observed,"missing_minutes":(q.end-q.start)/MINUTE-observed as i64,"synthetic":m.synthetic,"notes":m.notes}),
    )
}
fn development_job(store: &Store, id: &str) -> Result<Job> {
    let job = store.job(id)?;
    ensure!(job.period == Period::Development, "development jobs only");
    ensure_research_window(job.config.start, job.config.end)?;
    Ok(job)
}
pub fn trace(store: &Store, id: &str) -> Result<Value> {
    development_job(store, id)?;
    read_json(&store.root.join("jobs").join(id).join("trace.json"))
        .context("trace unavailable: submit development with an explicit trace window")
}
pub fn diagnostics(store: &Store, id: &str) -> Result<Value> {
    let j = development_job(store, id)?;
    let directory = store.root.join("jobs").join(id);
    let failure = read_json::<Value>(&directory.join("diagnostic.json")).ok();
    let metrics = j
        .result
        .as_ref()
        .map(|v| v["metrics"].clone())
        .unwrap_or(Value::Null);
    let trace=read_json::<Value>(&directory.join("trace.json")).ok().map(|v|json!({"frames":v["events"].as_array().map(Vec::len),"truncated":v["truncated"],"request":v["request"]}));
    Ok(
        json!({"job":j.id,"state":j.state,"failure":failure.or_else(||j.error.map(|e|json!({"code":"evaluation_error","message":e}))),"metrics":metrics,"trace":trace,"hints":{"undefined_sharpe":"Check sharpe_days/warnings for missing UTC boundaries, too few days or zero return variance; do not invent returns across gaps","no_trades":"Check warmup, rolling-window readiness, submitted weights, minimum notional, cash and pending intent expiry","fills":"A decision cannot fill on its generating bar; inspect next-frame fills/pending state","risk":"Halts/stale data and eligibility reasons are part of the result, not compiler failures"}}),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    struct Orders {
        invalid: bool,
    }
    impl tc_sdk::Strategy for Orders {
        fn on_frame(&mut self, _: &Frame) -> Result<Vec<Intent>> {
            Ok(vec![Intent {
                market: "BTC-EUR".into(),
                target_weight: if self.invalid {
                    2.into()
                } else {
                    rust_decimal::Decimal::new(5, 1)
                },
            }])
        }
    }
    #[test]
    fn traces_preserve_next_bar_causality_and_capture_invalid_decisions() -> Result<()> {
        let config = RunConfig::new(0, 4 * MINUTE);
        let rows: Vec<_> = (0..4)
            .map(|i| Candle {
                market: "BTC-EUR".into(),
                time: i * MINUTE,
                open: 100.into(),
                high: 100.into(),
                low: 100.into(),
                close: 100.into(),
                volume: 1000.into(),
            })
            .collect();
        let mut trace = TraceCollector::new(Some(TraceRequest {
            start: 0,
            end: 4 * MINUTE,
            max_frames: 2,
        }));
        let report = tc_engine::backtest_with_observer(
            &rows,
            config.clone(),
            &mut Orders { invalid: false },
            |_, _, _| Ok(()),
            |f, i, s, e| trace.observe(f, i, s, e),
        )?;
        let original = tc_engine::backtest(
            &rows,
            config.clone(),
            &mut Orders { invalid: false },
            |_, _, _| Ok(()),
        )?;
        assert_eq!(
            serde_json::to_value(report)?,
            serde_json::to_value(original)?
        );
        assert!(trace.events[0]["fills"].as_array().unwrap().is_empty());
        assert_eq!(trace.events[1]["fills"][0]["time"], MINUTE);
        assert!(trace.truncated);
        assert_eq!(trace.events.len(), 2);
        let mut failed = TraceCollector::new(Some(TraceRequest {
            start: 0,
            end: 4 * MINUTE,
            max_frames: 4,
        }));
        let error = tc_engine::backtest_with_observer(
            &rows,
            config,
            &mut Orders { invalid: true },
            |_, _, _| Ok(()),
            |f, i, s, e| failed.observe(f, i, s, e),
        )
        .unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<tc_engine::ExecutionFailure>()
                .unwrap()
                .code,
            "invalid_intent"
        );
        assert_eq!(failed.events[0]["intents"][0]["target_weight"], "2");
        assert!(
            failed.events[0]["error"]
                .as_str()
                .unwrap()
                .contains("weight")
        );
        Ok(())
    }
    #[test]
    fn trace_and_candle_windows_reject_validation_and_reserved_data_before_io() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path())?;
        let campaign = Campaign {
            final_test: None,
            objective: Objective::SharpeDailyV1,
            target_cagr: 0.15,
            id: "c".into(),
            dataset: "missing".into(),
            development_start: 0,
            development_end: 10 * MINUTE,
            validation_start: 10 * MINUTE,
            validation_end: 20 * MINUTE,
        };
        store.put("campaign", "c", &campaign)?;
        let error = candles(
            &store,
            CandleQuery {
                campaign: "c".into(),
                market: "BTC-EUR".into(),
                start: 10 * MINUTE,
                end: 11 * MINUTE,
                limit: 1,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("inside development"));
        let legacy = Campaign {
            development_start: FINAL_TEST_START,
            development_end: FINAL_TEST_END,
            ..campaign
        };
        store.put("campaign", "c", &legacy)?;
        assert!(
            candles(
                &store,
                CandleQuery {
                    campaign: "c".into(),
                    market: "BTC-EUR".into(),
                    start: FINAL_TEST_START,
                    end: FINAL_TEST_START + MINUTE,
                    limit: 1
                }
            )
            .is_err()
        );
        assert!(
            TraceRequest {
                start: 0,
                end: 11 * MINUTE,
                max_frames: 1
            }
            .validate(&RunConfig::new(0, 10 * MINUTE))
            .is_err()
        );
        assert!(
            TraceRequest {
                start: FINAL_TEST_START,
                end: FINAL_TEST_START + MINUTE,
                max_frames: 1
            }
            .validate(&RunConfig::new(FINAL_TEST_START, FINAL_TEST_END))
            .is_err()
        );
        assert_eq!(store.ledger()?["used"], 0);
        Ok(())
    }
}
