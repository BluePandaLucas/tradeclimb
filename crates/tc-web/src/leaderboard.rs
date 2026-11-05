use super::*;

#[component]
pub fn Leaderboard(
    data: Value,
    selected: RwSignal<String>,
    tab: RwSignal<String>,
    run: RwSignal<String>,
) -> impl IntoView {
    move || {
        let groups = items(&data, "leaderboards");
        let chosen = selected.get();
        let group = groups
            .iter()
            .find(|g| text(g, "id") == chosen)
            .or(groups.first())
            .cloned()
            .unwrap_or(Value::Null);
        let group_id = text(&group, "id");
        let strategies = items(&data, "strategies");
        let campaigns = items(&data, "campaigns");
        view! {
            <section class="panel leaderboard">
                <div class="panel-heading"><div><h2>"Strategy leaderboard"</h2><p>"Highest campaign score first · returns after execution costs"</p></div></div>
                <div class="leaderboard-rules"><p>"Compare one campaign, period, and execution configuration at a time. Each strategy version uses its latest completed run. Exact ties share a rank; unscored runs are excluded. Archived versions remain visible. Eligibility is shown separately from rank."</p>
                <label>"Comparison"<select prop:value=group_id disabled=groups.is_empty() on:change=move|ev|selected.set(event_target_value(&ev))>
                {groups.iter().map(|g| {
                    let campaign = campaigns.iter().find(|c| c["id"] == g["campaign"]);
                    let dataset = campaign.map(|c| dataset_name(&data,&text(c,"dataset"))).unwrap_or_default();
                    view!{<option value=text(g,"id")>{format!("{} · {} · {} · {} · {} → {} · fee {} / slip {} bps",short(&text(g,"campaign")),dataset,text(g,"period"),score_label(g),date(&g["config"]["start"]),date(&g["config"]["end"]),g["config"]["fee_bps"],g["config"]["slippage_bps"])}</option>}
                }).collect_view()}
                </select></label>
                <p>"Validation comparisons show only the campaign score. No additional validation detail is revealed."</p></div>
                {if groups.is_empty() {view!{<div class="empty"><h3>"No ranked strategies yet"</h3><p>"Complete a backtest with a defined objective score to enter the leaderboard."</p><button class="secondary" on:click=move|_|tab.set("Backtests".into())>"Go to Backtests"</button></div>}.into_any()}
                else {view!{<div class="table-scroll"><table><thead><tr><th>"Rank"</th><th>"Strategy version"</th><th>{score_label(&group)}</th><th>"Eligibility"</th><th>"Run created"</th></tr></thead><tbody>
                {items(&group,"entries").into_iter().map(|entry| {
                    let strategy = strategies.iter().find(|s| s["id"] == entry["strategy"]);
                    let name = strategy.map(|s|text(&s["spec"],"name")).unwrap_or_else(||short(&text(&entry,"strategy")));
                    let archived = strategy.is_some_and(|s|s["archived"] == true);
                    let id=text(&entry,"job");
                    view!{<tr><td>{entry["rank"].to_string()}</td><td><button class="run-name" on:click=move|_|{tab.set("Backtests".into());run.set(id.clone());}>{name}</button><small>{short(&text(&entry,"strategy"))}{if archived {" · Archived"} else {""}}</small></td><td class="numeric" class:risk=entry["score"].as_f64().is_some_and(|n|n<0.0)>{score_value(&group,&entry["score"])}</td><td>{text(&entry,"eligibility")}</td><td>{date(&entry["created_at"])}</td></tr>}
                }).collect_view()}
                </tbody></table></div>}.into_any()}}
            </section>
        }
    }
}
