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
    status: RwSignal<String>,
    pending: RwSignal<Vec<String>>,
    snapshot: RwSignal<Value>,
) -> impl IntoView {
    let strategies = Memo::new(move |_| snapshot.with(|s| items(s, "strategies")));
    let papers = Memo::new(move |_| snapshot.with(|s| items(s, "paper")));
    let selected = RwSignal::new(String::new());
    let candidates = Memo::new(move |_| {
        snapshot.with(|s| {
            let active: std::collections::HashSet<_> = items(s, "strategies")
                .into_iter()
                .filter(|v| v["archived"] != true)
                .map(|v| text(&v, "id"))
                .collect();
            items(s, "paper_candidates")
                .into_iter()
                .filter(|c| c["strategy"].as_str().is_some_and(|id| active.contains(id)))
                .collect::<Vec<_>>()
        })
    });
    let candidate = Memo::new(move |_| {
        candidates.with(|all| {
            all.iter()
                .find(|c| text(c, "strategy") == selected.get())
                .cloned()
        })
    });
    Effect::new(move |_| {
        let current = selected.get();
        if !strategies.with(|all| {
            all.iter()
                .any(|s| text(s, "id") == current && s["archived"] != true)
        }) {
            let next = candidates
                .with(|all| all.first().map(|c| text(c, "strategy")))
                .unwrap_or_else(|| {
                    strategies.with(|all| {
                        all.iter()
                            .find(|s| s["archived"] != true)
                            .map(|s| text(s, "id"))
                            .unwrap_or_default()
                    })
                });
            if next != current {
                selected.set(next);
            }
        }
    });
    view! {<section class="panel"><div class="panel-heading"><div><h2>"Paper portfolios"</h2><p>"Approve an eligible Bitvavo development evaluation. Maximum drawdown: 15%; growth aspiration: 15%. Candidates qualify by Sharpe; scalar validation does not certify risk."</p></div></div>
    <div class="paper-grid"><StrategyPicker snapshot=snapshot selected=selected/>
    <button class="secondary" data-strategy-id=move||selected.get() disabled=move||candidate.get().is_none() || selected.get().is_empty() || pending.with(|keys|keys.contains(&format!("start:{}",selected.get()))) || papers.with(|all|all.iter().any(|p|text(p,"strategy")==selected.get() && active(p))) on:click=move|_|{let id=selected.get_untracked();paper_action("/api/paper".into(),json!({"strategy":id,"job":candidate.get_untracked().map(|c|text(&c,"job")).unwrap_or_default(),"approve":true}),format!("start:{id}"),status,pending,snapshot);}>{move||if pending.with(|keys|keys.contains(&format!("start:{}",selected.get()))) {"Starting…"}else if papers.with(|all|all.iter().any(|p|text(p,"strategy")==selected.get() && active(p))) {"Session active"}else if candidate.get().is_none(){"Evaluation required"}else{"Approve paper session"}}</button></div>
    {move||if papers.with(Vec::is_empty){view!{<div class="empty">"No paper portfolios yet. Start a strategy to follow live market observations."</div>}.into_any()}
    else{papers.get().into_iter().map(|p|{
        let id=text(&p,"id");let strategy_id=text(&p,"strategy");let name=strategies.with(|all|all.iter().find(|s|text(s,"id")==strategy_id).map(|s|text(&s["spec"],"name")).unwrap_or_else(||short(&strategy_id)));let state=text(&p,"state");let can_stop=matches!(state.as_str(),"starting"|"running");let stopping=state=="stopping";let stopped=state=="stopped";
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
