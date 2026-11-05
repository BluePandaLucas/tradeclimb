use super::*;
use axum::{
    Extension, Json, Router,
    extract::{Path as RoutePath, Query, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{
        Html, IntoResponse, Response,
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
struct ApiError(anyhow::Error, StatusCode);
impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        Self(e.into(), StatusCode::BAD_REQUEST)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.1, Json(json!({"error":self.0.to_string()}))).into_response()
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
fn session_cookie(request: &Request) -> Option<&str> {
    request
        .headers()
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|cookie| cookie.strip_prefix("tc_session="))
}
#[derive(Clone)]
struct AgentTransport;
async fn agent_transport(mut request: Request, next: Next) -> Response {
    if !request.uri().path().starts_with("/api/") {
        return StatusCode::FORBIDDEN.into_response();
    }
    request.extensions_mut().insert(AgentTransport);
    next.run(request).await
}
async fn authenticated(State(store): State<Store>, mut request: Request, next: Next) -> Response {
    let bearer = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));
    let principal = match auth::authenticate(&store, bearer, session_cookie(&request)) {
        Ok(p) => p,
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error":"authentication required"})),
            )
                .into_response();
        }
    };
    if request.extensions().get::<AgentTransport>().is_some() && principal.role != auth::Role::Agent
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !auth::allowed(
        &store,
        &principal,
        request.method().as_str(),
        request.uri().path(),
    ) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"agent credentials permit development only"})),
        )
            .into_response();
    }
    request.extensions_mut().insert(principal);
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}
async fn login_page() -> Response {
    let mut response = Html(include_str!("login.html")).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
        .headers_mut()
        .insert("x-frame-options", header::HeaderValue::from_static("DENY"));
    response
}
#[derive(Deserialize)]
struct Login {
    key: String,
}
async fn login(
    State(app): State<Arc<App>>,
    headers: header::HeaderMap,
    Json(input): Json<Login>,
) -> Response {
    match auth::login(&app.store, &input.key) {
        Ok(session) => {
            let secure = if headers
                .get(header::ORIGIN)
                .and_then(|h| h.to_str().ok())
                .is_some_and(|s| s.starts_with("https://"))
            {
                "; Secure"
            } else {
                ""
            };
            let mut response = Json(json!({"role":"owner"})).into_response();
            if let Ok(cookie) = header::HeaderValue::from_str(&format!(
                "tc_session={session}; Path=/; HttpOnly; SameSite=Strict; Max-Age=43200{secure}"
            )) {
                response.headers_mut().insert(header::SET_COOKIE, cookie);
            }
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("no-store"),
            );
            response
        }
        Err(_) => (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"sign-in failed"})),
        )
            .into_response(),
    }
}
async fn session(Extension(principal): Extension<auth::Principal>) -> Api<Value> {
    Ok(Json(
        json!({"role":principal.role,"id":principal.id,"expires":principal.expires}),
    ))
}
async fn logout(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<auth::Principal>,
) -> std::result::Result<Response, ApiError> {
    auth::logout(&app.store, &principal)?;
    let mut response = Json(json!({"signed_out":true})).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        header::HeaderValue::from_static(
            "tc_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0",
        ),
    );
    Ok(response)
}
fn routes(store: Store) -> Router {
    let protected = Router::new()
        .route("/api/agent/capabilities", get(agent_capabilities))
        .route("/api/agent/kit", get(agent_kit))
        .route("/api/agent/candles", get(agent_candles))
        .route(
            "/api/agent/candidates",
            post(candidate_submit).layer(axum::extract::DefaultBodyLimit::max(2 * 1024 * 1024)),
        )
        .route("/api/agent/candidates/{id}", get(candidate_status))
        .route("/api/agent/candidates/{id}/cancel", post(candidate_cancel))
        .route("/api/agent/jobs/{id}/trace", get(agent_trace))
        .route("/api/agent/jobs/{id}/diagnostics", get(agent_diagnostics))
        .route("/api/session", get(session))
        .route("/api/logout", post(logout))
        .route("/api/overview", get(overview))
        .route("/api/events", get(events))
        .route("/api/jobs", post(submit))
        .route("/api/jobs/index", get(job_index))
        .route("/api/jobs/{id}/cancel", post(cancel))
        .route("/api/jobs/{id}", get(job))
        .route("/api/jobs/{id}/trades", get(trades))
        .route("/api/jobs/{id}/reveal", post(reveal_validation))
        .route("/api/campaigns", post(campaign))
        .route("/api/strategies/{id}/archive", post(archive))
        .route("/api/paper", post(start_paper))
        .route("/api/paper/{id}/stop", post(stop_paper))
        .route("/api/tune", post(tune))
        .route("/api/tuning/{id}/cancel", post(cancel_tuning))
        .route("/api/infer", post(inference))
        .route_layer(middleware::from_fn_with_state(store.clone(), authenticated));
    protected
        .route("/login", get(login_page).post(login))
        .with_state(Arc::new(App { store }))
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn(same_origin))
}
fn store_approval_missing(store: &Store, id: &str) -> bool {
    store.get::<Value>("paper_approval", id).is_err()
}
pub async fn serve(
    store: Store,
    bind: &str,
    assets: &std::path::Path,
    workers: usize,
) -> Result<()> {
    let address: std::net::SocketAddr = bind
        .parse()
        .context("bind must be a loopback IP and port")?;
    ensure!(
        address.ip().is_loopback(),
        "authenticated TC binds to loopback; use an HTTPS reverse proxy for remote access"
    );
    let assets = assets
        .canonicalize()
        .context("web assets directory missing")?;
    ensure!(
        !store.root.starts_with(&assets) && !assets.starts_with(&store.root),
        "web assets and private data must be separate directories"
    );
    let listener = tokio::net::TcpListener::bind(address).await?;
    let _ownership = ServiceOwnership::acquire(&store)?;
    for kind in ["worker_lease", "paper_lease", "candidate_lease"] {
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
    let candidate_store = store.clone();
    let candidates = tokio::spawn(async move { candidates::supervisor(candidate_store).await });
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
                        && store_approval_missing(&paper_store, &p.id)
                    {
                        let _ = paper_store.stop_paper(&p.id);
                        continue;
                    }
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
    // A separate, agent-only transport is the sole host socket mounted in
    // the development sandbox. Owner cookies/credentials cannot use it.
    use std::os::unix::fs::PermissionsExt;
    let agent_directory = tempfile::Builder::new().prefix("tc-agent-api-").tempdir()?;
    fs::set_permissions(agent_directory.path(), fs::Permissions::from_mode(0o755))?;
    let socket = agent_directory.path().join("api.sock");
    let unix = tokio::net::UnixListener::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o666))?;
    save_json(
        &store.root.join("auth/agent-endpoint.json"),
        &json!({"socket":socket,"pid":std::process::id()}),
    )?;
    let agent_router = routes(store.clone()).layer(middleware::from_fn(agent_transport));
    let agent_shutdown = store.shutdown.clone();
    let agent_server = tokio::spawn(async move {
        axum::serve(unix, agent_router)
            .with_graceful_shutdown(async move {
                while !agent_shutdown.load(std::sync::atomic::Ordering::Acquire) {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await
    });
    let router = routes(store.clone()).fallback_service(
        tower_http::services::ServeDir::new(&assets).append_index_html_on_directories(true),
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
    for task in [history, papers, candidates] {
        match tokio::time::timeout(Duration::from_secs(6), task).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => tracing::error!(%error, "background supervisor failed"),
            Ok(Err(error)) => tracing::error!(%error, "background supervisor task failed"),
            Err(error) => tracing::warn!(%error, "background shutdown deadline reached"),
        }
    }
    if tokio::time::timeout(Duration::from_secs(2), agent_server)
        .await
        .is_err()
    {
        tracing::warn!("agent API shutdown deadline reached");
    }
    let _ = fs::remove_file(store.root.join("auth/agent-endpoint.json"));
    drop(agent_directory);
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
    let counts = store.db()?.query_row("SELECT COUNT(*), COALESCE(SUM(json_extract(data, '$.state') = 'completed'), 0), COALESCE(SUM(json_extract(data, '$.state') IN ('queued', 'preparing', 'running')), 0) FROM jobs", [], |row| Ok(json!({"total":row.get::<_,u64>(0)?,"completed":row.get::<_,u64>(1)?,"active":row.get::<_,u64>(2)?})))?;
    Ok(
        json!({"job_counts":counts,"leaderboards":leaderboard::groups(store)?,"strategies":store.objects::<RegisteredStrategy>("strategy")?,"campaigns":store.objects::<Campaign>("campaign")?,"jobs":store.jobs()?.into_iter().take(50).collect::<Vec<_>>(),"paper":store.objects::<PaperSession>("paper")?,"datasets":tc_data::list(&store.root)?,"validation":store.ledger()?,"tuning":store.objects::<Value>("tuning")?,"paper_candidates":store.paper_candidates()?,"paper_approvals":store.objects::<Value>("paper_approval")?,"time":now()}),
    )
}
fn scoped_overview(store: &Store, principal: &auth::Principal) -> Result<Value> {
    let mut value = overview_value(store)?;
    if principal.role == auth::Role::Agent {
        value["job_counts"]=store.db()?.query_row("SELECT COUNT(*),COALESCE(SUM(json_extract(data,'$.state')='completed'),0),COALESCE(SUM(json_extract(data,'$.state') IN ('queued','preparing','running')),0) FROM jobs WHERE json_extract(data,'$.period')='development' AND NOT(json_extract(data,'$.config.start')<?1 AND json_extract(data,'$.config.end')>?2)",params![FINAL_TEST_END,FINAL_TEST_START],|r|Ok(json!({"total":r.get::<_,u64>(0)?,"completed":r.get::<_,u64>(1)?,"active":r.get::<_,u64>(2)?})))?;
        for field in ["jobs", "leaderboards"] {
            if let Some(rows) = value[field].as_array_mut() {
                rows.retain(|row| row["period"] == "development");
            }
        }
        value
            .as_object_mut()
            .context("overview object")?
            .remove("validation");
        value
            .as_object_mut()
            .context("overview object")?
            .remove("paper");
        value
            .as_object_mut()
            .context("overview object")?
            .remove("paper_candidates");
        value
            .as_object_mut()
            .context("overview object")?
            .remove("paper_approvals");
    }
    Ok(value)
}
async fn overview(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<auth::Principal>,
) -> Api<Value> {
    Ok(Json(scoped_overview(&app.store, &principal)?))
}
async fn events(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<auth::Principal>,
) -> Sse<impl tokio_stream::Stream<Item = std::result::Result<Event, Infallible>>> {
    let shutdown = app.store.shutdown.clone();
    let auth_store = app.store.clone();
    let identity = principal.clone();
    let stream = IntervalStream::new(tokio::time::interval(Duration::from_secs(1)))
        .take_while(move |_| {
            !shutdown.load(std::sync::atomic::Ordering::Acquire)
                && auth::active(&auth_store, &identity)
        })
        .map(move |_| {
            let data = scoped_overview(&app.store, &principal)
                .and_then(|v| Ok(serde_json::to_string(&v)?))
                .unwrap_or_else(|_| "{\"error\":\"snapshot unavailable\"}".into());
            Ok(Event::default().event("snapshot").data(data))
        });
    Sse::new(stream).keep_alive(KeepAlive::default())
}
/// Full history without curves, scoped before delivery to development agents.
async fn job_index(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<auth::Principal>,
) -> Api<Vec<Value>> {
    let db = app.store.db()?;
    let mut query=db.prepare("SELECT json_remove(data,'$.config','$.result.equity','$.result.fills','$.result.benchmarks') FROM jobs WHERE json_extract(data,'$.config.start') IS NOT NULL AND json_extract(data,'$.config.end') IS NOT NULL AND NOT(json_extract(data,'$.config.start')<?1 AND json_extract(data,'$.config.end')>?2) AND (?3 OR json_extract(data,'$.period')='development') ORDER BY rowid DESC")?;
    let rows = query
        .query_map(
            params![
                FINAL_TEST_END,
                FINAL_TEST_START,
                principal.role == auth::Role::Owner
            ],
            |r| r.get::<_, String>(0),
        )?
        .map(|r| Ok(serde_json::from_str::<Value>(&r?)?))
        .collect::<Result<Vec<_>>>()?;
    Ok(Json(rows))
}
async fn agent_capabilities(State(app): State<Arc<App>>) -> Api<Value> {
    Ok(Json(agent_api::capabilities(&app.store)?))
}
async fn agent_kit(State(app): State<Arc<App>>) -> Api<Value> {
    Ok(Json(candidates::kit(&app.store)?))
}
async fn agent_candles(
    State(app): State<Arc<App>>,
    Query(query): Query<agent_api::CandleQuery>,
) -> Api<Value> {
    let store = app.store.clone();
    Ok(Json(
        report_work(&app.store, move || agent_api::candles(&store, query)).await?,
    ))
}
async fn candidate_submit(
    State(app): State<Arc<App>>,
    Extension(p): Extension<auth::Principal>,
    Json(input): Json<candidates::CandidateInput>,
) -> Api<candidates::Candidate> {
    Ok(Json(candidates::submit(
        &app.store,
        input,
        (p.role == auth::Role::Agent).then_some(p.id.as_str()),
    )?))
}
async fn candidate_status(
    State(app): State<Arc<App>>,
    Extension(p): Extension<auth::Principal>,
    RoutePath(id): RoutePath<String>,
) -> Api<candidates::Candidate> {
    Ok(Json(candidates::get(
        &app.store,
        &id,
        (p.role == auth::Role::Agent).then_some(p.id.as_str()),
    )?))
}
async fn candidate_cancel(
    State(app): State<Arc<App>>,
    Extension(p): Extension<auth::Principal>,
    RoutePath(id): RoutePath<String>,
) -> Api<candidates::Candidate> {
    Ok(Json(candidates::cancel(
        &app.store,
        &id,
        (p.role == auth::Role::Agent).then_some(p.id.as_str()),
    )?))
}
async fn agent_trace(State(app): State<Arc<App>>, RoutePath(id): RoutePath<String>) -> Api<Value> {
    Ok(Json(agent_api::trace(&app.store, &id)?))
}
async fn agent_diagnostics(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
) -> Api<Value> {
    let store = app.store.clone();
    Ok(Json(
        report_work(&app.store, move || agent_api::diagnostics(&store, &id)).await?,
    ))
}
async fn submit(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<auth::Principal>,
    Json(input): Json<Submission>,
) -> Api<Job> {
    if principal.role == auth::Role::Agent && input.period != Period::Development {
        return Err(ApiError(
            anyhow::anyhow!("agents may submit development only"),
            StatusCode::FORBIDDEN,
        ));
    }
    Ok(Json(auth::submit(
        &app.store,
        input,
        (principal.role == auth::Role::Agent).then_some(principal.id.as_str()),
    )?))
}
async fn cancel(State(app): State<Arc<App>>, RoutePath(id): RoutePath<String>) -> Api<Job> {
    Ok(Json(app.store.cancel(&id)?))
}
#[derive(Deserialize, Default)]
struct ChartWindow {
    #[serde(default)]
    days: u32,
}
async fn report_work<T: Send + 'static>(
    store: &Store,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    let permit = store.report_slots.clone().acquire_owned().await?;
    tokio::task::spawn_blocking(move || {
        // Cancellation of the HTTP future must not release the permit while
        // its blocking computation is still consuming memory/CPU.
        let _permit = permit;
        work()
    })
    .await?
}
async fn reveal_validation(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<auth::Principal>,
    RoutePath(id): RoutePath<String>,
) -> Api<Value> {
    if principal.role != auth::Role::Owner {
        return Err(anyhow::anyhow!("owner access required").into());
    }
    let job = app.store.job(&id)?;
    ensure_research_window(job.config.start, job.config.end)?;
    if !(job.period == Period::Validation && job.state == "completed") {
        return Err(anyhow::anyhow!("completed validation evaluation required").into());
    }
    let store = app.store.clone();
    let value = report_work(&app.store, move || -> Result<Value> {
        store.owner_report(&id)?;
        let mut value = serde_json::to_value(&job)?;
        value["result"] = store.detail_view(&job)?;
        value["owner_detail"] = json!(true);
        Ok(value)
    })
    .await?;
    Ok(Json(value))
}
async fn job(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<auth::Principal>,
    RoutePath(id): RoutePath<String>,
    Query(window): Query<ChartWindow>,
) -> Api<Value> {
    let mut job = app.store.job(&id)?;
    ensure_research_window(job.config.start, job.config.end)?;
    if ![0, 7, 30, 90].contains(&window.days) {
        return Err(anyhow::anyhow!("chart windows are 7, 30, or 90 days").into());
    }
    let owner_detail = job.period == Period::Validation
        && principal.role == auth::Role::Owner
        && app.store.get::<Value>("validation_reveal", &id).is_ok();
    if job.period == Period::Development || owner_detail {
        let directory = app.store.root.join("jobs").join(&id);
        let name = if job.state == "completed" {
            "result.json"
        } else {
            "progress.json"
        };
        let value = if owner_detail {
            let (store, job) = (app.store.clone(), job.clone());
            Some(report_work(&app.store, move || store.detail_view(&job)).await?)
        } else {
            read_json::<Value>(&directory.join(name)).ok()
        };
        if let Some(mut value) = value {
            let needs_comparison = job.state == "completed"
                && value["benchmarks"]
                    .as_array()
                    .is_some_and(|rows| rows.iter().any(|b| b.get("comparison").is_none()));
            if needs_comparison && directory.join("report.json.gz").exists() {
                let (store, job) = (app.store.clone(), job.clone());
                value = report_work(&app.store, move || store.detail_view(&job)).await?;
            }
            if window.days > 0 && job.state == "completed" {
                let cached = directory.join(format!("detail-v3-chart-{}.json", window.days));
                value = if let Ok(v) = read_json::<Value>(&cached) {
                    v
                } else {
                    let path = directory.join("report.json.gz");
                    let cutoff = (job.config.end - i64::from(window.days) * 1440 * MINUTE)
                        .max(job.config.start);
                    report_work(&app.store, move || -> Result<Value> {
                        let mut report = read_report(&path)?;
                        report.equity.retain(|p| p.time >= cutoff);
                        report.fills.retain(|f| f.time >= cutoff);
                        let mut view = serde_json::to_value(RunView::from_report(&report))?;
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
                    .await?
                };
            }
            job.result = Some(value);
        }
    }
    let timing = app.store.get::<timing::Timing>("job_timing", &job.id).ok();
    let mut value = serde_json::to_value(job)?;
    value["timing"] = serde_json::to_value(timing)?;
    value["owner_detail"] = json!(owner_detail);
    Ok(Json(value))
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
    Extension(principal): Extension<auth::Principal>,
    RoutePath(id): RoutePath<String>,
    Query(page): Query<Page>,
) -> Api<Value> {
    let job = app.store.job(&id)?;
    ensure_research_window(job.config.start, job.config.end)?;
    if job.period == Period::Validation
        && !(principal.role == auth::Role::Owner
            && app.store.get::<Value>("validation_reveal", &id).is_ok())
    {
        return Err(
            anyhow::anyhow!("reveal validation details as owner before accessing trades").into(),
        );
    }
    let path = app.store.root.join("jobs").join(id).join("report.json.gz");
    let report = report_work(&app.store, move || read_report(&path)).await?;
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
    Ok(Json(
        app.store.campaign_with_objective(
            v["dataset"].as_str().context("dataset required")?,
            v["target_cagr"].as_f64().unwrap_or(0.15),
            v.get("objective")
                .map(|v| serde_json::from_value(v.clone()))
                .transpose()?
                .unwrap_or(Objective::SharpeDailyV1),
        )?,
    ))
}
async fn archive(State(app): State<Arc<App>>, RoutePath(id): RoutePath<String>) -> Api<Value> {
    let mut s: RegisteredStrategy = app.store.get("strategy", &id)?;
    s.archived = !s.archived;
    app.store.put("strategy", &id, &s)?;
    Ok(Json(json!({"archived":s.archived})))
}
async fn start_paper(State(app): State<Arc<App>>, Json(v): Json<Value>) -> Api<PaperSession> {
    Ok(Json(
        app.store.approve_paper(
            v["strategy"].as_str().context("strategy required")?,
            v["job"]
                .as_str()
                .context("eligible development job required")?,
            v["approve"].as_bool() == Some(true),
        )?,
    ))
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
async fn tune(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<auth::Principal>,
    Json(input): Json<Tune>,
) -> Api<Value> {
    ensure_tune(&input)?;
    let parent: RegisteredStrategy = app.store.get("strategy", &input.strategy)?;
    let campaign: Campaign = app.store.get("campaign", &input.campaign)?;
    if input.space.is_empty() {
        ensure_tune_kind(&parent.spec.kind)?;
    }
    let id = uuid::Uuid::new_v4().to_string();
    let return_id = id.clone();
    let store = app.store.clone();
    store.put(
        "tuning",
        &id,
        &json!({"id":id,"state":"running","seed":input.seed,"trials":input.trials,"strategy":input.strategy,"campaign":input.campaign,"objective":campaign.objective,"jobs":[],"agent":if principal.role==auth::Role::Agent {Some(principal.id.clone())}else{None}}),
    )?;
    tokio::spawn(async move {
        let result = run_tuning(&store, &input, &parent, &id).await;
        if let Err(e) = result {
            let _ = update_tuning(&store, &id, json!({"state":"failed","error":e.to_string()}));
        }
    });
    Ok(Json(json!({"id":return_id,"state":"running"})))
}
// Preserve trial membership and experiment context through every state change.
fn update_tuning(store: &Store, id: &str, changes: Value) -> Result<()> {
    let mut value: Value = store.get("tuning", id)?;
    let cancelled = value["state"] == "cancelled";
    if let (Some(target), Some(changes)) = (value.as_object_mut(), changes.as_object()) {
        target.extend(changes.clone());
    }
    if cancelled {
        value["state"] = json!("cancelled");
    }
    store.put("tuning", id, &value)
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
        let actor = store.get::<Value>("tuning", id)?;
        let job = auth::submit(
            store,
            Submission {
                timing: None,
                trace: None,
                strategy: s.id,
                campaign: input.campaign.clone(),
                period: Period::Development,
                idempotency_key: format!("tune:{id}:{trial}"),
                slippage_bps: None,
            },
            actor["agent"].as_str(),
        )?;
        jobs.push(job.id.clone());
        // Publish membership immediately, rather than waiting for completion.
        update_tuning(store, id, json!({"jobs":jobs}))?;
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
                    result["metrics"][j.objective.metric()]
                        .as_f64()
                        .unwrap_or(f64::NEG_INFINITY)
                } else {
                    f64::NEG_INFINITY
                };
                let minimum_improvement = match j.objective {
                    Objective::NetCagr => 0.0025,
                    Objective::SharpeDailyV1 => 0.01,
                };
                if score > best + minimum_improvement {
                    best = score;
                    stale = 0;
                } else {
                    stale += 1;
                }
                break;
            }
        }
        update_tuning(
            store,
            id,
            json!({"state":"running","jobs":jobs,"completed":trial+1,"best":if best.is_finite(){Some(best)}else{None},"stalled":stale}),
        )?;
    }
    update_tuning(
        store,
        id,
        json!({"state":"completed","jobs":jobs,"best":if best.is_finite(){Some(best)}else{None},"stop_reason":if stale>=20{"stalled"}else{"budget_exhausted"}}),
    )?;
    Ok(())
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tower::ServiceExt;
    fn owner_request(store: &Store) -> axum::http::request::Builder {
        let key = fs::read_to_string(auth::owner_key_path(store)).unwrap();
        axum::http::Request::builder().header("authorization", format!("Bearer {}", key.trim()))
    }
    #[tokio::test]
    async fn revoked_agent_stream_closes_and_validation_cannot_consume_budget() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path()).unwrap();
        let file = temp.path().join("agent.key");
        let credential = auth::issue_agent(&store, "stream", &file, 30).unwrap();
        let key = fs::read_to_string(file).unwrap();
        let request=axum::http::Request::builder().method("POST").uri("/api/jobs").header("host","localhost").header("authorization",format!("Bearer {}",key.trim())).header("content-type","application/json")
            .body(axum::body::Body::from(serde_json::to_vec(&json!({"strategy":"none","campaign":"none","period":"validation","idempotency_key":"denied"})).unwrap())).unwrap();
        assert_eq!(
            routes(store.clone())
                .oneshot(request)
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(store.ledger().unwrap()["used"], 0);
        let request = axum::http::Request::builder()
            .uri("/api/events")
            .header("host", "localhost")
            .header("authorization", format!("Bearer {}", key.trim()))
            .body(axum::body::Body::empty())
            .unwrap();
        let mut stream = routes(store.clone())
            .oneshot(request)
            .await
            .unwrap()
            .into_body()
            .into_data_stream();
        assert!(stream.next().await.is_some());
        auth::revoke_agent(&store, credential["id"].as_str().unwrap()).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), stream.next())
                .await
                .unwrap()
                .is_none()
        );
    }
    #[tokio::test]
    async fn http_rejects_anonymous_and_agent_owner_actions() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path()).unwrap();
        let keyfile = temp.path().join("agent.key");
        auth::issue_agent(&store, "dev", &keyfile, 30).unwrap();
        let key = fs::read_to_string(keyfile).unwrap();
        let app = routes(store.clone());
        let anonymous = axum::http::Request::builder()
            .uri("/api/overview")
            .header("host", "localhost")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(anonymous).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        for path in [
            "/api/paper",
            "/api/campaigns",
            "/api/infer",
            "/api/strategies/x/archive",
        ] {
            let request = axum::http::Request::builder()
                .method("POST")
                .uri(path)
                .header("host", "localhost")
                .header("authorization", format!("Bearer {}", key.trim()))
                .body(axum::body::Body::empty())
                .unwrap();
            assert_eq!(
                app.clone().oneshot(request).await.unwrap().status(),
                StatusCode::FORBIDDEN
            );
        }
        let request = axum::http::Request::builder()
            .uri("/api/overview")
            .header("host", "localhost")
            .header("authorization", format!("Bearer {}", key.trim()))
            .body(axum::body::Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 1_000_000)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(
            body.get("validation").is_none()
                && body.get("paper").is_none()
                && body.get("paper_candidates").is_none()
        );
    }
    #[test]
    fn tuning_updates_keep_trial_membership_and_context() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        store.put("tuning","t",&json!({"id":"t","state":"running","strategy":"parent","campaign":"c","trials":20,"jobs":[]})).unwrap();
        update_tuning(&store, "t", json!({"jobs":["trial"],"completed":1})).unwrap();
        update_tuning(&store, "t", json!({"state":"cancelled"})).unwrap();
        update_tuning(&store, "t", json!({"state":"completed","best":0.2})).unwrap();
        let t: Value = store.get("tuning", "t").unwrap();
        assert_eq!(t["jobs"], json!(["trial"]));
        assert_eq!(t["strategy"], "parent");
        assert_eq!(t["trials"], 20);
        assert_eq!(t["state"], "cancelled");
    }
    #[tokio::test]
    async fn open_browser_stream_ends_when_shutdown_is_requested() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let request = owner_request(&store)
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
    async fn history_index_includes_old_runs_without_chart_payloads() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let db = store.db().unwrap();
        for n in 0..650 {
            let id = format!("job-{n}");
            let data = json!({"id":id,"strategy":"s","period":"development","state":"completed","config":{"start":0,"end":60000,"large":"execution"},"result":{"metrics":{"net_cagr":0.1},"equity":[1,2],"fills":[3],"benchmarks":[4]}});
            db.execute(
                "INSERT INTO jobs VALUES(?1,?1,?1,?2)",
                rusqlite::params![id, data.to_string()],
            )
            .unwrap();
        }
        let response = routes(store.clone())
            .oneshot(
                owner_request(&store)
                    .uri("/api/jobs/index")
                    .header("host", "localhost")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
            .await
            .unwrap();
        let rows: Vec<Value> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(rows.len(), 650);
        assert_eq!(rows[649]["id"], "job-0");
        assert_eq!(rows[0]["result"]["metrics"]["net_cagr"], 0.1);
        for row in rows {
            assert!(row.get("config").is_none());
            assert!(row["result"].get("equity").is_none());
            assert!(row["result"].get("fills").is_none());
            assert!(row["result"].get("benchmarks").is_none());
        }
    }
    #[tokio::test]
    async fn owner_reveal_is_audited_but_agents_and_history_remain_restricted() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let start = timestamp("2024-01-01").unwrap();
        let rows = (0..30)
            .flat_map(|i| {
                MARKETS.iter().map(move |market| Candle {
                    market: (*market).into(),
                    time: start + i * MINUTE,
                    open: 100.into(),
                    high: 100.into(),
                    low: 100.into(),
                    close: 100.into(),
                    volume: 100.into(),
                })
            })
            .collect();
        let dataset =
            tc_data::snapshot(&store.root, rows, start, start + 30 * MINUTE, true, false).unwrap();
        let campaign = store.campaign(&dataset.id).unwrap();
        let strategy = store
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
        let mut job = store
            .submit(Submission {
                timing: None,
                trace: None,
                strategy: strategy.id,
                campaign: campaign.id,
                period: Period::Validation,
                idempotency_key: "validation".into(),
                slippage_bps: None,
            })
            .unwrap();
        worker(&store.root, &job.id).unwrap();
        job.state = "completed".into();
        job.result =
            Some(read_json(&store.root.join("jobs").join(&job.id).join("result.json")).unwrap());
        store.update_job(&job).unwrap();
        let before = store.ledger().unwrap();
        let response = routes(store.clone())
            .oneshot(
                owner_request(&store)
                    .method("POST")
                    .uri(format!("/api/jobs/{}/reveal", job.id))
                    .header("host", "localhost")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["owner_detail"], true);
        assert!(value["result"]["equity"].is_array());
        assert!(value["result"]["benchmarks"][0]["comparison"].is_object());
        assert_eq!(store.ledger().unwrap()["used"], before["used"]);
        assert_eq!(store.ledger().unwrap()["revealed"], true);
        let token = dir.path().join("agent.key");
        auth::issue_agent(&store, "test", &token, 1).unwrap();
        let key = fs::read_to_string(token).unwrap();
        for (method, path, status) in [
            (
                "GET",
                format!("/api/jobs/{}", job.id),
                StatusCode::FORBIDDEN,
            ),
            (
                "GET",
                format!("/api/jobs/{}/trades", job.id),
                StatusCode::FORBIDDEN,
            ),
            (
                "POST",
                format!("/api/jobs/{}/reveal", job.id),
                StatusCode::FORBIDDEN,
            ),
            (
                "GET",
                format!("/api/agent/jobs/{}/trace", job.id),
                StatusCode::FORBIDDEN,
            ),
            (
                "GET",
                format!("/api/agent/jobs/{}/diagnostics", job.id),
                StatusCode::FORBIDDEN,
            ),
            ("GET", "/api/jobs/index".into(), StatusCode::OK),
            ("GET", "/api/overview".into(), StatusCode::OK),
        ] {
            let response = routes(store.clone())
                .oneshot(
                    axum::http::Request::builder()
                        .method(method)
                        .uri(&path)
                        .header("host", "localhost")
                        .header("authorization", format!("Bearer {}", key.trim()))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), status);
            let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
                .await
                .unwrap();
            let text = String::from_utf8(bytes.to_vec()).unwrap();
            assert!(!text.contains(&job.id));
            if path == "/api/overview" {
                let value: Value = serde_json::from_str(&text).unwrap();
                assert_eq!(value["job_counts"]["total"], 0);
            }
        }
        for path in [
            format!("/api/jobs/{}?days=7", job.id),
            format!("/api/jobs/{}/trades", job.id),
        ] {
            let response = routes(store.clone())
                .oneshot(
                    owner_request(&store)
                        .uri(path)
                        .header("host", "localhost")
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
        let response = routes(store.clone())
            .oneshot(
                owner_request(&store)
                    .uri("/api/jobs/index")
                    .header("host", "localhost")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
            .await
            .unwrap();
        let rows: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(rows[0]["result"].get("equity").is_none());
    }
    #[tokio::test]
    async fn validation_http_never_loads_local_detail_files() {
        for objective in [Objective::NetCagr, Objective::SharpeDailyV1] {
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
                final_test: None,
                objective,
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
                    timing: None,
                    trace: None,
                    strategy: strategy.id,
                    campaign: campaign.id,
                    period: Period::Validation,
                    idempotency_key: "validation".into(),
                    slippage_bps: None,
                })
                .unwrap();
            job.state = "completed".into();
            job.result = Some(json!({objective.metric():0.2}));
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
                let req = owner_request(&store)
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
            let req = owner_request(&store)
                .uri(format!("/api/jobs/{}/trades", job.id))
                .header("host", "localhost")
                .body(axum::body::Body::empty())
                .unwrap();
            assert_eq!(
                routes(store).oneshot(req).await.unwrap().status(),
                StatusCode::BAD_REQUEST
            );
        }
    }
    #[tokio::test]
    async fn reserved_dates_are_hidden_from_http_detail_overview_and_leaderboards() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let job = Job {
            objective: Objective::SharpeDailyV1,
            id: "reserved".into(),
            strategy: "strategy".into(),
            campaign: "legacy".into(),
            period: Period::Development,
            state: "completed".into(),
            created_at: 1,
            updated_at: 1,
            progress: 1.0,
            error: None,
            result: Some(json!({"metrics":{"sharpe":99.0},"equity":"SECRET"})),
            config: RunConfig::new(FINAL_TEST_START, FINAL_TEST_END),
        };
        store
            .db()
            .unwrap()
            .execute(
                "INSERT INTO jobs(id,key,fingerprint,data) VALUES(?1,?1,?1,?2)",
                params![job.id, serde_json::to_string(&job).unwrap()],
            )
            .unwrap();
        save_json(
            &store.root.join("jobs/reserved/result.json"),
            &json!({"equity":"SECRET"}),
        )
        .unwrap();
        for route in [
            "/api/jobs/reserved?days=7",
            "/api/jobs/reserved/trades",
            "/api/overview",
            "/api/jobs/index",
        ] {
            let req = owner_request(&store)
                .uri(route)
                .header("host", "localhost")
                .body(axum::body::Body::empty())
                .unwrap();
            let response = routes(store.clone()).oneshot(req).await.unwrap();
            assert_eq!(
                response.status(),
                if route == "/api/overview" || route == "/api/jobs/index" {
                    StatusCode::OK
                } else {
                    StatusCode::BAD_REQUEST
                }
            );
            let body = axum::body::to_bytes(response.into_body(), 1_000_000)
                .await
                .unwrap();
            assert!(!String::from_utf8(body.to_vec()).unwrap().contains("SECRET"));
        }
    }
    #[tokio::test]
    async fn dns_rebinding_host_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let req = owner_request(&store)
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
        let store = Store::open(dir.path()).unwrap();
        let app = routes(store.clone());
        let request = owner_request(&store)
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
