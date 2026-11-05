//! Owner-only chronological evaluation; final-test consumption is separate from research jobs.
use super::*;

pub(crate) fn initialize(store: &Store) -> Result<()> {
    store.db()?.execute_batch("CREATE TABLE IF NOT EXISTS final_test(id INTEGER PRIMARY KEY CHECK(id=1),plan TEXT NOT NULL,state TEXT NOT NULL,started INTEGER NOT NULL,finished INTEGER,result TEXT,error TEXT);")?;
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FinalPlan {
    pub id: String,
    pub development_job: String,
    pub strategy: RegisteredStrategy,
    pub campaign: Campaign,
    pub config: RunConfig,
    pub inference_config_hash: String,
    pub engine_build_hash: String,
    pub dataset_manifest_hash: String,
    pub created_at: i64,
}
impl FinalPlan {
    fn identity(&self) -> Result<String> {
        Ok(digest(&serde_json::to_vec(&(
            "final_test_v1",
            &self.development_job,
            &self.strategy,
            &self.campaign,
            &self.config,
            &self.inference_config_hash,
            &self.engine_build_hash,
            &self.dataset_manifest_hash,
            self.created_at,
        ))?))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WalkForward {
    pub id: String,
    pub campaign: String,
    pub candidates: Vec<String>,
    pub folds: usize,
    pub state: String,
    pub results: Vec<Value>,
    pub warnings: Vec<String>,
}
pub(crate) fn engine_build_hash() -> Result<String> {
    static HASH: std::sync::OnceLock<std::result::Result<String, String>> =
        std::sync::OnceLock::new();
    HASH.get_or_init(|| {
        (|| -> Result<String> { Ok(digest(&fs::read(std::env::current_exe()?)?)) })()
            .map_err(|e| e.to_string())
    })
    .clone()
    .map_err(anyhow::Error::msg)
}
pub(crate) fn manifest_hash(store: &Store, dataset: &str) -> Result<String> {
    ensure!(
        dataset.len() == 64 && dataset.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid dataset identity"
    );
    Ok(digest(&fs::read(
        store
            .root
            .join("datasets")
            .join(dataset)
            .join("manifest.json"),
    )?))
}
fn inference_config_hash() -> String {
    digest(
        &serde_json::to_vec(&(
            std::env::var("TC_MODEL_MAP").ok(),
            std::env::var("TC_OLLAMA_URL").ok(),
        ))
        .unwrap_or_default(),
    )
}
fn fold_windows(c: &Campaign, folds: usize) -> Result<Vec<(i64, i64)>> {
    ensure!((2..=12).contains(&folds), "use 2–12 walk-forward folds");
    ensure_research_window(c.development_start, c.development_end)?;
    let total = (c.development_end - c.development_start) / MINUTE;
    let initial = total / 2;
    ensure!(
        initial >= 2 * 1440 && (total - initial) / folds as i64 >= 2 * 1440,
        "initial training and each test fold need at least two days"
    );
    Ok((0..folds)
        .map(|i| {
            (
                c.development_start
                    + (initial + (total - initial) * i as i64 / folds as i64) * MINUTE,
                c.development_start
                    + (initial + (total - initial) * (i + 1) as i64 / folds as i64) * MINUTE,
            )
        })
        .collect())
}
fn selection_score(metrics: &Metrics) -> Option<f64> {
    metrics
        .sharpe
        .filter(|s| s.is_finite() && metrics.eligible && metrics.max_drawdown <= 0.15)
}
impl Store {
    /// No price observations are read or final feedback consumed when preparing a plan.
    pub fn prepare_final(&self, development_job: &str) -> Result<FinalPlan> {
        ensure!(
            self.final_status()?["state"] == "sealed",
            "final test already consumed"
        );
        let job = self.job(development_job)?;
        ensure!(
            self.paper_candidates()?
                .iter()
                .any(|v| v["job"] == development_job),
            "latest qualifying native Sharpe development evaluation required"
        );
        let campaign: Campaign = self.get("campaign", &job.campaign)?;
        ensure!(
            campaign.final_test.as_ref().is_some_and(|f| f.range_covered
                && f.start == FINAL_TEST_START
                && f.end == FINAL_TEST_END),
            "campaign must reserve and cover the complete final test"
        );
        let mut plan = FinalPlan {
            dataset_manifest_hash: manifest_hash(self, &campaign.dataset)?,
            id: String::new(),
            development_job: development_job.into(),
            strategy: self.get("strategy", &job.strategy)?,
            campaign,
            config: RunConfig::new(FINAL_TEST_START, FINAL_TEST_END),
            inference_config_hash: inference_config_hash(),
            engine_build_hash: engine_build_hash()?,
            created_at: now(),
        };
        plan.id = plan.identity()?;
        self.put("final_plan", &plan.id, &plan)?;
        Ok(plan)
    }
    pub fn final_status(&self) -> Result<Value> {
        Ok(self
            .db()?
            .query_row(
                "SELECT plan,state,started,finished,result,error FROM final_test WHERE id=1",
                [],
                |r| {
                    let result: Option<String> = r.get(4)?;
                    Ok(
                        json!({"plan":r.get::<_,String>(0)?,"state":r.get::<_,String>(1)?,
                    "started":r.get::<_,i64>(2)?,"finished":r.get::<_,Option<i64>>(3)?,
                    "result":result.and_then(|s|serde_json::from_str::<Value>(&s).ok()),
                    "error":r.get::<_,Option<String>>(5)?,"consumed":true}),
                    )
                },
            )
            .optional()?
            .unwrap_or_else(|| json!({"state":"sealed","consumed":false})))
    }
    fn claim_final(&self, plan: &FinalPlan) -> Result<()> {
        let mut db = self.db()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure!(
            tx.query_row("SELECT COUNT(*) FROM final_test", [], |r| r
                .get::<_, i64>(0))?
                == 0,
            "final test already consumed; failures and interrupted runs cannot be retried"
        );
        tx.execute(
            "INSERT INTO final_test(id,plan,state,started)VALUES(1,?1,'running',?2)",
            params![plan.id, now()],
        )?;
        tx.execute(
            "INSERT INTO objects VALUES('audit',?1,?2)",
            params![
                uuid::Uuid::new_v4().to_string(),
                serde_json::to_string(
                    &json!({"action":"consume_final_test","plan":plan.id,"time":now()})
                )?
            ],
        )?;
        tx.commit()?;
        Ok(())
    }
    /// The explicit consume flag is a deliberate, irreversible owner action.
    pub fn execute_final(&self, id: &str, consume: bool) -> Result<Value> {
        ensure!(
            consume,
            "explicit --consume-final-test required; this spends the untouched final test even on failure"
        );
        let plan: FinalPlan = self.get("final_plan", id)?;
        ensure!(
            plan.id == id && plan.identity()? == id,
            "final plan identity mismatch"
        );
        ensure!(
            plan.config.start == FINAL_TEST_START && plan.config.end == FINAL_TEST_END,
            "invalid final window"
        );
        ensure!(
            plan.inference_config_hash == inference_config_hash(),
            "inference configuration changed since freeze"
        );
        ensure!(
            plan.engine_build_hash == engine_build_hash()?,
            "evaluation binary changed since freeze"
        );
        let registered: RegisteredStrategy = self.get("strategy", &plan.strategy.id)?;
        ensure!(
            serde_json::to_value(&registered)? == serde_json::to_value(&plan.strategy)?,
            "frozen strategy changed"
        );
        // Resolve executable checksums before claiming; no reserved data has been read.
        self.resolve_strategy(&plan.strategy)?;
        self.claim_final(&plan)?;
        let result = (|| -> Result<Value> {
            ensure!(
                manifest_hash(self, &plan.campaign.dataset)? == plan.dataset_manifest_hash,
                "frozen dataset manifest changed"
            );
            let (manifest, rows) = tc_data::load(&self.root, &plan.campaign.dataset)?;
            ensure!(
                manifest.accepted
                    && !manifest.synthetic
                    && manifest.venue == "bitvavo"
                    && manifest.start <= FINAL_TEST_START
                    && manifest.end >= FINAL_TEST_END,
                "final test requires the frozen accepted native dataset"
            );
            let report = self.evaluate(
                &rows,
                &plan.strategy,
                plan.config.clone(),
                &format!("final:{}", plan.id),
                true,
            )?;
            let directory = self.root.join("final-test").join(&plan.id);
            save_report(&directory.join("report.json.gz"), &report)?;
            Ok(
                json!({"metrics":report.metrics,"halted":report.halted,"stale":report.stale,
                "warning":"Final test consumed; subsequent strategy decisions cannot claim this window is untouched. Historical model-knowledge limitations remain."}),
            )
        })();
        match result {
            Ok(value) => {
                self.db()?.execute(
                    "UPDATE final_test SET state='completed',finished=?1,result=?2 WHERE id=1",
                    params![now(), serde_json::to_string(&value)?],
                )?;
            }
            Err(error) => {
                self.db()?.execute(
                    "UPDATE final_test SET state='failed',finished=?1,error=?2 WHERE id=1",
                    params![now(), error.to_string()],
                )?;
                return Err(error.context("final test is consumed and cannot be retried"));
            }
        }
        self.final_status()
    }
    pub fn final_report(&self) -> Result<Report> {
        let status = self.final_status()?;
        ensure!(status["state"] == "completed", "no completed final report");
        read_report(
            &self
                .root
                .join("final-test")
                .join(status["plan"].as_str().context("missing plan")?)
                .join("report.json.gz"),
        )
    }
    pub fn walk_forward(
        &self,
        campaign: &str,
        mut candidates: Vec<String>,
        folds: usize,
    ) -> Result<WalkForward> {
        candidates.sort();
        candidates.dedup();
        ensure!(
            !candidates.is_empty() && candidates.len() <= 32,
            "select 1–32 immutable candidate versions"
        );
        let c: Campaign = self.get("campaign", campaign)?;
        ensure!(
            c.objective == Objective::SharpeDailyV1,
            "walk-forward selection requires Sharpe objective"
        );
        let windows = fold_windows(&c, folds)?;
        let specs: Vec<RegisteredStrategy> = candidates
            .iter()
            .map(|id| self.get("strategy", id))
            .collect::<Result<_>>()?;
        for s in &specs {
            ensure!(!s.archived, "candidate archived");
            self.resolve_strategy(s)?;
        }
        let mut run = WalkForward { id:uuid::Uuid::new_v4().to_string(),campaign:campaign.into(),candidates,folds,state:"running".into(),results:vec![],warnings:vec![
            "Exploratory development data only. Candidate authors may already know these dates; this is not an untouched test.".into(),
            "Expanding training starts with half of development; only eligible training Sharpe selects the next fold's version. Each evaluation starts with fresh capital and strategy state.".into(),
            "Fold Sharpe values are not averaged. Trial counts are descriptive, not a multiple-testing correction or confidence guarantee.".into(),
        ] };
        self.put("walk_forward", &run.id, &run)?;
        let result = (|| -> Result<()> {
            let (manifest, rows) = tc_data::load(&self.root, &c.dataset)?;
            ensure!(manifest.accepted, "dataset not accepted");
            for (i, (start, end)) in windows.into_iter().enumerate() {
                let mut training = vec![];
                let mut selected: Option<(f64, &RegisteredStrategy)> = None;
                for s in &specs {
                    let report = self.evaluate(
                        &rows,
                        s,
                        RunConfig::new(c.development_start, start),
                        &format!("wf:{}:{i}:train:{}", run.id, s.id),
                        false,
                    )?;
                    if let Some(score) = selection_score(&report.metrics)
                        && selected.as_ref().is_none_or(|(best, _)| score > *best)
                    {
                        selected = Some((score, s));
                    }
                    training.push(json!({"strategy":s.id,"metrics":report.metrics}));
                }
                let test = if let Some((_, s)) = selected {
                    let report = self.evaluate(
                        &rows,
                        s,
                        RunConfig::new(start, end),
                        &format!("wf:{}:{i}:test", run.id),
                        false,
                    )?;
                    json!({"strategy":s.id,"metrics":report.metrics,"halted":report.halted,"stale":report.stale})
                } else {
                    json!({"skipped":"no eligible training candidate with defined Sharpe"})
                };
                run.results.push(json!({"train_start":c.development_start,"train_end":start,"test_start":start,"test_end":end,"training":training,"test":test}));
                self.put("walk_forward", &run.id, &run)?;
            }
            Ok(())
        })();
        run.state = if result.is_ok() {
            "completed"
        } else {
            "failed"
        }
        .into();
        self.put("walk_forward", &run.id, &run)?;
        result?;
        Ok(run)
    }
    fn evaluate(
        &self,
        rows: &[Candle],
        strategy: &RegisteredStrategy,
        config: RunConfig,
        scope: &str,
        final_test: bool,
    ) -> Result<Report> {
        config.validate()?;
        if !final_test {
            ensure_research_window(config.start, config.end)?;
        }
        let rows: Vec<_> = rows
            .iter()
            .filter(|c| {
                c.time >= config.start - 1440 * MINUTE
                    && c.time < config.end
                    && (final_test || !overlaps_final(c.time, c.time + MINUTE))
            })
            .cloned()
            .collect();
        self.put("evaluation_scope",scope,&json!({"state":"running","strategy":strategy.id,"config":config,"final_test":final_test,"started":now()}))?;
        let result = (|| -> Result<Report> {
            let spec = self.resolve_strategy(strategy)?;
            let broker = matches!(spec.kind, StrategyKind::Executable { .. })
                .then(|| broker::ModelBroker::start(self.clone(), scope))
                .transpose()?;
            let mut agent: Box<dyn tc_sdk::Strategy> = match &spec.kind {
                StrategyKind::Executable { path } => {
                    Box::new(tc_engine::ProcessStrategy::spawn_with_broker(
                        path,
                        &spec,
                        broker.as_ref().map(|b| b.path()),
                    )?)
                }
                _ => tc_engine::strategy(&spec)?,
            };
            tc_engine::backtest(&rows, config.clone(), agent.as_mut(), |_, _, _| Ok(()))
        })();
        self.put("evaluation_scope",scope,&json!({"state":if result.is_ok(){"completed"}else{"failed"},"strategy":strategy.id,"config":config,"final_test":final_test,"finished":now()}))?;
        result
    }
    pub fn research_diagnostics(&self) -> Result<Value> {
        let db = self.db()?;
        let jobs: i64 = db.query_row(
            "SELECT COUNT(*) FROM jobs WHERE json_extract(data,'$.period')='development'",
            [],
            |r| r.get(0),
        )?;
        let versions:i64=db.query_row("SELECT COUNT(DISTINCT json_extract(data,'$.strategy')) FROM jobs WHERE json_extract(data,'$.period')='development'",[],|r|r.get(0))?;
        let evaluations:i64=db.query_row("SELECT COUNT(*) FROM objects WHERE kind='evaluation_scope' AND json_extract(data,'$.final_test')=0",[],|r|r.get(0))?;
        Ok(
            json!({"development_job_attempts":jobs,"development_versions_evaluated":versions,"walk_forward_evaluation_attempts":evaluations,"validation":self.ledger()?,"final_test":self.final_status()?,"warning":"Attempts include failures and repeated jobs; they are not independent trials. External/offline research is unobserved. No adjusted significance or performance guarantee is implied."}),
        )
    }
}
fn save_report(path: &Path, report: &Report) -> Result<()> {
    let mut zip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    serde_json::to_writer(&mut zip, report)?;
    tc_data::atomic_write(path, &zip.finish()?)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Store, Campaign, RegisteredStrategy) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let start = timestamp("2026-01-01").unwrap();
        let manifest =
            tc_data::snapshot(&store.root, vec![], start, FINAL_TEST_END, false, true).unwrap();
        let c = store.campaign(&manifest.id).unwrap();
        let s = store
            .register(
                StrategySpec {
                    parameters: Default::default(),
                    name: "Cash".into(),
                    description: "fixture".into(),
                    kind: StrategyKind::Cash,
                    markets: vec!["BTC-EUR".into()],
                },
                None,
                None,
            )
            .unwrap();
        (dir, store, c, s)
    }
    #[test]
    fn folds_are_chronological_development_only_and_do_not_use_validation() {
        let (_dir, store, c, _s) = fixture();
        let windows = fold_windows(&c, 3).unwrap();
        assert!(windows[0].0 > c.development_start);
        assert_eq!(windows.last().unwrap().1, c.development_end);
        for pair in windows.windows(2) {
            assert_eq!(pair[0].1, pair[1].0);
        }
        for &(start, end) in &windows {
            assert!(start < end && end <= c.validation_start && end <= FINAL_TEST_START);
        }
        assert!(fold_windows(&c, 1).is_err());
        assert_eq!(store.ledger().unwrap()["used"], 0);
        assert_eq!(store.final_status().unwrap()["state"], "sealed");
    }
    #[test]
    fn final_requires_explicit_action_and_is_global_even_when_execution_fails() {
        let (_dir, store, c, s) = fixture();
        // Metadata-only fixture: no actual reserved candles are present or evaluated.
        let mut plan = FinalPlan {
            dataset_manifest_hash: manifest_hash(&store, &c.dataset).unwrap(),
            id: String::new(),
            development_job: "fixture".into(),
            strategy: s,
            campaign: c,
            config: RunConfig::new(FINAL_TEST_START, FINAL_TEST_END),
            inference_config_hash: inference_config_hash(),
            engine_build_hash: engine_build_hash().unwrap(),
            created_at: now(),
        };
        plan.id = plan.identity().unwrap();
        store.put("final_plan", &plan.id, &plan).unwrap();
        assert!(store.execute_final(&plan.id, false).is_err());
        assert_eq!(store.final_status().unwrap()["state"], "sealed");
        // Fail integrity after the consumption boundary; no strategy sees holdout prices.
        let manifest_path = store
            .root
            .join("datasets")
            .join(&plan.campaign.dataset)
            .join("manifest.json");
        fs::remove_file(manifest_path).unwrap();
        assert!(store.execute_final(&plan.id, true).is_err());
        assert_eq!(store.final_status().unwrap()["state"], "failed");
        assert!(store.execute_final(&plan.id, true).is_err());
        assert_eq!(store.ledger().unwrap()["used"], 0);
        let backup = store.root.join("saved");
        store.backup(&backup).unwrap();
        let restored = Store::open(&backup).unwrap();
        assert_eq!(restored.final_status().unwrap()["state"], "failed");
    }
    #[test]
    fn final_claim_is_race_safe_and_preparation_rejects_unqualified_candidates() {
        let (_dir, store, c, s) = fixture();
        assert!(store.prepare_final("missing").is_err());
        let plan = FinalPlan {
            dataset_manifest_hash: manifest_hash(&store, &c.dataset).unwrap(),
            id: "fixture".into(),
            development_job: "none".into(),
            strategy: s,
            campaign: c,
            config: RunConfig::new(FINAL_TEST_START, FINAL_TEST_END),
            inference_config_hash: inference_config_hash(),
            engine_build_hash: engine_build_hash().unwrap(),
            created_at: now(),
        };
        let mut threads = vec![];
        for _ in 0..4 {
            let (store, plan) = (store.clone(), plan.clone());
            threads.push(std::thread::spawn(move || store.claim_final(&plan).is_ok()));
        }
        let successes = threads
            .into_iter()
            .map(|t| usize::from(t.join().unwrap()))
            .sum::<usize>();
        assert_eq!(successes, 1);
    }
    #[test]
    fn generated_final_fixture_completes_once_and_report_survives_backup() {
        let (_dir, store, mut campaign, strategy) = fixture();
        // Generated control-flow fixture; these are not historical market observations.
        let rows = (0..3)
            .map(|i| Candle {
                market: "BTC-EUR".into(),
                time: FINAL_TEST_START + i * MINUTE,
                open: 100.into(),
                high: 100.into(),
                low: 100.into(),
                close: 100.into(),
                volume: 1.into(),
            })
            .collect();
        let manifest = tc_data::snapshot(
            &store.root,
            rows,
            FINAL_TEST_START,
            FINAL_TEST_END,
            false,
            true,
        )
        .unwrap();
        campaign.dataset = manifest.id;
        let mut plan = FinalPlan {
            dataset_manifest_hash: manifest_hash(&store, &campaign.dataset).unwrap(),
            id: String::new(),
            development_job: "generated-fixture".into(),
            strategy,
            campaign,
            config: RunConfig::new(FINAL_TEST_START, FINAL_TEST_END),
            inference_config_hash: inference_config_hash(),
            engine_build_hash: engine_build_hash().unwrap(),
            created_at: now(),
        };
        plan.id = plan.identity().unwrap();
        store.put("final_plan", &plan.id, &plan).unwrap();
        let status = store.execute_final(&plan.id, true).unwrap();
        assert_eq!(status["state"], "completed");
        assert_eq!(status["consumed"], true);
        assert_eq!(store.final_report().unwrap().config.start, FINAL_TEST_START);
        assert!(store.execute_final(&plan.id, true).is_err());
        assert!(store.jobs().unwrap().is_empty());
        let backup = store.root.join("backup-final");
        store.backup(&backup).unwrap();
        let restored = Store::open(backup).unwrap();
        assert_eq!(restored.final_status().unwrap()["state"], "completed");
        assert_eq!(restored.final_report().unwrap().config.end, FINAL_TEST_END);
    }
}
