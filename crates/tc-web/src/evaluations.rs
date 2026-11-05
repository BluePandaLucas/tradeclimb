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
    let filters_open = RwSignal::new(false);
    let page = RwSignal::new(0_usize);
    let history = RwSignal::new(Vec::<Value>::new());
    let history_ready = RwSignal::new(false);
    spawn_local(async move {
        let loaded = async {
            let response = Request::get("/api/jobs/index").send().await.ok()?;
            if !response.ok() {
                return None;
            }
            response.json::<Vec<Value>>().await.ok()
        }
        .await;
        if let Some(mut rows) = loaded {
            let live = history.get_untracked();
            let ids = rows
                .iter()
                .map(|j| text(j, "id"))
                .collect::<std::collections::HashSet<_>>();
            rows.extend(live.into_iter().filter(|j| !ids.contains(&text(j, "id"))));
            history.set(rows);
        } else {
            status.set(
                "Run history could not load. Showing the live feed; reopen this page to retry."
                    .into(),
            );
        }
        history_ready.set(true);
    });
    Effect::new(move |_| {
        let recent = snapshot.with(|s| items(s, "jobs"));
        history.update(|all| {
            let positions = all
                .iter()
                .enumerate()
                .map(|(i, j)| (text(j, "id"), i))
                .collect::<std::collections::HashMap<_, _>>();
            for j in recent {
                if let Some(&i) = positions.get(&text(&j, "id")) {
                    all[i] = j;
                } else {
                    all.push(j);
                }
            }
        });
    });
    // Merge live summaries into the history fetched once. Charts stay on demand.
    let indexed_jobs = Memo::new(move |_| {
        let mut by_id = history.with(|all| {
            all.iter()
                .map(|j| (text(j, "id"), j.clone()))
                .collect::<std::collections::HashMap<_, _>>()
        });
        snapshot.with(|s| {
            for j in items(s, "jobs") {
                by_id.insert(text(&j, "id"), j);
            }
        });
        let mut all = by_id.into_values().collect::<Vec<_>>();
        all.sort_by(|a, b| {
            b["created_at"]
                .as_i64()
                .cmp(&a["created_at"].as_i64())
                .then_with(|| text(b, "id").cmp(&text(a, "id")))
        });
        all
    });
    let show_trials = RwSignal::new(false);
    let tuning_view = RwSignal::new(false);
    let tuning_page = RwSignal::new(0_usize);
    let strategy_filter = RwSignal::new(String::new());
    let search = RwSignal::new(String::new());
    let state = RwSignal::new(String::new());
    let strategy_names = Memo::new(move |_| {
        snapshot.with(|s| {
            items(s, "strategies")
                .into_iter()
                .map(|s| (text(&s, "id"), text(&s["spec"], "name")))
                .collect::<std::collections::HashMap<_, _>>()
        })
    });
    let known_jobs = RwSignal::new(std::collections::HashSet::<String>::new());
    Effect::new(move |_| {
        search.track();
        state.track();
        strategy_filter.track();
        show_trials.track();
        history_ready.track();
        page.set(0);
        tuning_page.set(0);
        known_jobs
            .set(indexed_jobs.with_untracked(|all| all.iter().map(|j| text(j, "id")).collect()));
    });
    let tuning = Memo::new(move |_| {
        snapshot.with(|s| {
            items(s, "tuning")
                .into_iter()
                .filter(|t| {
                    (strategy_filter.get().is_empty()
                        || text(t, "strategy") == strategy_filter.get())
                        && (state.get().is_empty()
                            || if state.get() == "active" {
                                running(t)
                            } else {
                                text(t, "state") == state.get()
                            })
                        && format!(
                            "{} {}",
                            text(t, "id"),
                            strategy_names.with(|names| names
                                .get(&text(t, "strategy"))
                                .cloned()
                                .unwrap_or_default())
                        )
                        .to_lowercase()
                        .contains(&search.get().to_lowercase())
                })
                .collect::<Vec<_>>()
        })
    });
    Effect::new(move |_| {
        let maximum = tuning.with(|all| all.len().div_ceil(5).max(1)) - 1;
        if tuning_page.get_untracked() > maximum {
            tuning_page.set(maximum);
        }
    });
    let matching_jobs = Memo::new(move |_| {
        let data = snapshot.get();
        let trials = items(&data, "tuning")
            .into_iter()
            .flat_map(|t| items(&t, "jobs"))
            .filter_map(|id| id.as_str().map(str::to_owned))
            .collect::<std::collections::HashSet<_>>();
        indexed_jobs
            .get()
            .into_iter()
            .filter(|j| show_trials.get() || !trials.contains(&text(j, "id")))
            .filter(|j| {
                strategy_filter.get().is_empty() || text(j, "strategy") == strategy_filter.get()
            })
            .filter(|j| {
                state.get().is_empty()
                    || if state.get() == "active" {
                        running(j)
                    } else {
                        text(j, "state") == state.get()
                    }
            })
            .filter(|j| {
                format!(
                    "{} {}",
                    text(j, "id"),
                    strategy_names
                        .with(|names| names.get(&text(j, "strategy")).cloned().unwrap_or_default())
                )
                .to_lowercase()
                .contains(&search.get().to_lowercase())
            })
            .collect::<Vec<_>>()
    });
    Effect::new(move |_| {
        if known_jobs.with_untracked(|ids| ids.is_empty()) && !matching_jobs.with(Vec::is_empty) {
            known_jobs.set(matching_jobs.with(|jobs| jobs.iter().map(|j| text(j, "id")).collect()));
        }
    });
    let jobs = Memo::new(move |_| {
        matching_jobs.with(|all| {
            all.iter()
                .filter(|j| known_jobs.with(|ids| ids.contains(&text(j, "id"))))
                .cloned()
                .collect::<Vec<_>>()
        })
    });
    let arriving = move || {
        matching_jobs.with(|all| {
            all.iter()
                .filter(|j| !known_jobs.with(|ids| ids.contains(&text(j, "id"))))
                .count()
        })
    };
    let pages = move || jobs.with(|j| j.len().div_ceil(PAGE_SIZE).max(1));
    let current = move || page.get().min(pages() - 1);
    let common_objective = Memo::new(move |_| {
        jobs.with(|all| {
            let shown = all
                .iter()
                .skip(current() * PAGE_SIZE)
                .take(PAGE_SIZE)
                .collect::<Vec<_>>();
            shown
                .first()
                .map(|j| score_label(j))
                .filter(|label| shown.iter().all(|j| score_label(j) == *label))
        })
    });
    let common_window = Memo::new(move |_| {
        let data = snapshot.get();
        let windows = jobs.with(|all| {
            all.iter()
                .skip(current() * PAGE_SIZE)
                .take(PAGE_SIZE)
                .map(|j| job_window(&data, j))
                .collect::<Vec<_>>()
        });
        windows
            .first()
            .cloned()
            .flatten()
            .filter(|first| windows.iter().all(|w| w.as_ref() == Some(first)))
    });
    view! {
        <section class="panel evaluations">
        <div class="panel-heading"><div><h2>"Strategy evaluations"</h2><p>"Net results, with a separate validation window"</p></div><span class="run-order" title="Ordered by submission time, newest first">"↓ Newest first"</span></div>
        <div class="research-toolbar" class:filters-open=move||filters_open.get()><div class="research-tabs"><button class:active=move||!tuning_view.get() on:click=move|_|tuning_view.set(false)>"Backtests"<span>{move||jobs.with(Vec::len)}</span></button><button class:active=move||tuning_view.get() on:click=move|_|tuning_view.set(true)>"Tuning runs"<span>{move||tuning.with(Vec::len)}</span></button><button class="mobile-filters" aria-expanded=move||filters_open.get().to_string() on:click=move|_|filters_open.update(|v| *v = !*v)>{move||if filters_open.get(){"Hide filters"}else{"Filters ⌄"}}</button></div>
        <div class="research-search"><input type="search" aria-label="Search runs" placeholder="Search strategy or run ID…" prop:value=move||search.get() on:input=move|ev|search.set(event_target_value(&ev))/><select aria-label="Run status" prop:value=move||state.get() on:change=move|ev|state.set(event_target_value(&ev))><option value="">"All statuses"</option><option value="active">"Active"</option><option value="completed">"Completed"</option><option value="failed">"Failed"</option><option value="cancelled">"Cancelled"</option></select></div>
        <div class="research-scope"><StrategyPicker snapshot=snapshot selected=strategy_filter archived=true/><Show when=move||!strategy_filter.get().is_empty()><button class="secondary" on:click=move|_|strategy_filter.set(String::new())>"All strategies"</button></Show><div class="arrival-status"><Show when={move||arriving()>0} fallback=||view!{<span class="subtle">"Live updates · selection preserved"</span>}><button class="secondary" on:click=move|_|{known_jobs.set(indexed_jobs.with_untracked(|all|all.iter().map(|j|text(j,"id")).collect()));page.set(0);}>{move||format!("Show {} new runs",arriving())}</button></Show></div></div></div>
        <Show when=move||tuning_view.get()><div class="tuning-sessions">
        <For each={move||tuning.get().into_iter().skip(tuning_page.get()*5).take(5).map(|t|text(&t,"id")).collect::<Vec<_>>()} key={|id:&String|id.clone()} children={move|id:String|{
            let tuning = Memo::new(move |_| snapshot.with(|s|items(s,"tuning").into_iter().find(|t|text(t,"id")==id).unwrap_or(Value::Null)));
            view!{<div class="tuning-session"><div class="tuning-summary"><strong>{move||strategy_names.with(|names|names.get(&text(&tuning.get(),"strategy")).cloned().unwrap_or_else(||short(&text(&tuning.get(),"strategy"))))}</strong><span>{move||{let t=tuning.get();format!("Tuning {} · {} · {} / {} trials completed · best {} {}{}",short(&text(&t,"id")),text(&t,"state"),t["completed"].as_u64().unwrap_or_else(||if t["state"]=="completed" {items(&t,"jobs").len() as u64}else{0}),t["trials"].as_u64().unwrap_or(20),score_label(&t),score_value(&t,&t["best"]),t["error"].as_str().map(|e|format!(" · {e}")).unwrap_or_default())}}</span><progress aria-label="Tuning progress" max=move||tuning.get()["trials"].as_u64().unwrap_or(20) value=move||tuning.with(|t|t["completed"].as_u64().unwrap_or_else(||if t["state"]=="completed" {items(t,"jobs").len() as u64}else{0}))></progress></div><Show when=move||running(&tuning.get())><button class="secondary" on:click=move|_|action(format!("/api/tuning/{}/cancel",text(&tuning.get_untracked(),"id")),json!({}),status)>"Stop tuning"</button></Show></div>}
        }}/>
        </div><Show when=move||tuning.with(Vec::is_empty)><div class="empty">"No tuning runs match. Try another strategy or status."</div></Show><div class="evaluation-pagination"><span>{move||format!("{} tuning runs · Page {} of {}",tuning.with(Vec::len),tuning_page.get()+1,tuning.with(|t|t.len().div_ceil(5).max(1)))}</span><button class="secondary" disabled=move||tuning_page.get()==0 on:click=move|_|tuning_page.update(|p|*p=p.saturating_sub(1))>"Previous tuning"</button><button class="secondary" disabled={move||(tuning_page.get()+1)*5>=tuning.with(Vec::len)} on:click=move|_|tuning_page.update(|p|*p+=1)>"Next tuning"</button></div></Show>
        <Show when=move||!tuning_view.get()>
        <Show when=move||snapshot.with(|s|!items(s,"tuning").is_empty())><div class="evaluation-filters"><label class="check-label"><input type="checkbox" prop:checked=move||show_trials.get() on:change=move|ev|{show_trials.set(event_target_checked(&ev));page.set(0);}/>"Show individual tuning trials"</label><details class="trial-help"><summary>"About tuning trials"</summary><p>"Trials stay in Tuning runs unless you include them here. Overview totals include all trials."</p></details></div></Show>
        <Show when=move||!jobs.with(Vec::is_empty) fallback=||view!{<div class="empty"><h3>"No matching backtests"</h3><p>"Try another strategy, search, or status. New evaluations appear here as they arrive."</p></div>}>
        <details class="run-window-context"><summary>{move||common_window.get().map(|w|format!("Evaluated · {} · UTC",compact_window(&w))).unwrap_or("Multiple evaluation windows · dates shown per run".into())}</summary><p>{move||common_window.get().map(|w|format!("Exact window: {w} UTC")).unwrap_or("Exact UTC dates appear below each run name.".into())}</p></details><div class="table-scroll"><table class:single-objective=move||common_objective.get().is_some()><thead><tr><th>"Strategy / run"</th><th>"Net return"</th><th title="Objective score">{move||common_objective.get().unwrap_or("Score")}</th><th title="Maximum drawdown">"Max DD"</th><th class="evaluation-heading"><div class="meta-column-headings"><span>"Period"</span><span>"Status"</span><span>"Eligibility"</span></div></th><th>"Details"</th></tr></thead><tbody>
        <For each={move||jobs.with(|j|j.iter().skip(current()*PAGE_SIZE).take(PAGE_SIZE).map(|j|text(j,"id")).collect::<Vec<_>>())} key={|id:&String|id.clone()} children={move|id:String|{
            let select_id=StoredValue::new(id.clone());
            let job=Memo::new(move |_|jobs.with(|jobs|jobs.iter().find(|j|text(j,"id")==id).cloned().unwrap_or(Value::Null)));
            let name=move||strategy_names.with(|names|names.get(&text(&job.get(),"strategy")).cloned().unwrap_or_else(||short(&text(&job.get(),"strategy"))));
            view!{<tr class="evaluation-row" data-run-id=select_id.get_value() class:selected=move||selected.get()==select_id.get_value() on:click=move|_|selected.set(select_id.get_value())>
            <td data-label="Strategy / run"><button class="run-name" on:click=move|ev|{ev.stop_propagation();selected.set(select_id.get_value());}>{name}<span class="mobile-detail-link">"View details →"</span></button><Show when=move||common_window.get().is_none()><span class="run-window">{move||job_window(&snapshot.get(),&job.get()).unwrap_or("Window unavailable".into())}</span></Show><small>{short(&select_id.get_value())}</small><Show when=move||synthetic_job(&snapshot.get(),&job.get())><span class="badge fixture-badge">"Synthetic fixture"</span></Show></td>
            <td data-label="Net return" class="numeric metric-return" class:neutral=move||job.with(|j|j["result"]["metrics"]["net_return"].as_f64().is_none_or(|n|n==0.0)) class:risk=move||job.with(|j|j["result"]["metrics"]["net_return"].as_f64().is_some_and(|n|n<0.0))>{move||job.with(|j|if j["period"]=="validation"{"Restricted".into()}else{percent(&j["result"]["metrics"]["net_return"])})}</td>
            <td data-label=move||score_label(&job.get()) class="numeric metric-score" class:missing-metric=move||job.with(undefined_sharpe) class:neutral=move||job.with(|j|j["result"]["metrics"][if is_sharpe(j){"sharpe"}else{"net_cagr"}].as_f64().is_none()) class:risk=move||job.with(|j|j["result"]["metrics"][if is_sharpe(j){"sharpe"}else{"net_cagr"}].as_f64().is_some_and(|n|n<0.0))><span class="score-kind">{move||score_label(&job.get())}</span>" "<span class="score-value">{move||job.with(job_score_value)}</span><Show when=move||job.with(undefined_sharpe)><span class="missing-metric-label">{move||job.with(|j|if j["result"]["metrics"]["trades"]==0{"No variation"}else{"Insufficient data"})}</span><small class="metric-explanation">{move||job.with(|j|if j["result"]["metrics"]["trades"]==0{"No variation"}else{"Insufficient data"})}</small></Show></td>
            <td data-label="Max drawdown" class="numeric metric-drawdown" class:risk=move||job.with(|j|j["result"]["metrics"]["max_drawdown"].as_f64().is_some_and(|n|n>0.0)) class:neutral=move||job.with(|j|!j["result"]["metrics"]["max_drawdown"].as_f64().is_some_and(|n|n>0.0))>{move||job.with(|j|if j["period"]=="validation"{"Restricted".into()}else{drawdown(&j["result"]["metrics"]["max_drawdown"])})}</td>
            <td class="evaluation-meta"><div class="meta-values"><div class="eval-state-line"><span class="meta-label">"Period"</span><span class="badge">{move||text(&job.get(),"period")}</span></div><div class="eval-state-line"><span class="meta-label">"Status"</span><span class="state" data-state=move||text(&job.get(),"state")>{move||job.with(|j|if j["period"]!="validation" && running(j){format!("{} · {:.0}%",text(j,"state"),j["progress"].as_f64().unwrap_or(0.0)*100.0)}else{text(j,"state")})}</span></div><div class="eval-state-line"><span class="meta-label">"Eligibility"</span><span class="eligibility-text">{move||job.with(|j|if j["period"]=="validation"{if is_sharpe(j){"Sharpe only"}else{"CAGR only"}}else if j["result"]["metrics"]["eligible"]==true{"Eligible"}else if j["state"]=="completed"{"Review"}else{"—"})}<Show when=move||job.with(|j|j["state"]=="completed" && j["result"]["metrics"]["eligible"]==false)><small class="metric-explanation">{move||job.with(|j|if items(&j["result"]["metrics"],"reasons").iter().any(|r|r.as_str().is_some_and(|s|s.contains("positive mean"))){"No monthly gain"}else{"Inspect eligibility reasons"})}</small></Show></span></div></div></td>
            <td class="row-actions"><button class="secondary row-details" on:click=move|ev|{ev.stop_propagation();selected.set(select_id.get_value());}>"Details →"</button><Show when=move||running(&job.get())><button class="text-button" on:click=move|ev|{ev.stop_propagation();action(format!("/api/jobs/{}/cancel",select_id.get_value()),json!({}),status);}>"Cancel"</button></Show></td></tr>}
        }}/>
        </tbody></table></div>
        <div class="evaluation-pagination"><span>{move||if pages()==1{format!("{} runs",jobs.with(Vec::len))}else{format!("Page {} of {} · {} runs",current()+1,pages(),jobs.with(Vec::len))}}</span><Show when={move||pages()>1}><button class="secondary" disabled=move||current()==0 on:click=move|_|page.set(current().saturating_sub(1))>"Previous runs"</button><button class="secondary" disabled={move||current()+1>=pages()} on:click=move|_|page.set(current()+1)>"Next runs"</button></Show></div>
        </Show>
        </Show>
        </section>
    }
}
fn reveal_validation(
    id: String,
    detail: RwSignal<Value>,
    status: RwSignal<String>,
    pending: RwSignal<bool>,
    chart_days: RwSignal<i64>,
) {
    if pending.get_untracked() {
        return;
    }
    pending.set(true);
    status.set("Opening owner validation details…".into());
    spawn_local(async move {
        let result = async {
            let response = Request::post(&format!("/api/jobs/{id}/reveal"))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if response.status() == 401 {
                sign_in();
                return Err("Sign in again".into());
            }
            let value: Value = response.json().await.map_err(|e| e.to_string())?;
            if !response.ok() {
                return Err(text(&value, "error"));
            }
            detail.set(value);
            chart_days.set(0);
            Ok(())
        }
        .await;
        status.set(match result {
            Ok(()) => "Validation revealed to owner; agents remain restricted.".into(),
            Err(e) => e,
        });
        pending.set(false);
    });
}
#[component]
fn BenchmarkTable(benchmarks: Vec<Value>) -> impl IntoView {
    view! {<section class="benchmark-comparison"><h3>"Against buy-and-hold · full run"</h3><p class="muted">"Five-asset basket is the fixed reference; BTC is a secondary comparison. Information ratio (IR) compares average daily outperformance with its variability. References remain invested through drawdowns."</p>
    <div class="table-scroll"><table><thead><tr><th>"Reference"</th><th>"Sharpe"</th><th>"Sharpe difference"</th><th>"Information ratio"</th><th>"Return difference"</th></tr></thead><tbody>{benchmarks.into_iter().map(|b|{let c=&b["comparison"];view!{<tr><td>{text(&b,"name")}</td><td>{score_value(&json!({"objective":"sharpe_daily_v1"}),&c["benchmark_sharpe"])}</td><td>{score_value(&json!({"objective":"sharpe_daily_v1"}),&c["sharpe_difference"])}</td><td>{score_value(&json!({"objective":"sharpe_daily_v1"}),&c["information_ratio"])}</td><td>{format!("{} pp",percent(&c["net_return_difference"]).trim_end_matches('%'))}</td></tr>}}).collect_view()}</tbody></table></div>
    <p class="chart-note">"Daily UTC returns, annualized; undefined ratios show —. Return differences are percentage points. Comparisons do not change the campaign ranking or risk eligibility."</p></section>}
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
    let reveal_pending = RwSignal::new(false);
    view! {
    {move||{let job=detail.get(); if job.is_null() {return ().into_any();} let validation=job["period"]=="validation";let result=&job["result"];let metric=&result["metrics"];view!{<section class="panel run-detail"><div class="panel-heading"><div><h2>{items(&snapshot.get(),"strategies").iter().find(|s|s["id"]==job["strategy"]).map(|s|text(&s["spec"],"name")).unwrap_or("Run details".into())}<span class="detail-caption">{format!(" / {} · {}",text(&job,"period"),text(&job,"state"))}</span></h2><p>{format!("{} → {} · EUR · {} bps fee + {} bps adverse allowance per fill",date(&job["config"]["start"]),date(&job["config"]["end"]),job["config"]["fee_bps"],job["config"]["slippage_bps"])}</p></div><div class="run-provenance"><span class="badge">{if synthetic_job(&snapshot.get(),&job){"Synthetic fixture"}else{"Evaluation"}}</span><code>{short(&text(&job,"id"))}</code></div></div>{if validation && job["owner_detail"]!=true {let id=text(&job,"id");view!{<div class="empty"><h3>{job_score(&job)}</h3><p>"Owner access includes metrics, equity and trades. Opening details records validation as revealed; agents remain restricted."</p><button class="secondary" disabled=move||reveal_pending.get() on:click=move|_|reveal_validation(id.clone(),detail,status,reveal_pending,chart_days)>{move||if reveal_pending.get(){"Opening details…"}else{"View validation details"}}</button></div>}.into_any()}else{let benches=items(result,"benchmarks").into_iter().filter(|b|if text(b,"name").starts_with("BTC"){show_btc.get()}else{show_basket.get()}).collect::<Vec<_>>();let equity=items(result,"equity");let fills=items(result,"fills");let marker_total=result["marker_total"].as_u64().unwrap_or(fills.len() as u64) as usize;let comparison=items(result,"benchmarks");let id=text(&job,"id");view!{<div class="run-scope">{format!("{}-day {} evaluation · costs included",(job["config"]["end"].as_i64().unwrap_or(0)-job["config"]["start"].as_i64().unwrap_or(0)).max(0)/86_400_000,if synthetic_job(&snapshot.get(),&job){"synthetic"}else{job["period"].as_str().unwrap_or("research")})}</div><div class="detail-metrics"><div>"SHARPE"<strong class:neutral={metric["sharpe"].is_null()}>{score_value(&json!({"objective":"sharpe_daily_v1"}),&metric["sharpe"])}</strong></div><div>"NET CAGR"<strong class:risk={metric["net_cagr"].as_f64().is_some_and(|n|n<0.0)}>{percent(&metric["net_cagr"])}</strong><small class="annualization-note">{format!("Annualized · {} days",(job["config"]["end"].as_i64().unwrap_or(0)-job["config"]["start"].as_i64().unwrap_or(0)).max(0)/86_400_000)}</small></div><div>"MAX DRAWDOWN"<strong class:risk={metric["max_drawdown"].as_f64().is_some_and(|n|n>0.0)} class:neutral={!metric["max_drawdown"].as_f64().is_some_and(|n|n>0.0)}>{drawdown(&metric["max_drawdown"])}</strong></div><div>"ELIGIBILITY"<strong class:risk={metric["eligible"]!=true}>{if metric["eligible"]==true{"Eligible"}else{"Review"}}</strong></div><div>"NET RETURN"<strong class:risk={metric["net_return"].as_f64().is_some_and(|n|n<0.0)}>{percent(&metric["net_return"])}</strong></div><div>"FEES"<strong>{metric["total_fees"].as_str().and_then(|s|s.parse::<f64>().ok()).map(money).unwrap_or("—".into())}</strong></div><div>"FILLS"<strong>{metric["trades"].to_string()}</strong></div><div>"MEAN MONTHLY RETURN"<strong>{percent(&metric["mean_monthly_return"])}</strong></div></div><div class="chart-controls"><label>"Chart window"<select prop:value=move||chart_days.get().to_string() on:change=move|ev|chart_days.set(event_target_value(&ev).parse().unwrap_or(0))><option value="0">"Full period"</option><option value="90">"Last 90 days"</option><option value="30">"Last 30 days"</option><option value="7">"Last 7 days"</option></select></label><label class="check-label"><input type="checkbox" prop:checked=move||show_btc.get() on:change=move|ev|show_btc.set(event_target_checked(&ev))/>"BTC buy & hold"</label><label class="check-label"><input type="checkbox" prop:checked=move||show_basket.get() on:change=move|ev|show_basket.set(event_target_checked(&ev))/>"Five-asset buy & hold"</label></div><Chart points=equity fills=fills label="Net portfolio equity" benchmarks=benches marker_total=marker_total/><BenchmarkTable benchmarks=comparison/><div class="warnings">{items(metric,"warnings").into_iter().chain(items(metric,"reasons")).map(|w|view!{<p>{w.as_str().unwrap_or("").to_string()}</p>}).collect_view()}</div><div class="detail-actions"><a class="secondary" href=format!("/api/jobs/{id}/trades?limit=100") target="_blank">"Exact trades (JSON) ↗"</a><button class="secondary" disabled=move||tuning_pending.get()||snapshot.with(|s|items(s,"tuning").iter().any(running)) on:click={let sid=text(&job,"strategy");let cid=text(&job,"campaign");move|_|start_tuning(sid.clone(),cid.clone(),snapshot,tuning_pending,status)}>{move||if tuning_pending.get(){"Starting tuning…"}else if snapshot.with(|s|items(s,"tuning").iter().any(running)){"Tuning in progress…"}else{"Tune parameters · 20 trials"}}</button></div>}.into_any()}}</section>}.into_any()}}
    }
}
