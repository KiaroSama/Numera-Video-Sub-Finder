use crate::{
    extract::Extract,
    helpers, ipc,
    models::Models,
    review::Review,
    state::Ui,
    workspace::{History, Queue, Settings},
};
use leptos::prelude::*;
use serde_json::json;
use wasm_bindgen::{JsCast, closure::Closure};

pub fn mount() {
    let theme = web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item("nvsf-theme").ok().flatten())
        .unwrap_or_else(|| "system".into());
    let theme = helpers::theme_value(&theme).to_string();
    if let Some(root) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    {
        let _ = root.set_attribute("data-theme", &theme);
    }
    leptos::mount::mount_to_body(move || view! {<App initial_theme=theme.clone()/>});
}

#[component]
fn App(initial_theme: String) -> impl IntoView {
    let ui = Ui::new(initial_theme.clone());
    let main = NodeRef::<leptos::html::Main>::new();
    let routes = [
        ("Extract", "M4 5h16v14H4z M10 9l6 3-6 3z"),
        ("Queue", "M5 6h14 M5 12h14 M5 18h9"),
        ("Review", "M4 5h16v14H4z M8 9h8 M8 13h5"),
        ("Models", "M12 3l9 5-9 5-9-5z M3 12l9 5 9-5 M3 16l9 5 9-5"),
        ("History", "M3 12a9 9 0 1 0 3-7 M3 3v5h5 M12 7v5l3 2"),
        ("Settings", "M4 7h16 M4 17h16 M8 4v6 M16 14v6"),
    ];
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(e) = ipc::call::<()>("ui_ready", json!({"theme":initial_theme})).await {
            ui.error.set(Some(e));
        }
        match ipc::call::<Vec<ipc::QueueEntry>>("queue_list", json!({})).await {
            Ok(entries) => ui.queue.set(entries),
            Err(e) => ui.error.set(Some(e)),
        }
    });
    Effect::new(move |_| {
        let dirty = ui.dirty.get() || ui.draft_dirty.get();
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(e) = ipc::call::<()>("set_unsaved", json!({"dirty":dirty})).await {
                ui.error.set(Some(e));
            }
        });
    });
    Effect::new(move |_| {
        let route = ui.route.get();
        if let Some(window) = web_sys::window() {
            let _ = window.location().set_hash(&route.to_lowercase());
        }
        if let Some(main) = main.get() {
            let _ = main.focus();
        }
    });
    if let Some(window) = web_sys::window() {
        if let Ok(hash) = window.location().hash() {
            if let Some(route) = helpers::route_value(&hash) {
                ui.route.set(route.into());
            }
        }
        let route_listener = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
            if let Some(window) = web_sys::window() {
                if let Ok(hash) = window.location().hash() {
                    if let Some(route) = helpers::route_value(&hash) {
                        ui.route.set(route.into());
                    }
                }
            }
        });
        let _ = window.add_event_listener_with_callback(
            "hashchange",
            route_listener.as_ref().unchecked_ref(),
        );
        let route_listener = StoredValue::new_local(route_listener);
        on_cleanup(move || {
            if let Some(window) = web_sys::window() {
                route_listener.with_value(|listener| {
                    let _ = window.remove_event_listener_with_callback(
                        "hashchange",
                        listener.as_ref().unchecked_ref(),
                    );
                });
            }
        });
        let keyboard = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
            move |event: web_sys::KeyboardEvent| {
                if event.is_composing()
                    || !(event.ctrl_key() || event.meta_key())
                    || event.alt_key()
                {
                    return;
                }
                let editing = event
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                    .is_some_and(|e| {
                        matches!(e.tag_name().as_str(), "INPUT" | "TEXTAREA")
                            || e.get_attribute("contenteditable").as_deref() == Some("true")
                    });
                match event.key().to_lowercase().as_str() {
                    "s" if ui.project.with_untracked(Option::is_some) => {
                        event.prevent_default();
                        ui.persist(None, false, false);
                    }
                    "z" if !editing => {
                        event.prevent_default();
                        ui.travel(event.shift_key());
                    }
                    _ => {}
                }
            },
        );
        let _ =
            window.add_event_listener_with_callback("keydown", keyboard.as_ref().unchecked_ref());
        let keyboard = StoredValue::new_local(keyboard);
        on_cleanup(move || {
            if let Some(window) = web_sys::window() {
                keyboard.with_value(|listener| {
                    let _ = window.remove_event_listener_with_callback(
                        "keydown",
                        listener.as_ref().unchecked_ref(),
                    );
                });
            }
        });
    }
    on_cleanup(move || {
        let url = ui.preview_url.get_untracked();
        if !url.is_empty() {
            let _ = web_sys::Url::revoke_object_url(&url);
        }
    });
    #[cfg(feature = "e2e")]
    let fixture_controls = view! { <crate::e2e::FixtureControls ui/> }.into_any();
    #[cfg(not(feature = "e2e"))]
    let fixture_controls = ().into_any();
    view! {
        <a class="skip-link" href="#main-content">"Skip to workspace"</a>
        <div class="app-shell">
            <aside class="sidebar">
                <a class="brand" href="#extract" on:click=move |_|ui.route.set("Extract".into()) aria-label="Numera Video Sub Finder home">
                    <img src="logo.png" width="44" height="44" alt="Official Numera logo"/>
                    <span><strong>"NUMERA"</strong><small>"VIDEO SUB FINDER"</small></span>
                </a>
                <div class="sidebar-caption">"LOCAL SUBTITLE WORKSPACE"</div>
                <nav aria-label="Workspace navigation">{routes.into_iter().map(|(route,path)|view!{
                    <button class="nav-item" class:active=move ||ui.route.get()==route aria-current=move ||if ui.route.get()==route{Some("page")}else{None}
                        on:click=move |_|ui.route.set(route.into())>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><path d=path/></svg><span>{route}</span>
                    </button>
                }).collect_view()}</nav>
                <div class="sidebar-footer"><strong>"Built for local work."</strong><p>"Your video stays on this device. No cloud transcription. No telemetry."</p><span class="version mono">{concat!("v",env!("CARGO_PKG_VERSION"))}</span></div>
            </aside>
            <div class="workspace">
                <header class="topbar"><div><span class="eyebrow">"NUMERA / WORKSPACE"</span><h1>{move ||ui.route.get()}</h1></div>
                    <div class="topbar-actions"><Show when=move ||ui.busy.get()><span role="status" class="muted">"Native operation in progress…"</span></Show>
                        <button title="Open a saved review project" disabled=move ||ui.busy.get()||ui.active() on:click={move |_|ui.request::<Option<nvsf_core::Project>>("pick_project",json!({}),move |p|if let Some(p)=p{ui.load(p);})}>"Open project"</button>
                        <label class="sr-only" for="header-theme">"Workspace theme"</label><select id="header-theme" class="theme-select" prop:value=move ||ui.theme.get() on:change=move |e|ui.set_theme(event_target_value(&e))>
                            <option value="system">"System"</option><option value="dark">"Dark"</option><option value="light">"Light"</option>
                        </select>
                    </div>
                </header>
                <main id="main-content" tabindex="-1" node_ref=main>
                    {fixture_controls}
                    <Show when=move ||ui.error.with(Option::is_some)><div class="message error" role="alert"><strong>"Action could not complete"</strong><p>{move ||ui.error.get().unwrap_or_default()}</p><button on:click=move |_|ui.error.set(None)>"Dismiss error"</button></div></Show>
                    <Show when=move ||!ui.notice.get().is_empty()><div class="message notice" role="status"><span>{move ||ui.notice.get()}</span><button on:click=move |_|ui.notice.set(String::new())>"Dismiss"</button></div></Show>
                    <div hidden=move ||ui.route.get()!="Extract"><Extract ui/></div>
                    <div hidden=move ||ui.route.get()!="Queue"><Queue ui/></div>
                    <div hidden=move ||ui.route.get()!="Review"><Review ui/></div>
                    <div hidden=move ||ui.route.get()!="Models"><Models ui/></div>
                    <div hidden=move ||ui.route.get()!="History"><History ui/></div>
                    <div hidden=move ||ui.route.get()!="Settings"><Settings ui/></div>
                </main>
                <footer class="statusbar"><span>"Local processing · explicit model downloads only"</span><span>{move ||if ui.active(){"Job active — checkpoints preserved"}else{"No active extraction"}}</span></footer>
            </div>
        </div>
    }
}
