//! Pinned source admission: source is data until the offline build sandbox starts.
use super::*;
use std::collections::BTreeMap;
pub mod synthetic;
const MANIFEST: &str = include_str!("../../../examples/strategy/Cargo.toml");
const LOCK: &str = include_str!("../../../examples/strategy/Cargo.lock");
const EXAMPLE: &str = include_str!("../../../examples/strategy/src/main.rs");
const BIN: &str = "tc-example-strategy";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateInput {
    pub platform: String,
    pub idempotency_key: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub parameters: BTreeMap<String, Value>,
    pub markets: Vec<String>,
    pub files: BTreeMap<String, String>,
    #[serde(default)]
    pub model_fixtures: BTreeMap<String, String>,
    #[serde(default)]
    pub synthetic_suite: Option<synthetic::Suite>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub agent: Option<String>,
    pub fingerprint: String,
    pub platform: String,
    pub name: String,
    pub state: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub strategy: Option<String>,
    pub diagnostic: Option<Value>,
    pub preflight: Option<Value>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Platform {
    id: String,
    compiler: String,
    files: BTreeMap<String, String>,
}
pub fn prepare_platform(store: &Store, sdk: &Path) -> Result<String> {
    let mut files = BTreeMap::new();
    files.insert(
        "Cargo.toml".into(),
        fs::read_to_string(sdk.join("Cargo.toml"))?,
    );
    for name in ["tc-core", "tc-sdk"] {
        let base = sdk.join("crates").join(name);
        let mut source = vec![];
        collect_source(&base, &base, &mut source)?;
        for (path, bytes) in source {
            files.insert(format!("crates/{name}/{path}"), String::from_utf8(bytes)?);
        }
    }
    let compiler = std::process::Command::new("rustc")
        .arg("--version")
        .output()?;
    ensure!(
        compiler.status.success(),
        "trusted Rust compiler unavailable"
    );
    let compiler = String::from_utf8(compiler.stdout)?.trim().to_string();
    let id = digest(&serde_json::to_vec(&(
        &files, &compiler, MANIFEST, LOCK, EXAMPLE,
    ))?);
    let directory = store.root.join("platforms").join(&id);
    for (path, body) in &files {
        tc_data::atomic_write(&directory.join(path), body.as_bytes())?;
    }
    store.put(
        "agent_platform",
        &id,
        &Platform {
            id: id.clone(),
            compiler,
            files,
        },
    )?;
    store.put("agent_platform", "current", &json!({"id":id}))?;
    Ok(id)
}
fn platform(store: &Store) -> Result<Platform> {
    let current: Value = store.get("agent_platform", "current")?;
    store.get(
        "agent_platform",
        current["id"].as_str().context("platform unavailable")?,
    )
}
pub fn kit(store: &Store) -> Result<Value> {
    let p = platform(store)?;
    let mut files: BTreeMap<String, String> = p
        .files
        .into_iter()
        .map(|(k, v)| (format!("sdk/{k}"), v))
        .collect();
    files.insert(
        "Cargo.toml".into(),
        MANIFEST.replace("../../crates/tc-sdk", "sdk/crates/tc-sdk"),
    );
    files.insert("Cargo.lock".into(), LOCK.into());
    files.insert("src/main.rs".into(), EXAMPLE.into());
    for (name, gaps) in [("regular", false), ("gapped", true)] {
        let (config, candles) = synthetic_fixture(gaps)?;
        files.insert(format!("fixtures/{name}.json"), serde_json::to_string_pretty(&json!({"version":"synthetic-contract-v1","synthetic":true,"performance_evidence":false,"config":config,"candles":candles}))?);
    }

    files.insert(
        "synthetic-tests.md".into(),
        include_str!("../../../docs/synthetic-tests.md").into(),
    );
    files.insert(
        "tc_agent.py".into(),
        include_str!("../../../scripts/tc_agent.py").into(),
    );
    files.insert(
        "AGENT.md".into(),
        include_str!("../../../docs/agent-template.md").into(),
    );
    files.insert(
        ".tc-agent.json".into(),
        serde_json::to_string_pretty(&json!({"platform":p.id}))?,
    );
    Ok(json!({"version":"agent-kit-v1","platform":p.id,"compiler":p.compiler,"files":files}))
}
fn validate(input: &CandidateInput) -> Result<StrategySpec> {
    ensure!(
        !input.idempotency_key.is_empty() && input.idempotency_key.len() <= 200,
        "idempotency_key must contain 1–200 bytes"
    );
    ensure!(
        input.files.contains_key("src/main.rs") && input.files.len() <= 64,
        "include src/main.rs and at most 64 Rust source files"
    );
    let mut size = 0;
    for (path, body) in &input.files {
        ensure!(
            path.len() <= 200
                && path.starts_with("src/")
                && path.ends_with(".rs")
                && !path.contains('\\')
                && path.split('/').all(|p| !p.is_empty()
                    && p != "."
                    && p != ".."
                    && p.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))),
            "invalid Rust source path"
        );
        ensure!(body.len() <= 65_536, "source file exceeds 64 KiB");
        size += body.len();
    }
    ensure!(size <= 262_144, "source bundle exceeds 256 KiB");
    ensure!(
        input.model_fixtures.len() <= 16
            && input
                .model_fixtures
                .iter()
                .all(|(k, v)| !k.is_empty() && k.len() <= 100 && v.len() <= 32768),
        "model fixture limits exceeded"
    );
    let spec = StrategySpec {
        name: input.name.clone(),
        description: input.description.clone(),
        parameters: input.parameters.clone(),
        markets: input.markets.clone(),
        kind: StrategyKind::Executable {
            path: String::new(),
        },
    };
    spec.validate()?;
    if let Some(suite) = &input.synthetic_suite {
        suite.validate(&input.markets)?;
    }
    Ok(spec)
}
fn active(tx: &rusqlite::Transaction<'_>, agent: Option<&str>) -> Result<()> {
    if let Some(id) = agent {
        ensure!(tx.query_row("SELECT EXISTS(SELECT 1 FROM agent_credentials WHERE id=?1 AND revoked=0 AND expires>?2)",params![id,now()],|r|r.get::<_,bool>(0))?,"agent credential expired or revoked");
    }
    Ok(())
}
pub fn submit(store: &Store, input: CandidateInput, agent: Option<&str>) -> Result<Candidate> {
    validate(&input)?;
    let p = platform(store)?;
    ensure!(
        input.platform == p.id,
        "platform_changed: initialize a fresh kit and copy your source into it"
    );
    if let Some(parent) = &input.parent {
        let _: RegisteredStrategy = store.get("strategy", parent)?;
    }
    let mut fingerprint = digest(&serde_json::to_vec(&(
        &input.platform,
        &input.name,
        &input.description,
        &input.parent,
        &input.parameters,
        &input.markets,
        &input.files,
        &input.model_fixtures,
    ))?);
    if let Some(suite) = &input.synthetic_suite {
        fingerprint = digest(&serde_json::to_vec(&(&fingerprint, suite))?);
    }
    let key = digest(&serde_json::to_vec(&(
        agent.unwrap_or("owner"),
        &input.idempotency_key,
    ))?);
    let mut db = store.db()?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    active(&tx, agent)?;
    if let Some(data) = tx
        .query_row(
            "SELECT data FROM objects WHERE kind='candidate_key' AND id=?1",
            [&key],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        let old: Value = serde_json::from_str(&data)?;
        ensure!(
            old["fingerprint"] == fingerprint,
            "idempotency key used for different source/metadata"
        );
        let id = old["id"].as_str().context("invalid candidate key")?;
        let data: String = tx.query_row(
            "SELECT data FROM objects WHERE kind='candidate' AND id=?1",
            [id],
            |r| r.get(0),
        )?;
        return Ok(serde_json::from_str(&data)?);
    }
    let queued:i64=tx.query_row("SELECT COUNT(*) FROM objects WHERE kind='candidate' AND json_extract(data,'$.state') IN ('queued','building','preflight')",[],|r|r.get(0))?;
    ensure!(
        queued < 8,
        "candidate_queue_full: at most eight outstanding builds"
    );
    let c = Candidate {
        id: uuid::Uuid::new_v4().to_string(),
        agent: agent.map(str::to_owned),
        fingerprint: fingerprint.clone(),
        platform: p.id,
        name: input.name.clone(),
        state: "queued".into(),
        created_at: now(),
        updated_at: now(),
        strategy: None,
        diagnostic: None,
        preflight: Some(evidence(&input)?),
    };
    save_json(
        &store
            .root
            .join("candidates")
            .join(&c.id)
            .join("request.json"),
        &input,
    )?;
    tx.execute(
        "INSERT INTO objects VALUES('candidate',?1,?2)",
        params![c.id, serde_json::to_string(&c)?],
    )?;
    tx.execute(
        "INSERT INTO objects VALUES('candidate_key',?1,?2)",
        params![
            key,
            json!({"id":c.id,"fingerprint":fingerprint}).to_string()
        ],
    )?;
    tx.commit()?;
    Ok(c)
}
pub fn get(store: &Store, id: &str, agent: Option<&str>) -> Result<Candidate> {
    let c: Candidate = store.get("candidate", id)?;
    ensure!(
        agent.is_none() || c.agent.as_deref() == agent,
        "candidate belongs to another credential"
    );
    Ok(c)
}
pub fn cancel(store: &Store, id: &str, agent: Option<&str>) -> Result<Candidate> {
    let c = get(store, id, agent)?;
    store.db()?.execute("UPDATE objects SET data=json_set(data,'$.state','cancelled','$.updated_at',?1) WHERE kind='candidate' AND id=?2 AND json_extract(data,'$.state') IN ('queued','building','preflight')",params![now(),id])?;
    get(store, &c.id, agent)
}
fn update(store: &Store, c: &Candidate) -> Result<()> {
    store.db()?.execute("UPDATE objects SET data=?1 WHERE kind='candidate' AND id=?2 AND json_extract(data,'$.state') NOT IN ('cancelled','completed')",params![serde_json::to_string(c)?,c.id])?;
    Ok(())
}
fn synthetic_fixture(gaps: bool) -> Result<(RunConfig, Vec<Candle>)> {
    let start = timestamp("2024-01-01")?;
    let config = RunConfig::new(start + 10 * MINUTE, start + 90 * MINUTE);
    let candles: Vec<_> = (0..90)
        .filter(|i| !gaps || !(40..48).contains(i))
        .flat_map(|i| {
            MARKETS.iter().enumerate().map(move |(m, market)| {
                let p = rust_decimal::Decimal::from(100 + m as i64 * 10 + i % 5);
                Candle {
                    market: (*market).into(),
                    time: start + i * MINUTE,
                    open: p,
                    high: p,
                    low: p,
                    close: p,
                    volume: 1000.into(),
                }
            })
        })
        .collect();
    Ok((config, candles))
}
fn standard_preflight(
    store: &Store,
    spec: &StrategySpec,
    fixtures: &BTreeMap<String, String>,
) -> Result<Value> {
    let mut cases = vec![];
    for gaps in [false, true] {
        let (config, candles) = synthetic_fixture(gaps)?;
        let mut signatures = vec![];
        for _ in 0..2 {
            let broker = broker::ModelBroker::fixtures(store.clone(), fixtures.clone())?;
            let mut strategy = match &spec.kind {
                StrategyKind::Executable { path } => {
                    tc_engine::ProcessStrategy::spawn_with_broker(path, spec, Some(broker.path()))?
                }
                _ => anyhow::bail!("external candidate required"),
            };
            let mut decisions = Vec::new();
            tc_engine::backtest_with_observer(
                &candles,
                config.clone(),
                &mut strategy,
                |_, _, _| Ok(()),
                |f, intents, _, error| {
                    decisions.push(
                        json!({"time":f.time,"warmup":f.warmup,"intents":intents,"error":error}),
                    );
                    Ok(())
                },
            )?;
            signatures.push(digest(&serde_json::to_vec(&decisions)?));
        }
        ensure!(
            signatures[0] == signatures[1],
            "non_deterministic: use explicit parameters/seeds and causal frame inputs"
        );
        cases.push(json!({"name":if gaps{"missing_bars"}else{"regular_bars"},"passed":true,"decision_hash":signatures[0]}));
    }
    Ok(
        json!({"version":"synthetic-contract-v1","passed":true,"cases":cases,"model_calls":"fixtures only; no provider access","performance_evidence":false}),
    )
}
fn evidence(input: &CandidateInput) -> Result<Value> {
    Ok(
        json!({"version":"synthetic-behavior-v1","synthetic":true,"performance_evidence":false,
        "passed":false,"status":"not_run","platform":input.platform,
        "source_hash":digest(&serde_json::to_vec(&input.files)?),
        "parameters_hash":digest(&serde_json::to_vec(&input.parameters)?),
        "markets":input.markets,
        "model_fixtures_hash":digest(&serde_json::to_vec(&input.model_fixtures)?),
        "scenario_suite_hash":digest(&serde_json::to_vec(&input.synthetic_suite)?),
        "execution_hash":digest(&serde_json::to_vec(&(include_str!("../../../crates/tc-engine/src/lib.rs"), include_str!("candidates/synthetic.rs"), include_str!("candidates.rs")))?),
        "strategy":null,"standard":null,"custom":null}),
    )
}
fn preflight(
    store: &Store,
    spec: &StrategySpec,
    input: &CandidateInput,
    c: &mut Candidate,
) -> Result<()> {
    let mut report = evidence(input)?;
    if let StrategyKind::Executable { path } = &spec.kind {
        report["executable_hash"] = json!(digest(&fs::read(path)?));
    }
    report["status"] = json!("running");
    c.preflight = Some(report.clone());
    update(store, c)?;
    let standard = standard_preflight(store, spec, &input.model_fixtures);
    report["standard"] = match standard {
        Ok(value) => value,
        Err(error) => {
            json!({"passed":false,"error":error.to_string().chars().take(2000).collect::<String>()})
        }
    };
    // Custom expectations never replace the mandatory protocol/replay checks.
    if let Some(suite) = &input.synthetic_suite {
        report["custom"] = match synthetic::run(store, spec, &input.model_fixtures, suite) {
            Ok(value) => value,
            Err(error) => {
                json!({"passed":false,"error":error.to_string().chars().take(2000).collect::<String>()})
            }
        };
    }
    let passed = report["standard"]["passed"] == true
        && (input.synthetic_suite.is_none() || report["custom"]["passed"] == true);
    report["passed"] = json!(passed);
    report["status"] = json!("completed");
    // Preserve the original standard-preflight cases for existing clients.
    report["cases"] = report["standard"]["cases"].clone();
    c.preflight = Some(report);
    update(store, c)?;
    ensure!(
        passed,
        "synthetic preflight failed; inspect preflight evidence"
    );
    Ok(())
}
pub fn worker(root: &Path, id: &str) -> Result<()> {
    let store = Store::open(root)?;
    let mut c: Candidate = store.get("candidate", id)?;
    let dir = store.root.join("candidates").join(id);
    let result = (|| -> Result<()> {
        let input: CandidateInput = read_json(&dir.join("request.json"))?;
        let mut spec = validate(&input)?;
        ensure!(c.state == "building", "candidate not active");
        let _: Platform = store.get("agent_platform", &c.platform)?;
        let project = dir.join("project");
        tc_data::atomic_write(&project.join("Cargo.toml"), MANIFEST.as_bytes())?;
        tc_data::atomic_write(&project.join("Cargo.lock"), LOCK.as_bytes())?;
        for (path, body) in &input.files {
            tc_data::atomic_write(&project.join(path), body.as_bytes())?;
        }
        let artifact = dir.join("strategy");
        let build = build::build_captured(
            &project,
            &store.root.join("platforms").join(&c.platform),
            BIN,
            &artifact,
        )?;
        c.diagnostic = Some(
            json!({"code":"build_ok","message":"offline build completed","log":build["diagnostics"]}),
        );
        c.state = "preflight".into();
        c.updated_at = now();
        update(&store, &c)?;
        spec.kind = StrategyKind::Executable {
            path: artifact.to_string_lossy().into(),
        };
        preflight(&store, &spec, &input, &mut c)?;
        let registered = store.retain_strategy(spec, input.parent, Some(&project))?;
        let mut db = store.db()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        active(&tx, c.agent.as_deref())?;
        let state: String = tx.query_row(
            "SELECT json_extract(data,'$.state') FROM objects WHERE kind='candidate' AND id=?1",
            [id],
            |r| r.get(0),
        )?;
        ensure!(state == "preflight", "candidate cancelled or interrupted");
        tx.execute(
            "INSERT OR IGNORE INTO objects VALUES('strategy',?1,?2)",
            params![registered.id, serde_json::to_string(&registered)?],
        )?;
        if let Some(report) = &mut c.preflight {
            report["strategy"] = json!(&registered.id);
        }
        c.strategy = Some(registered.id);
        c.state = "completed".into();
        c.updated_at = now();
        tx.execute(
            "UPDATE objects SET data=?1 WHERE kind='candidate' AND id=?2",
            params![serde_json::to_string(&c)?, id],
        )?;
        tx.commit()?;
        Ok(())
    })();
    if let Err(error) = &result {
        let (code, log) = error
            .downcast_ref::<build::BuildFailure>()
            .map(|e| (e.code, e.diagnostics.clone()))
            .unwrap_or((
                if c.state == "preflight" {
                    "preflight_failed"
                } else {
                    "build_failed"
                },
                String::new(),
            ));
        c.state = "failed".into();
        c.updated_at = now();
        c.diagnostic = Some(
            json!({"code":code,"message":error.to_string().chars().take(2000).collect::<String>(),"log":log,"retryable":false}),
        );
        update(&store, &c)?;
    }
    result
}
pub async fn supervisor(store: Store) -> Result<()> {
    for mut c in store.objects::<Candidate>("candidate")? {
        if matches!(c.state.as_str(), "building" | "preflight") {
            c.state = "interrupted".into();
            c.updated_at = now();
            update(&store, &c)?;
        }
    }
    while !store.shutdown.load(std::sync::atomic::Ordering::Acquire) {
        let candidate:Option<Candidate>=store.db()?.query_row("SELECT data FROM objects WHERE kind='candidate' AND json_extract(data,'$.state')='queued' ORDER BY rowid LIMIT 1",[],|r|r.get::<_,String>(0)).optional()?.map(|s|serde_json::from_str(&s)).transpose()?;
        if let Some(mut c) = candidate {
            c.state = "building".into();
            c.updated_at = now();
            update(&store, &c)?;
            let mut command = tokio::process::Command::new(std::env::current_exe()?);
            command
                .arg("--data-dir")
                .arg(&store.root)
                .arg("candidate-worker")
                .arg(&c.id)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true);
            #[cfg(unix)]
            command.process_group(0);
            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(error) => {
                    c.state = "failed".into();
                    c.updated_at = now();
                    c.diagnostic = Some(
                        json!({"code":"worker_interrupted","message":format!("could not start candidate worker: {error}; restart the service after replacing its binary"),"retryable":true}),
                    );
                    update(&store, &c)?;
                    continue;
                }
            };
            let pid = child.id();
            let _guard = ProcessGroupGuard {
                kind: "candidate_lease".into(),
                pid,
                store: store.clone(),
                id: c.id.clone(),
            };
            if let Some(pid) = pid {
                store.put(
                    "candidate_lease",
                    &c.id,
                    &WorkerLease {
                        pid,
                        start: process_start(pid),
                    },
                )?;
            }
            let started = Instant::now();
            loop {
                let current: Candidate = store.get("candidate", &c.id)?;
                if store.shutdown.load(std::sync::atomic::Ordering::Acquire)
                    || current.state == "cancelled"
                    || started.elapsed() > Duration::from_secs(1920)
                    || current.state == "preflight" && now() - current.updated_at > 120_000
                {
                    let _ = child.kill().await;
                    break;
                }
                if child.try_wait()?.is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let mut current: Candidate = store.get("candidate", &c.id)?;
            if matches!(current.state.as_str(), "building" | "preflight") {
                current.state = if store.shutdown.load(std::sync::atomic::Ordering::Acquire) {
                    "interrupted"
                } else {
                    "failed"
                }
                .into();
                current.diagnostic = Some(
                    json!({"code":"worker_interrupted","message":"worker exited, exceeded its deadline, or service stopped","retryable":true}),
                );
                current.updated_at = now();
                update(&store, &current)?;
            }
        } else {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn input() -> CandidateInput {
        CandidateInput {
            platform: "fixture".into(),
            idempotency_key: "one".into(),
            name: "Candidate".into(),
            description: String::new(),
            parent: None,
            parameters: BTreeMap::new(),
            markets: vec!["BTC-EUR".into()],
            files: BTreeMap::from([("src/main.rs".into(), EXAMPLE.into())]),
            model_fixtures: BTreeMap::new(),
            synthetic_suite: None,
        }
    }
    #[test]
    fn public_kit_contains_exact_preflight_fixtures_and_no_credentials() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path())?;
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let id = prepare_platform(&store, &root)?;
        assert_eq!(prepare_platform(&store, &root)?, id);
        let kit = kit(&store)?;
        assert_eq!(kit["platform"], id);
        assert!(kit["files"]["sdk/crates/tc-sdk/src/lib.rs"].is_string());
        for (name, gaps) in [("regular", false), ("gapped", true)] {
            let fixture: Value = serde_json::from_str(
                kit["files"][format!("fixtures/{name}.json")]
                    .as_str()
                    .unwrap(),
            )?;
            let (config, candles) = synthetic_fixture(gaps)?;
            assert_eq!(fixture["config"], serde_json::to_value(config)?);
            assert_eq!(fixture["candles"], serde_json::to_value(candles)?);
            assert_eq!(fixture["performance_evidence"], false);
        }
        let owner = fs::read_to_string(auth::owner_key_path(&store))?;
        assert!(!serde_json::to_string(&kit)?.contains(owner.trim()));
        Ok(())
    }
    #[test]
    fn admission_rejects_paths_manifests_and_oversized_sources() {
        for path in [
            "Cargo.toml",
            "build.rs",
            "src/../leak.rs",
            "/src/main.rs",
            "src//a.rs",
            "src/a\\b.rs",
        ] {
            let mut i = input();
            i.files.insert(path.into(), "fn main() {}".into());
            assert!(validate(&i).is_err(), "{path}");
        }
        let mut i = input();
        i.files.insert("src/main.rs".into(), "x".repeat(65537));
        assert!(validate(&i).is_err());
        assert!(validate(&input()).is_ok());
    }
    #[test]
    fn admission_is_scoped_idempotent_and_revocation_cancels_publication() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("data"))?;
        store.put(
            "agent_platform",
            "fixture",
            &Platform {
                id: "fixture".into(),
                compiler: "test".into(),
                files: BTreeMap::new(),
            },
        )?;
        store.put("agent_platform", "current", &json!({"id":"fixture"}))?;
        let a = auth::issue_agent(&store, "a", &temp.path().join("a.key"), 30)?;
        let b = auth::issue_agent(&store, "b", &temp.path().join("b.key"), 30)?;
        let a = a["id"].as_str().unwrap();
        let b = b["id"].as_str().unwrap();
        let c = submit(&store, input(), Some(a))?;
        assert_eq!(submit(&store, input(), Some(a))?.id, c.id);
        assert_ne!(submit(&store, input(), Some(b))?.id, c.id);
        assert!(get(&store, &c.id, Some(b)).is_err());
        let mut changed = input();
        changed
            .files
            .insert("src/main.rs".into(), "fn main() {}".into());
        assert!(submit(&store, changed, Some(a)).is_err());
        let mut changed = input();
        let (config, candles) = synthetic_fixture(false)?;
        let candles = candles
            .into_iter()
            .filter(|c| c.market == "BTC-EUR")
            .collect();
        let assertion_time = config.start;
        changed.synthetic_suite = Some(synthetic::Suite {
            version: 1,
            scenarios: vec![synthetic::Scenario {
                name: "behavior".into(),
                config,
                candles,
                assertions: vec![synthetic::Assertion::FramePresent {
                    time: assertion_time,
                    present: true,
                }],
            }],
        });
        assert!(
            submit(&store, changed, Some(a))
                .unwrap_err()
                .to_string()
                .contains("idempotency key")
        );
        auth::revoke_agent(&store, a)?;
        assert_eq!(get(&store, &c.id, Some(a))?.state, "cancelled");
        let db = store.db()?;
        let tx = db.unchecked_transaction()?;
        assert!(active(&tx, Some(a)).is_err());
        drop(tx);
        assert!(submit(&store, input(), Some(a)).is_err());
        // A stale worker cannot overwrite cancellation with success or failure.
        let mut stale = c;
        stale.state = "completed".into();
        update(&store, &stale)?;
        assert_eq!(get(&store, &stale.id, Some(a))?.state, "cancelled");
        assert!(store.objects::<RegisteredStrategy>("strategy")?.is_empty());
        Ok(())
    }
}
