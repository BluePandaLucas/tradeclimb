//! A bounded, searchable library shared by research, history and paper trading.
use super::*;
use std::collections::{BTreeMap, HashMap, HashSet};
const PAGE: usize = 30;
#[derive(Clone, PartialEq)]
struct Family {
    id: String,
    name: String,
    versions: Vec<Value>,
}
fn families(strategies: &[Value], archived: bool) -> Vec<Family> {
    let by_id: HashMap<_, _> = strategies.iter().map(|s| (text(s, "id"), s)).collect();
    let mut roots = HashMap::<String, String>::new();
    let mut grouped = BTreeMap::<String, Family>::new();
    for s in strategies {
        let id = text(s, "id");
        let mut path = Vec::new();
        let mut seen = HashSet::new();
        let mut root = id.clone();
        loop {
            if let Some(cached) = roots.get(&root) {
                root = cached.clone();
                break;
            }
            if !seen.insert(root.clone()) {
                root = seen.into_iter().min().unwrap_or(root);
                break;
            }
            path.push(root.clone());
            match by_id.get(&root).and_then(|s| s["parent"].as_str()) {
                Some(parent) if by_id.contains_key(parent) => root = parent.into(),
                _ => break,
            }
        }
        for id in path {
            roots.insert(id, root.clone());
        }
        if !archived && s["archived"] == true {
            continue;
        }
        let family = grouped.entry(root.clone()).or_insert_with(|| Family {
            name: by_id
                .get(&root)
                .map(|s| text(&s["spec"], "name"))
                .unwrap_or_else(|| short(&root)),
            id: root,
            versions: Vec::new(),
        });
        family.versions.push(s.clone());
    }
    let mut result: Vec<_> = grouped.into_values().collect();
    for f in &mut result {
        f.versions.sort_by(|a, b| {
            b["created_at"]
                .as_i64()
                .cmp(&a["created_at"].as_i64())
                .then_with(|| text(a, "id").cmp(&text(b, "id")))
        });
    }
    result.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.id.cmp(&b.id))
    });
    result
}
fn matches(s: &Value, query: &str) -> bool {
    format!(
        "{} {} {}",
        text(&s["spec"], "name"),
        text(&s["spec"], "description"),
        text(s, "id")
    )
    .to_lowercase()
    .contains(query)
}
#[component]
pub fn StrategyPicker(
    snapshot: RwSignal<Value>,
    selected: RwSignal<String>,
    #[prop(default = false)] archived: bool,
) -> impl IntoView {
    let dialog = NodeRef::<leptos::html::Dialog>::new();
    let trigger = NodeRef::<leptos::html::Button>::new();
    let query = RwSignal::new(String::new());
    let family = RwSignal::new(String::new());
    let family_page = RwSignal::new(0_usize);
    let version_page = RwSignal::new(0_usize);
    // Snapshot progress changes do not rebuild the library unless strategies change.
    let strategies = Memo::new(move |_| snapshot.with(|s| items(s, "strategies")));
    let library = Memo::new(move |_| strategies.with(|s| families(s, archived)));
    let filtered = Memo::new(move |_| {
        let q = query.get().trim().to_lowercase();
        library.with(|all| {
            all.iter()
                .filter(|f| {
                    f.name.to_lowercase().contains(&q) || f.versions.iter().any(|s| matches(s, &q))
                })
                .cloned()
                .collect::<Vec<_>>()
        })
    });
    let chosen = Memo::new(move |_| {
        filtered.with(|all| {
            all.iter()
                .find(|f| f.id == family.get())
                .or(all.first())
                .cloned()
        })
    });
    let versions = Memo::new(move |_| {
        let q = query.get().trim().to_lowercase();
        chosen
            .get()
            .map(|f| {
                let family_match = f.name.to_lowercase().contains(&q);
                f.versions
                    .into_iter()
                    .filter(|s| family_match || matches(s, &q))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });
    Effect::new(move |_| {
        let maximum = filtered.with(|all| all.len().div_ceil(PAGE).max(1)) - 1;
        if family_page.get_untracked() > maximum {
            family_page.set(maximum);
        }
    });
    Effect::new(move |_| {
        let maximum = versions.with(|all| all.len().div_ceil(PAGE).max(1)) - 1;
        if version_page.get_untracked() > maximum {
            version_page.set(maximum);
        }
    });
    let label = move || {
        strategies.with(|all| {
            all.iter()
                .find(|s| text(s, "id") == selected.get())
                .map(|s| text(&s["spec"], "name"))
                .unwrap_or("Choose strategy".into())
        })
    };
    view! {
    <div class="strategy-picker">
    <button node_ref=trigger class="picker-trigger" aria-haspopup="dialog" on:click=move|_|{
        query.set(String::new());version_page.set(0);
        let id=selected.get_untracked();
        if let Some((index,f))=library.with_untracked(|all|all.iter().enumerate().find(|(_,f)|f.versions.iter().any(|s|text(s,"id")==id)).map(|(i,f)|(i,f.clone()))) {
            family.set(f.id);family_page.set(index/PAGE);
            version_page.set(f.versions.iter().position(|s|text(s,"id")==id).unwrap_or(0)/PAGE);
        } else {family_page.set(0);}
        if let Some(d)=dialog.get(){let _=d.show_modal();}
    }><span class="picker-symbol" aria-hidden="true">"◈"</span><span class="picker-label">{label}<small>{move||if selected.get().is_empty(){"Search strategies and versions".into()}else{format!("Version {}",short(&selected.get()))}}</small></span><span aria-hidden="true">"⌄"</span></button>
    <dialog node_ref=dialog on:close=move|_|{if let Some(button)=trigger.get(){let _=button.focus();}} on:keydown=move|ev|{if ev.key()=="Escape"{ev.prevent_default();ev.stop_propagation();if let Some(d)=dialog.get(){d.close();}}} class="strategy-dialog" aria-label="Strategy library" on:click=move|ev|{if ev.target()==ev.current_target() && let Some(d)=dialog.get(){d.close();}}>
      <div class="library-header"><div><h2>"Strategy library"</h2><p>"Find an idea. Choose an exact version."</p></div><button class="icon-button" aria-label="Close strategy library" on:click=move|_|{if let Some(d)=dialog.get(){d.close();}}>"×"</button></div>
      <div class="library-search"><input autofocus type="search" aria-label="Search strategies and versions" placeholder="Search names, descriptions, or version IDs…" prop:value=move||query.get() on:input=move|ev|{query.set(event_target_value(&ev));family_page.set(0);version_page.set(0);}/></div>
      <div class="library-columns">
      <section aria-label="Strategies"><div class="library-column-heading">"STRATEGIES"<span>{move||filtered.with(Vec::len)}</span></div>
      <div class="library-list">
      <For each=move||filtered.with(|all|all.iter().skip(family_page.get()*PAGE).take(PAGE).cloned().collect::<Vec<_>>()) key=|f|f.id.clone() children=move|f|{
          let id=f.id.clone();let active_id=f.id.clone();
          view!{<button class="family-option" class:active=move||chosen.get().is_some_and(|f|f.id==active_id) on:click=move|_|{family.set(id.clone());version_page.set(0);}>
          <span>{f.name}</span><small>{format!("{} versions",f.versions.len())}</small></button>}
      }/>
      <Show when=move||filtered.with(Vec::is_empty)><div class="library-empty">"No matching strategies."<p>"Try a name or part of a version ID."</p></div></Show>
      </div><div class="library-pager"><button aria-label="Previous strategies" disabled=move||family_page.get()==0 on:click=move|_|family_page.update(|p|*p=p.saturating_sub(1))>"←"</button><span>{move||format!("{} / {}",family_page.get()+1,filtered.with(|f|f.len().div_ceil(PAGE).max(1)))}</span><button aria-label="Next strategies" disabled={move||(family_page.get()+1)*PAGE>=filtered.with(Vec::len)} on:click=move|_|family_page.update(|p|*p+=1)>"→"</button></div></section>
      <section aria-label="Versions"><div class="library-column-heading"><span class="truncate">{move||chosen.get().map(|f|f.name).unwrap_or("VERSIONS".into())}</span><span>{move||versions.with(Vec::len)}</span></div><div class="library-list">
      {move||versions.with(|all|all.iter().skip(version_page.get()*PAGE).take(PAGE).cloned().collect::<Vec<_>>()).into_iter().map(|s|{
          let id=text(&s,"id");let active_id=id.clone();let label=format!("Select {} version {}",text(&s["spec"],"name"),short(&id));
          view!{<button class="version-option" class:active=move||selected.get()==active_id aria-label=label on:click=move|_|{selected.set(id.clone());if let Some(d)=dialog.get(){d.close();}}><span class="version-title">{text(&s["spec"],"name")}</span><small>{format!("{} · {}{}",short(&text(&s,"id")),date(&s["created_at"]),if s["archived"]==true{" · archived"}else{""})}</small><p>{text(&s["spec"],"description")}</p></button>}
      }).collect_view()}
      <Show when=move||versions.with(Vec::is_empty)><div class="library-empty">"No versions to show."</div></Show>
      </div><div class="library-pager"><button aria-label="Previous versions" disabled=move||version_page.get()==0 on:click=move|_|version_page.update(|p|*p=p.saturating_sub(1))>"←"</button><span>{move||format!("{} / {}",version_page.get()+1,versions.with(|f|f.len().div_ceil(PAGE).max(1)))}</span><button aria-label="Next versions" disabled={move||(version_page.get()+1)*PAGE>=versions.with(Vec::len)} on:click=move|_|version_page.update(|p|*p+=1)>"→"</button></div></section>
      </div><div class="library-footer"><span>"Versions grouped by parent lineage"</span><span>"Esc to close"</span></div>
    </dialog></div>
    }
}
#[component]
pub fn History(
    snapshot: RwSignal<Value>,
    status: RwSignal<String>,
    selected: RwSignal<String>,
) -> impl IntoView {
    Effect::new(move |_| {
        if selected.get().is_empty()
            && let Some(first) = snapshot.with(|s| items(s, "strategies").into_iter().next())
        {
            selected.set(text(&first, "id"));
        }
    });
    view! {<section class="panel"><div class="panel-heading"><div><h2>"Strategy history book"</h2><p>"Browse immutable versions, descriptions, and lineage"</p></div><span class="badge">{move||snapshot.with(|s|format!("{} versions",items(s,"strategies").len()))}</span></div>
    <div class="history-browser"><StrategyPicker snapshot=snapshot selected=selected archived=true/>
    {move||snapshot.with(|s|items(s,"strategies").into_iter().find(|s|text(s,"id")==selected.get())).map(|s|{
        let id=text(&s,"id");view!{<article class="history-entry"><div class="section-row"><h3>{text(&s["spec"],"name")}</h3><button class="secondary" on:click=move|_|action(format!("/api/strategies/{id}/archive"),json!({}),status)>{if s["archived"]==true{"Restore"}else{"Archive"}}</button></div><code>{text(&s,"id")}</code><p>{text(&s["spec"],"description")}</p><p>{format!("Created {} · Parent {}",date(&s["created_at"]),s["parent"].as_str().unwrap_or("Original strategy"))}</p><pre>{serde_json::to_string_pretty(&s["spec"]["parameters"]).unwrap_or_default()}</pre></article>}
    })}</div></section>}
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lineage_survives_archived_roots_and_distinct_names() {
        let data = vec![
            json!({"id":"v2","parent":"v1","spec":{"name":"Renamed"}}),
            json!({"id":"v1","archived":true,"spec":{"name":"Original"}}),
            json!({"id":"other","spec":{"name":"Original"}}),
        ];
        let grouped = families(&data, false);
        assert_eq!(grouped.len(), 2);
        assert_eq!(
            grouped
                .iter()
                .find(|f| f.id == "v1")
                .map(|f| f.versions.len()),
            Some(1)
        );
        assert_eq!(
            families(&data, true)
                .iter()
                .find(|f| f.id == "v1")
                .map(|f| f.versions.len()),
            Some(2)
        );
    }
    #[test]
    fn broken_and_cyclic_lineage_terminate() {
        let data = vec![
            json!({"id":"a","parent":"b"}),
            json!({"id":"b","parent":"a"}),
            json!({"id":"c","parent":"missing"}),
        ];
        assert_eq!(families(&data, true).len(), 2);
    }
}
