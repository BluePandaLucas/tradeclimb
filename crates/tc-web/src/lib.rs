//! Rust browser UI. Accounting and strategy execution stay in native workers.
mod paper;
mod theme;
use gloo_net::http::Request;
use leptos::{prelude::*, task::spawn_local};
use paper::PaperPanel;
use serde_json::{Value, json};
use theme::ThemeToggle;
use wasm_bindgen::{JsCast, prelude::*};
fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or("").into()
}
fn items(v: &Value, key: &str) -> Vec<Value> {
    v[key].as_array().cloned().unwrap_or_default()
}
fn percent(v: &Value) -> String {
    v.as_f64()
        .map_or("—".into(), |n| format!("{:+.2}%", n * 100.0))
}
fn money(v: f64) -> String {
    format!("€{v:.2}")
}
fn short(v: &str) -> String {
    v.chars().take(8).collect()
}
fn date(v: &Value) -> String {
    v.as_f64()
        .map(|n| {
            js_sys::Date::new(&JsValue::from_f64(n))
                .to_iso_string()
                .as_string()
                .unwrap_or_default()
                .replace('T', " ")
                .chars()
                .take(16)
                .collect()
        })
        .unwrap_or_default()
}
fn hydrate_selected(
    mut data: Value,
    snapshot: RwSignal<Value>,
    selected: RwSignal<String>,
    days: RwSignal<i64>,
) {
    let id = selected.get_untracked();
    let window = days.get_untracked();
    if id.is_empty() {
        snapshot.set(data);
        return;
    }
    spawn_local(async move {
        if let Ok(response) = Request::get(&format!("/api/jobs/{id}?days={window}"))
            .send()
            .await
            && let Ok(job) = response.json::<Value>().await
            && let Some(jobs) = data["jobs"].as_array_mut()
            && let Some(entry) = jobs.iter_mut().find(|j| text(j, "id") == id)
        {
            *entry = job;
        }
        if selected.get_untracked() == id && days.get_untracked() == window {
            snapshot.set(data);
        }
    });
}
fn action(path: String, body: Value, status: RwSignal<String>) {
    spawn_local(async move {
        status.set("Working…".into());
        let result = async {
            let response = Request::post(&path)
                .json(&body)
                .map_err(|e| e.to_string())?
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let value: Value = response.json().await.map_err(|e| e.to_string())?;
            if response.ok() {
                Ok(value)
            } else {
                Err(value["error"]
                    .as_str()
                    .unwrap_or("Request failed")
                    .to_string())
            }
        }
        .await;
        status.set(match result {
            Ok(_) => "Saved".into(),
            Err(e) => e,
        });
    });
}
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}
#[component]
fn App() -> impl IntoView {
    let snapshot = RwSignal::new(json!({}));
    let status = RwSignal::new("Connecting to local workspace…".to_string());
    let online = RwSignal::new(false);
    let tab = RwSignal::new("Overview".to_string());
    let selected = RwSignal::new(String::new());
    let selected_strategy = RwSignal::new(String::new());
    let selected_campaign = RwSignal::new(String::new());
    let paper_pending = RwSignal::new(Vec::<String>::new());
    let period = RwSignal::new("development".to_string());
    let chart_days = RwSignal::new(0_i64);
    let show_btc = RwSignal::new(true);
    let show_basket = RwSignal::new(true);
    Effect::new(move || {
        let data = snapshot.get();
        let strategies = items(&data, "strategies")
            .into_iter()
            .filter(|s| s["archived"] != true)
            .collect::<Vec<_>>();
        if !strategies
            .iter()
            .any(|s| text(s, "id") == selected_strategy.get())
            && let Some(first) = strategies.first()
        {
            selected_strategy.set(text(first, "id"));
        }
        let campaigns = items(&data, "campaigns");
        if !campaigns
            .iter()
            .any(|c| text(c, "id") == selected_campaign.get())
            && let Some(first) = campaigns.first()
        {
            selected_campaign.set(text(first, "id"));
        }
    });
    Effect::new(move || {
        let _ = selected.get();
        let _ = chart_days.get();
        hydrate_selected(snapshot.get_untracked(), snapshot, selected, chart_days);
    });
    Effect::new(move || {
        if tab.get() == "Overview"
            && selected.get().is_empty()
            && let Some(id) = snapshot.with(|data| {
                items(data, "jobs")
                    .into_iter()
                    .find(|job| job["period"] == "development" && job["state"] == "completed")
                    .map(|job| text(&job, "id"))
            })
        {
            selected.set(id);
        }
    });
    spawn_local(async move {
        match Request::get("/api/overview").send().await {
            Ok(r) => match r.json::<Value>().await {
                Ok(v) => {
                    hydrate_selected(v, snapshot, selected, chart_days);
                    status.set("Ready".into());
                    online.set(true);
                }
                Err(e) => status.set(e.to_string()),
            },
            Err(e) => status.set(e.to_string()),
        }
    });
    if let Ok(source) = web_sys::EventSource::new("/api/events") {
        let callback = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(
            move |event: web_sys::MessageEvent| {
                if let Some(raw) = event.data().as_string()
                    && let Ok(v) = serde_json::from_str::<Value>(&raw)
                {
                    hydrate_selected(v, snapshot, selected, chart_days);
                    online.set(true);
                }
            },
        );
        let _ =
            source.add_event_listener_with_callback("snapshot", callback.as_ref().unchecked_ref());
        callback.forget();
        let error = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| online.set(false));
        source.set_onerror(Some(error.as_ref().unchecked_ref()));
        error.forget();
        on_cleanup(move || source.close());
    }
    view! {
    <div class="shell">
    <aside class="sidebar"><a class="brand" href="/"><span class="brand-mark">"↗"</span><span>"TradeClimb"<small>"RESEARCH WORKSPACE"</small></span></a>
    <div class="nav-label">"WORKSPACE"</div><nav>{["Overview","Backtests","Paper trading","History"].into_iter().enumerate().map(move|(i,name)|view!{<button aria-label=name class:active=move||tab.get()==name on:click=move|_|{tab.set(name.into());selected.set(String::new());}><span class="nav-icon" aria-hidden="true">{["◈","⌁","◉","▤"][i]}</span>{name}</button>}).collect_view()}</nav>
    <div class="sidebar-bottom"><span class="mode-dot"></span>"Paper only"<p>"Local workspace · trusted code"</p><div class="version">"TC / v0.1"</div></div></aside>
    <main><header class="topbar"><div class="breadcrumb">"Workspace"<span>"/"</span>{move||tab.get()}</div><div class="connection"><span class:offline=move||!online.get() class="live-dot"></span><span class="connection-text">{move||if online.get(){"Connected"}else{"Reconnecting"}}</span><ThemeToggle/><span class="pill">"NO REAL ORDERS"</span></div></header>
    <div class="content"><div class="page-heading"><div><div class="eyebrow">"TRADECLIMB / STRATEGY RESEARCH"</div><h1>{move||tab.get()}</h1><p>"Explore ideas. Measure honestly. Keep climbing."</p></div><div class="date-label"><span class="goal-label">{move||{let campaigns=items(&snapshot.get(),"campaigns");let chosen=selected_campaign.get();let campaign=campaigns.iter().find(|c|text(c,"id")==chosen).or(campaigns.first());format!("GOAL {} NET CAGR",campaign.map(|c|percent(&c["target_cagr"])).unwrap_or("+15.00%".into()))}}</span>{move||date(&snapshot.get()["time"])}</div></div>
    <div class="controls"><label>"Strategy"<select on:change=move|ev|selected_strategy.set(event_target_value(&ev))><For each={move || items(&snapshot.get(),"strategies").into_iter().filter(|s|s["archived"]!=true).collect::<Vec<_>>()} key={|s:&Value|text(s,"id")} children={move |s| view!{<option value=text(&s,"id")>{text(&s["spec"],"name")}</option>}}/></select></label>
    <label class="campaign-select">"Evaluation campaign"<select on:change=move|ev|selected_campaign.set(event_target_value(&ev))><For each={move || items(&snapshot.get(),"campaigns")} key={|c:&Value|text(c,"id")} children={move |c| view!{<option value=text(&c,"id")>{format!("{} · {} → {} · goal {}",short(&text(&c,"dataset")),date(&c["development_start"]),date(&c["validation_end"]),percent(&c["target_cagr"]))}</option>}}/></select></label>
    <label>"Period"<select on:change=move|ev|period.set(event_target_value(&ev))><option value="development">"Development"</option><option value="validation">"Validation · CAGR only"</option></select></label>
    <button class="primary" on:click=move|_|{let s=snapshot.get();let strategy=if selected_strategy.get().is_empty(){text(&items(&s,"strategies").into_iter().find(|s|s["archived"]!=true).unwrap_or(Value::Null),"id")}else{selected_strategy.get()};let campaign=if selected_campaign.get().is_empty(){text(&items(&s,"campaigns").into_iter().next().unwrap_or(Value::Null),"id")}else{selected_campaign.get()};action("/api/jobs".into(),json!({"strategy":strategy,"campaign":campaign,"period":period.get(),"idempotency_key":format!("web-{}",js_sys::Date::now())}),status);}>"＋ Run backtest"</button>
    </div>
    <div class="statusline" role="status">{move||status.get()}</div>
    {move||{let data=snapshot.get();let jobs=items(&data,"jobs");let strategies=items(&data,"strategies");let papers=items(&data,"paper");let active=jobs.iter().filter(|j|["queued","preparing","running"].contains(&text(j,"state").as_str())).count();let completed=jobs.iter().filter(|j|j["state"]=="completed").count();view!{
    <div class="stats"><Stat label="STRATEGY VERSIONS" value=strategies.len().to_string() note="Every evaluated idea is retained"/><Stat label="ACTIVE EVALUATIONS" value=active.to_string() note="Bounded local workers"/><Stat label="COMPLETED BACKTESTS" value=completed.to_string() note="Costs included in all results"/><Stat label="VALIDATION BUDGET" value=format!("{} / {}",data["validation"]["used"],data["validation"]["maximum"]) note=if data["validation"]["revealed"]==true{"Owner has revealed validation"}else{"Campaign-wide scalar feedback"}/></div>
    {if items(&data,"datasets").iter().any(|d|d["synthetic"]==true){view!{<div class="notice">"◇ Synthetic fixtures are present. Their curves verify the platform and are not market performance evidence."</div>}.into_any()}else{().into_any()}}
    {if items(&data,"datasets").iter().any(|d|d["accepted"]==false){view!{<div class="notice">"A downloaded historical snapshot needs coverage review. It is excluded from research campaigns until explicitly accepted."</div>}.into_any()}else{().into_any()}}
    {if tab.get()=="Paper trading" {view!{<PaperPanel strategies=strategies.clone() papers=papers status=status pending=paper_pending snapshot=snapshot/>}.into_any()}
    else if tab.get()=="History" {view!{<section class="panel"><div class="panel-heading"><div><h2>"Strategy history book"</h2><p>"Immutable versions, descriptions, and lineage"</p></div></div>{strategies.into_iter().map(|s|{let id=text(&s,"id");view!{<article class="history-entry"><div class="section-row"><h3>{text(&s["spec"],"name")}</h3><code>{short(&id)}</code><button class="secondary" on:click=move|_|action(format!("/api/strategies/{id}/archive"),json!({}),status)>{if s["archived"]==true{"Restore"}else{"Archive"}}</button></div><p>{text(&s["spec"],"description")}</p><p class="muted">{format!("{} · {}",date(&s["created_at"]),s["spec"]["kind"]["kind"].as_str().unwrap_or("strategy"))}</p></article>}}).collect_view()}</section>}.into_any()}
    else {view!{<section class="panel"><div class="panel-heading"><div><h2>"Strategy evaluations"</h2><p>"Net results, with a separate validation window"</p></div><span class="subtle">"Select a run to inspect"</span></div>{if jobs.is_empty(){view!{<div class="empty"><h3>"Your research starts here"</h3><p>"Import market data and register a strategy with the TC CLI, then submit your first backtest."</p><code>"cargo run --bin tc -- demo"</code><p>"Demo creates explicitly labeled synthetic fixtures."</p></div>}.into_any()}else{view!{<div class="table-scroll"><table><thead><tr><th>"STRATEGY / RUN"</th><th>"PERIOD"</th><th>"STATUS"</th><th>"NET CAGR"</th><th>"MAX DRAWDOWN"</th><th>"ELIGIBILITY"</th><th>""</th></tr></thead><tbody>{jobs.iter().map(|j|{let id=text(j,"id");let select_id=id.clone();let strategy=text(j,"strategy");let name=strategies.iter().find(|s|text(s,"id")==strategy).map(|s|text(&s["spec"],"name")).unwrap_or_else(||short(&strategy));let result=&j["result"];let validation=j["period"]=="validation";let growth=if validation{&result["net_cagr"]}else{&result["metrics"]["net_cagr"]};let active=["queued","running","preparing"].contains(&text(j,"state").as_str());view!{<tr class:selected=move||selected.get()==select_id><td><button class="run-name" on:click=move|_|selected.set(id.clone())>{name}</button><small>{short(&text(j,"id"))}</small></td><td><span class="badge">{text(j,"period")}</span></td><td><span class="state">{text(j,"state")}</span>{if !validation && active{format!(" · {:.0}%",j["progress"].as_f64().unwrap_or(0.0)*100.0)}else{String::new()}}</td><td class="numeric">{percent(growth)}</td><td class="numeric risk">{if validation{"Restricted".into()}else{percent(&result["metrics"]["max_drawdown"])}}</td><td>{if validation{"CAGR only"}else if result["metrics"]["eligible"]==true{"Eligible"}else if j["state"]=="completed"{"Review"}else{"—"}}</td><td>{if active{let id=text(j,"id");view!{<button class="text-button" on:click=move|_|action(format!("/api/jobs/{id}/cancel"),json!({}),status)>"Cancel"</button>}.into_any()}else{().into_any()}}</td></tr>}}).collect_view()}</tbody></table></div>}.into_any()}}</section>}.into_any()}}
    {jobs.iter().find(|j|text(j,"id")==selected.get()).map(|job|{let job=job.clone();let validation=job["period"]=="validation";let result=&job["result"];let metric=&result["metrics"];view!{<section class="panel run-detail"><div class="panel-heading"><div><h2>"Run details"</h2><p>{format!("{} → {} · EUR · {} bps fee + {} bps adverse allowance per fill",date(&job["config"]["start"]),date(&job["config"]["end"]),job["config"]["fee_bps"],job["config"]["slippage_bps"])}</p></div><code>{short(&text(&job,"id"))}</code></div>{if validation{view!{<div class="empty"><h3>{format!("Net CAGR {}",percent(&result["net_cagr"]))}</h3><p>"Validation detail is withheld. Explicit local owner inspection marks validation as revealed."</p></div>}.into_any()}else{let benches=items(result,"benchmarks").into_iter().filter(|b|if text(b,"name").starts_with("BTC"){show_btc.get()}else{show_basket.get()}).collect::<Vec<_>>();let equity=items(result,"equity");let fills=items(result,"fills");let id=text(&job,"id");view!{<div class="detail-metrics"><div>"NET RETURN"<strong>{percent(&metric["net_return"])}</strong></div><div>"FEES"<strong>{metric["total_fees"].as_str().and_then(|s|s.parse::<f64>().ok()).map(money).unwrap_or("—".into())}</strong></div><div>"FILLS"<strong>{metric["trades"].to_string()}</strong></div><div>"MEAN MONTH"<strong>{percent(&metric["mean_monthly_return"])}</strong></div></div><div class="chart-controls"><label>"Chart window"<select prop:value=move||chart_days.get().to_string() on:change=move|ev|chart_days.set(event_target_value(&ev).parse().unwrap_or(0))><option value="0">"Full period"</option><option value="90">"Last 90 days"</option><option value="30">"Last 30 days"</option><option value="7">"Last 7 days"</option></select></label><label class="check-label"><input type="checkbox" prop:checked=move||show_btc.get() on:change=move|ev|show_btc.set(event_target_checked(&ev))/>"BTC reference"</label><label class="check-label"><input type="checkbox" prop:checked=move||show_basket.get() on:change=move|ev|show_basket.set(event_target_checked(&ev))/>"Basket reference"</label></div><Chart points=equity fills=fills label="Net portfolio equity" benchmarks=benches/><p class="chart-note">"Green ▲ buys · red ▼ sells · ≤1,500 curve points / ≤200 markers. Metrics use full-resolution observations."</p><div class="warnings">{items(metric,"warnings").into_iter().chain(items(metric,"reasons")).map(|w|view!{<p>{w.as_str().unwrap_or("").to_string()}</p>}).collect_view()}</div><div class="detail-actions"><a class="secondary" href=format!("/api/jobs/{id}/trades?limit=100") target="_blank">"Exact trades (JSON) ↗"</a><button class="secondary" on:click={let sid=text(&job,"strategy");let cid=text(&job,"campaign");move|_|action("/api/tune".into(),json!({"strategy":sid,"campaign":cid,"trials":20,"seed":42}),status)}>"Tune parameters · 20 trials"</button></div>}.into_any()}}</section>}})}
    <footer>"TC · Research, not a promise of returns."<span>"Fees included / independent virtual portfolios / no real orders"</span></footer>
    }}}
    </div></main></div>
    }
}
#[component]
fn Stat(label: &'static str, value: String, note: &'static str) -> impl IntoView {
    view! {<div class="stat"><div class="stat-label">{label}</div><div class="stat-value">{value}</div><div class="stat-note">{note}</div></div>}
}
#[component]
fn Chart(
    points: Vec<Value>,
    fills: Vec<Value>,
    label: &'static str,
    benchmarks: Vec<Value>,
) -> impl IntoView {
    let xy: Vec<(f64, f64)> = points
        .iter()
        .filter_map(|p| Some((p["time"].as_f64()?, p["equity"].as_f64()?)))
        .collect();
    if xy.len() < 2 {
        return view! {<div class="chart-empty">"Waiting for equity observations…"</div>}
            .into_any();
    }
    let start = xy[0].0;
    let end = xy.last().map_or(start + 1.0, |p| p.0);
    let mut low = xy.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let mut high = xy.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    for b in &benchmarks {
        for p in items(b, "equity") {
            if let Some(v) = p["equity"].as_f64() {
                low = low.min(v);
                high = high.max(v);
            }
        }
    }
    let pad = ((high - low) * 0.12).max(10.0);
    low = (low - pad).max(0.0);
    high += pad;
    let x = move |time: f64| 65.0 + (time - start) / (end - start).max(1.0) * 1000.0;
    let y = move |value: f64| 260.0 - (value - low) / (high - low) * 225.0;
    let line = xy
        .iter()
        .map(|p| format!("{:.2},{:.2}", x(p.0), y(p.1)))
        .collect::<Vec<_>>()
        .join(" ");
    let area = format!("65,275 {line} 1065,275");
    let ticks=(0..5).map(|i|{let value=low+(high-low)*i as f64/4.0;view!{<g><line x1="65" x2="1065" y1=y(value) y2=y(value) stroke="var(--chart-grid)" stroke-dasharray="3 5"/><text x="0" y=y(value)+4.0 fill="var(--muted)" font-size="11">{money(value)}</text></g>}}).collect_view();
    let benchmark_lines=benchmarks.iter().enumerate().map(|(i,b)|{let points=items(b,"equity").iter().filter_map(|p|Some(format!("{:.2},{:.2}",x(p["time"].as_f64()?),y(p["equity"].as_f64()?)))).collect::<Vec<_>>().join(" ");view!{<polyline points=points fill="none" stroke=if i==0{"#9aabc2"}else{"#c1b49c"} stroke-width="1.6" stroke-dasharray="5 4"/>}}).collect_view();
    let markers=fills.iter().take(200).filter_map(|f|{let time=f["time"].as_f64()?;let nearest=xy.iter().min_by(|a,b|(a.0-time).abs().total_cmp(&(b.0-time).abs()))?;let buy=f["side"]=="buy";let label=format!("{} {} at {}",if buy{"Buy"}else{"Sell"},text(f,"market"),date(&f["time"]));Some(view!{<circle cx=x(time) cy=y(nearest.1) r="3" fill=if buy{"var(--positive)"}else{"var(--negative)"} stroke="var(--surface)" stroke-width="1"><title>{label}</title></circle>})}).collect_view();
    view!{<div class="chart"><div class="chart-title">{label}<span>"EUR · net of execution costs"</span></div><svg viewBox="0 0 1100 305" role="img" aria-label=label><defs><linearGradient id="equity-fill" x1="0" x2="0" y1="0" y2="1"><stop offset="0%" stop-color="#258b6a" stop-opacity="0.13"/><stop offset="100%" stop-color="#258b6a" stop-opacity="0"/></linearGradient></defs>{ticks}<polygon points=area fill="url(#equity-fill)"/>{benchmark_lines}<polyline points=line fill="none" stroke="var(--positive)" stroke-width="2.2" stroke-linejoin="round"/>{markers}<text x="65" y="299" fill="var(--muted)" font-size="11">{date(&points[0]["time"])}</text><text x="925" y="299" fill="var(--muted)" font-size="11">{date(&points[points.len()-1]["time"])}</text></svg><div class="legend"><span>"● Strategy"</span>{benchmarks.into_iter().map(|b|view!{<span>{format!("┄ {}",text(&b,"name"))}</span>}).collect_view()}</div></div>}.into_any()
}
