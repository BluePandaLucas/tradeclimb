//! Rust browser UI. Accounting and strategy execution stay in native workers.
mod evaluations;
mod leaderboard;
mod paper;
mod strategies;
mod theme;
use evaluations::{Evaluations, RunDetail};
use gloo_net::http::Request;
use leaderboard::Leaderboard;
use leptos::{prelude::*, task::spawn_local};
use paper::PaperPanel;
use serde_json::{Value, json};
use strategies::{History, StrategyPicker};
use theme::ThemeToggle;
use wasm_bindgen::{JsCast, prelude::*};
fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or("").into()
}
fn items(v: &Value, key: &str) -> Vec<Value> {
    v[key].as_array().cloned().unwrap_or_default()
}
fn dataset_name(data: &Value, id: &str) -> String {
    let datasets = items(data, "datasets");
    let venue = datasets
        .iter()
        .find(|d| text(d, "id") == id)
        .map(|d| text(d, "venue"))
        .unwrap_or_default();
    let name = match venue.as_str() {
        "bitvavo" => "Bitvavo",
        "kraken" => "Kraken",
        "synthetic-fixture" => "Fixture",
        _ => "Dataset",
    };
    format!("{name} {}", short(id))
}
fn synthetic_job(data: &Value, job: &Value) -> bool {
    items(data, "campaigns")
        .iter()
        .find(|c| c["id"] == job["campaign"])
        .is_some_and(|c| {
            items(data, "datasets")
                .iter()
                .any(|d| d["id"] == c["dataset"] && d["synthetic"] == true)
        })
}
fn percent(v: &Value) -> String {
    v.as_f64()
        .map_or("—".into(), |n| format!("{:+.2}%", n * 100.0))
}
fn drawdown(v: &Value) -> String {
    v.as_f64()
        .map(|n| format!("{:.2}%", n.abs() * 100.0))
        .unwrap_or_else(|| "—".into())
}
fn money(v: f64) -> String {
    let raw = format!("{:.2}", v.abs());
    let (whole, cents) = raw.split_once('.').unwrap_or((&raw, "00"));
    let grouped = whole
        .chars()
        .rev()
        .enumerate()
        .flat_map(|(i, c)| {
            if i > 0 && i % 3 == 0 {
                vec![',', c]
            } else {
                vec![c]
            }
        })
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{}€{grouped}.{cents}", if v < 0.0 { "−" } else { "" })
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
fn sign_in() {
    if let Some(window) = web_sys::window() {
        let _ = window.location().set_href("/login");
    }
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
            if response.status() == 401 {
                sign_in();
                return Err("Sign in required".into());
            }
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
    let sidebar_collapsed = RwSignal::new(
        web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
            .and_then(|s| s.get_item("tc-sidebar-collapsed").ok().flatten())
            .map(|v| v == "true")
            .unwrap_or_else(|| {
                web_sys::window()
                    .and_then(|w| w.match_media("(max-width: 650px)").ok().flatten())
                    .is_some_and(|m| m.matches())
            }),
    );
    let leaderboard_group = RwSignal::new(String::new());
    let snapshot = RwSignal::new(json!({}));
    let status = RwSignal::new("Connecting to local workspace…".to_string());
    let online = RwSignal::new(false);
    let tab = RwSignal::new("Overview".to_string());
    let selected = RwSignal::new(String::new());
    let run_detail = RwSignal::new(Value::Null);
    let tuning_pending = RwSignal::new(false);
    let selected_strategy = RwSignal::new(String::new());
    let selected_history = RwSignal::new(String::new());
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
        let datasets = items(&data, "datasets");
        let native = |c: &&Value| {
            datasets
                .iter()
                .any(|d| d["id"] == c["dataset"] && d["venue"] == "bitvavo")
        };
        let preferred = campaigns
            .iter()
            .filter(native)
            .find(|c| is_sharpe(c))
            .or_else(|| campaigns.iter().find(native))
            .or(campaigns.first());
        if !campaigns
            .iter()
            .any(|c| text(c, "id") == selected_campaign.get())
            && let Some(first) = preferred
        {
            selected_campaign.set(text(first, "id"));
        }
    });
    // Selection detail is independent of the streamed list. Completed charts
    // are fetched once per run/window, and stale requests cannot replace it.
    let detail_key = Memo::new(move |_| {
        let id = selected.get();
        let job = snapshot.with(|data| {
            items(data, "jobs")
                .into_iter()
                .find(|j| text(j, "id") == id)
        });
        (
            id,
            chart_days.get(),
            job.as_ref()
                .map(|j| (text(j, "state"), j["updated_at"].as_i64().unwrap_or(0))),
        )
    });
    Effect::new(move || {
        let key = detail_key.get();
        if key.0.is_empty() {
            run_detail.set(Value::Null);
            return;
        }
        if run_detail.with_untracked(|job| text(job, "id") != key.0) {
            run_detail.set(Value::Null);
        }
        spawn_local(async move {
            if let Ok(response) = Request::get(&format!("/api/jobs/{}?days={}", key.0, key.1))
                .send()
                .await
                && response.ok()
                && let Ok(job) = response.json::<Value>().await
                && detail_key.get_untracked() == key
            {
                run_detail.update(|current| {
                    if *current != job {
                        *current = job;
                    }
                });
            }
        });
    });
    spawn_local(async move {
        match Request::get("/api/overview").send().await {
            Ok(r) if r.status() == 401 => sign_in(),
            Ok(r) => match r.json::<Value>().await {
                Ok(v) => {
                    snapshot.set(v);
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
                    snapshot.set(v);
                    online.set(true);
                }
            },
        );
        let _ =
            source.add_event_listener_with_callback("snapshot", callback.as_ref().unchecked_ref());
        callback.forget();
        let error = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
            online.set(false);
            spawn_local(async move {
                if let Ok(response) = Request::get("/api/session").send().await
                    && response.status() == 401
                {
                    sign_in();
                }
            });
        });
        source.set_onerror(Some(error.as_ref().unchecked_ref()));
        error.forget();
        on_cleanup(move || source.close());
    }
    view! {
    <a class="skip-link" href="#main-content">"Skip to content"</a><div class="shell" class:sidebar-collapsed=move||sidebar_collapsed.get()>
    <aside class="sidebar"><a class="brand" href="/"><span class="brand-mark">"↗"</span><span>"TradeClimb"<small>"RESEARCH WORKSPACE"</small></span></a>
    <button class="sidebar-toggle" aria-controls="workspace-navigation" aria-expanded=move||(!sidebar_collapsed.get()).to_string() aria-label=move||if sidebar_collapsed.get(){"Expand sidebar"}else{"Collapse sidebar"} title=move||if sidebar_collapsed.get(){"Expand sidebar"}else{"Collapse sidebar"} on:click=move|_|{
        let next = !sidebar_collapsed.get();sidebar_collapsed.set(next);
        if let Some(storage) = web_sys::window().and_then(|w|w.local_storage().ok().flatten()) {let _ = storage.set_item("tc-sidebar-collapsed", &next.to_string());}
    }><span aria-hidden="true">{move||if sidebar_collapsed.get(){"»"}else{"«"}}</span></button>
    <div class="nav-label">"WORKSPACE"</div><nav id="workspace-navigation">{["Overview","Backtests","Leaderboard","Paper trading","History"].into_iter().enumerate().map(move|(i,name)|view!{<button aria-label=name title=name aria-current=move||if tab.get()==name{Some("page")}else{None} class:active=move||tab.get()==name on:click=move|_|{tab.set(name.into());selected.set(String::new());}><span class="nav-icon" aria-hidden="true"><svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d={["M3 3h7v7H3z M14 3h7v7h-7z M3 14h7v7H3z M14 14h7v7h-7z","M4 3v18h17 M7 15l4-5 4 3 5-8","M4 20v-6h4v6 M10 20V9h4v11 M16 20V4h4v16","M12 3a9 9 0 1 0 9 9h-9z M15 3v6h6a9 9 0 0 0-6-6","M5 4h14v16H5z M8 8h8 M8 12h8 M8 16h5"][i]}/></svg></span><span class="nav-text">{name}</span></button>}).collect_view()}</nav>
    <div class="sidebar-bottom"><span class="mode-dot"></span>"Paper only"<p>"Local workspace · isolated strategies"</p><div class="version">"TC / v0.1"</div></div></aside>
    <main id="main-content"><header class="topbar"><div class="breadcrumb">"Workspace"<span>"/"</span>{move||tab.get()}</div><div class="connection"><span class="live-dot" class:offline=move||!online.get()></span><span class="connection-text">{move||if online.get(){"Connected"}else{"Reconnecting"}}</span><ThemeToggle/><button class="text-button" on:click=move|_|{spawn_local(async move {let _=Request::post("/api/logout").send().await;sign_in();});}>"Sign out"</button><span class="pill">"NO REAL ORDERS"</span></div></header>
    <div class="content" data-page=move||tab.get()><div class="page-heading"><div><div class="eyebrow">"TRADECLIMB / STRATEGY RESEARCH"</div><h1>{move||tab.get()}</h1><p>{move||match tab.get().as_str(){"Backtests"=>"Configure an evaluation. Compare results after costs.","Leaderboard"=>"Compare strategies within the same evaluation campaign.","Paper trading"=>"Observe strategies in independent virtual portfolios.","History"=>"Trace every strategy version and research decision.",_=>"Your strategies, evaluations and research progress."}}</p></div><div class="heading-actions"><Show when=move||tab.get()=="Overview"><button class="primary" on:click=move|_|tab.set("Backtests".into())>"＋ New backtest"</button></Show><div class="date-label"><span class="goal-label">{move||{let campaigns=items(&snapshot.get(),"campaigns");let chosen=selected_campaign.get();let campaign=campaigns.iter().find(|c|text(c,"id")==chosen).or(campaigns.first());campaign.map(|c|if is_sharpe(c) {"OBJECTIVE · SHARPE".into()} else {format!("GOAL {} NET CAGR",percent(&c["target_cagr"]))}).unwrap_or("OBJECTIVE · SHARPE".into())}}</span>{move||format!("Updated {} UTC",date(&snapshot.get()["time"]))}</div></div></div>
    <Show when=move||tab.get()=="Backtests"><div class="controls"><div class="strategy-control"><span class="field-label">"Strategy / version"</span><StrategyPicker snapshot=snapshot selected=selected_strategy/></div>
    <label class="campaign-select">"Evaluation campaign"<select prop:value=move||selected_campaign.get() on:change=move|ev|selected_campaign.set(event_target_value(&ev))><For each={move || items(&snapshot.get(),"campaigns")} key={|c:&Value|text(c,"id")} children={move |c| view!{<option value=text(&c,"id")>{format!("{} · {}",dataset_name(&snapshot.get(),&text(&c,"dataset")),score_label(&c))}</option>}}/></select><small class="campaign-period">{move||items(&snapshot.get(),"campaigns").iter().find(|c|text(c,"id")==selected_campaign.get()).map(|c|format!("{} → {} UTC",date(&c["development_start"]),date(&c["validation_end"]))).unwrap_or_default()}</small></label>
    <label>"Period"<select prop:value=move||period.get() on:change=move|ev|period.set(event_target_value(&ev))><option value="development">"Development"</option><option value="validation">"Validation · owner details available"</option></select></label>
    <button class="primary" on:click=move|_|{let s=snapshot.get();let strategy=if selected_strategy.get().is_empty(){text(&items(&s,"strategies").into_iter().find(|s|s["archived"]!=true).unwrap_or(Value::Null),"id")}else{selected_strategy.get()};let campaign=if selected_campaign.get().is_empty(){text(&items(&s,"campaigns").into_iter().next().unwrap_or(Value::Null),"id")}else{selected_campaign.get()};action("/api/jobs".into(),json!({"strategy":strategy,"campaign":campaign,"period":period.get(),"idempotency_key":format!("web-{}",js_sys::Date::now())}),status);}>"＋ Run backtest"</button>
    </div></Show>
    <div class="notice research-context"><strong>"Reserved final test"</strong>" · 1 Aug–31 Oct 2026."<span class="desktop-context">" Excluded from development and validation."</span></div>
    <div class="statusline" role="status">{move||{let s=status.get();if s=="Ready"{String::new()}else{s}}}</div>
    {move||{let data=snapshot.get();let jobs=items(&data,"jobs");let strategies=items(&data,"strategies");let active=jobs.iter().filter(|j|["queued","preparing","running"].contains(&text(j,"state").as_str())).count();let active=data["job_counts"]["active"].as_u64().unwrap_or(active as u64);let completed=data["job_counts"]["completed"].as_u64().unwrap_or_else(||jobs.iter().filter(|j|j["state"]=="completed").count() as u64);view!{
    {if tab.get()=="Overview" {view!{<div class="stats"><Stat label="STRATEGY VERSIONS" value=strategies.len().to_string() note="Every evaluated idea is retained"/><Stat label="ACTIVE EVALUATIONS" value=active.to_string() note="Bounded local workers"/><Stat label="COMPLETED EVALUATIONS" value=completed.to_string() note="Includes individual tuning trials"/><Stat label="VALIDATION BUDGET" value=format!("{} / {}",data["validation"]["used"],data["validation"]["maximum"]) note=if data["validation"]["revealed"]==true{"Owner has revealed validation"}else{"Campaign-wide scalar feedback"}/></div>}.into_any()}else{().into_any()}}
    {if items(&data,"datasets").iter().any(|d|d["synthetic"]==true){view!{<div class="notice synthetic-context"><strong>"Synthetic data present"</strong><span class="desktop-context">" · Fixture results test the platform; they are not market performance evidence."</span><span class="mobile-context">" · Not market performance evidence."</span></div>}.into_any()}else{().into_any()}}
    {if items(&data,"datasets").iter().any(|d|d["accepted"]==false){view!{<div class="notice">"A downloaded historical snapshot needs coverage review. It is excluded from research campaigns until explicitly accepted."</div>}.into_any()}else{().into_any()}}
    {if tab.get()=="Leaderboard" {view!{<Leaderboard data=data.clone() selected=leaderboard_group tab=tab run=selected/>}.into_any()}
    else {().into_any()}}
    <footer>"TC · Research, not a promise of returns."<span>"Fees included / independent virtual portfolios / no real orders"</span></footer>
    }}}
    <Show when=move||tab.get()=="History"><History snapshot=snapshot status=status selected=selected_history/></Show>
    <Show when=move||tab.get()=="Paper trading"><PaperPanel status=status pending=paper_pending snapshot=snapshot/></Show>
    <Show when=move||["Overview","Backtests"].contains(&tab.get().as_str())>
        <Evaluations snapshot=snapshot selected=selected status=status/>
        <RunDetail snapshot=snapshot detail=run_detail status=status chart_days=chart_days show_btc=show_btc show_basket=show_basket tuning_pending=tuning_pending/>
    </Show>
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
    #[prop(default = 0)] marker_total: usize,
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
    let rough_step = (high - low) / 4.0;
    let magnitude = 10_f64.powf(rough_step.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .into_iter()
        .map(|n| n * magnitude)
        .find(|n| *n >= rough_step)
        .unwrap_or(magnitude * 10.0);
    low = (low / step).floor() * step;
    high = (high / step).ceil() * step;
    let tick_count = ((high - low) / step).round() as usize;
    let x = move |time: f64| 65.0 + (time - start) / (end - start).max(1.0) * 1000.0;
    let y = move |value: f64| 260.0 - (value - low) / (high - low) * 225.0;
    let line = xy
        .iter()
        .map(|p| format!("{:.2},{:.2}", x(p.0), y(p.1)))
        .collect::<Vec<_>>()
        .join(" ");
    let area = format!("65,275 {line} 1065,275");
    let ticks=(0..=tick_count).map(|i|{let value=low+step*i as f64;view!{<g><line x1="65" x2="1065" y1=y(value) y2=y(value) stroke="var(--chart-grid)" stroke-dasharray="3 5"/><text x="0" y=y(value)+4.0 fill="var(--muted)" font-size="13">{money(value).trim_end_matches(".00").to_string()}</text></g>}}).collect_view();
    let color = |b: &Value| {
        if text(b, "name").starts_with("BTC") {
            "var(--benchmark-btc)"
        } else {
            "var(--benchmark-basket)"
        }
    };
    let benchmark_lines=benchmarks.iter().map(|b|{let points=items(b,"equity").iter().filter_map(|p|Some(format!("{:.2},{:.2}",x(p["time"].as_f64()?),y(p["equity"].as_f64()?)))).collect::<Vec<_>>().join(" ");view!{<polyline class="benchmark-line" points=points fill="none" stroke=color(b) stroke-opacity="1" stroke-dasharray=if text(b,"name").starts_with("BTC"){"7 4"}else{"2 4"} stroke-width="2" vector-effect="non-scaling-stroke"/>}}).collect_view();
    let total = marker_total.max(fills.len());
    let candidates: Vec<_> = fills
        .iter()
        .filter_map(|f| {
            let time = f["time"].as_f64()?;
            if time < start || time > end {
                return None;
            }
            let nearest = xy
                .iter()
                .min_by(|a, b| (a.0 - time).abs().total_cmp(&(b.0 - time).abs()))?;
            Some((x(time), y(nearest.1), f))
        })
        .collect();
    let isolated: Vec<_> = candidates
        .iter()
        .enumerate()
        .filter(|(i, p)| {
            !candidates
                .iter()
                .enumerate()
                .any(|(j, q)| *i != j && (p.0 - q.0).hypot(p.1 - q.1) < 16.0)
        })
        .map(|(_, p)| p)
        .collect();
    let dense = total > 100 || isolated.len() * 2 < candidates.len();
    let visible = if dense { vec![] } else { isolated };
    let shown = visible.len();
    let markers=visible.into_iter().map(|(px,py,f)|{let buy=f["side"]=="buy";let title=format!("{} {} at {}",if buy{"Buy"}else{"Sell"},text(f,"market"),date(&f["time"]));view!{<circle class="trade-marker" cx=*px cy=*py r="3" fill=if buy{"var(--positive)"}else{"var(--negative)"} stroke="var(--surface)" stroke-width="1"><title>{title}</title></circle>}}).collect_view();
    let marker_note = if shown < total {
        format!(
            "{} trade markers hidden for clarity. Use a shorter window or exact trades.",
            total - shown
        )
    } else {
        String::new()
    };
    view!{<div class="chart"><div class="chart-title">{label}<span>"EUR · net of execution costs"</span></div><svg viewBox="0 0 1100 305" role="img" aria-label=label><defs><linearGradient id="equity-fill" x1="0" x2="0" y1="0" y2="1"><stop offset="0%" stop-color="#258b6a" stop-opacity="0.13"/><stop offset="100%" stop-color="#258b6a" stop-opacity="0"/></linearGradient></defs>{ticks}<polygon points=area fill="url(#equity-fill)"/>{benchmark_lines}<polyline points=line fill="none" stroke="var(--positive)" stroke-width="2.2" stroke-linejoin="round"/>{markers}{(0..=4).map(|i|{let t=start+(end-start)*i as f64/4.0;let label=date(&json!(t)).chars().take(if end-start<172_800_000.0{16}else{10}).collect::<String>();view!{<text x=x(t) y="299" text-anchor=if i==0{"start"}else if i==4{"end"}else{"middle"} fill="var(--muted)" font-size="13">{label}</text>}}).collect_view()}</svg><div class="legend-heading">"Final equity"</div><div class="legend" aria-label="Final equity by portfolio"><span class="strategy-legend">"━ Strategy "<b>{xy.last().map(|p|money(p.1)).unwrap_or_default()}</b></span>{benchmarks.into_iter().map(|b|view!{<span style=format!("color:{}",color(&b))><i class="legend-swatch" style=if text(&b,"name").starts_with("BTC"){"border-top-style:dashed"}else{"border-top-style:dotted"}></i>{text(&b,"name")}<b>{items(&b,"equity").last().and_then(|p|p["equity"].as_f64()).map(money).unwrap_or_default()}</b></span>}).collect_view()}</div><p class="chart-note marker-note">{marker_note}</p></div>}.into_any()
}

fn is_sharpe(v: &Value) -> bool {
    v["objective"] == "sharpe_daily_v1"
}
fn score_label(v: &Value) -> &'static str {
    if is_sharpe(v) { "Sharpe" } else { "Net CAGR" }
}
fn score_value(v: &Value, value: &Value) -> String {
    if is_sharpe(v) {
        value
            .as_f64()
            .map(|n| format!("{n:.2}"))
            .unwrap_or_else(|| "—".into())
    } else {
        percent(value)
    }
}
fn job_score(v: &Value) -> String {
    let key = if is_sharpe(v) { "sharpe" } else { "net_cagr" };
    let result = if v["period"] == "validation" {
        &v["result"]
    } else {
        &v["result"]["metrics"]
    };
    format!("{} {}", score_label(v), score_value(v, &result[key]))
}
