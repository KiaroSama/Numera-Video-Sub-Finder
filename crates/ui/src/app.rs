use crate::{
    close_modal::CloseModal,
    extract::Extract,
    helpers,
    hotkeys::{self, ShortcutsModal},
    ipc,
    models::Models,
    review::Review,
    state::Ui,
    toolbar::Topbar,
    update_banner::UpdateBanner,
    workspace::{History, Queue, Settings},
};
use leptos::{ev, prelude::*};
use serde_json::json;
use wasm_bindgen::{JsCast, closure::Closure};
use wasm_bindgen_futures::spawn_local;

pub fn mount() {
    #[cfg(feature = "e2e")]
    if let Some(window) = web_sys::window() {
        let _ = js_sys::Reflect::set(
            &window,
            &"__NVSF_E2E__".into(),
            &wasm_bindgen::JsValue::TRUE,
        );
    }
    let saved = web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item("nvsf-theme").ok().flatten());
    let theme = helpers::resolve_theme(saved.as_deref(), true).to_string();
    if let Some(root) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    {
        let _ = root.set_attribute("data-theme", &theme);
    }
    leptos::mount::mount_to_body(move || view! {<App initial_theme=theme.clone()/>});
}

fn listen_native(ui: Ui, drop_ready: RwSignal<bool>, initial_theme: String) {
    let dropped = |ui: Ui| {
        Closure::<dyn FnMut(wasm_bindgen::JsValue)>::new(move |event| {
            ui.dragging.set(false);
            if let Some(token) = js_sys::Reflect::get(&event, &"payload".into())
                .ok()
                .and_then(|value| value.as_string())
            {
                ui.accept_drop(token);
            }
        })
    };
    let callbacks = [
        (
            "nvsf://close-requested",
            Closure::<dyn FnMut(wasm_bindgen::JsValue)>::new(move |_| {
                ui.studio.shortcuts.set(false);
                ui.studio.close_open.set(true);
            }),
        ),
        (
            "nvsf://drag-state",
            Closure::<dyn FnMut(wasm_bindgen::JsValue)>::new(move |event| {
                let active = js_sys::Reflect::get(&event, &"payload".into())
                    .ok()
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                ui.dragging.set(active);
            }),
        ),
        ("nvsf://files-dropped", dropped(ui)),
        ("nvsf://drop-paths", dropped(ui)),
    ];
    let listeners = callbacks
        .iter()
        .map(|(event, callback)| {
            (
                *event,
                callback
                    .as_ref()
                    .unchecked_ref::<js_sys::Function>()
                    .clone(),
            )
        })
        .collect::<Vec<_>>();
    let callbacks = StoredValue::new_local(callbacks);
    let unlisteners = StoredValue::new_local(Vec::<js_sys::Function>::new());
    spawn_local(async move {
        let mut ready = true;
        for (event, callback) in listeners {
            match ipc::listen_event(event, &callback).await {
                Ok(Some(unlisten)) if !drop_ready.is_disposed() => {
                    unlisteners.update_value(|items| items.push(unlisten));
                    if event == "nvsf://close-requested"
                        && let Some(window) = web_sys::window()
                    {
                        let _ = js_sys::Reflect::set(
                            &window,
                            &"__NVSF_CLOSE_READY__".into(),
                            &wasm_bindgen::JsValue::TRUE,
                        );
                    }
                }
                Ok(Some(unlisten)) => {
                    let _ = unlisten.call0(&wasm_bindgen::JsValue::NULL);
                    return;
                }
                Ok(None) => {
                    if event != "nvsf://close-requested" {
                        ready = false;
                    }
                }
                Err(error) if !drop_ready.is_disposed() => {
                    if event != "nvsf://close-requested" {
                        ready = false;
                        ui.error.set(Some(format!(
                            "Native file-drop subscription failed: {error}"
                        )));
                    }
                }
                Err(_) => return,
            }
        }
        if drop_ready.is_disposed() {
            return;
        }
        drop_ready.set(ready);
        if let Err(error) = ipc::call::<()>("ui_ready", json!({"theme":initial_theme})).await {
            ui.error.set(Some(error));
        }
        match ipc::call::<ipc::Doctor>("doctor", json!({})).await {
            Ok(report) => {
                if report
                    .dependencies
                    .iter()
                    .any(|row| row.state != "verified")
                {
                    ui.push_notice(
                        "A native dependency needs attention. Run resource checks in Settings.",
                    );
                }
                ui.diagnostics.set(Some(report));
            }
            Err(error) => ui.error.set(Some(error)),
        }
        match ipc::call::<Vec<ipc::QueueEntry>>("queue_list", json!({})).await {
            Ok(entries) => ui.queue.set(entries),
            Err(error) => ui.error.set(Some(error)),
        }
    });
    on_cleanup(move || {
        if let Some(window) = web_sys::window() {
            let _ = js_sys::Reflect::delete_property(&window, &"__NVSF_CLOSE_READY__".into());
        }
        unlisteners.with_value(|list| {
            for unlisten in list {
                let _ = unlisten.call0(&wasm_bindgen::JsValue::NULL);
            }
        });
        callbacks.with_value(|_| {});
    });
}

#[component]
fn App(initial_theme: String) -> impl IntoView {
    let ui = Ui::new(initial_theme.clone());
    let drop_ready = RwSignal::new(false);
    crate::maintenance::watch(ui);
    crate::player::watch_playback(ui);
    hotkeys::register(ui);
    if let Some(window) = web_sys::window() {
        let snapshot = Closure::<dyn FnMut(bool) -> wasm_bindgen::JsValue>::new(move |closing| {
            let state = ui.close_snapshot(closing);
            serde::Serialize::serialize(&state, &serde_wasm_bindgen::Serializer::json_compatible())
                .unwrap_or(wasm_bindgen::JsValue::NULL)
        });
        if js_sys::Reflect::set(&window, &"__NVSF_REVIEW_STATE__".into(), snapshot.as_ref())
            .is_err()
        {
            ui.error.set(Some(
                "Review close protection could not initialize; closing will be refused".into(),
            ));
        }
        let snapshot = StoredValue::new_local(snapshot);
        on_cleanup(move || {
            if let Some(window) = web_sys::window() {
                let _ = js_sys::Reflect::delete_property(&window, &"__NVSF_REVIEW_STATE__".into());
            }
            snapshot.with_value(|_| {});
        });
    }
    Effect::new(move |_| {
        if !ui.closing.get()
            && let Some(project) = ui.pending_project.get_untracked()
        {
            ui.pending_project.set(None);
            ui.load(project);
        }
    });
    let main = NodeRef::<leptos::html::Main>::new();
    let icons = [
        "M4 5h16v14H4z M10 9l6 3-6 3z",
        "M5 6h14 M5 12h14 M5 18h9",
        "M4 5h16v14H4z M8 9h8 M8 13h5",
        "M12 3l9 5-9 5-9-5z M3 12l9 5 9-5 M3 16l9 5 9-5",
        "M3 12a9 9 0 1 0 3-7 M3 3v5h5 M12 7v5l3 2",
        "M4 7h16 M4 17h16 M8 4v6 M16 14v6",
    ];
    Effect::new(move |_| {
        let review_dirty = ui.dirty.get() || ui.draft_dirty.get();
        let dirty = ui.extract_dirty() || review_dirty;
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(e) = ipc::call::<()>(
                "set_unsaved",
                json!({"dirty": dirty, "reviewDirty": review_dirty}),
            )
            .await
            {
                ui.error.set(Some(e));
            }
        });
    });
    Effect::new(move |_| {
        let route = ui.route.get();
        if route != "Extract" && ui.studio.focus_mode.get_untracked() {
            ui.toggle_focus_mode();
        }
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
    }
    listen_native(ui, drop_ready, initial_theme);
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
    let about = RwSignal::new(false);
    Effect::new(move |_| {
        let focus = ui.studio.focus_mode.get();
        if let Some(root) = document().document_element() {
            let _ = if focus {
                root.set_attribute("data-focus-mode", "true")
            } else {
                root.remove_attribute("data-focus-mode")
            };
        }
    });
    Effect::new(move |previous: Option<Option<String>>| {
        let token = ui
            .source
            .with(|source| source.as_ref().map(|source| source.token.clone()));
        if token.is_some() && previous.as_ref() != Some(&token) {
            ui.capture_extract_baseline();
        }
        token
    });
    Effect::new(move |_| {
        let pending = ui.busy.get() || ui.preview_busy.get() || ui.studio.initial.get();
        if !pending && ui.studio.loading_video.get_untracked() {
            ui.studio.loading_video.set(false);
        }
    });
    view! {
        <a class="skip-link" href="#main-content">"Skip to workspace"</a>
        <div class="app-shell shell" class:focus-mode=move ||ui.studio.focus_mode.get()
            data-native-drop-ready=move ||drop_ready.get().to_string()>
            <crate::menubar::MenuBar ui about/>
            <aside class="sidebar" id="workspace-sidebar">
                <a class="brand" href="#extract" on:click=move |_|ui.route.set("Extract".into()) aria-label="Numera Video Sub Finder home">
                    <img src="logo.svg" width="32" height="32" alt=""/>
                    <span><strong>"Numera"</strong><small>"Video Sub Finder"</small><small class="version mono">{concat!("(v", env!("CARGO_PKG_VERSION"), ")")}</small></span>
                </a>
                <nav aria-label="Workspace navigation">{helpers::ROUTES.into_iter().zip(icons).map(|(route,path)|view!{
                    <button class="nav-item" title=route class:active=move ||ui.route.get()==route aria-current=move ||if ui.route.get()==route{Some("page")}else{None}
                        on:click=move |_|ui.route.set(route.into())>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><path d=path/></svg><span>{route}</span>
                    </button>
                }).collect_view()}</nav>
                <div class="sidebar-footer"><span class="version mono">{concat!("v",env!("CARGO_PKG_VERSION"))}</span></div>
            </aside>
            <div class="workspace">
                <Topbar ui/>
                <UpdateBanner ui/>
                <main id="main-content" tabindex="-1" node_ref=main class:extract-workspace=move ||ui.route.get()=="Extract">
                    {fixture_controls}
                    <Show when=move ||ui.error.with(Option::is_some)><div class="message error" role="alert"><strong>"Action could not complete"</strong><p>{move ||ui.error.get().unwrap_or_default()}</p><button on:click=move |_|ui.error.set(None)>"Dismiss error"</button></div></Show>
                    <div class="extract-route" hidden=move ||ui.route.get()!="Extract"><Extract ui/></div>
                    <div hidden=move ||ui.route.get()!="Queue"><Queue ui/></div>
                    <div hidden=move ||ui.route.get()!="Review"><Review ui/></div>
                    <div hidden=move ||ui.route.get()!="Models"><Models ui/></div>
                    <div hidden=move ||ui.route.get()!="History"><History ui/></div>
                    <div hidden=move ||ui.route.get()!="Settings"><Settings ui/></div>
                </main>
                <footer class="statusbar" role="status">{move ||if ui.busy.get(){"Native operation in progress"}else if ui.active(){"Extraction active"}else{"No active extraction"}}</footer>
            </div>
            <Notices ui about/>
        </div>
        <crate::menubar::AboutModal visible=about/>
        <ShortcutsModal ui/>
        <CloseModal ui/>
        <Show when=move || ui.dragging.get()>
            <div class="drop-overlay" role="status" aria-live="polite">
                <div class="drop-card">
                    <strong>"Drop video files or a folder"</strong>
                    <span>"Single video opens in Extract · multiple videos or folders add to Queue"</span>
                </div>
            </div>
        </Show>
    }
}

#[component]
fn Notices(ui: Ui, about: RwSignal<bool>) -> impl IntoView {
    let hovered = RwSignal::new(false);
    let focused = RwSignal::new(false);
    let showing = move || {
        ui.toast_visible.get()
            && !about.get()
            && !ui.studio.shortcuts.get()
            && !ui.studio.close_open.get()
    };

    Effect::new(move |_| {
        let message = ui.notice.get();
        // Other workspaces still use the original notice signal.
        if !ui
            .notifications
            .with_untracked(|items| items.first().is_some_and(|item| item.message == message))
        {
            ui.record_notice(&message);
        }
    });
    Effect::new(move |_| {
        if !showing() {
            hovered.set(false);
            focused.set(false);
        }
    });
    Effect::new(
        move |previous: Option<Option<gloo_timers::callback::Timeout>>| {
            drop(previous);
            let _ = ui
                .notifications
                .with(|items| items.first().map(|item| item.id));
            let paused = hovered.get() || focused.get();
            (showing() && !paused).then(|| {
                gloo_timers::callback::Timeout::new(5_000, move || {
                    ui.toast_visible.set(false);
                })
            })
        },
    );
    Effect::new(move |_| {
        if ui.notifications_open.get() {
            wasm_bindgen_futures::spawn_local(async move {
                if ui.notifications_open.get_untracked() {
                    crate::menubar::focus("#notifications-panel");
                }
            });
        }
    });
    let outside = window_event_listener(ev::pointerdown, move |event| {
        if ui.notifications_open.get_untracked()
            && !event
                .target()
                .and_then(|node| node.dyn_into::<web_sys::Element>().ok())
                .is_some_and(|node| {
                    node.closest("#header-notifications, #notifications-panel")
                        .ok()
                        .flatten()
                        .is_some()
                })
        {
            ui.close_notifications(false);
        }
    });
    on_cleanup(move || outside.remove());

    view! {
        <section id="notifications-panel" class="notifications-panel" aria-labelledby="notifications-title"
            tabindex="-1" hidden=move || !ui.notifications_open.get() inert=move || !ui.notifications_open.get()>
            <div class="panel-heading">
                <h2 id="notifications-title">"Notifications"</h2>
                <button type="button" class="ghost" on:click=move |_|ui.close_notifications(true)>"Close"</button>
            </div>
            <div class="button-row">
                <button type="button" disabled=move ||!ui.notifications.with(|items|items.iter().any(|item|!item.read))
                    on:click=move |_|{
                        crate::menubar::focus("#notifications-panel");
                        ui.notifications.update(|items|for item in items { item.read = true; });
                    }>"Mark all read"</button>
                <button type="button" disabled=move ||ui.notifications.with(Vec::is_empty)
                    on:click=move |_|ui.clear_notifications()>"Clear all"</button>
            </div>
            <Show when=move ||ui.notifications.with(Vec::is_empty)>
                <p class="hint">"No notifications yet."</p>
            </Show>
            <ul class="notifications-list">
                <For each=move ||ui.notifications.get() key=|item|(item.id, item.read) children=move |item| view! {
                    <li><p>{item.message}</p><small>{item.time}{if item.read { "" } else { " · Unread" }}</small></li>
                }/>
            </ul>
        </section>
        <div class="toast-stack" aria-label="Latest notification">
            <div class="toast-item" class:notice=showing hidden=move ||!showing() inert=move ||!showing()
                on:mouseenter=move |_|hovered.set(true) on:mouseleave=move |_|hovered.set(false)
                on:focusin=move |_|focused.set(true) on:focusout=move |_|focused.set(false)>
                <span role="status">{move ||if showing() { ui.notice.get() } else { String::new() }}</span>
                <button type="button" on:click=move |_|{
                    if focused.get_untracked() && !ui.studio.focus_mode.get_untracked() {
                        crate::menubar::focus("#header-notifications");
                    } else if focused.get_untracked() {
                        crate::menubar::focus("#main-content");
                    }
                    hovered.set(false);
                    focused.set(false);
                    ui.toast_visible.set(false);
                }>"Dismiss"</button>
            </div>
        </div>
    }
}
