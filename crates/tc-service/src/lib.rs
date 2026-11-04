//! Local control plane: durable jobs, validation ledger, and paper portfolios.
pub mod auth;
mod broker;
mod build;
mod http;
mod inference;
mod leaderboard;
mod paper;
mod promotion;
use anyhow::{Context, Result, ensure};
pub use build::{build_strategy, sandbox_check};
pub use http::serve;
pub use inference::infer;
pub use paper::{PaperSession, run_paper};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tc_core::*;
use tc_data::{digest, read_json, save_json};

#[derive(Clone)]
pub struct Store {
    pub root: PathBuf,
    pub shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
    paper_controls: std::sync::Arc<
        std::sync::Mutex<std::collections::BTreeMap<String, std::sync::Arc<paper::PaperControl>>>,
    >,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegisteredStrategy {
    pub id: String,
    pub spec: StrategySpec,
    pub created_at: i64,
    pub parent: Option<String>,
    pub executable_hash: Option<String>,
    pub source_hash: Option<String>,
    pub archived: bool,
}
/// Owner-reserved final window: metadata may be inspected, financial feedback may not.
pub const FINAL_TEST_START: i64 = 1_785_542_400_000;
pub const FINAL_TEST_END: i64 = 1_793_491_200_000;
pub(crate) fn overlaps_final(start: i64, end: i64) -> bool {
    start < FINAL_TEST_END && end > FINAL_TEST_START
}
fn ensure_research_window(start: i64, end: i64) -> Result<()> {
    ensure!(
        !overlaps_final(start, end),
        "August–October 2026 is reserved for the untouched final test; create a current campaign"
    );
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FinalTestWindow {
    pub start: i64,
    pub end: i64,
    pub range_covered: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Campaign {
    #[serde(default)]
    pub final_test: Option<FinalTestWindow>,
    #[serde(default)]
    pub objective: Objective,
    #[serde(default = "initial_target")]
    pub target_cagr: f64,
    pub id: String,
    pub dataset: String,
    pub development_start: i64,
    pub development_end: i64,
    pub validation_start: i64,
    pub validation_end: i64,
}
fn initial_target() -> f64 {
    0.15
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Period {
    Development,
    Validation,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Submission {
    pub strategy: String,
    pub campaign: String,
    pub period: Period,
    pub idempotency_key: String,
    #[serde(default)]
    pub slippage_bps: Option<u32>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    #[serde(default)]
    pub objective: Objective,
    pub id: String,
    pub strategy: String,
    pub campaign: String,
    pub period: Period,
    pub state: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub progress: f64,
    pub error: Option<String>,
    pub result: Option<Value>,
    pub config: RunConfig,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerRequest {
    #[serde(default)]
    pub objective: Objective,
    pub id: String,
    pub dataset: String,
    pub spec: StrategySpec,
    pub period: Period,
    pub config: RunConfig,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunView {
    pub metrics: Metrics,
    pub equity: Vec<EquityPoint>,
    pub fills: Vec<Fill>,
    pub marker_total: usize,
    pub config: RunConfig,
    pub halted: bool,
    pub stale: bool,
}
impl RunView {
    pub fn from_report(r: &Report) -> Self {
        Self {
            metrics: r.metrics.clone(),
            equity: sample(&r.equity, 1500),
            fills: r.fills.iter().take(200).cloned().collect(),
            marker_total: r.fills.len(),
            config: r.config.clone(),
            halted: r.halted,
            stale: r.stale,
        }
    }
}
impl Store {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(root.as_ref())?;
        let root = fs::canonicalize(root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let meta = fs::metadata(&root)?;
            ensure!(
                root.parent().is_some() && meta.permissions().mode() & 0o1000 == 0,
                "use a dedicated private data directory"
            );
            ensure!(
                meta.uid() == fs::metadata("/proc/self")?.uid(),
                "data directory must be owned by the TC process user"
            );
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        }
        let store = Self {
            root,
            shutdown: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            paper_controls: Default::default(),
        };
        let db = store.db()?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS objects(kind TEXT NOT NULL,id TEXT NOT NULL,data TEXT NOT NULL,PRIMARY KEY(kind,id)); CREATE TABLE IF NOT EXISTS jobs(id TEXT PRIMARY KEY,key TEXT UNIQUE NOT NULL,fingerprint TEXT NOT NULL,data TEXT NOT NULL); CREATE TABLE IF NOT EXISTS validation(fingerprint TEXT PRIMARY KEY,job TEXT NOT NULL); CREATE TABLE IF NOT EXISTS ledger(id INTEGER PRIMARY KEY CHECK(id=1),used INTEGER NOT NULL,maximum INTEGER NOT NULL,revealed INTEGER NOT NULL); INSERT OR IGNORE INTO ledger VALUES(1,0,10,0); CREATE TABLE IF NOT EXISTS paper_events(session TEXT NOT NULL,sequence INTEGER NOT NULL,frame TEXT NOT NULL,fills TEXT NOT NULL,PRIMARY KEY(session,sequence)); CREATE TABLE IF NOT EXISTS model_cache(scope TEXT NOT NULL,key TEXT NOT NULL,response TEXT NOT NULL,PRIMARY KEY(scope,key));")?;
        auth::initialize(&store)?;
        Ok(store)
    }
    pub fn db(&self) -> Result<Connection> {
        let c = Connection::open(self.root.join("tradeclimb.db"))?;
        c.busy_timeout(Duration::from_secs(5))?;
        c.pragma_update(None, "journal_mode", "WAL")?;
        Ok(c)
    }
    pub fn put(&self, kind: &str, id: &str, value: &impl Serialize) -> Result<()> {
        self.db()?.execute("INSERT INTO objects(kind,id,data)VALUES(?1,?2,?3)ON CONFLICT(kind,id)DO UPDATE SET data=excluded.data",params![kind,id,serde_json::to_string(value)?])?;
        Ok(())
    }
    pub fn get<T: serde::de::DeserializeOwned>(&self, kind: &str, id: &str) -> Result<T> {
        let text: String = self
            .db()?
            .query_row(
                "SELECT data FROM objects WHERE kind=?1 AND id=?2",
                params![kind, id],
                |r| r.get(0),
            )
            .with_context(|| format!("unknown {kind}"))?;
        Ok(serde_json::from_str(&text)?)
    }
    pub fn objects<T: serde::de::DeserializeOwned>(&self, kind: &str) -> Result<Vec<T>> {
        let db = self.db()?;
        let mut query = db.prepare("SELECT data FROM objects WHERE kind=?1 ORDER BY rowid DESC")?;
        let rows = query.query_map([kind], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    pub fn register(
        &self,
        spec: StrategySpec,
        parent: Option<String>,
        project: Option<&Path>,
    ) -> Result<RegisteredStrategy> {
        spec.validate()?;
        if let Some(id) = &parent {
            let _: RegisteredStrategy = self.get("strategy", id)?;
        }
        let mut spec = spec;
        let mut executable_hash = None;
        let mut source_hash = None;
        let mut source = Vec::<(String, Vec<u8>)>::new();
        if let StrategyKind::Executable { path } = &spec.kind {
            let bytes = fs::read(path)?;
            let hash = digest(&bytes);
            executable_hash = Some(hash.clone());
            let artifact = self.root.join("strategies").join(&hash);
            fs::create_dir_all(&artifact)?;
            let executable = artifact.join("strategy");
            if !executable.exists() {
                tc_data::atomic_write(&executable, &bytes)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))?;
                }
            }
            spec.kind = StrategyKind::Executable {
                path: executable.to_string_lossy().into(),
            };
            if let Some(project) = project {
                collect_source(project, project, &mut source)?;
                source.sort_by(|a, b| a.0.cmp(&b.0));
                source_hash = Some(digest(&serde_json::to_vec(&source)?));
                let src = artifact
                    .join("source")
                    .join(source_hash.as_ref().context("missing source hash")?);
                for (name, bytes) in &source {
                    tc_data::atomic_write(&src.join(name), bytes)?;
                }
            }
        }
        let id = digest(&serde_json::to_vec(&(
            &spec,
            &executable_hash,
            &source_hash,
        ))?);
        if let Ok(existing) = self.get::<RegisteredStrategy>("strategy", &id) {
            return Ok(existing);
        }
        let registered = RegisteredStrategy {
            id: id.clone(),
            spec,
            created_at: now(),
            parent,
            executable_hash,
            source_hash,
            archived: false,
        };
        self.put("strategy", &id, &registered)?;
        Ok(registered)
    }
    pub fn resolve_strategy(&self, registered: &RegisteredStrategy) -> Result<StrategySpec> {
        let mut spec = registered.spec.clone();
        if let Some(hash) = &registered.executable_hash {
            let path = self.root.join("strategies").join(hash).join("strategy");
            ensure!(
                digest(&fs::read(&path)?) == *hash,
                "strategy executable checksum mismatch"
            );
            spec.kind = StrategyKind::Executable {
                path: path.to_string_lossy().into(),
            };
        }
        Ok(spec)
    }
    pub fn campaign(&self, dataset: &str) -> Result<Campaign> {
        self.campaign_with_target(dataset, 0.15)
    }
    pub fn campaign_with_target(&self, dataset: &str, target_cagr: f64) -> Result<Campaign> {
        self.campaign_with_objective(dataset, target_cagr, Objective::SharpeDailyV1)
    }
    pub fn campaign_with_objective(
        &self,
        dataset: &str,
        target_cagr: f64,
        objective: Objective,
    ) -> Result<Campaign> {
        ensure!(
            target_cagr.is_finite() && target_cagr > 0.0 && target_cagr <= 10.0,
            "target CAGR must be a positive fraction at most 10"
        );
        let manifest = tc_data::list(&self.root)?
            .into_iter()
            .find(|m| m.id == dataset)
            .context("unknown dataset")?;
        ensure!(
            manifest.accepted,
            "dataset failed coverage gate; inspect and explicitly accept gaps during import if appropriate"
        );
        let full =
            manifest.start <= timestamp("2022-11-01")? && manifest.end >= timestamp("2026-11-01")?;
        let start = if full {
            timestamp("2022-11-01")?
        } else {
            manifest.start
        };
        let end = if full {
            timestamp("2026-11-01")?
        } else {
            manifest.end
        };
        let final_test = overlaps_final(start, end).then_some(FinalTestWindow {
            start: FINAL_TEST_START,
            end: FINAL_TEST_END,
            range_covered: manifest.start <= FINAL_TEST_START && manifest.end >= FINAL_TEST_END,
        });
        let end = if final_test.is_some() {
            end.min(FINAL_TEST_START)
        } else {
            end
        };
        ensure!(
            end > start,
            "dataset has no research interval before the reserved final test"
        );
        let desired = timestamp("2025-11-01")?;
        let split = if full {
            desired
        } else {
            start + ((end - start) / MINUTE * 3 / 4) * MINUTE
        };
        ensure!(split > start && split < end, "dataset too short to split");
        let id = digest(&serde_json::to_vec(&(
            dataset,
            start,
            split,
            end,
            target_cagr,
            objective,
            &final_test,
        ))?);
        let c = Campaign {
            final_test,
            objective,
            target_cagr,
            id: id.clone(),
            dataset: dataset.into(),
            development_start: start,
            development_end: split,
            validation_start: split,
            validation_end: end,
        };
        self.put("campaign", &id, &c)?;
        Ok(c)
    }
    pub fn jobs(&self) -> Result<Vec<Job>> {
        let db = self.db()?;
        let mut q = db.prepare("SELECT data FROM jobs ORDER BY rowid DESC LIMIT 500")?;
        q.query_map([], |r| r.get::<_, String>(0))?
            .map(|r| {
                let mut job: Job = serde_json::from_str(&r?)?;
                if overlaps_final(job.config.start, job.config.end) {
                    job.result = None;
                    job.progress = 0.0;
                }
                Ok(job)
            })
            .collect()
    }
    pub fn job(&self, id: &str) -> Result<Job> {
        let text: String =
            self.db()?
                .query_row("SELECT data FROM jobs WHERE id=?1", [id], |r| r.get(0))?;
        Ok(serde_json::from_str(&text)?)
    }
    pub fn update_job(&self, job: &Job) -> Result<()> {
        self.db()?.execute(
            "UPDATE jobs SET data=?1 WHERE id=?2 AND (json_extract(data,'$.state') != 'cancelled' OR json_extract(?1,'$.state') = 'cancelled')",
            params![serde_json::to_string(job)?, job.id],
        )?;
        Ok(())
    }
    pub fn submit(&self, request: Submission) -> Result<Job> {
        ensure!(
            !request.idempotency_key.is_empty() && request.idempotency_key.len() <= 200,
            "idempotency key required"
        );
        let strategy: RegisteredStrategy = self.get("strategy", &request.strategy)?;
        ensure!(!strategy.archived, "strategy archived");
        let campaign: Campaign = self.get("campaign", &request.campaign)?;
        let (start, end) = match request.period {
            Period::Development => (campaign.development_start, campaign.development_end),
            Period::Validation => (campaign.validation_start, campaign.validation_end),
        };
        let mut config = RunConfig::new(start, end);
        if let Some(bps) = request.slippage_bps {
            ensure!(
                request.period == Period::Development,
                "validation cost assumptions are frozen"
            );
            config.slippage_bps = bps;
        }
        config.validate()?;
        ensure_research_window(config.start, config.end)?;
        let fingerprint_inputs =
            serde_json::to_vec(&(&strategy.id, &campaign.dataset, &request.period, &config))?;
        // Preserve V1 cache identities; isolate V2 scoring feedback from old runs.
        let fingerprint = if campaign.objective == Objective::NetCagr {
            digest(&fingerprint_inputs)
        } else {
            digest(&serde_json::to_vec(&(
                fingerprint_inputs,
                &campaign.id,
                campaign.objective,
            ))?)
        };
        let mut db = self.db()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT fingerprint,data FROM jobs WHERE key=?1",
                [&request.idempotency_key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((old, data)) = existing {
            ensure!(
                old == fingerprint,
                "idempotency key already used for different inputs"
            );
            return Ok(serde_json::from_str(&data)?);
        }
        if request.period == Period::Validation {
            let existing: Option<String> = tx
                .query_row(
                    "SELECT job FROM validation WHERE fingerprint=?1",
                    [&fingerprint],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(id) = existing {
                drop(tx);
                return self.job(&id);
            }
            let used: i64 = tx.query_row("SELECT used FROM ledger WHERE id=1", [], |r| r.get(0))?;
            let maximum: i64 =
                tx.query_row("SELECT maximum FROM ledger WHERE id=1", [], |r| r.get(0))?;
            ensure!(used < maximum, "validation budget exhausted");
            tx.execute("UPDATE ledger SET used=used+1 WHERE id=1", [])?;
        }
        let queued:i64=tx.query_row("SELECT COUNT(*) FROM jobs WHERE json_extract(data,'$.state') IN ('queued','running','preparing')",[],|r|r.get(0))?;
        ensure!(queued < 200, "job queue full");
        let id = uuid::Uuid::new_v4().to_string();
        let job = Job {
            objective: campaign.objective,
            id: id.clone(),
            strategy: strategy.id.clone(),
            campaign: campaign.id,
            period: request.period.clone(),
            state: "queued".into(),
            created_at: now(),
            updated_at: now(),
            progress: 0.0,
            error: None,
            result: None,
            config: config.clone(),
        };
        let worker = WorkerRequest {
            objective: campaign.objective,
            id: id.clone(),
            dataset: campaign.dataset,
            spec: self.resolve_strategy(&strategy)?,
            period: request.period.clone(),
            config,
        };
        save_json(
            &self.root.join("jobs").join(&id).join("request.json"),
            &worker,
        )?;
        tx.execute(
            "INSERT INTO jobs VALUES(?1,?2,?3,?4)",
            params![
                id,
                request.idempotency_key,
                fingerprint,
                serde_json::to_string(&job)?
            ],
        )?;
        if request.period == Period::Validation {
            tx.execute(
                "INSERT INTO validation VALUES(?1,?2)",
                params![fingerprint, id],
            )?;
        }
        tx.commit()?;
        Ok(job)
    }
    pub fn ledger(&self) -> Result<Value> {
        Ok(self.db()?.query_row("SELECT used,maximum,revealed FROM ledger WHERE id=1",[],|r|Ok(json!({"used":r.get::<_,i64>(0)?,"maximum":r.get::<_,i64>(1)?,"revealed":r.get::<_,bool>(2)?})))?)
    }
    pub fn extend_budget(&self, additional: u32) -> Result<()> {
        ensure!(
            additional > 0 && additional <= 100,
            "invalid budget extension"
        );
        self.db()?.execute(
            "UPDATE ledger SET maximum=maximum+?1 WHERE id=1",
            [additional],
        )?;
        self.put(
            "audit",
            &uuid::Uuid::new_v4().to_string(),
            &json!({"time":now(),"action":"extend_validation_budget","additional":additional}),
        )
    }
    pub fn owner_report(&self, id: &str) -> Result<Report> {
        let j = self.job(id)?;
        ensure_research_window(j.config.start, j.config.end)?;
        if j.period == Period::Validation {
            self.db()?
                .execute("UPDATE ledger SET revealed=1 WHERE id=1", [])?;
            self.put(
                "audit",
                &uuid::Uuid::new_v4().to_string(),
                &json!({"time":now(),"action":"owner_revealed_validation","job":id}),
            )?;
        }
        read_report(&self.root.join("jobs").join(id).join("report.json.gz"))
    }
    pub fn cancel(&self, id: &str) -> Result<Job> {
        let mut j = self.job(id)?;
        ensure!(
            ["queued", "preparing", "running"].contains(&j.state.as_str()),
            "job is not active"
        );
        j.state = "cancelled".into();
        j.updated_at = now();
        self.update_job(&j)?;
        Ok(j)
    }
    pub fn backup(&self, destination: &Path) -> Result<()> {
        ensure!(!destination.exists(), "backup destination already exists");
        fs::create_dir_all(destination)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(destination, fs::Permissions::from_mode(0o700))?;
        }
        let db = self.db()?;
        let mut target = Connection::open(destination.join("tradeclimb.db"))?;
        let backup = rusqlite::backup::Backup::new(&db, &mut target)?;
        backup.run_to_completion(100, Duration::from_millis(10), None)?;
        drop(backup);
        target.execute(
            "DELETE FROM objects WHERE kind IN ('service_owner','worker_lease','paper_lease')",
            [],
        )?;
        for name in ["datasets", "strategies", "jobs"] {
            copy_tree(&self.root.join(name), &destination.join(name))?;
        }
        Ok(())
    }
}
fn collect_source(root: &Path, path: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    use rustix::fs::{CWD, Mode, OFlags, ResolveFlags, openat2};
    use std::os::fd::AsRawFd;
    ensure!(root == path, "snapshot must start at its root");
    // Kernel path resolution rejects symlinks at every component, including
    // concurrent directory swaps by an untrusted agent. No unsafe fallback.
    let directory = openat2(
        CWD,
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
    )?;
    fn visit(
        directory: &std::os::fd::OwnedFd,
        relative: &Path,
        out: &mut Vec<(String, Vec<u8>)>,
        total: &mut usize,
        entries: &mut usize,
        depth: usize,
    ) -> Result<()> {
        ensure!(depth <= 64, "strategy source nesting exceeds limit");
        let fd = openat2(
            directory,
            if relative.as_os_str().is_empty() {
                Path::new(".")
            } else {
                relative
            },
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )?;
        let capability_path = PathBuf::from(format!("/proc/self/fd/{}", fd.as_raw_fd()));
        ensure!(
            fs::symlink_metadata(capability_path.join("tradeclimb.db")).is_err(),
            "private TC data directories cannot be strategy source"
        );
        for entry in fs::read_dir(&capability_path)? {
            let name = entry?.file_name();
            *entries += 1;
            ensure!(*entries <= 20_000, "too many source entries");
            if ["target", ".git", ".env", "var"].contains(&name.to_string_lossy().as_ref())
                || name.to_string_lossy().starts_with(".env.")
            {
                continue;
            }
            let path = relative.join(name);
            let opened = openat2(
                directory,
                &path,
                OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
                ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
            )?;
            let file = fs::File::from(opened);
            let metadata = file.metadata()?;
            if metadata.is_dir() {
                visit(directory, &path, out, total, entries, depth + 1)?;
                continue;
            }
            ensure!(
                metadata.is_file(),
                "strategy source must contain only regular files"
            );
            ensure!(out.len() < 10_000, "too many strategy source files");
            let mut bytes = vec![];
            file.take(10 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
            ensure!(bytes.len() <= 10 * 1024 * 1024, "source file too large");
            *total += bytes.len();
            ensure!(*total <= 104_857_600, "strategy source exceeds 100 MiB");
            out.push((path.to_string_lossy().into(), bytes));
        }
        Ok(())
    }
    visit(&directory, Path::new(""), out, &mut 0, &mut 0, 0)
}
fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }
    fs::create_dir_all(destination)?;
    for e in fs::read_dir(source)? {
        let e = e?;
        if e.file_type()?.is_dir() {
            copy_tree(&e.path(), &destination.join(e.file_name()))?;
        } else {
            fs::copy(e.path(), destination.join(e.file_name()))?;
        }
    }
    Ok(())
}
pub fn read_report(path: &Path) -> Result<Report> {
    let mut bytes = Vec::new();
    flate_read(path, &mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}
fn flate_read(path: &Path, bytes: &mut Vec<u8>) -> Result<()> {
    use flate2::read::GzDecoder;
    GzDecoder::new(fs::File::open(path)?).read_to_end(bytes)?;
    Ok(())
}
fn research_observation(time: i64, config: &RunConfig) -> bool {
    time >= config.start - 1440 * MINUTE
        && time < config.end
        && !overlaps_final(time, time + MINUTE)
}
pub fn worker(root: &Path, id: &str) -> Result<()> {
    let directory = root.join("jobs").join(id);
    let mut request: WorkerRequest = read_json(&directory.join("request.json"))?;
    ensure_research_window(request.config.start, request.config.end)?;
    let store = Store::open(root)?;
    let job = store.job(id)?;
    let registered: RegisteredStrategy = store.get("strategy", &job.strategy)?;
    request.spec = store.resolve_strategy(&registered)?;
    let (_, candles) = tc_data::load(root, &request.dataset)?;
    let relevant: Vec<_> = candles
        .into_iter()
        .filter(|c| research_observation(c.time, &request.config))
        .collect();
    let _broker = matches!(request.spec.kind, StrategyKind::Executable { .. })
        .then(|| broker::ModelBroker::start(store.clone(), id))
        .transpose()?;
    let mut strategy: Box<dyn tc_sdk::Strategy> = match &request.spec.kind {
        StrategyKind::Executable { path } => {
            Box::new(tc_engine::ProcessStrategy::spawn_with_broker(
                path,
                &request.spec,
                _broker.as_ref().map(|b| b.path()),
            )?)
        }
        _ => tc_engine::strategy(&request.spec)?,
    };
    let mut last = Instant::now() - Duration::from_secs(1);
    let report = tc_engine::backtest(
        &relevant,
        request.config,
        strategy.as_mut(),
        |index, total, sim| {
            if last.elapsed() >= Duration::from_millis(500) {
                if request.period == Period::Development {
                    save_json(
                        &directory.join("progress.json"),
                        &json!({"progress":index as f64/total.max(1)as f64,"equity":sample(&sim.equity,1500)}),
                    )?;
                }
                last = Instant::now();
            }
            Ok(())
        },
    )?;
    let mut zip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    serde_json::to_writer(&mut zip, &report)?;
    tc_data::atomic_write(&directory.join("report.json.gz"), &zip.finish()?)?;
    let result = if request.period == Period::Validation {
        let metrics = serde_json::to_value(&report.metrics)?;
        json!({request.objective.metric():metrics[request.objective.metric()]})
    } else {
        let mut value = serde_json::to_value(RunView::from_report(&report))?;
        let mut benchmarks = vec![];
        for (name, markets) in [
            ("BTC buy & hold", vec!["BTC-EUR".to_string()]),
            (
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
                &relevant,
                config,
                &mut tc_sdk::Builtin::new(&spec)?,
                |_, _, _| Ok(()),
            )?;
            benchmarks.push(json!({"name":name,"equity":sample(&benchmark.equity,1500),"net_cagr":benchmark.metrics.net_cagr,"max_drawdown":benchmark.metrics.max_drawdown}));
        }
        value["benchmarks"] = json!(benchmarks);
        value
    };
    save_json(&directory.join("result.json"), &result)?;
    Ok(())
}
fn summary_result(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.remove("equity");
        object.remove("fills");
        object.remove("benchmarks");
    }
    value
}
#[derive(Serialize, Deserialize)]
struct WorkerLease {
    pid: u32,
    start: Option<String>,
}
fn process_start(pid: u32) -> Option<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)
        .map(str::to_string)
}
struct ProcessGroupGuard {
    kind: String,
    pid: Option<u32>,
    store: Store,
    id: String,
}
impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", "--", &format!("-{pid}")])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        if let Ok(db) = self.store.db() {
            let _ = db.execute(
                "DELETE FROM objects WHERE kind=?1 AND id=?2",
                params![self.kind, self.id],
            );
        }
    }
}
struct ServiceOwnership(Store);
impl ServiceOwnership {
    fn acquire(store: &Store) -> Result<Self> {
        let mut db = store.db()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prior: Option<String> = tx
            .query_row(
                "SELECT data FROM objects WHERE kind='service_owner' AND id='singleton'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(prior) = prior {
            let lease: WorkerLease = serde_json::from_str(&prior)?;
            ensure!(
                !(lease.start.is_some() && process_start(lease.pid) == lease.start),
                "another service already owns this data directory"
            );
        }
        let pid = std::process::id();
        let lease = WorkerLease {
            pid,
            start: process_start(pid),
        };
        tx.execute(
            "INSERT OR REPLACE INTO objects VALUES('service_owner','singleton',?1)",
            [serde_json::to_string(&lease)?],
        )?;
        tx.commit()?;
        Ok(Self(store.clone()))
    }
}
impl Drop for ServiceOwnership {
    fn drop(&mut self) {
        if let Ok(db) = self.0.db() {
            let _ = db.execute(
                "DELETE FROM objects WHERE kind='service_owner' AND id='singleton'",
                [],
            );
        }
    }
}
pub async fn supervisor(store: Store, workers: usize) -> Result<()> {
    for mut tuning in store.objects::<Value>("tuning")? {
        if tuning["state"] == "running" {
            tuning["state"] = json!("interrupted");
            if let Some(id) = tuning["id"].as_str() {
                store.put("tuning", id, &tuning)?;
            }
        }
    }
    for mut j in store.jobs()? {
        if ["running", "preparing"].contains(&j.state.as_str()) {
            j.state = "interrupted".into();
            j.error = Some("service restarted; submit a new run to retry".into());
            store.update_job(&j)?;
        }
    }
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(workers.clamp(1, 8)));
    let mut tasks = tokio::task::JoinSet::new();
    let mut running = std::collections::BTreeSet::new();
    loop {
        if store.shutdown.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        while tasks.try_join_next().is_some() {}
        for job in store.jobs()?.into_iter().rev() {
            if job.state != "queued" || running.contains(&job.id) {
                continue;
            }
            let Ok(permit) = semaphore.clone().try_acquire_owned() else {
                break;
            };
            running.insert(job.id.clone());
            let store = store.clone();
            tasks.spawn(async move {
                let _permit = permit;
                if let Err(e) = supervise_one(&store, &job.id).await {
                    tracing::error!(job=%job.id,error=%e,"worker supervisor failed");
                    if let Ok(mut j) = store.job(&job.id) {
                        j.state = "failed".into();
                        j.error = Some("worker failed; inspect locally".into());
                        let _ = store.update_job(&j);
                    }
                }
            });
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    for mut job in store.jobs()? {
        if matches!(job.state.as_str(), "running" | "preparing") {
            job.state = "interrupted".into();
            job.error = Some("service stopped; submit a new run to retry".into());
            store.update_job(&job)?;
        }
    }
    Ok(())
}

async fn supervise_one(store: &Store, id: &str) -> Result<()> {
    let mut job = store.job(id)?;
    if job.state != "queued" {
        return Ok(());
    }
    job.state = "running".into();
    job.updated_at = now();
    store.update_job(&job)?;
    let executable = std::env::current_exe()?;
    let mut command = tokio::process::Command::new(executable);
    command
        .env("TC_MODEL_SCOPE", id)
        .arg("--data-dir")
        .arg(&store.root)
        .arg("worker")
        .arg(id)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn()?;
    let pid = child.id();
    let _group = ProcessGroupGuard {
        kind: "worker_lease".into(),
        pid,
        store: store.clone(),
        id: id.into(),
    };
    if let Some(pid) = pid {
        store.put(
            "worker_lease",
            id,
            &WorkerLease {
                pid,
                start: process_start(pid),
            },
        )?;
    }
    let started = Instant::now();
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let mut current = store.job(id)?;
        if current.state == "cancelled" || started.elapsed() > Duration::from_secs(1800) {
            if let Some(pid) = pid {
                #[cfg(unix)]
                {
                    let _ = tokio::process::Command::new("kill")
                        .args(["-TERM", "--", &format!("-{pid}")])
                        .status()
                        .await;
                }
            }
            let _ = child.kill().await;
            let _ = child.wait().await;
            if current.state != "cancelled" {
                current.state = "failed".into();
                current.error = Some("worker deadline exceeded".into());
                store.update_job(&current)?;
            }
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            current.updated_at = now();
            if status.success() {
                let result: Value =
                    read_json(&store.root.join("jobs").join(id).join("result.json"))?;
                current.result = Some(summary_result(result));
                current.progress = 1.0;
                current.state = "completed".into();
            } else {
                current.state = "failed".into();
                current.error = Some("worker failed; inspect locally".into());
            }
            store.update_job(&current)?;
            return Ok(());
        }
        if current.period == Period::Development
            && let Ok(v) =
                read_json::<Value>(&store.root.join("jobs").join(id).join("progress.json"))
        {
            current.progress = v["progress"].as_f64().unwrap_or(0.0);
            current.result = Some(summary_result(v));
            current.updated_at = now();
            store.update_job(&current)?;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn setup() -> (tempfile::TempDir, Store, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let s = store
            .register(
                StrategySpec {
                    parameters: Default::default(),
                    name: "Hold".into(),
                    description: "Fixture".into(),
                    kind: StrategyKind::BuyHold,
                    markets: vec!["BTC-EUR".into()],
                },
                None,
                None,
            )
            .unwrap();
        let c = Campaign {
            final_test: None,
            objective: Objective::NetCagr,
            target_cagr: 0.15,
            id: "campaign".into(),
            dataset: "fixture".into(),
            development_start: 0,
            development_end: 10 * MINUTE,
            validation_start: 10 * MINUTE,
            validation_end: 20 * MINUTE,
        };
        store.put("campaign", &c.id, &c).unwrap();
        (dir, store, s.id, c.id)
    }
    fn request(strategy: &str, campaign: &str, key: &str) -> Submission {
        Submission {
            strategy: strategy.into(),
            campaign: campaign.into(),
            period: Period::Validation,
            idempotency_key: key.into(),
            slippage_bps: None,
        }
    }
    #[test]
    fn post_holdout_warmup_never_delivers_reserved_candles() {
        let config = RunConfig::new(FINAL_TEST_END, FINAL_TEST_END + 2 * MINUTE);
        let times = [
            FINAL_TEST_END - 2 * MINUTE,
            FINAL_TEST_END - MINUTE,
            FINAL_TEST_END,
            FINAL_TEST_END + MINUTE,
            FINAL_TEST_END + 2 * MINUTE,
        ];
        let delivered: Vec<_> = times
            .into_iter()
            .filter(|t| research_observation(*t, &config))
            .collect();
        assert_eq!(delivered, [FINAL_TEST_END, FINAL_TEST_END + MINUTE]);
    }
    #[test]
    fn full_campaign_reserves_final_quarter_and_preserves_development() {
        let (_dir, store, _, _) = setup();
        let start = timestamp("2022-10-31").unwrap();
        let manifest =
            tc_data::snapshot(&store.root, vec![], start, FINAL_TEST_END, true, true).unwrap();
        for objective in [Objective::NetCagr, Objective::SharpeDailyV1] {
            let campaign = store
                .campaign_with_objective(&manifest.id, 0.15, objective)
                .unwrap();
            assert_eq!(campaign.development_start, timestamp("2022-11-01").unwrap());
            assert_eq!(campaign.development_end, timestamp("2025-11-01").unwrap());
            assert_eq!(campaign.validation_start, campaign.development_end);
            assert_eq!(campaign.validation_end, FINAL_TEST_START);
            let holdout = campaign.final_test.unwrap();
            assert_eq!(holdout.start, FINAL_TEST_START);
            assert_eq!(holdout.end, FINAL_TEST_END);
            assert!(holdout.range_covered);
        }
    }
    #[test]
    fn legacy_or_forged_campaign_cannot_evaluate_reserved_dates_or_spend_budget() {
        let (_dir, store, strategy, id) = setup();
        let mut campaign: Campaign = store.get("campaign", &id).unwrap();
        campaign.development_start = FINAL_TEST_START - MINUTE;
        campaign.development_end = FINAL_TEST_START + MINUTE;
        campaign.validation_start = FINAL_TEST_START;
        campaign.validation_end = FINAL_TEST_END;
        store.put("campaign", &id, &campaign).unwrap();
        let request = request(&strategy, &id, "holdout");
        assert!(store.submit(request.clone()).is_err());
        assert!(
            store
                .submit(Submission {
                    period: Period::Development,
                    ..request
                })
                .is_err()
        );
        assert_eq!(store.ledger().unwrap()["used"], 0);
        assert!(!overlaps_final(FINAL_TEST_START - MINUTE, FINAL_TEST_START));
        assert!(!overlaps_final(FINAL_TEST_END, FINAL_TEST_END + MINUTE));
        // Post-holdout warmup frames must not carry any of the reserved minutes.
        assert!(overlaps_final(FINAL_TEST_END - MINUTE, FINAL_TEST_END));
        assert!(!overlaps_final(FINAL_TEST_END, FINAL_TEST_END + MINUTE));
    }
    #[test]
    fn old_reserved_jobs_are_redacted_and_cannot_be_inspected_or_restarted() {
        let (_dir, store, strategy, campaign) = setup();
        let mut job = store.submit(request(&strategy, &campaign, "old")).unwrap();
        job.config = RunConfig::new(FINAL_TEST_START, FINAL_TEST_END);
        job.state = "completed".into();
        job.progress = 1.0;
        job.result = Some(json!({"sharpe":99.0,"equity":"SECRET"}));
        store.update_job(&job).unwrap();
        assert!(store.jobs().unwrap()[0].result.is_none());
        assert_eq!(store.jobs().unwrap()[0].progress, 0.0);
        assert!(store.owner_report(&job.id).is_err());
        let path = store.root.join("jobs").join(&job.id).join("request.json");
        let mut request: WorkerRequest = read_json(&path).unwrap();
        request.config = job.config;
        save_json(&path, &request).unwrap();
        assert!(
            worker(&store.root, &job.id)
                .unwrap_err()
                .to_string()
                .contains("untouched final test")
        );
    }
    #[test]
    fn legacy_objectives_are_not_reinterpreted() {
        let (_dir, store, strategy, campaign) = setup();
        let c: Campaign = store.get("campaign", &campaign).unwrap();
        let mut value = serde_json::to_value(c).unwrap();
        value.as_object_mut().unwrap().remove("objective");
        let old: Campaign = serde_json::from_value(value).unwrap();
        assert_eq!(old.objective, Objective::NetCagr);
        let j = store
            .submit(request(&strategy, &campaign, "legacy"))
            .unwrap();
        let mut value = serde_json::to_value(j).unwrap();
        value.as_object_mut().unwrap().remove("objective");
        assert_eq!(
            serde_json::from_value::<Job>(value).unwrap().objective,
            Objective::NetCagr
        );
    }
    #[test]
    fn objective_separates_validation_feedback_but_not_global_budget() {
        let (_dir, store, strategy, campaign) = setup();
        let legacy = store
            .submit(request(&strategy, &campaign, "legacy"))
            .unwrap();
        let mut c: Campaign = store.get("campaign", &campaign).unwrap();
        c.id = "sharpe-campaign".into();
        c.objective = Objective::SharpeDailyV1;
        store.put("campaign", &c.id, &c).unwrap();
        let sharpe = store.submit(request(&strategy, &c.id, "sharpe")).unwrap();
        assert_ne!(legacy.id, sharpe.id);
        assert_eq!(sharpe.objective, Objective::SharpeDailyV1);
        let persisted: WorkerRequest = read_json(
            &store
                .root
                .join("jobs")
                .join(&sharpe.id)
                .join("request.json"),
        )
        .unwrap();
        assert_eq!(persisted.objective, Objective::SharpeDailyV1);
        assert_eq!(
            store.submit(request(&strategy, &c.id, "again")).unwrap().id,
            sharpe.id
        );
        assert_eq!(store.ledger().unwrap()["used"], 2);
    }
    #[test]
    fn validation_cache_and_idempotency_are_durable() {
        let (_dir, store, strategy, campaign) = setup();
        let first = store
            .submit(request(&strategy, &campaign, "first"))
            .unwrap();
        let repeat = store
            .submit(request(&strategy, &campaign, "second"))
            .unwrap();
        assert_eq!(first.id, repeat.id);
        assert_eq!(store.ledger().unwrap()["used"], 1);
        let reopened = Store::open(&store.root).unwrap();
        assert_eq!(
            reopened
                .submit(request(&strategy, &campaign, "third"))
                .unwrap()
                .id,
            first.id
        );
        assert_eq!(reopened.ledger().unwrap()["used"], 1);
        let mut mismatch = request(&strategy, &campaign, "first");
        mismatch.period = Period::Development;
        assert!(store.submit(mismatch).is_err());
    }
    #[test]
    fn validation_budget_is_global_and_race_safe() {
        let (_dir, store, _, campaign) = setup();
        let handles: Vec<_> = (0..16)
            .map(|i| {
                let store = store.clone();
                let campaign = campaign.clone();
                std::thread::spawn(move || {
                    let s = store
                        .register(
                            StrategySpec {
                                parameters: Default::default(),
                                name: format!("version {i}"),
                                description: "Fixture".into(),
                                kind: StrategyKind::MovingAverage {
                                    fast: i + 1,
                                    slow: 50,
                                },
                                markets: vec!["BTC-EUR".into()],
                            },
                            None,
                            None,
                        )
                        .unwrap();
                    store
                        .submit(request(&s.id, &campaign, &format!("candidate {i}")))
                        .is_ok()
                })
            })
            .collect();
        let success = handles
            .into_iter()
            .filter_map(|h| h.join().ok())
            .filter(|v| *v)
            .count();
        assert_eq!(success, 10);
        assert_eq!(store.ledger().unwrap()["used"], 10);
    }
    #[test]
    fn only_one_service_can_own_a_data_directory() {
        let (_dir, store, _, _) = setup();
        let guard = ServiceOwnership::acquire(&store).unwrap();
        assert!(ServiceOwnership::acquire(&store).is_err());
        let destination = store.root.join("copy");
        store.backup(&destination).unwrap();
        let copied = Store::open(destination).unwrap();
        let _other = ServiceOwnership::acquire(&copied).unwrap();
        drop(guard);
        assert!(ServiceOwnership::acquire(&store).is_ok());
    }
    #[test]
    fn cancellation_cannot_be_overwritten_by_stale_worker_state() {
        let (_dir, store, strategy, campaign) = setup();
        let mut job = store
            .submit(request(&strategy, &campaign, "cancel-race"))
            .unwrap();
        store.cancel(&job.id).unwrap();
        job.state = "completed".into();
        store.update_job(&job).unwrap();
        assert_eq!(store.job(&job.id).unwrap().state, "cancelled");
    }
    #[test]
    fn sqlite_backup_restores_ledger_and_registry() {
        let (dir, store, strategy, _) = setup();
        store.extend_budget(2).unwrap();
        let destination = dir.path().join("backup");
        store.backup(&destination).unwrap();
        let restored = Store::open(destination).unwrap();
        let s: RegisteredStrategy = restored.get("strategy", &strategy).unwrap();
        assert_eq!(s.id, strategy);
        assert_eq!(restored.ledger().unwrap()["maximum"], 12);
    }
}
