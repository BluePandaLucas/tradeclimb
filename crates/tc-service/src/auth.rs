//! Single-owner browser sessions and independently revocable development credentials.
use super::*;
use std::{fs::OpenOptions, io::Write};
use subtle::ConstantTimeEq;

const SESSION_MS: i64 = 12 * 60 * 60 * 1000;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Agent,
}
#[derive(Clone)]
pub(crate) struct Principal {
    pub role: Role,
    pub id: String,
    pub session_hash: Option<String>,
    pub key_hash: String,
    pub expires: i64,
}
fn random_key() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
fn private_file(path: &Path, value: &str) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(value.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}
pub fn owner_key_path(store: &Store) -> PathBuf {
    store.root.join("auth/owner.key")
}
fn owner_hash(store: &Store) -> Result<String> {
    let path = owner_key_path(store);
    let metadata = fs::symlink_metadata(&path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "owner key must be a regular private file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "owner key permissions must be 0600"
        );
    }
    let key = fs::read_to_string(path)?;
    ensure!(
        key.trim().len() == 64,
        "owner key is invalid; rotate it using the local CLI"
    );
    Ok(digest(key.trim().as_bytes()))
}
pub(crate) fn initialize(store: &Store) -> Result<()> {
    let directory = store.root.join("auth");
    fs::create_dir_all(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    }
    let path = owner_key_path(store);
    if !path.exists() {
        let mut file = tempfile::NamedTempFile::new_in(&directory)?;
        writeln!(file, "{}", random_key())?;
        file.as_file().sync_all()?;
        if let Err(error) = file.persist_noclobber(&path) {
            ensure!(
                error.error.kind() == std::io::ErrorKind::AlreadyExists,
                "cannot create owner key: {}",
                error.error
            );
        }
    }
    owner_hash(store)?;
    store.db()?.execute_batch("CREATE TABLE IF NOT EXISTS agent_credentials(id TEXT PRIMARY KEY,name TEXT NOT NULL,key_hash TEXT UNIQUE NOT NULL,created INTEGER NOT NULL,expires INTEGER NOT NULL,revoked INTEGER NOT NULL DEFAULT 0); CREATE TABLE IF NOT EXISTS owner_sessions(key_hash TEXT PRIMARY KEY,owner_hash TEXT NOT NULL,expires INTEGER NOT NULL);")?;
    Ok(())
}
pub fn issue_agent(store: &Store, name: &str, output: &Path, days: u32) -> Result<Value> {
    ensure!(
        !name.trim().is_empty() && name.len() <= 100,
        "agent name required (at most 100 bytes)"
    );
    ensure!(
        days > 0 && days <= 365,
        "credential lifetime must be 1..365 days"
    );
    let key = random_key();
    let id = uuid::Uuid::new_v4().to_string();
    let expires = now() + i64::from(days) * 86_400_000;
    private_file(output, &key)?;
    if let Err(error) = store.db()?.execute(
        "INSERT INTO agent_credentials(id,name,key_hash,created,expires) VALUES(?1,?2,?3,?4,?5)",
        params![id, name, digest(key.as_bytes()), now(), expires],
    ) {
        let _ = fs::remove_file(output);
        return Err(error.into());
    }
    store.put(
        "audit",
        &uuid::Uuid::new_v4().to_string(),
        &json!({"action":"issue_agent","id":id,"name":name,"expires":expires,"time":now()}),
    )?;
    Ok(
        json!({"id":id,"name":name,"role":"agent","expires":expires,"credential_file":output,"scope":"development"}),
    )
}
pub fn revoke_agent(store: &Store, id: &str) -> Result<()> {
    let mut db = store.db()?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure!(
        tx.execute("UPDATE agent_credentials SET revoked=1 WHERE id=?1", [id])? == 1,
        "unknown agent credential"
    );
    tx.execute("UPDATE jobs SET data=json_set(data,'$.state','cancelled','$.updated_at',?1) WHERE id IN (SELECT id FROM objects WHERE kind='job_agent' AND json_extract(data,'$.id')=?2) AND json_extract(data,'$.state') IN ('queued','preparing','running')",params![now(),id])?;
    let cancelled = {
        let mut query = tx.prepare("SELECT id FROM jobs WHERE id IN (SELECT id FROM objects WHERE kind='job_agent' AND json_extract(data,'$.id')=?1) AND json_extract(data,'$.state')='cancelled'")?;
        query
            .query_map([id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for job in cancelled {
        timing::terminal(&tx, &job, "cancelled")?;
    }
    tx.execute("UPDATE objects SET data=json_set(data,'$.state','cancelled') WHERE kind='tuning' AND json_extract(data,'$.agent')=?1 AND json_extract(data,'$.state')='running'",[id])?;
    tx.execute(
        "INSERT INTO objects VALUES('audit',?1,?2)",
        params![
            uuid::Uuid::new_v4().to_string(),
            serde_json::to_string(&json!({"action":"revoke_agent","id":id,"time":now()}))?
        ],
    )?;
    tx.execute("UPDATE objects SET data=json_set(data,'$.state','cancelled','$.updated_at',?1) WHERE kind='candidate' AND json_extract(data,'$.agent')=?2 AND json_extract(data,'$.state') IN ('queued','building','preflight')",params![now(),id])?;
    tx.commit()?;
    Ok(())
}
pub fn status(store: &Store) -> Result<Value> {
    let db = store.db()?;
    let mut query = db.prepare(
        "SELECT id,name,created,expires,revoked FROM agent_credentials ORDER BY created DESC",
    )?;
    let agents = query.query_map([], |row| Ok(json!({"id":row.get::<_,String>(0)?,"name":row.get::<_,String>(1)?,"created":row.get::<_,i64>(2)?,"expires":row.get::<_,i64>(3)?,"revoked":row.get::<_,bool>(4)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
    let endpoint = read_json::<Value>(&store.root.join("auth/agent-endpoint.json")).ok();
    Ok(
        json!({"owner_key_file":owner_key_path(store),"agents":agents,"session_hours":12,"agent_endpoint":endpoint}),
    )
}
pub fn rotate_owner(store: &Store) -> Result<()> {
    let directory = store.root.join("auth");
    let mut file = tempfile::NamedTempFile::new_in(&directory)?;
    writeln!(file, "{}", random_key())?;
    file.as_file().sync_all()?;
    file.persist(owner_key_path(store))?;
    store.db()?.execute("DELETE FROM owner_sessions", [])?;
    store.put(
        "audit",
        &uuid::Uuid::new_v4().to_string(),
        &json!({"action":"rotate_owner_key","time":now()}),
    )
}
pub(crate) fn authenticate(
    store: &Store,
    bearer: Option<&str>,
    session: Option<&str>,
) -> Result<Principal> {
    let owner = owner_hash(store)?;
    if let Some(key) = bearer {
        ensure!(key.len() == 64, "invalid credential");
        let hash = digest(key.as_bytes());
        if bool::from(hash.as_bytes().ct_eq(owner.as_bytes())) {
            return Ok(Principal {
                role: Role::Owner,
                id: "owner".into(),
                session_hash: None,
                key_hash: owner,
                expires: i64::MAX,
            });
        }
        let entry: Option<(String,i64)> = store.db()?.query_row("SELECT id,expires FROM agent_credentials WHERE key_hash=?1 AND revoked=0 AND expires>?2", params![hash,now()], |row|Ok((row.get(0)?,row.get(1)?))).optional()?;
        let (id, expires) = entry.context("invalid or revoked credential")?;
        return Ok(Principal {
            role: Role::Agent,
            id,
            session_hash: None,
            key_hash: hash,
            expires,
        });
    }
    let key = session.context("authentication required")?;
    ensure!(key.len() == 64, "invalid session");
    let hash = digest(key.as_bytes());
    let entry: Option<i64> = store
        .db()?
        .query_row(
            "SELECT expires FROM owner_sessions WHERE key_hash=?1 AND owner_hash=?2 AND expires>?3",
            params![hash, owner, now()],
            |r| r.get(0),
        )
        .optional()?;
    Ok(Principal {
        role: Role::Owner,
        id: "owner".into(),
        session_hash: Some(hash),
        key_hash: owner,
        expires: entry.context("session expired")?,
    })
}
pub(crate) fn active(store: &Store, principal: &Principal) -> bool {
    if principal.expires <= now() {
        return false;
    }
    if principal.role == Role::Agent {
        return store.db().and_then(|db|Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM agent_credentials WHERE id=?1 AND key_hash=?2 AND revoked=0 AND expires>?3)",params![principal.id,principal.key_hash,now()],|r|r.get::<_,bool>(0))?)).unwrap_or(false);
    }
    if !owner_hash(store)
        .is_ok_and(|hash| bool::from(hash.as_bytes().ct_eq(principal.key_hash.as_bytes())))
    {
        return false;
    }
    principal.session_hash.as_ref().is_none_or(|hash| {
        store
            .db()
            .and_then(|db| {
                Ok(db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM owner_sessions WHERE key_hash=?1 AND expires>?2)",
                    params![hash, now()],
                    |r| r.get::<_, bool>(0),
                )?)
            })
            .unwrap_or(false)
    })
}
pub(crate) fn login(store: &Store, key: &str) -> Result<String> {
    let principal = authenticate(store, Some(key), None)?;
    ensure!(
        principal.role == Role::Owner,
        "only the owner can create a browser session"
    );
    let session = random_key();
    let db = store.db()?;
    db.execute("DELETE FROM owner_sessions WHERE expires<=?1", [now()])?;
    db.execute(
        "INSERT INTO owner_sessions VALUES(?1,?2,?3)",
        params![
            digest(session.as_bytes()),
            principal.key_hash,
            now() + SESSION_MS
        ],
    )?;
    Ok(session)
}
pub(crate) fn logout(store: &Store, principal: &Principal) -> Result<()> {
    if let Some(hash) = &principal.session_hash {
        store
            .db()?
            .execute("DELETE FROM owner_sessions WHERE key_hash=?1", [hash])?;
    }
    Ok(())
}
pub(crate) fn allowed(store: &Store, principal: &Principal, method: &str, path: &str) -> bool {
    if principal.role == Role::Owner {
        return true;
    }
    if method == "GET"
        && matches!(
            path,
            "/api/overview" | "/api/events" | "/api/session" | "/api/jobs/index"
        )
    {
        return true;
    }
    if method == "POST" && matches!(path, "/api/jobs" | "/api/tune") {
        return true;
    }
    if method == "GET"
        && matches!(
            path,
            "/api/agent/capabilities" | "/api/agent/kit" | "/api/agent/candles"
        )
        || method == "POST" && path == "/api/agent/candidates"
    {
        return true;
    }
    if let Some(tail) = path.strip_prefix("/api/agent/candidates/") {
        let mut parts = tail.split('/');
        let id = parts.next().unwrap_or_default();
        let action = parts.next();
        if parts.next().is_some() {
            return false;
        }
        return store
            .get::<candidates::Candidate>("candidate", id)
            .is_ok_and(|c| c.agent.as_deref() == Some(principal.id.as_str()))
            && (method == "GET" && action.is_none()
                || method == "POST" && action == Some("cancel"));
    }
    if method == "GET"
        && let Some(tail) = path.strip_prefix("/api/agent/jobs/")
    {
        let mut parts = tail.split('/');
        let id = parts.next().unwrap_or_default();
        let action = parts.next();
        return parts.next().is_none()
            && matches!(action, Some("trace" | "diagnostics"))
            && store.job(id).is_ok_and(|j| {
                j.period == Period::Development && !overlaps_final(j.config.start, j.config.end)
            });
    }
    if let Some(tail) = path.strip_prefix("/api/jobs/") {
        let mut parts = tail.split('/');
        let id = parts.next().unwrap_or_default();
        let action = parts.next();
        if parts.next().is_some() {
            return false;
        }
        let Ok(job) = store.job(id) else {
            return false;
        };
        if job.period != Period::Development || overlaps_final(job.config.start, job.config.end) {
            return false;
        }
        if method == "GET" && matches!(action, None | Some("trades")) {
            return true;
        }
        if method == "POST" && action == Some("cancel") {
            return store
                .get::<Value>("job_agent", id)
                .is_ok_and(|v| v["id"] == principal.id);
        }
    }
    if method == "POST"
        && let Some(id) = path
            .strip_prefix("/api/tuning/")
            .and_then(|s| s.strip_suffix("/cancel"))
    {
        return store
            .get::<Value>("tuning", id)
            .is_ok_and(|v| v["agent"] == principal.id);
    }
    false
}
pub(crate) fn submit(store: &Store, mut request: Submission, agent: Option<&str>) -> Result<Job> {
    if let Some(id) = agent {
        let valid:bool=store.db()?.query_row("SELECT EXISTS(SELECT 1 FROM agent_credentials WHERE id=?1 AND revoked=0 AND expires>?2)",params![id,now()],|r|r.get(0))?;
        ensure!(valid, "agent credential expired or revoked");
        ensure!(
            request.period == Period::Development,
            "agents may submit development only"
        );
        ensure!(
            !request.idempotency_key.is_empty() && request.idempotency_key.len() <= 200,
            "idempotency key required (at most 200 bytes)"
        );
        request.idempotency_key =
            format!("agent:{id}:{}", digest(request.idempotency_key.as_bytes()));
    }
    store.submit_scoped(request, agent)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn revocation_and_concurrent_submissions_leave_no_active_or_unowned_work() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let credential = issue_agent(&store, "race", &dir.path().join("agent.key"), 1).unwrap();
        let agent = credential["id"].as_str().unwrap().to_string();
        let dataset = tc_data::snapshot(&store.root, vec![], 0, 30 * MINUTE, true, true).unwrap();
        let c = store.campaign(&dataset.id).unwrap();
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
        let request = Submission {
            timing: Some(timing::TimingContext {
                run_id: "run".into(),
                parent_step_id: "step".into(),
            }),
            trace: None,
            strategy: s.id,
            campaign: c.id,
            period: Period::Development,
            idempotency_key: "initial".into(),
            slippage_bps: None,
        };
        submit(&store, request.clone(), Some(&agent)).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(10));
        let mut threads = vec![];
        for i in 0..8 {
            let (store, agent, barrier, mut request) = (
                store.clone(),
                agent.clone(),
                barrier.clone(),
                request.clone(),
            );
            request.idempotency_key = format!("race-{i}");
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                let _ = submit(&store, request, Some(&agent));
            }));
        }
        let (other, agent_id, ready) = (store.clone(), agent.clone(), barrier.clone());
        let revoked = std::thread::spawn(move || {
            ready.wait();
            revoke_agent(&other, &agent_id).unwrap();
        });
        barrier.wait();
        for thread in threads {
            thread.join().unwrap();
        }
        revoked.join().unwrap();
        for job in store.jobs().unwrap() {
            assert_eq!(job.state, "cancelled");
            let timing: timing::Timing = store.get("job_timing", &job.id).unwrap();
            assert!(timing.steps.iter().all(|s| s.status == "cancelled"));
            assert_eq!(
                store.get::<Value>("job_agent", &job.id).unwrap()["id"],
                agent
            );
        }
        assert!(submit(&store, request, Some(&agent)).is_err());
        assert_eq!(store.ledger().unwrap()["used"], 0);
    }
    #[test]
    fn credentials_are_private_scoped_revocable_and_never_listed() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path().join("data")).unwrap();
        let owner = fs::read_to_string(owner_key_path(&store)).unwrap();
        let session = login(&store, owner.trim()).unwrap();
        let principal = authenticate(&store, None, Some(&session)).unwrap();
        assert_eq!(principal.role, Role::Owner);
        let path = temp.path().join("agent.key");
        let issued = issue_agent(&store, "developer", &path, 30).unwrap();
        let key = fs::read_to_string(&path).unwrap();
        let agent = authenticate(&store, Some(key.trim()), None).unwrap();
        assert_eq!(agent.role, Role::Agent);
        assert!(login(&store, key.trim()).is_err());
        let listing = serde_json::to_string(&status(&store).unwrap()).unwrap();
        assert!(!listing.contains(key.trim()) && !listing.contains(owner.trim()));
        assert!(!allowed(&store, &agent, "POST", "/api/paper"));
        assert!(!allowed(&store, &agent, "POST", "/api/infer"));
        assert!(!allowed(&store, &agent, "POST", "/api/campaigns"));
        revoke_agent(&store, issued["id"].as_str().unwrap()).unwrap();
        assert!(!active(&store, &agent));
        assert!(authenticate(&store, Some(key.trim()), None).is_err());
        rotate_owner(&store).unwrap();
        assert!(!active(&store, &principal));
        assert!(authenticate(&store, Some(owner.trim()), None).is_err());
        assert!(authenticate(&store, None, Some(&session)).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&store.root).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn expired_credentials_and_logged_out_sessions_stop_working() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path()).unwrap();
        let path = temp.path().join("agent.key");
        let issued = issue_agent(&store, "expired", &path, 1).unwrap();
        store
            .db()
            .unwrap()
            .execute(
                "UPDATE agent_credentials SET expires=0 WHERE id=?1",
                [issued["id"].as_str().unwrap()],
            )
            .unwrap();
        assert!(
            authenticate(&store, Some(fs::read_to_string(path).unwrap().trim()), None).is_err()
        );
        let owner = fs::read_to_string(owner_key_path(&store)).unwrap();
        let session = login(&store, owner.trim()).unwrap();
        let principal = authenticate(&store, None, Some(&session)).unwrap();
        logout(&store, &principal).unwrap();
        assert!(!active(&store, &principal));
    }
}
