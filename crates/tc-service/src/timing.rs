//! Durable local timing. This is not a remote event collector or the Python SDK database.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TimingContext {
    pub run_id: String,
    pub parent_step_id: String,
}
impl TimingContext {
    pub(crate) fn validate(&self) -> Result<()> {
        for id in [&self.run_id, &self.parent_step_id] {
            ensure!(
                !id.is_empty() && id.len() <= 200 && !id.chars().any(char::is_control),
                "invalid timing context ID"
            );
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Step {
    pub step_id: String,
    pub parent_step_id: String,
    pub name: String,
    pub category: String,
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub duration_ms: Option<u64>,
    pub duration_source: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Timing {
    pub context: TimingContext,
    pub tc_job_id: String,
    pub metadata: Value,
    pub steps: Vec<Step>,
}
pub(crate) fn create(
    tx: &rusqlite::Transaction<'_>,
    job: &Job,
    context: &TimingContext,
    dataset: &str,
    spec: &StrategySpec,
) -> Result<()> {
    let timing = Timing {
        context: context.clone(),
        tc_job_id: job.id.clone(),
        metadata: json!({"dataset":dataset,"strategy":job.strategy,"campaign":job.campaign,
            "config":job.config,"objective":job.objective,"period":job.period,
            "instrument_count":spec.markets.len(),"runner_version":env!("CARGO_PKG_VERSION"),
            "runner_commit":option_env!("TC_RUNNER_COMMIT"),"worker_size":"one process",
            "cache_status":"uncached; OS page cache unknown"}),
        steps: vec![Step {
            step_id: uuid::Uuid::new_v4().to_string(),
            parent_step_id: context.parent_step_id.clone(),
            name: "queued".into(),
            category: "queue".into(),
            status: "running".into(),
            started_at: job.created_at,
            finished_at: None,
            duration_ms: None,
            duration_source: None,
        }],
    };
    tx.execute(
        "INSERT INTO objects VALUES('job_timing',?1,?2)",
        params![job.id, serde_json::to_string(&timing)?],
    )?;
    Ok(())
}
fn load(tx: &rusqlite::Transaction<'_>, id: &str) -> Result<Option<Timing>> {
    let data: Option<String> = tx
        .query_row(
            "SELECT data FROM objects WHERE kind='job_timing' AND id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    data.map(|s| Ok(serde_json::from_str(&s)?)).transpose()
}
fn save(tx: &rusqlite::Transaction<'_>, timing: &Timing) -> Result<()> {
    tx.execute(
        "UPDATE objects SET data=?1 WHERE kind='job_timing' AND id=?2",
        params![serde_json::to_string(timing)?, timing.tc_job_id],
    )?;
    Ok(())
}
fn close(step: &mut Step, status: &str, duration: Option<Duration>) {
    let end = now();
    step.duration_source = Some(
        if duration.is_some() {
            "monotonic"
        } else {
            "wall_clock"
        }
        .into(),
    );
    step.status = status.into();
    step.finished_at = Some(end);
    step.duration_ms = Some(
        duration
            .map(|d| d.as_millis().min(u64::MAX as u128) as u64)
            .unwrap_or_else(|| end.saturating_sub(step.started_at).max(0) as u64),
    );
}
pub(crate) fn terminal(tx: &rusqlite::Transaction<'_>, id: &str, state: &str) -> Result<()> {
    if let Some(mut timing) = load(tx, id)? {
        let status = match state {
            "completed" => "succeeded",
            "cancelled" => "cancelled",
            "interrupted" => "interrupted",
            "timed_out" => "timed_out",
            _ => "failed",
        };
        for step in &mut timing.steps {
            if step.status == "running" {
                close(step, status, None);
            }
        }
        save(tx, &timing)?;
    }
    Ok(())
}
/// A phase closes on ordinary errors/unwinding; the supervisor reconciles killed workers.
pub(crate) struct Phase {
    store: Store,
    job: String,
    id: Option<String>,
    started: Instant,
}
impl Phase {
    pub(crate) fn start(store: &Store, job: &str, name: &str) -> Result<Self> {
        let mut db = store.db()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut id = None;
        if let Some(mut timing) = load(&tx, job)? {
            let state: String = tx.query_row(
                "SELECT json_extract(data,'$.state') FROM jobs WHERE id=?1",
                [job],
                |r| r.get(0),
            )?;
            ensure!(
                matches!(state.as_str(), "queued" | "preparing" | "running"),
                "job is terminal"
            );
            for step in &mut timing.steps {
                if step.name == "queued" && step.status == "running" {
                    close(step, "succeeded", None);
                }
            }
            let step_id = uuid::Uuid::new_v4().to_string();
            tracing::info!(job, step_id, run_id=%timing.context.run_id, phase=name, "timing phase started");
            timing.steps.push(Step {
                step_id: step_id.clone(),
                parent_step_id: timing.context.parent_step_id.clone(),
                name: name.into(),
                category: "execution".into(),
                status: "running".into(),
                started_at: now(),
                finished_at: None,
                duration_ms: None,
                duration_source: None,
            });
            save(&tx, &timing)?;
            id = Some(step_id);
        }
        tx.commit()?;
        Ok(Self {
            store: store.clone(),
            job: job.into(),
            id,
            started: Instant::now(),
        })
    }
    pub(crate) fn finish(mut self) -> Result<()> {
        self.close("succeeded")
    }
    fn close(&mut self, status: &str) -> Result<()> {
        let Some(id) = self.id.as_ref() else {
            return Ok(());
        };
        let mut db = self.store.db()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(mut timing) = load(&tx, &self.job)? {
            if let Some(step) = timing
                .steps
                .iter_mut()
                .find(|s| &s.step_id == id && s.status == "running")
            {
                close(step, status, Some(self.started.elapsed()));
                tracing::info!(job=%self.job,step_id=%id,status,"timing phase finished");
            }
            save(&tx, &timing)?;
        }
        tx.commit()?;
        self.id = None;
        Ok(())
    }
}
impl Drop for Phase {
    fn drop(&mut self) {
        let _ = self.close("failed");
    }
}
