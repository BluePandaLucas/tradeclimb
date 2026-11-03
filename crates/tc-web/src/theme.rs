use super::*;
fn apply(mode: &str) {
    if let Some(window) = web_sys::window() {
        if let Some(root) = window
            .document()
            .and_then(|document| document.document_element())
        {
            let _ = root.set_attribute("data-theme", mode);
        }
        if let Ok(Some(storage)) = window.local_storage() {
            let _ = storage.set_item("tc-theme", mode);
        }
    }
}
#[component]
pub fn ThemeToggle() -> impl IntoView {
    let window = web_sys::window();
    let initial = window
        .as_ref()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item("tc-theme").ok().flatten())
        .filter(|mode| matches!(mode.as_str(), "light" | "dark" | "system"))
        .unwrap_or_else(|| "system".into());
    let mode = RwSignal::new(initial);
    apply(&mode.get_untracked());
    let media = window.and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten());
    let system_dark = RwSignal::new(media.as_ref().is_some_and(|m| m.matches()));
    if let Some(media) = media {
        let observed = media.clone();
        let listener =
            Closure::<dyn FnMut(web_sys::Event)>::new(move |_| system_dark.set(observed.matches()));
        let _ = media.add_event_listener_with_callback("change", listener.as_ref().unchecked_ref());
        let listener = StoredValue::new_local(listener);
        on_cleanup(move || {
            listener.with_value(|listener| {
                let _ = media.remove_event_listener_with_callback(
                    "change",
                    listener.as_ref().unchecked_ref(),
                );
            })
        });
    }
    let dark = move || mode.get() == "dark" || (mode.get() == "system" && system_dark.get());
    view! {<div class="theme-controls"><button class="theme-toggle" aria-label="Toggle dark mode" aria-pressed=move||dark().to_string() title="Toggle light and dark mode" on:click=move|_|{let next=if dark(){"light"}else{"dark"};mode.set(next.into());apply(next);}>
        <span aria-hidden="true">{move||if dark(){"☀"}else{"☾"}}</span>
    </button><button class="theme-system" aria-label="Use system theme" title="Follow your system theme" disabled=move||mode.get()=="system" on:click=move|_|{mode.set("system".into());apply("system");}>"System"</button></div>}
}
