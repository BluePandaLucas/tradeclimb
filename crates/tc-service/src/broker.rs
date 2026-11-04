//! One bounded local socket per confined strategy, bound to an owner-selected scope.
use super::*;
#[cfg(unix)]
use std::{
    io::{Read, Write},
    os::unix::net::UnixListener,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[cfg(unix)]
pub(crate) struct ModelBroker {
    directory: tempfile::TempDir,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
#[cfg(unix)]
impl ModelBroker {
    pub fn start(store: Store, scope: &str) -> Result<Self> {
        let directory = tempfile::tempdir()?;
        let listener = UnixListener::bind(directory.path().join("model.sock"))?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        let scope = scope.to_string();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let thread = std::thread::spawn(move || {
            while !cancelled.load(Ordering::Acquire) {
                let (mut stream, _) = match listener.accept() {
                    Ok(v) => v,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(20));
                        continue;
                    }
                    Err(_) => break,
                };
                let mut handle = || -> Result<tc_sdk::ModelResponse> {
                    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
                    let mut length = [0; 4];
                    let deadline = Instant::now() + Duration::from_secs(2);
                    read_deadline(&mut stream, &mut length, deadline)?;
                    let length = u32::from_le_bytes(length) as usize;
                    ensure!(length <= 65_536, "model request frame too large");
                    let mut bytes = vec![0; length];
                    read_deadline(&mut stream, &mut bytes, deadline)?;
                    let request: tc_sdk::ModelRequest = serde_json::from_slice(&bytes)?;
                    runtime.block_on(async {
                        tokio::select! {
                            result = infer(&store, inference::InferenceRequest {scope: scope.clone(), request}) => result,
                            () = async { while !cancelled.load(Ordering::Acquire) { tokio::time::sleep(Duration::from_millis(20)).await; } } => anyhow::bail!("model broker stopped"),
                        }
                    })
                };
                let reply = match handle() {
                    Ok(response) if response.text.len() <= 131_072 => {
                        tc_sdk::ModelReply::Ok(response)
                    }
                    Ok(_) => tc_sdk::ModelReply::Error("model response exceeds size limit".into()),
                    Err(error) => {
                        tracing::warn!(scope, error=%error, "sandbox model call rejected");
                        tc_sdk::ModelReply::Error(
                            "model call rejected or failed; inspect the owner log".into(),
                        )
                    }
                };
                if let Ok(bytes) = serde_json::to_vec(&reply) {
                    let mut frame = (bytes.len() as u32).to_le_bytes().to_vec();
                    frame.extend(bytes);
                    let deadline = Instant::now() + Duration::from_secs(2);
                    let mut remaining = frame.as_slice();
                    while !remaining.is_empty() {
                        let Some(timeout) = deadline.checked_duration_since(Instant::now()) else {
                            break;
                        };
                        let _ = stream.set_write_timeout(Some(timeout));
                        match stream.write(remaining) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => remaining = &remaining[n..],
                        }
                    }
                }
            }
        });
        Ok(Self {
            directory,
            stop,
            thread: Some(thread),
        })
    }
    pub fn path(&self) -> &Path {
        self.directory.path()
    }
}
#[cfg(unix)]
impl Drop for ModelBroker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(unix)]
fn read_deadline(
    stream: &mut std::os::unix::net::UnixStream,
    mut bytes: &mut [u8],
    deadline: Instant,
) -> Result<()> {
    while !bytes.is_empty() {
        stream.set_read_timeout(Some(
            deadline
                .checked_duration_since(Instant::now())
                .context("model request deadline")?,
        ))?;
        let n = stream.read(bytes)?;
        ensure!(n > 0, "incomplete model request");
        bytes = &mut bytes[n..];
    }
    Ok(())
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tc_sdk::{ModelClient, ModelRequest, SocketModelClient};
    fn scope(store: &Store) -> String {
        let s = store
            .register(
                StrategySpec {
                    parameters: Default::default(),
                    name: "broker".into(),
                    description: "test".into(),
                    kind: StrategyKind::Cash,
                    markets: vec!["BTC-EUR".into()],
                },
                None,
                None,
            )
            .unwrap();
        store.start_paper(&s.id).unwrap().id
    }
    #[test]
    fn broker_fixes_scope_replays_cache_and_enforces_budget() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let id = scope(&store);
        let broker = ModelBroker::start(store.clone(), &id).unwrap();
        let mut client = SocketModelClient {
            path: broker.path().join("model.sock"),
        };
        let request = ModelRequest {
            capability: "fake".into(),
            prompt: "same".into(),
            max_tokens: 10,
        };
        assert_eq!(client.infer(&request).unwrap().text, "same");
        assert_eq!(client.infer(&request).unwrap().text, "same");
        let mut stream = std::os::unix::net::UnixStream::connect(&client.path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        tc_sdk::write_message(&mut stream, &json!({"scope":"another-owner-scope","capability":"fake","prompt":"same","max_tokens":10})).unwrap();
        let reply: tc_sdk::ModelReply = tc_sdk::read_message(&mut stream).unwrap();
        assert!(matches!(reply, tc_sdk::ModelReply::Ok(_)));
        for i in 0..63 {
            client
                .infer(&ModelRequest {
                    prompt: format!("unique-{i}"),
                    ..request.clone()
                })
                .unwrap();
        }
        assert!(
            client
                .infer(&ModelRequest {
                    prompt: "over-budget".into(),
                    ..request
                })
                .is_err()
        );
        let count: i64 = store
            .db()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM model_cache WHERE scope=?1",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 64);
        let other: i64 = store
            .db()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM model_cache WHERE scope != ?1",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(other, 0);
    }
    #[test]
    fn oversized_or_stalled_requests_do_not_block_broker_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let id = scope(&store);
        let broker = ModelBroker::start(store, &id).unwrap();
        let mut stream =
            std::os::unix::net::UnixStream::connect(broker.path().join("model.sock")).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream.write_all(&u32::MAX.to_le_bytes()).unwrap();
        assert!(matches!(
            tc_sdk::read_message::<tc_sdk::ModelReply>(&mut stream).unwrap(),
            tc_sdk::ModelReply::Error(_)
        ));
        let mut stalled =
            std::os::unix::net::UnixStream::connect(broker.path().join("model.sock")).unwrap();
        stalled.write_all(&65536_u32.to_le_bytes()).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let start = Instant::now();
        drop(broker);
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}
