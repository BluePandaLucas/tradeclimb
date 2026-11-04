use super::*;
const PAGE_SIZE: usize = 10;
fn running(v: &Value) -> bool {
    matches!(
        v["state"].as_str(),
        Some("running" | "queued" | "preparing")
    )
}
fn start_tuning(
    strategy: String,
    campaign: String,
    snapshot: RwSignal<Value>,
    pending: RwSignal<bool>,
    status: RwSignal<String>,
) {
    if pending.get_untracked()
        || snapshot.with_untracked(|s| items(s, "tuning").iter().any(running))
    {
        return;
    }
    pending.set(true);
    status.set("Starting parameter tuning…".into());
    spawn_local(async move {
        let result = async {
            let response = Request::post("/api/tune")
                .json(&json!({"strategy":strategy,"campaign":campaign,"trials":20,"seed":42}))
                .map_err(|e| e.to_string())?
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let value: Value = response.json().await.map_err(|e| e.to_string())?;
            if !response.ok() {
                return Err(text(&value, "error"));
            }
            snapshot.update(|s| {
                if !s["tuning"].is_array() {
                    s["tuning"] = json!([]);
                }
                if let Some(list) = s["tuning"].as_array_mut()
                    && !list.iter().any(|t| t["id"] == value["id"])
                {
                    list.push(value);
                }
            });
            Ok(())
        }
        .await;
        status.set(match result {
            Ok(()) => "Tuning started · results appear below".into(),
            Err(e) => e,
        });
        pending.set(false);
    });
}

#[component]
pub fn Evaluations(
    snapshot: RwSignal<Value>,
    selected: RwSignal<String>,
    status: RwSignal<String>,
) -> impl IntoView {
    let page = RwSignal::new(0_usize);
    let show_trials = RwSignal::new(false);
    let jobs = Memo::new(move |_| {
        let data = snapshot.get();
        let trials = items(&data, "tuning")
            .into_iter()
            .flat_map(|t| items(&t, "jobs"))
            .collect::<Vec<_>>();
        items(&data, "jobs")
            .into_iter()
            .filter(|j| show_trials.get() || !trials.iter().any(|id| id == &j["id"]))
            .collect::<Vec<_>>()
    });
    let pages = move || jobs.with(|j| j.len().div_ceil(PAGE_SIZE).max(1));
    let current = move || page.get().min(pages() - 1);
    view! {
        <section class="panel evaluations">
        <div class="panel-heading"><div><h2>"Strategy evaluations"</h2><p>"Net results, with a separate validation window"</p></div><span class="subtle">"Select a row to inspect"</span></div>
        <div class="tuning-sessions">
        <For each={move||items(&snapshot.get(),"tuning").into_iter().map(|t|text(&t,"id")).collect::<Vec<_>>()} key={|id:&String|id.clone()} children={move|id:String|{
            let tuning = Memo::new(move |_| snapshot.with(|s|items(s,"tuning").into_iter().find(|t|text(t,"id")==id).unwrap_or(Value::Null)));
            view!{<div class="tuning-session"><span>{move||{let t=tuning.get();format!("Tuning {} · {} · {} / {} trials completed · best CAGR {}{}",short(&text(&t,"id")),text(&t,"state"),t["completed"].as_u64().unwrap_or_else(||if t["state"]=="completed" {items(&t,"jobs").len() as u64}else{0}),t["trials"].as_u64().unwrap_or(20),percent(&t["best"]),t["error"].as_str().map(|e|format!(" · {e}")).unwrap_or_default())}}</span><Show when=move||running(&tuning.get())><button class="secondary" on:click=move|_|action(format!("/api/tuning/{}/cancel",text(&tuning.get_untracked(),"id")),json!({}),status)>"Stop tuning"</button></Show></div>}
        }}/>
        </div>
        <Show when=move||snapshot.with(|s|!items(s,"tuning").is_empty())><div class="evaluation-filters"><label class="check-label"><input type="checkbox" prop:checked=move||show_trials.get() on:change=move|ev|{show_trials.set(event_target_checked(&ev));page.set(0);}/>"Show individual tuning trials"</label><span>"Tuning trials are grouped above and hidden by default."</span></div></Show>
        <Show when=move||!jobs.with(Vec::is_empty) fallback=||view!{<div class="empty"><h3>"Your research starts here"</h3><p>"Import market data and register a strategy with the TC CLI, then submit your first backtest."</p><code>"cargo run --bin tc -- demo"</code><p>"Demo creates explicitly labeled synthetic fixtures."</p></div>}>
        <div class="table-scroll"><table><thead><tr><th>"STRATEGY / RUN"</th><th>"PERIOD"</th><th>"STATUS"</th><th>"NET CAGR"</th><th>"MAX DRAWDOWN"</th><th>"ELIGIBILITY"</th><th>""</th></tr></thead><tbody>
        <For each={move||jobs.with(|j|j.iter().skip(current()*PAGE_SIZE).take(PAGE_SIZE).map(|j|text(j,"id")).collect::<Vec<_>>())} key={|id:&String|id.clone()} children={move|id:String|{
            let select_id=StoredValue::new(id.clone());
            let job=Memo::new(move |_|jobs.with(|jobs|jobs.iter().find(|j|text(j,"id")==id).cloned().unwrap_or(Value::Null)));
            let name=move||snapshot.with(|s|items(s,"strategies").iter().find(|s|s["id"]==job.get()["strategy"]).map(|s|text(&s["spec"],"name")).unwrap_or_else(||short(&text(&job.get(),"strategy"))));
            let growth=move||job.with(|j|if j["period"]=="validation" {percent(&j["result"]["net_cagr"])}else{percent(&j["result"]["metrics"]["net_cagr"])});
            view!{<tr class="evaluation-row" data-run-id=select_id.get_value() class:selected=move||selected.get()==select_id.get_value() on:click=move|_|selected.set(select_id.get_value())>
            <td><button class="run-name" on:click=move|ev|{ev.stop_propagation();selected.set(select_id.get_value());}>{name}</button><small>{short(&select_id.get_value())}</small></td>
            <td><span class="badge">{move||text(&job.get(),"period")}</span></td>
            <td><span class="state">{move||text(&job.get(),"state")}</span>{move||job.with(|j|if j["period"]!="validation" && running(j){format!(" · {:.0}%",j["progress"].as_f64().unwrap_or(0.0)*100.0)}else{String::new()})}</td>
            <td class="numeric">{growth}</td><td class="numeric risk">{move||job.with(|j|if j["period"]=="validation"{"Restricted".into()}else{percent(&j["result"]["metrics"]["max_drawdown"])})}</td>
            <td>{move||job.with(|j|if j["period"]=="validation"{"CAGR only"}else if j["result"]["metrics"]["eligible"]==true{"Eligible"}else if j["state"]=="completed"{"Review"}else{"—"})}</td>
            <td><Show when=move||running(&job.get())><button class="text-button" on:click=move|ev|{ev.stop_propagation();action(format!("/api/jobs/{}/cancel",select_id.get_value()),json!({}),status);}>"Cancel"</button></Show></td></tr>}
        }}/>
        </tbody></table></div>
        <div class="evaluation-pagination"><span>{move||format!("Page {} of {} · {} recent runs",current()+1,pages(),jobs.with(Vec::len))}</span><button class="secondary" disabled=move||current()==0 on:click=move|_|page.set(current().saturating_sub(1))>"Previous runs"</button><button class="secondary" disabled={move||current()+1>=pages()} on:click=move|_|page.set(current()+1)>"Next runs"</button></div>
        </Show>
        </section>
    }
}
#[component]
pub fn RunDetail(
    snapshot: RwSignal<Value>,
    detail: RwSignal<Value>,
    status: RwSignal<String>,
    chart_days: RwSignal<i64>,
    show_btc: RwSignal<bool>,
    show_basket: RwSignal<bool>,
    tuning_pending: RwSignal<bool>,
) -> impl IntoView {
    view! {
    {move||{let job=detail.get(); if job.is_null() {return ().into_any();} let validation=job["period"]=="validation";let result=&job["result"];let metric=&result["metrics"];view!{<section class="panel run-detail"><div class="panel-heading"><div><h2>"Run details"</h2><p>{format!("{} → {} · EUR · {} bps fee + {} bps adverse allowance per fill",date(&job["config"]["start"]),date(&job["config"]["end"]),job["config"]["fee_bps"],job["config"]["slippage_bps"])}</p></div><code>{short(&text(&job,"id"))}</code></div>{if validation{view!{<div class="empty"><h3>{format!("Net CAGR {}",percent(&result["net_cagr"]))}</h3><p>"Validation detail is withheld. Explicit local owner inspection marks validation as revealed."</p></div>}.into_any()}else{let benches=items(result,"benchmarks").into_iter().filter(|b|if text(b,"name").starts_with("BTC"){show_btc.get()}else{show_basket.get()}).collect::<Vec<_>>();let equity=items(result,"equity");let fills=items(result,"fills");let id=text(&job,"id");view!{<div class="detail-metrics"><div>"NET RETURN"<strong>{percent(&metric["net_return"])}</strong></div><div>"FEES"<strong>{metric["total_fees"].as_str().and_then(|s|s.parse::<f64>().ok()).map(money).unwrap_or("—".into())}</strong></div><div>"FILLS"<strong>{metric["trades"].to_string()}</strong></div><div>"MEAN MONTH"<strong>{percent(&metric["mean_monthly_return"])}</strong></div></div><div class="chart-controls"><label>"Chart window"<select prop:value=move||chart_days.get().to_string() on:change=move|ev|chart_days.set(event_target_value(&ev).parse().unwrap_or(0))><option value="0">"Full period"</option><option value="90">"Last 90 days"</option><option value="30">"Last 30 days"</option><option value="7">"Last 7 days"</option></select></label><label class="check-label"><input type="checkbox" prop:checked=move||show_btc.get() on:change=move|ev|show_btc.set(event_target_checked(&ev))/>"BTC reference"</label><label class="check-label"><input type="checkbox" prop:checked=move||show_basket.get() on:change=move|ev|show_basket.set(event_target_checked(&ev))/>"Basket reference"</label></div><Chart points=equity fills=fills label="Net portfolio equity" benchmarks=benches/><p class="chart-note">"Green ▲ buys · red ▼ sells · ≤1,500 curve points / ≤200 markers. Metrics use full-resolution observations."</p><div class="warnings">{items(metric,"warnings").into_iter().chain(items(metric,"reasons")).map(|w|view!{<p>{w.as_str().unwrap_or("").to_string()}</p>}).collect_view()}</div><div class="detail-actions"><a class="secondary" href=format!("/api/jobs/{id}/trades?limit=100") target="_blank">"Exact trades (JSON) ↗"</a><button class="secondary" disabled=move||tuning_pending.get()||snapshot.with(|s|items(s,"tuning").iter().any(running)) on:click={let sid=text(&job,"strategy");let cid=text(&job,"campaign");move|_|start_tuning(sid.clone(),cid.clone(),snapshot,tuning_pending,status)}>{move||if tuning_pending.get(){"Starting tuning…"}else if snapshot.with(|s|items(s,"tuning").iter().any(running)){"Tuning in progress…"}else{"Tune parameters · 20 trials"}}</button></div>}.into_any()}}</section>}.into_any()}}
    }
}
