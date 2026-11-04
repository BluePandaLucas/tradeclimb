use super::*;
use axum::{
    Json, Router,
    extract::{Path as RoutePath, Query, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use std::{convert::Infallible, sync::Arc};
use tokio_stream::{StreamExt, wrappers::IntervalStream};
#[derive(Clone)]
struct App {
    store: Store,
}
struct ApiError(anyhow::Error);
impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":self.0.to_string()})),
        )
            .into_response()
    }
}
type Api<T> = std::result::Result<Json<T>, ApiError>;
async fn same_origin(request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<axum::http::uri::Authority>().ok());
    let allowed = host.as_ref().is_some_and(|authority| {
        let name = authority.host().trim_matches(['[', ']']);
        name == "localhost"
            || name.parse::<std::net::IpAddr>().is_ok()
            || std::env::var("TC_ALLOWED_HOSTS")
                .unwrap_or_default()
                .split(',')
                .any(|h| !h.is_empty() && h.trim() == name)
    });
    if !allowed {
        return StatusCode::FORBIDDEN.into_response();
    }

    if !["GET", "HEAD", "OPTIONS"].contains(&request.method().as_str()) {
        if let Some(site) = request
            .headers()
            .get("sec-fetch-site")
            .and_then(|v| v.to_str().ok())
            && site == "cross-site"
        {
            return StatusCode::FORBIDDEN.into_response();
        }
        if let Some(origin) = request
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
        {
            let host = request
                .headers()
                .get(header::HOST)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if origin != format!("http://{host}") && origin != format!("https://{host}") {
                return StatusCode::FORBIDDEN.into_response();
            }
        }
    }
    next.run(request).await
}
fn routes(store: Store) -> Router {
    Router::new()
        .route("/api/overview", get(overview))
        .route("/api/events", get(events))
        .route("/api/jobs", post(submit))
        .route("/api/jobs/{id}/cancel", post(cancel))
        .route("/api/jobs/{id}", get(job))
        .route("/api/jobs/{id}/trades", get(trades))
        .route("/api/campaigns", post(campaign))
        .route("/api/strategies/{id}/archive", post(archive))
        .route("/api/paper", post(start_paper))
        .route("/api/paper/{id}/stop", post(stop_paper))
        .route("/api/tune", post(tune))
        .route("/api/tuning/{id}/cancel", post(cancel_tuning))
        .route("/api/infer", post(inference))
        .with_state(Arc::new(App { store }))
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn(same_origin))
}
pub async fn serve(
    store: Store,
    bind: &str,
    assets: &std::path::Path,
    workers: usize,
) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    let _ownership = ServiceOwnership::acquire(&store)?;
    for kind in ["worker_lease", "paper_lease"] {
        for lease in store.objects::<WorkerLease>(kind)? {
            if lease.start.is_some() && process_start(lease.pid) == lease.start {
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
        store
            .db()?
            .execute("DELETE FROM objects WHERE kind=?1", [kind])?;
    }
    for mut p in store.objects::<PaperSession>("paper")? {
        if p.state == "stopping" {
            p.state = "stopped".into();
            p.activity = "Session stopped".into();
            p.updated_at = now();
            store.put("paper", &p.id, &p)?;
        }
    }
    let history_store = store.clone();
    let history = tokio::spawn(async move { supervisor(history_store, workers).await });
    let paper_store = store.clone();
    let papers = tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        let mut active = std::collections::BTreeSet::new();
        while !paper_store
            .shutdown
            .load(std::sync::atomic::Ordering::Acquire)
        {
            while tasks.try_join_next().is_some() {}
            if let Ok(sessions) = paper_store.objects::<PaperSession>("paper") {
                for p in sessions {
                    if matches!(p.state.as_str(), "starting" | "running")
                        && active.insert(p.id.clone())
                    {
                        let store = paper_store.clone();
                        tasks.spawn_blocking(move || {
                            if let Err(error) = run_paper(store, p.id.clone()) {
                                tracing::warn!(session=%p.id,%error,"paper session halted");
                            }
                        });
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        while tasks.join_next().await.is_some() {}
        Ok::<(), anyhow::Error>(())
    });
    let router = routes(store.clone()).fallback_service(
        tower_http::services::ServeDir::new(assets).append_index_html_on_directories(true),
    );
    let (close, closed) = tokio::sync::oneshot::channel::<()>();
    let server = std::future::IntoFuture::into_future(
        axum::serve(listener, router).with_graceful_shutdown(async {
            let _ = closed.await;
        }),
    );
    tokio::pin!(server);
    tracing::info!(%bind,"TradeClimb paper-only service ready");
    let result = tokio::select! {
        result=&mut server=>result,
        _=shutdown_signal()=>{
            tracing::info!("Shutting down TradeClimb");
            store.request_shutdown();let _=close.send(());
            match tokio::time::timeout(Duration::from_secs(5),&mut server).await {
                Ok(result)=>result,
                Err(_)=>{tracing::warn!("Closing remaining HTTP connections after shutdown deadline");Ok(())}
            }
        }
    };
    store.request_shutdown();
    for task in [history, papers] {
        if let Err(error) = tokio::time::timeout(Duration::from_secs(6), task).await {
            tracing::warn!(%error,"background shutdown deadline reached");
        }
    }
    result?;
    Ok(())
}
async fn shutdown_signal() {
    use std::io::{BufRead, IsTerminal};
    let (typed, keyboard) = tokio::sync::oneshot::channel();
    if std::io::stdin().is_terminal() {
        eprintln!("Press Ctrl+C, or type q / exit and Enter, to stop TradeClimb.");
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                match line {
                    Ok(line)
                        if matches!(
                            line.trim().to_ascii_lowercase().as_str(),
                            "q" | "quit" | "exit"
                        ) =>
                    {
                        break;
                    }
                    Ok(_) => eprintln!("Type q or exit, then Enter, to stop TradeClimb."),
                    Err(_) => break,
                }
            }
            let _ = typed.send(());
        });
    }
    let keyboard = async {
        if keyboard.await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate=>{},_=keyboard=>{}}
}

fn overview_value(store: &Store) -> Result<Value> {
    Ok(
        json!({"leaderboards":leaderboard::groups(store)?,"strategies":store.objects::<RegisteredStrategy>("strategy")?,"campaigns":store.objects::<Campaign>("campaign")?,"jobs":store.jobs()?.into_iter().take(50).collect::<Vec<_>>(),"paper":store.objects::<PaperSession>("paper")?,"datasets":tc_data::list(&store.root)?,"validation":store.ledger()?,"tuning":store.objects::<Value>("tuning")?,"time":now()}),
    )
}
async fn overview(State(app): State<Arc<App>>) -> Api<Value> {
    Ok(Json(overview_value(&app.store)?))
}
async fn events(
    State(app): State<Arc<App>>,
) -> Sse<impl tokio_stream::Stream<Item = std::result::Result<Event, Infallible>>> {
    let shutdown = app.store.shutdown.clone();
    let stream = IntervalStream::new(tokio::time::interval(Duration::from_secs(1)))
        .take_while(move |_| !shutdown.load(std::sync::atomic::Ordering::Acquire))
        .map(move |_| {
            let data = overview_value(&app.store)
                .and_then(|v| Ok(serde_json::to_string(&v)?))
                .unwrap_or_else(|_| "{\"error\":\"snapshot unavailable\"}".into());
            Ok(Event::default().event("snapshot").data(data))
        });
    Sse::new(stream).keep_alive(KeepAlive::default())
}
async fn submit(State(app): State<Arc<App>>, Json(input): Json<Submission>) -> Api<Job> {
    Ok(Json(app.store.submit(input)?))
}
async fn cancel(State(app): State<Arc<App>>, RoutePath(id): RoutePath<String>) -> Api<Job> {
    Ok(Json(app.store.cancel(&id)?))
}
#[derive(Deserialize, Default)]
struct ChartWindow {
    #[serde(default)]
    days: u32,
}
async fn job(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Query(window): Query<ChartWindow>,
) -> Api<Job> {
    let mut job = app.store.job(&id)?;
    if job.period == Period::Development {
        let directory = app.store.root.join("jobs").join(&id);
        let name = if job.state == "completed" {
            "result.json"
        } else {
            "progress.json"
        };
        if let Ok(mut value) = read_json::<Value>(&directory.join(name)) {
            if window.days > 0 && job.state == "completed" {
                if ![7, 30, 90].contains(&window.days) {
                    return Err(anyhow::anyhow!("chart windows are 7, 30, or 90 days").into());
                }
                let cached = directory.join(format!("chart-{}.json", window.days));
                value = if let Ok(v) = read_json::<Value>(&cached) {
                    v
                } else {
                    let path = directory.join("report.json.gz");
                    let cutoff = (job.config.end - i64::from(window.days) * 1440 * MINUTE)
                        .max(job.config.start);

                    tokio::task::spawn_blocking(move || -> Result<Value> {
                        let mut r = read_report(&path)?;
                        r.equity.retain(|p| p.time >= cutoff);
                        r.fills.retain(|f| f.time >= cutoff);
                        let mut view = serde_json::to_value(RunView::from_report(&r))?;
                        if let Some(benchmarks) = value["benchmarks"].as_array_mut() {
                            for b in benchmarks.iter_mut() {
                                if let Some(points) = b["equity"].as_array_mut() {
                                    points.retain(|p| {
                                        p["time"].as_i64().is_some_and(|t| t >= cutoff)
                                    });
                                }
                            }
                            view["benchmarks"] = json!(benchmarks);
                        }
                        save_json(&cached, &view)?;
                        Ok(view)
                    })
                    .await??
                };
            }
            job.result = Some(value);
        }
    }
    Ok(Json(job))
}
#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    offset: usize,
    #[serde(default = "page_size")]
    limit: usize,
}
fn page_size() -> usize {
    100
}
async fn trades(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Query(page): Query<Page>,
) -> Api<Value> {
    let job = app.store.job(&id)?;
    if job.period == Period::Validation {
        return Err(anyhow::anyhow!(
            "validation exposes CAGR only; use explicit local owner inspection to reveal details"
        )
        .into());
    }
    let path = app.store.root.join("jobs").join(id).join("report.json.gz");
    let report = tokio::task::spawn_blocking(move || read_report(&path)).await??;
    let data: Vec<_> = report
        .fills
        .iter()
        .skip(page.offset)
        .take(page.limit.clamp(1, 1000))
        .collect();
    Ok(Json(
        json!({"total":report.fills.len(),"offset":page.offset,"fills":data}),
    ))
}
async fn campaign(State(app): State<Arc<App>>, Json(v): Json<Value>) -> Api<Campaign> {
    Ok(Json(app.store.campaign_with_target(
        v["dataset"].as_str().context("dataset required")?,
        v["target_cagr"].as_f64().unwrap_or(0.15),
    )?))
}
async fn archive(State(app): State<Arc<App>>, RoutePath(id): RoutePath<String>) -> Api<Value> {
    let mut s: RegisteredStrategy = app.store.get("strategy", &id)?;
    s.archived = !s.archived;
    app.store.put("strategy", &id, &s)?;
    Ok(Json(json!({"archived":s.archived})))
}
async fn start_paper(State(app): State<Arc<App>>, Json(v): Json<Value>) -> Api<PaperSession> {
    Ok(Json(app.store.start_paper(
        v["strategy"].as_str().context("strategy required")?,
    )?))
}
async fn stop_paper(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
) -> Api<PaperSession> {
    Ok(Json(app.store.stop_paper(&id)?))
}
async fn inference(
    State(app): State<Arc<App>>,
    Json(v): Json<super::inference::InferenceRequest>,
) -> Api<tc_sdk::ModelResponse> {
    Ok(Json(super::infer(&app.store, v).await?))
}
#[derive(Deserialize)]
struct Tune {
    #[serde(default)]
    space: Vec<tc_sdk::ParameterRange>,
    #[serde(default)]
    method: tc_sdk::SearchMethod,
    strategy: String,
    campaign: String,
    #[serde(default = "default_trials")]
    trials: usize,
    #[serde(default)]
    seed: u64,
}
fn default_trials() -> usize {
    20
}
async fn tune(State(app): State<Arc<App>>, Json(input): Json<Tune>) -> Api<Value> {
    ensure_tune(&input)?;
    let parent: RegisteredStrategy = app.store.get("strategy", &input.strategy)?;
    if input.space.is_empty() {
        ensure_tune_kind(&parent.spec.kind)?;
    }
    let id = uuid::Uuid::new_v4().to_string();
    let return_id = id.clone();
    let store = app.store.clone();
    store.put(
        "tuning",
        &id,
        &json!({"id":id,"state":"running","seed":input.seed,"trials":input.trials}),
    )?;
    tokio::spawn(async move {
        let result = run_tuning(&store, &input, &parent, &id).await;
        if let Err(e) = result {
            let _ = store.put(
                "tuning",
                &id,
                &json!({"id":id,"state":"failed","error":e.to_string()}),
            );
        }
    });
    Ok(Json(json!({"id":return_id,"state":"running"})))
}
async fn cancel_tuning(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
) -> Api<Value> {
    let mut value: Value = app.store.get("tuning", &id)?;
    value["state"] = json!("cancelled");
    app.store.put("tuning", &id, &value)?;
    Ok(Json(value))
}
fn ensure_tune(input: &Tune) -> Result<()> {
    ensure!(
        input.trials > 0 && input.trials <= 100,
        "tuning budget is 1..100"
    );
    Ok(())
}
fn ensure_tune_kind(kind: &StrategyKind) -> Result<()> {
    ensure!(
        matches!(kind, StrategyKind::MovingAverage { .. }),
        "built-in tuning currently supports moving-average fast/slow parameters"
    );
    Ok(())
}
async fn run_tuning(
    store: &Store,
    input: &Tune,
    parent: &RegisteredStrategy,
    id: &str,
) -> Result<()> {
    use rand::{Rng, SeedableRng};
    let mut rng = rand::rngs::StdRng::seed_from_u64(input.seed);
    let started = Instant::now();
    let mut stale = 0;
    let mut best = f64::NEG_INFINITY;
    let mut jobs = vec![];
    let mut seen = std::collections::BTreeSet::new();
    for trial in 0..input.trials {
        if store.get::<Value>("tuning", id)?["state"] == "cancelled" {
            return Ok(());
        }
        if stale >= 20 || started.elapsed() > Duration::from_secs(1800) {
            break;
        }
        let fast = rng.random_range(5..120);
        let slow = rng.random_range(fast + 1..600);
        let mut spec = parent.spec.clone();
        spec.name = format!("{} · {fast}/{slow}", parent.spec.name);
        if input.space.is_empty() {
            spec.kind = StrategyKind::MovingAverage { fast, slow };
        } else {
            spec.parameters = tc_sdk::propose(&input.space, &input.method, input.seed, trial)?;
            if let StrategyKind::MovingAverage { fast, slow } = &mut spec.kind {
                if let Some(v) = spec.parameters.get("fast").and_then(Value::as_u64) {
                    *fast = v as usize;
                }
                if let Some(v) = spec.parameters.get("slow").and_then(Value::as_u64) {
                    *slow = v as usize;
                }
            }
        }
        let s = store.register(spec, Some(parent.id.clone()), None)?;
        if !seen.insert(s.id.clone()) {
            continue;
        }
        let job = store.submit(Submission {
            strategy: s.id,
            campaign: input.campaign.clone(),
            period: Period::Development,
            idempotency_key: format!("tune:{id}:{trial}"),
            slippage_bps: None,
        })?;
        jobs.push(job.id.clone());
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if store.get::<Value>("tuning", id)?["state"] == "cancelled" {
                let _ = store.cancel(&job.id);
                return Ok(());
            }
            if started.elapsed() >= Duration::from_secs(1800) {
                let _ = store.cancel(&job.id);
                anyhow::bail!("tuning deadline exceeded");
            }
            let j = store.job(&job.id)?;
            if ["completed", "failed", "cancelled", "interrupted"].contains(&j.state.as_str()) {
                let result = j.result.unwrap_or(Value::Null);
                let score = if result["metrics"]["eligible"].as_bool() == Some(true) {
                    result["metrics"]["net_cagr"]
                        .as_f64()
                        .unwrap_or(f64::NEG_INFINITY)
                } else {
                    f64::NEG_INFINITY
                };
                if score > best + 0.0025 {
                    best = score;
                    stale = 0;
                } else {
                    stale += 1;
                }
                break;
            }
        }
        store.put("tuning",id,&json!({"id":id,"state":"running","seed":input.seed,"jobs":jobs,"completed":trial+1,"best":if best.is_finite(){Some(best)}else{None},"stalled":stale}))?;
    }
    store.put("tuning",id,&json!({"id":id,"state":"completed","jobs":jobs,"best":if best.is_finite(){Some(best)}else{None},"stop_reason":if stale>=20{"stalled"}else{"budget_exhausted"}}))?;
    Ok(())
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tower::ServiceExt;
    #[tokio::test]
    async fn open_browser_stream_ends_when_shutdown_is_requested() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let request = axum::http::Request::builder()
            .uri("/api/events")
            .header("host", "localhost")
            .body(axum::body::Body::empty())
            .unwrap();
        let response = routes(store.clone()).oneshot(request).await.unwrap();
        let mut stream = response.into_body().into_data_stream();
        assert!(stream.next().await.is_some());
        store.request_shutdown();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), stream.next())
                .await
                .unwrap()
                .is_none()
        );
    }
    #[tokio::test]
    async fn validation_http_never_loads_local_detail_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let strategy = store
            .register(
                StrategySpec {
                    parameters: Default::default(),
                    name: "Hold".into(),
                    description: "fixture".into(),
                    kind: StrategyKind::BuyHold,
                    markets: vec!["BTC-EUR".into()],
                },
                None,
                None,
            )
            .unwrap();
        let campaign = Campaign {
            target_cagr: 0.15,
            id: "campaign".into(),
            dataset: "fixture".into(),
            development_start: 0,
            development_end: MINUTE,
            validation_start: MINUTE,
            validation_end: 2 * MINUTE,
        };
        store.put("campaign", &campaign.id, &campaign).unwrap();
        let mut job = store
            .submit(Submission {
                strategy: strategy.id,
                campaign: campaign.id,
                period: Period::Validation,
                idempotency_key: "validation".into(),
                slippage_bps: None,
            })
            .unwrap();
        job.state = "completed".into();
        job.result = Some(json!({"net_cagr":0.2}));
        store.update_job(&job).unwrap();
        save_json(
            &store.root.join("jobs").join(&job.id).join("result.json"),
            &json!({"equity":"SECRET","fills":"SECRET"}),
        )
        .unwrap();
        for route in [
            format!("/api/jobs/{}?days=7", job.id),
            "/api/overview".into(),
        ] {
            let req = axum::http::Request::builder()
                .uri(route)
                .header("host", "localhost")
                .body(axum::body::Body::empty())
                .unwrap();
            let response = routes(store.clone()).oneshot(req).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
                .await
                .unwrap();
            let text = String::from_utf8(bytes.to_vec()).unwrap();
            assert!(!text.contains("SECRET"));
            assert!(!text.contains("\"fills\""));
        }
        let req = axum::http::Request::builder()
            .uri(format!("/api/jobs/{}/trades", job.id))
            .header("host", "localhost")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(
            routes(store).oneshot(req).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }
    #[tokio::test]
    async fn dns_rebinding_host_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let req = axum::http::Request::builder()
            .uri("/api/overview")
            .header("host", "attacker.invalid:8080")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(
            routes(Store::open(dir.path()).unwrap())
                .oneshot(req)
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    #[tokio::test]
    async fn cross_origin_mutation_is_denied() {
        let dir = tempfile::tempdir().unwrap();
        let app = routes(Store::open(dir.path()).unwrap());
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/api/jobs")
            .header("host", "localhost:8080")
            .header("origin", "https://evil.example")
            .header("content-type", "application/json")
            .body(axum::body::Body::from("{}"))
            .unwrap();
        let result = app.oneshot(request).await.unwrap();
        assert_eq!(result.status(), StatusCode::FORBIDDEN);
    }
}
