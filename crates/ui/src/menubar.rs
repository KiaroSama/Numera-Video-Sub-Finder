use crate::{close_modal::StudioDialog, maintenance::Status, state::Ui};
use leptos::{ev, prelude::*};
use serde_json::json;
use wasm_bindgen::JsCast;

const TITLES: [&str; 5] = ["File", "Edit", "View", "Tools", "Help"];
const IDS: [&str; 5] = ["file", "edit", "view", "tools", "help"];

#[derive(Clone, Copy)]
enum MenuAction {
    Open,
    Save(bool),
    Export,
    Undo,
    Redo,
    Add(bool),
    Delete,
    Zoom(i8),
    Theme,
    Route(&'static str),
    RunDoctor,
    Updates,
    Shortcuts,
    About,
}
use MenuAction::*;

const MENUS: [&[(&str, &str, MenuAction)]; 5] = [
    &[
        ("Open video", "Ctrl+O", Open),
        ("Save project", "Ctrl+S", Save(false)),
        ("Save project as", "Ctrl+Shift+S", Save(true)),
        ("Export subtitles", "", Export),
    ],
    &[
        ("Undo", "Ctrl+Z", Undo),
        ("Redo", "Ctrl+Y", Redo),
        ("Add subtitle region", "", Add(false)),
        ("Add exclusion region", "", Add(true)),
        ("Delete selected region", "Delete", Delete),
    ],
    &[
        ("Zoom to fit", "Ctrl+0", Zoom(0)),
        ("Zoom in", "Ctrl++", Zoom(1)),
        ("Zoom out", "Ctrl+-", Zoom(-1)),
        ("Toggle Dark / Light theme", "", Theme),
        ("Extract", "", Route("Extract")),
        ("Review", "", Route("Review")),
        ("Queue", "", Route("Queue")),
        ("Models", "", Route("Models")),
        ("History", "", Route("History")),
        ("Settings", "", Route("Settings")),
    ],
    &[
        ("Run diagnostics", "", RunDoctor),
        ("Check for updates", "", Updates),
        ("Open Queue", "", Route("Queue")),
        ("Manage Models", "", Route("Models")),
    ],
    &[
        ("Keyboard shortcuts", "F1", Shortcuts),
        ("About Numera Video Sub Finder", "", About),
    ],
];

impl MenuAction {
    fn disabled(self, ui: Ui) -> bool {
        ui.closing.get()
            || match self {
                Open | RunDoctor | Updates => ui.busy.get(),
                Save(_) => !ui.can_save(),
                Export => ui.project.with(Option::is_none),
                Undo => !ui.can_undo(),
                Redo => !ui.can_redo(),
                Add(excluded) => {
                    ui.busy.get()
                        || ui.active()
                        || ui.config.with(|c| {
                            if excluded {
                                c.exclusions.len() >= 32
                            } else {
                                c.regions.len() >= 8
                            }
                        })
                }
                Delete => {
                    ui.busy.get()
                        || ui.active()
                        || ui.route.get() != "Extract"
                        || ui.studio.roi.with(|r| {
                            r.is_none_or(|id| {
                                ui.config.with(|c| {
                                    if id.excluded {
                                        id.index >= c.exclusions.len()
                                    } else {
                                        c.regions.len() <= 1 || id.index >= c.regions.len()
                                    }
                                })
                            })
                        })
                }
                Zoom(_) => {
                    ui.source.with(Option::is_none)
                        || !matches!(ui.route.get().as_str(), "Extract" | "Review")
                }
                _ => false,
            }
    }

    fn invoke(self, ui: Ui, about: RwSignal<bool>) {
        match self {
            Open => ui.open_video(),
            Save(as_new) => ui.save(as_new),
            Export => {
                ui.route.set("Review".into());
                later(|| focus("#export-format"));
            }
            Undo => ui.undo(),
            Redo => ui.redo(),
            Add(excluded) => {
                ui.route.set("Extract".into());
                ui.add_region(excluded, nvsf_core::Rect::default());
                later(|| focus(".region-select[aria-pressed=true]"));
            }
            Delete => {
                ui.remove_selected_region();
            }
            Zoom(delta) => ui.change_zoom(delta),
            Theme => ui.toggle_theme(),
            Route(route) => ui.route.set(route.into()),
            RunDoctor => {
                ui.route.set("Settings".into());
                ui.request::<crate::ipc::Doctor>("doctor", json!({}), move |d| {
                    ui.diagnostics.set(Some(d))
                });
            }
            Updates => {
                ui.route.set("Settings".into());
                ui.request::<Status>("check_updates", json!({}), move |s| ui.maintenance.set(s));
            }
            Shortcuts => ui.studio.shortcuts.set(true),
            About => about.set(true),
        }
    }
}

fn focus(selector: &str) {
    if let Some(node) = document()
        .query_selector(selector)
        .ok()
        .flatten()
        .and_then(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
    {
        let _ = node.focus();
    }
}

fn later(action: impl FnOnce() + 'static) {
    wasm_bindgen_futures::spawn_local(async move { action() });
}

fn focus_trigger(menu: usize) {
    focus(&format!("#menu-trigger-{}", IDS[menu]));
}

fn move_item(menu: usize, key: &str) {
    let doc = document();
    let items: Vec<_> = (0..MENUS[menu].len())
        .filter_map(|i| {
            doc.query_selector(&format!("#menu-item-{}-{i}:enabled", IDS[menu]))
                .ok()?
                .and_then(|item| item.dyn_into::<web_sys::HtmlElement>().ok())
        })
        .collect();
    if items.is_empty() {
        return;
    }
    let active = document().active_element();
    let current = items.iter().position(|item| {
        active
            .as_ref()
            .is_some_and(|active| item.id() == active.id())
    });
    let count = items.len();
    let next = match key {
        "Home" => 0,
        "End" => count - 1,
        "ArrowUp" => current.map_or(count - 1, |i| (i + count - 1) % count),
        _ => current.map_or(0, |i| (i + 1) % count),
    };
    let _ = items[next].focus();
}

#[component]
pub(crate) fn MenuBar(ui: Ui, about: RwSignal<bool>) -> impl IntoView {
    let open = RwSignal::new(None::<usize>);
    let active = RwSignal::new(0_usize);
    let pointer = window_event_listener(ev::pointerdown, move |event| {
        if !event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|target| target.closest("#app-menubar").ok().flatten().is_some())
        {
            open.set(None);
        }
    });
    on_cleanup(move || pointer.remove());
    let keydown = move |event: web_sys::KeyboardEvent| {
        if event.is_composing() || event.alt_key() || event.ctrl_key() || event.meta_key() {
            return;
        }
        let menu = active.get_untracked();
        let key = event.key();
        let trigger = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|target| target.closest(".menu-trigger").ok().flatten().is_some());
        match key.as_str() {
            "Enter" | " " if trigger => {
                open.set(Some(menu));
                later(move || move_item(menu, "Home"));
            }
            "ArrowLeft" | "ArrowRight" => {
                let next = (menu + if key == "ArrowLeft" { IDS.len() - 1 } else { 1 }) % IDS.len();
                active.set(next);
                if open.get_untracked().is_some() {
                    open.set(Some(next));
                }
                focus_trigger(next);
            }
            "ArrowDown" | "ArrowUp" | "Home" | "End" => {
                open.set(Some(menu));
                later(move || move_item(menu, &key));
            }
            "Escape" if open.get_untracked().is_some() => {
                open.set(None);
                focus_trigger(menu);
            }
            "Tab" => {
                open.set(None);
                return;
            }
            _ => return,
        }
        event.prevent_default();
        event.stop_propagation();
    };
    view! {
        <div id="app-menubar" role="menubar" aria-label="Application menu" on:keydown=keydown>
            {(0..IDS.len()).map(|index| {
                let id = IDS[index];
                view! {
                    <button type="button" class="menu-trigger" id=format!("menu-trigger-{id}") role="menuitem" aria-haspopup="menu" data-menu=id
                        aria-expanded=move || if open.get() == Some(index) { "true" } else { "false" }
                        tabindex=move || if active.get() == index { "0" } else { "-1" }
                        on:pointerenter=move |_| if open.get_untracked().is_some() && open.get_untracked() != Some(index) { open.set(Some(index)); focus_trigger(index); }
                        on:focus=move |_| active.set(index)
                        on:click=move |_| { active.set(index); open.update(|current| *current = (*current != Some(index)).then_some(index)); }>
                        {TITLES[index]}
                    </button>
                    <div class="app-menu" role="menu" aria-labelledby=format!("menu-trigger-{id}") style=format!("--menu-left: {}px", 6 + 48 * index)
                        hidden=move || open.get() != Some(index)>
                        {MENUS[index].iter().copied().enumerate().map(|(item, (label, shortcut, action))| view! {
                            <button type="button" id=format!("menu-item-{id}-{item}") role="menuitem" tabindex="-1" disabled=move || action.disabled(ui)
                                on:click=move |_| {
                                    if !action.disabled(ui) {
                                        open.set(None);
                                        focus_trigger(index);
                                        action.invoke(ui, about);
                                    }
                                }>
                                <span>{label}</span>
                                {(!shortcut.is_empty()).then(|| view! { <kbd>{shortcut}</kbd> })}
                            </button>
                        }).collect_view()}
                    </div>
                }
            }).collect_view()}
        </div>
    }
}

#[component]
pub(crate) fn AboutModal(visible: RwSignal<bool>) -> impl IntoView {
    view! {
        <StudioDialog id="about-modal" label="about-title" role="dialog" visible=visible on_cancel=move || visible.set(false)>
            <h2 id="about-title">"Numera Video Sub Finder"</h2>
            <p>{concat!("Version ", env!("CARGO_PKG_VERSION"))}</p>
            <p>"GPL-3.0-or-later"</p>
            <p>"An offline desktop subtitle extraction and review app built with Rust, Tauri, and Leptos."</p>
            <div class="dialog-actions"><button type="button" autofocus on:click=move |_| visible.set(false)>"Close"</button></div>
        </StudioDialog>
    }
}
