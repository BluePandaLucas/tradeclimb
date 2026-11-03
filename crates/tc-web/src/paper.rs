use super::*;
fn active(p: &Value) -> bool {
    matches!(
        p["state"].as_str(),
        Some("starting" | "running" | "stopping")
    )
}
fn paper_action(
    path: String,
    body: Value,
    key: String,
    status: RwSignal<String>,
    pending: RwSignal<Vec<String>>,
    snapshot: RwSignal<Value>,
) {
    if pending.with_untracked(|keys| keys.contains(&key)) {
        return;
    }
    pending.update(|keys| keys.push(key.clone()));
    let stopping = path.ends_with("/stop");
    status.set(
        if stopping {
            "Requesting session stop…"
        } else {
            "Creating paper session…"
        }
        .into(),
    );
    spawn_local(async move {
        let result = async {
            let response = Request::post(&path)
                .json(&body)
                .map_err(|e| e.to_string())?
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let value: Value = response.json().await.map_err(|e| e.to_string())?;
            if !response.ok() {
                return Err(value["error"]
                    .as_str()
                    .unwrap_or("Paper request failed")
                    .to_string());
            }
            // Immediate feedback does not wait for the next SSE snapshot.
            let response = Request::get("/api/overview")
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let data: Value = response.json().await.map_err(|e| e.to_string())?;
            snapshot.set(data);
            Ok(())
        }
        .await;
        status.set(match result {
            Ok(()) => if stopping {
                "Stop requested"
            } else {
                "Paper session created"
            }
            .into(),
            Err(error) => error,
        });
        pending.update(|keys| keys.retain(|k| k != &key));
    });
}
#[component]
pub fn PaperPanel(
    strategies: Vec<Value>,
    papers: Vec<Value>,
    status: RwSignal<String>,
    pending: RwSignal<Vec<String>>,
    snapshot: RwSignal<Value>,
) -> impl IntoView {
    let sessions = papers.clone();
    view! {<section class="panel"><div class="panel-heading"><div><h2>"Paper portfolios"</h2><p>"Independent virtual capital · public Bitvavo data"</p></div></div>
    <div class="paper-grid">{strategies.iter().filter(|s|s["archived"]!=true).map(|s|{
        let id=text(s,"id");let existing=sessions.iter().any(|p|text(p,"strategy")==id && active(p));let key=format!("start:{id}");let disabled_key=key.clone();let label_key=key.clone();let name=text(&s["spec"],"name");let name_label=name.clone();
        view!{<button class="secondary" data-strategy-id=id.clone() disabled=move||existing||pending.with(|keys|keys.contains(&disabled_key)) on:click=move|_|paper_action("/api/paper".into(),json!({"strategy":id}),key.clone(),status,pending,snapshot)>{move||if pending.with(|keys|keys.contains(&label_key)){format!("Starting {name_label}…")}else if existing{format!("{name_label} · active")}else{format!("Start {name_label}")}}</button>}
    }).collect_view()}</div>
    {if papers.is_empty(){view!{<div class="empty">"No paper portfolios yet. Start a strategy to follow live market observations."</div>}.into_any()}
    else{papers.into_iter().map(|p|{
        let id=text(&p,"id");let strategy_id=text(&p,"strategy");let name=strategies.iter().find(|s|text(s,"id")==strategy_id).map(|s|text(&s["spec"],"name")).unwrap_or_else(||short(&strategy_id));let state=text(&p,"state");let can_stop=matches!(state.as_str(),"starting"|"running");let stopping=state=="stopping";let stopped=state=="stopped";
        let stop_id=id.clone();let key=format!("stop:{id}");let disabled_key=key.clone();let label_key=key.clone();let sim=&p["simulator"];let updated=if p["updated_at"].as_i64().unwrap_or(0)>0 {p["updated_at"].clone()}else{items(sim,"equity").last().map(|point|point["time"].clone()).unwrap_or_else(||p["created_at"].clone())};
        let activity=if text(&p,"activity").is_empty(){match state.as_str(){"running"=>"Live · waiting for market observations","stopped"=>"Session stopped","halted"=>"Session halted",_=>"Connecting to market data"}.to_string()}else{text(&p,"activity")};
        let equity=items(sim,"equity").last().and_then(|point|point["equity"].as_f64()).unwrap_or(10_000.0);let fills=items(sim,"fills").len();
        view!{<article class="paper-session" data-session-id=id.clone()><div class="section-row"><h3>{name}</h3><span class="badge">{state.clone()}</span><button class="secondary" disabled=move||!can_stop||pending.with(|keys|keys.contains(&disabled_key)) on:click=move|_|paper_action(format!("/api/paper/{stop_id}/stop"),json!({}),key.clone(),status,pending,snapshot)>{move||if stopping||pending.with(|keys|keys.contains(&label_key)){"Stopping…"}else if stopped{"Stopped"}else if !can_stop{"Session halted"}else{"Stop session"}}</button></div>
            <p class="paper-activity" role="status">{activity}</p><div class="paper-summary"><span>{format!("Virtual equity {}",money(equity))}</span><span>{format!("{fills} recent fills")}</span><span>{format!("Updated {} UTC",date(&updated))}</span></div>
            <p class="muted">{text(&p,"error")}</p><Chart points=items(sim,"equity") fills=items(sim,"fills") label="Paper portfolio equity" benchmarks=vec![]/>
        </article>}
    }).collect_view().into_any()}}
    </section>}
}
