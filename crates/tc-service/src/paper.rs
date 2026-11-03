use super::*;
use rust_decimal::Decimal;
use tc_engine::Simulator;
use tc_sdk::Strategy;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaperSession {
    pub id: String,
    pub strategy: String,
    pub state: String,
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub activity: String,
    pub last_bar: Option<i64>,
    pub sequence: i64,
    pub simulator: Simulator,
    pub error: Option<String>,
}
/// Wakes sleeping workers and is checked around every request/frame.
#[derive(Default)]
pub(super) struct PaperControl {
    cancelled: std::sync::atomic::AtomicBool,
    lock: std::sync::Mutex<()>,
    wake: std::sync::Condvar,
}
impl PaperControl {
    fn cancel(&self) {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Release);
        self.wake.notify_all();
    }
    fn check(&self, store: &Store) -> Result<()> {
        ensure!(
            !self.cancelled.load(std::sync::atomic::Ordering::Acquire)
                && !store.shutdown.load(std::sync::atomic::Ordering::Acquire),
            "paper session cancelled"
        );
        Ok(())
    }
    fn wait(&self, duration: Duration) {
        let guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let _wait = self.wake.wait_timeout_while(guard, duration, |_| {
            !self.cancelled.load(std::sync::atomic::Ordering::Acquire)
        });
    }
}
fn active(state: &str) -> bool {
    matches!(state, "starting" | "running")
}
impl Store {
    pub fn start_paper(&self, strategy: &str) -> Result<PaperSession> {
        ensure!(
            !self.shutdown.load(std::sync::atomic::Ordering::Acquire),
            "service is shutting down"
        );
        let registered: RegisteredStrategy = self.get("strategy", strategy)?;
        ensure!(!registered.archived, "strategy archived");
        let mut db = self.db()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing:Option<String>=tx.query_row("SELECT data FROM objects WHERE kind='paper' AND json_extract(data,'$.strategy')=?1 AND json_extract(data,'$.state') IN ('starting','running','stopping') LIMIT 1",[strategy],|r|r.get(0)).optional()?;
        if let Some(existing) = existing {
            let session: PaperSession = serde_json::from_str(&existing)?;
            ensure!(
                session.state != "stopping",
                "previous session is still stopping"
            );
            return Ok(session);
        }
        let count:i64=tx.query_row("SELECT COUNT(*) FROM objects WHERE kind='paper' AND json_extract(data,'$.state') IN ('starting','running','stopping')",[],|r|r.get(0))?;
        ensure!(count < 5, "maximum five active paper sessions");
        let start = now() / MINUTE * MINUTE;
        let session = PaperSession {
            id: uuid::Uuid::new_v4().to_string(),
            strategy: strategy.into(),
            state: "starting".into(),
            created_at: now(),
            updated_at: now(),
            activity: "Connecting to Bitvavo market data".into(),
            last_bar: None,
            sequence: 0,
            simulator: Simulator::new(RunConfig::new(start, start + 365 * 1440 * MINUTE))?,
            error: None,
        };
        tx.execute(
            "INSERT INTO objects VALUES('paper',?1,?2)",
            params![session.id, serde_json::to_string(&session)?],
        )?;
        tx.commit()?;
        Ok(session)
    }
    pub fn stop_paper(&self, id: &str) -> Result<PaperSession> {
        let controls = self
            .paper_controls
            .lock()
            .map_err(|_| anyhow::anyhow!("paper control lock poisoned"))?;
        // Serialize with journal checkpoints so stopping retains the latest
        // committed sequence and portfolio, even during an active candle.
        let mut db = self.db()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let data: String = tx.query_row(
            "SELECT data FROM objects WHERE kind='paper' AND id=?1",
            [id],
            |row| row.get(0),
        )?;
        let mut p: PaperSession = serde_json::from_str(&data)?;
        if !active(&p.state) {
            return Ok(p);
        }
        let control = controls.get(id);
        p.state = if control.is_some() {
            "stopping"
        } else {
            "stopped"
        }
        .into();
        p.activity = if control.is_some() {
            "Stopping session"
        } else {
            "Session stopped"
        }
        .into();
        p.updated_at = now();
        tx.execute(
            "UPDATE objects SET data=?1 WHERE kind='paper' AND id=?2",
            params![serde_json::to_string(&p)?, id],
        )?;
        tx.commit()?;
        if let Some(control) = control {
            control.cancel();
        }
        self.kill_paper_process(id);
        Ok(p)
    }
    fn kill_paper_process(&self, id: &str) {
        if let Ok(lease) = self.get::<WorkerLease>("paper_lease", id)
            && lease.start.is_some()
            && process_start(lease.pid) == lease.start
        {
            #[cfg(unix)]
            {
                let _ = std::process::Command::new("kill")
                    .args(["-KILL", "--", &format!("-{}", lease.pid)])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }
    }
    pub(super) fn request_shutdown(&self) {
        self.shutdown
            .store(true, std::sync::atomic::Ordering::Release);
        if let Ok(controls) = self.paper_controls.lock() {
            for (id, control) in controls.iter() {
                control.cancel();
                self.kill_paper_process(id);
            }
        }
    }
    fn claim_paper(&self, id: &str) -> Result<Option<std::sync::Arc<PaperControl>>> {
        let mut controls = self
            .paper_controls
            .lock()
            .map_err(|_| anyhow::anyhow!("paper control lock poisoned"))?;
        if controls.contains_key(id) || self.shutdown.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(None);
        }
        let mut session: PaperSession = self.get("paper", id)?;
        if !active(&session.state) {
            return Ok(None);
        }
        session.state = "starting".into();
        session.activity = if session.sequence > 0 {
            "Restoring strategy state"
        } else {
            "Connecting to Bitvavo market data"
        }
        .into();
        session.updated_at = now();
        self.put("paper", id, &session)?;
        let control = std::sync::Arc::new(PaperControl::default());
        controls.insert(id.into(), control.clone());
        Ok(Some(control))
    }
    fn finish_paper(&self, id: &str, result: &Result<()>) -> Result<()> {
        let mut controls = self
            .paper_controls
            .lock()
            .map_err(|_| anyhow::anyhow!("paper control lock poisoned"))?;
        let mut p: PaperSession = self.get("paper", id)?;
        if p.state == "stopping" {
            p.state = "stopped".into();
            p.activity = "Session stopped".into();
            p.error = None;
        } else if active(&p.state) {
            if self.shutdown.load(std::sync::atomic::Ordering::Acquire) {
                p.activity = "Service offline; session will resume on startup".into();
            } else if let Err(error) = result {
                p.state = "halted".into();
                p.activity = "Session halted".into();
                p.error = Some(format!("market feed or strategy failed: {error}"));
            } else {
                p.state = "stopped".into();
                p.activity = "Session stopped".into();
            }
        }
        p.updated_at = now();
        self.put("paper", id, &p)?;
        controls.remove(id);
        Ok(())
    }
    fn save_paper_status(&self, p: &PaperSession) -> Result<()> {
        self.db()?.execute("UPDATE objects SET data=?1 WHERE kind='paper' AND id=?2 AND json_extract(data,'$.state') IN ('starting','running')",params![serde_json::to_string(p)?,p.id])?;
        Ok(())
    }
    fn commit_paper(
        &self,
        session: &PaperSession,
        frame: Option<&Frame>,
        fills: &[Fill],
    ) -> Result<()> {
        let mut db = self.db()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state: String = tx.query_row(
            "SELECT json_extract(data,'$.state') FROM objects WHERE kind='paper' AND id=?1",
            [&session.id],
            |r| r.get(0),
        )?;
        if !active(&state) {
            return Ok(());
        }
        tx.execute(
            "INSERT INTO paper_events VALUES(?1,?2,?3,?4)",
            params![
                session.id,
                session.sequence,
                serde_json::to_string(&frame)?,
                serde_json::to_string(fills)?
            ],
        )?;
        tx.execute(
            "UPDATE objects SET data=?1 WHERE kind='paper' AND id=?2",
            params![serde_json::to_string(session)?, session.id],
        )?;
        tx.commit()?;
        Ok(())
    }
    fn replay_strategy(
        &self,
        id: &str,
        strategy: &mut dyn Strategy,
        control: &PaperControl,
    ) -> Result<()> {
        let db = self.db()?;
        let mut query =
            db.prepare("SELECT frame FROM paper_events WHERE session=?1 ORDER BY sequence")?;
        for row in query.query_map([id], |r| r.get::<_, String>(0))? {
            control.check(self)?;
            if let Some(frame) = serde_json::from_str::<Option<Frame>>(&row?)? {
                strategy.on_frame(&frame)?;
            }
        }
        Ok(())
    }
}
fn checkpoint(
    store: &Store,
    session: &mut PaperSession,
    frame: Option<&Frame>,
    before: usize,
) -> Result<()> {
    let fills = session.simulator.fills[before..].to_vec();
    session.sequence += 1;
    session.updated_at = now();
    session.simulator.equity = sample(&session.simulator.equity, 1500);
    if session.simulator.fills.len() > 200 {
        session
            .simulator
            .fills
            .drain(..session.simulator.fills.len() - 200);
    }
    store.commit_paper(session, frame, &fills)
}
fn quote(
    client: &reqwest::blocking::Client,
    market: &str,
) -> Result<(Decimal, Decimal, Decimal, Decimal)> {
    let started = Instant::now();
    let value: Value = client
        .get("https://api.bitvavo.com/v2/ticker/book")
        .query(&[("market", market)])
        .send()?
        .error_for_status()?
        .json()?;
    ensure!(
        started.elapsed() < Duration::from_secs(10),
        "stale quote response"
    );
    let value = if let Some(array) = value.as_array() {
        array.first().context("empty quote")?
    } else {
        &value
    };
    let decimal = |field: &str| -> Result<Decimal> {
        Ok(value[field]
            .as_str()
            .context("quote missing decimal")?
            .parse()?)
    };
    let (bid, ask, bid_size, ask_size) = (
        decimal("bid")?,
        decimal("ask")?,
        decimal("bidSize")?,
        decimal("askSize")?,
    );
    ensure!(
        bid > Decimal::ZERO && ask >= bid && bid_size >= Decimal::ZERO && ask_size >= Decimal::ZERO,
        "invalid quote"
    );
    Ok((bid, ask, bid_size, ask_size))
}
fn quoted_order(
    sim: &Simulator,
    intent: &Intent,
    quote: (Decimal, Decimal, Decimal, Decimal),
) -> Option<(Decimal, Decimal, Decimal)> {
    let gross = sim.portfolio.cash
        + sim
            .portfolio
            .positions
            .values()
            .map(|p| p.quantity * p.price)
            .sum::<Decimal>();
    let held = sim
        .portfolio
        .positions
        .get(&intent.market)
        .map_or(Decimal::ZERO, |p| p.quantity);
    let target = if sim.halted {
        Decimal::ZERO
    } else {
        gross * intent.target_weight
    };
    if target / quote.1 > held {
        Some((quote.1, quote.3, target / quote.1 - held))
    } else if target / quote.0 < held {
        Some((quote.0, quote.2, held - target / quote.0))
    } else {
        None
    }
}
fn execute_quotes(
    client: &reqwest::blocking::Client,
    session: &mut PaperSession,
    store: &Store,
    control: &PaperControl,
) -> Result<()> {
    let mut pending = std::mem::take(&mut session.simulator.pending);
    pending.sort_by_key(|p| p.intent.target_weight > Decimal::ZERO);
    let mut quotes = std::collections::BTreeMap::new();
    for p in pending {
        if now() - p.created > session.simulator.config.intent_ttl_ms && !session.simulator.halted {
            continue;
        }
        let market = p.intent.market.clone();
        control.check(store)?;
        let q = quote(client, &market)?;
        control.check(store)?;
        quotes.insert(market.clone(), q);
        let Some((price, size, needed)) = quoted_order(&session.simulator, &p.intent, q) else {
            continue;
        };
        if needed <= size {
            session.simulator.execute(
                &market,
                price,
                now(),
                p.intent.target_weight,
                if session.simulator.halted {
                    "risk"
                } else {
                    "signal"
                },
            )?;
        } else {
            session.simulator.pending.push(p);
        }
    }
    let held: Vec<_> = session
        .simulator
        .portfolio
        .positions
        .iter()
        .filter(|(_, p)| p.quantity > Decimal::ZERO)
        .map(|(m, _)| m.clone())
        .collect();
    for market in held {
        control.check(store)?;
        let (bid, _, _, _) = match quotes.get(&market) {
            Some(q) => *q,
            None => quote(client, &market)?,
        };
        control.check(store)?;
        if let Some(p) = session.simulator.portfolio.positions.get_mut(&market) {
            p.price = bid;
            p.observed_at = now();
        }
    }
    session.simulator.mark(&[], now())?;
    Ok(())
}
/// State, causal inputs, and exact new fills commit atomically. Quote-only ticks
/// are journaled without invoking the strategy, so replay preserves its state.
pub fn run_paper(store: Store, id: String) -> Result<()> {
    let Some(control) = store.claim_paper(&id)? else {
        return Ok(());
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_paper_inner(&store, &id, &control)
    }))
    .unwrap_or_else(|_| Err(anyhow::anyhow!("paper worker panicked")));
    store.finish_paper(&id, &result)?;
    if control.cancelled.load(std::sync::atomic::Ordering::Acquire)
        || store.shutdown.load(std::sync::atomic::Ordering::Acquire)
    {
        Ok(())
    } else {
        result
    }
}
fn run_paper_inner(store: &Store, id: &str, control: &PaperControl) -> Result<()> {
    control.check(store)?;
    let mut session: PaperSession = store.get("paper", id)?;
    let registered: RegisteredStrategy = store.get("strategy", &session.strategy)?;
    let spec = store.resolve_strategy(&registered)?;
    let (_lease, mut strategy): (Option<ProcessGroupGuard>, Box<dyn Strategy>) = match &spec.kind {
        StrategyKind::Executable { path } => {
            let process = tc_engine::ProcessStrategy::spawn_scoped(path, &spec, Some(id))?;
            let pid = process.process_id();
            store.put(
                "paper_lease",
                id,
                &WorkerLease {
                    pid,
                    start: process_start(pid),
                },
            )?;
            (
                Some(ProcessGroupGuard {
                    kind: "paper_lease".into(),
                    pid: Some(pid),
                    store: store.clone(),
                    id: id.into(),
                }),
                Box::new(process),
            )
        }
        _ => (None, tc_engine::strategy(&spec)?),
    };
    store.replay_strategy(id, strategy.as_mut(), control)?;
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(5))
        .build()?;
    loop {
        control.check(store)?;
        let current: PaperSession = store.get("paper", id)?;
        if !active(&current.state) {
            return Ok(());
        }
        if session.last_bar.is_some() {
            let before = session.simulator.fills.len();
            execute_quotes(&client, &mut session, store, control)?;
            checkpoint(store, &mut session, None, before)?;
        }
        let mut by_time = std::collections::BTreeMap::<i64, Vec<Candle>>::new();
        let closed = now() / MINUTE * MINUTE;
        if session
            .last_bar
            .is_some_and(|last| closed - last > 1440 * MINUTE)
        {
            session.state = "halted".into();
            session.error =
                Some("offline for over one day; inspect and start a new session".into());
            store.save_paper_status(&session)?;
            return Ok(());
        }
        for market in &registered.spec.markets {
            control.check(store)?;
            let value: Value = client
                .get(format!("https://api.bitvavo.com/v2/{market}/candles"))
                .query(&[("interval", "1m"), ("limit", "1440")])
                .send()?
                .error_for_status()?
                .json()?;
            control.check(store)?;
            for c in tc_data::parse_candles(market, &value)? {
                if c.time + MINUTE <= closed && session.last_bar.is_none_or(|last| c.time > last) {
                    by_time.entry(c.time).or_default().push(c);
                }
            }
        }
        let total = by_time.len();
        for (index, (time, candles)) in by_time.into_iter().enumerate() {
            control.check(store)?;
            let warmup = time + MINUTE < session.created_at || closed - (time + MINUTE) > MINUTE;
            session.activity = if warmup {
                format!("Warming up indicators · {}%", (index + 1) * 100 / total)
            } else {
                "Processing completed market candle".into()
            };
            let before = session.simulator.fills.len();
            let frame = Frame {
                protocol: PROTOCOL,
                time: time + MINUTE,
                candles,
                portfolio: session.simulator.portfolio.clone(),
                warmup,
            };
            let intents = strategy.on_frame(&frame)?;
            control.check(store)?;
            if !warmup {
                session.simulator.queue(intents, now())?;
            }
            session.last_bar = Some(time);
            checkpoint(store, &mut session, Some(&frame), before)?;
        }
        control.check(store)?;
        session.state = "running".into();
        session.activity = if session.simulator.halted {
            "Risk limit reached; new positions are halted"
        } else if session.simulator.pending.is_empty() {
            "Live · waiting for a strategy signal on a closed minute"
        } else {
            "Live · waiting for an executable quote"
        }
        .into();
        session.updated_at = now();
        store.save_paper_status(&session)?;
        control.wait(Duration::from_secs(10));
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn target_inside_spread_does_not_invent_an_opposite_side_fill() {
        let mut sim = Simulator::new(RunConfig::new(0, MINUTE)).unwrap();
        sim.portfolio.cash = 500.into();
        sim.portfolio.positions.insert(
            "BTC-EUR".into(),
            Position {
                quantity: 5.into(),
                price: 100.into(),
                observed_at: 0,
            },
        );
        let quote = (100.into(), 101.into(), 10.into(), 10.into());
        assert!(
            quoted_order(
                &sim,
                &Intent {
                    market: "BTC-EUR".into(),
                    target_weight: Decimal::new(5025, 4)
                },
                quote
            )
            .is_none()
        );
        let result = quoted_order(
            &sim,
            &Intent {
                market: "BTC-EUR".into(),
                target_weight: Decimal::new(6, 1),
            },
            quote,
        )
        .unwrap();
        assert_eq!(result.0, 101.into());
        assert!(result.2 < Decimal::ONE);
    }
    #[test]
    fn replay_and_state_restore_are_atomic() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let s = store
            .register(
                StrategySpec {
                    parameters: Default::default(),
                    name: "Cash".into(),
                    description: "replay".into(),
                    kind: StrategyKind::Cash,
                    markets: vec!["BTC-EUR".into()],
                },
                None,
                None,
            )
            .unwrap();
        let mut p = store.start_paper(&s.id).unwrap();
        p.sequence = 1;
        let frame = Frame {
            protocol: PROTOCOL,
            time: now(),
            candles: vec![],
            portfolio: p.simulator.portfolio.clone(),
            warmup: true,
        };
        store.commit_paper(&p, Some(&frame), &[]).unwrap();
        let restored: PaperSession = store.get("paper", &p.id).unwrap();
        assert_eq!(restored.sequence, 1);
        struct Counter(usize);
        impl Strategy for Counter {
            fn on_frame(&mut self, _: &Frame) -> Result<Vec<Intent>> {
                self.0 += 1;
                Ok(vec![])
            }
        }
        let mut counter = Counter(0);
        store
            .replay_strategy(&p.id, &mut counter, &PaperControl::default())
            .unwrap();
        assert_eq!(counter.0, 1);
        assert!(store.commit_paper(&p, Some(&frame), &[]).is_err());
        let restored: PaperSession = store.get("paper", &p.id).unwrap();
        assert_eq!(restored.sequence, 1);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod lifecycle_tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Store, String) {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let strategy = store
            .register(
                StrategySpec {
                    parameters: Default::default(),
                    name: "Cash".into(),
                    description: "lifecycle fixture".into(),
                    kind: StrategyKind::Cash,
                    markets: vec!["BTC-EUR".into()],
                },
                None,
                None,
            )
            .unwrap();
        (directory, store, strategy.id)
    }
    #[test]
    fn duplicate_starts_and_stopping_before_worker_claim_are_safe() {
        let (_directory, store, strategy) = fixture();
        let first = store.start_paper(&strategy).unwrap();
        assert_eq!(first.state, "starting");
        assert!(!first.activity.is_empty());
        assert_eq!(store.start_paper(&strategy).unwrap().id, first.id);
        assert_eq!(store.stop_paper(&first.id).unwrap().state, "stopped");
        assert!(store.claim_paper(&first.id).unwrap().is_none());
        assert_eq!(store.stop_paper(&first.id).unwrap().state, "stopped");
        assert_ne!(store.start_paper(&strategy).unwrap().id, first.id);
    }
    #[test]
    fn stop_cancels_replay_and_cannot_be_overwritten_by_old_checkpoints() {
        let (_directory, store, strategy) = fixture();
        let mut session = store.start_paper(&strategy).unwrap();
        for sequence in 1..=3 {
            session.sequence = sequence;
            let frame = Frame {
                protocol: PROTOCOL,
                time: sequence * MINUTE,
                candles: vec![],
                portfolio: session.simulator.portfolio.clone(),
                warmup: true,
            };
            store.commit_paper(&session, Some(&frame), &[]).unwrap();
        }
        let control = store.claim_paper(&session.id).unwrap().unwrap();
        struct StopOnFirst {
            store: Store,
            id: String,
            calls: usize,
        }
        impl Strategy for StopOnFirst {
            fn on_frame(&mut self, _: &Frame) -> Result<Vec<Intent>> {
                self.calls += 1;
                self.store.stop_paper(&self.id)?;
                Ok(vec![])
            }
        }
        let mut driver = StopOnFirst {
            store: store.clone(),
            id: session.id.clone(),
            calls: 0,
        };
        let result = store.replay_strategy(&session.id, &mut driver, &control);
        assert!(result.is_err());
        assert_eq!(driver.calls, 1);
        session.sequence += 1;
        store.commit_paper(&session, None, &[]).unwrap();
        let saved: PaperSession = store.get("paper", &session.id).unwrap();
        assert_eq!(saved.state, "stopping");
        assert_eq!(saved.sequence, 3);
        store.finish_paper(&session.id, &result).unwrap();
        assert_eq!(
            store
                .get::<PaperSession>("paper", &session.id)
                .unwrap()
                .state,
            "stopped"
        );
        assert!(store.paper_controls.lock().unwrap().is_empty());
    }
    #[test]
    fn cancellation_wakes_idle_polling_immediately() {
        let control = std::sync::Arc::new(PaperControl::default());
        let clone = control.clone();
        let (done, wait) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            clone.wait(Duration::from_secs(10));
            done.send(()).unwrap();
        });
        control.cancel();
        wait.recv_timeout(Duration::from_secs(1)).unwrap();
        thread.join().unwrap();
    }
    #[test]
    #[cfg(unix)]
    fn stop_interrupts_a_blocked_external_strategy_without_network_access() {
        use std::os::unix::fs::PermissionsExt;
        let (directory, store, _) = fixture();
        let executable = directory.path().join("blocked");
        fs::write(&executable, "#!/bin/sh\nsleep 30\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let strategy = store
            .register(
                StrategySpec {
                    parameters: Default::default(),
                    name: "Blocked".into(),
                    description: "IPC cancellation fixture".into(),
                    kind: StrategyKind::Executable {
                        path: executable.to_string_lossy().into(),
                    },
                    markets: vec!["BTC-EUR".into()],
                },
                None,
                None,
            )
            .unwrap();
        let mut session = store.start_paper(&strategy.id).unwrap();
        session.sequence = 1;
        let frame = Frame {
            protocol: PROTOCOL,
            time: MINUTE,
            candles: vec![],
            portfolio: session.simulator.portfolio.clone(),
            warmup: true,
        };
        store.commit_paper(&session, Some(&frame), &[]).unwrap();
        let clone = store.clone();
        let id = session.id.clone();
        let (done, wait) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            done.send(run_paper(clone, id)).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while store
            .get::<WorkerLease>("paper_lease", &session.id)
            .is_err()
        {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(store.stop_paper(&session.id).unwrap().state, "stopping");
        wait.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
        thread.join().unwrap();
        assert_eq!(
            store
                .get::<PaperSession>("paper", &session.id)
                .unwrap()
                .state,
            "stopped"
        );
        assert!(
            store
                .objects::<WorkerLease>("paper_lease")
                .unwrap()
                .is_empty()
        );
    }
}
