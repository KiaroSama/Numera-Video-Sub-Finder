use crate::state::Ui;
use leptos::prelude::*;

#[component]
pub(crate) fn Topbar(ui: Ui) -> impl IntoView {
    let project_busy =
        move || ui.busy.get() || ui.active() || ui.review_loading.get() || ui.closing.get();
    let unsaved = move || ui.dirty.get() || ui.draft_dirty.get() || ui.extract_dirty();
    let unread = move || {
        ui.notifications
            .with(|items| items.iter().filter(|item| !item.read).count())
    };
    view! {
        <header class="topbar">
            <h1>{move ||ui.route.get()}</h1>
            <div class="topbar-actions" role="group" aria-label="Workspace commands">
                <div class="toolbar-group">
                    <button id="header-undo" class="ghost icon-button" title="Undo (Ctrl+Z)" aria-label="Undo" disabled=move ||!ui.can_undo() on:click=move |_|ui.undo()>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M9 5 4 10l5 5 M4 10h10a5 5 0 0 1 0 10h-2"/></svg><span class="hotkey-hint" aria-hidden="true">"(Ctrl+Z)"</span>
                    </button>
                    <button id="header-redo" class="ghost icon-button" title="Redo (Ctrl+Y / Ctrl+Shift+Z)" aria-label="Redo" disabled=move ||!ui.can_redo() on:click=move |_|ui.redo()>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m15 5 5 5-5 5 M20 10H10a5 5 0 0 0 0 10h2"/></svg><span class="hotkey-hint" aria-hidden="true">"(Ctrl+Y)"</span>
                    </button>
                </div>
                <div class="toolbar-group">
                    <button id="header-open-video" class="ghost" title="Open video (Ctrl+O)" aria-label="Open video"
                        disabled=move ||ui.busy.get()||ui.closing.get() on:click=move |_|ui.open_video()>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><rect x="3" y="5" width="18" height="14" rx="2"/><path d="m10 9 5 3-5 3z"/></svg><span class="toolbar-label">"Open video"</span><span class="hotkey-hint" aria-hidden="true">"(Ctrl+O)"</span>
                    </button>
                    <button id="header-open-project" class="ghost" title="Open project (Ctrl+Shift+O)" aria-label="Open project"
                        disabled=project_busy on:click=move |_|ui.open_project()>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M3 7V5h6l2 3h10v11H3z"/></svg><span class="toolbar-label">"Open project"</span><span class="hotkey-hint" aria-hidden="true">"(Ctrl+Shift+O)"</span>
                    </button>
                </div>
                <div class="toolbar-group">
                    <button id="header-save" class="ghost" title="Save (Ctrl+S)" aria-label=move ||if unsaved(){"Save unsaved changes"}else{"Save"}
                        disabled=move ||!ui.can_save_project() on:click=move |_|ui.save(false)>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 4h13l3 3v13H4z M8 4v6h8V4 M8 20v-6h8v6"/></svg><span class="toolbar-label">"Save"</span><span class="hotkey-hint" aria-hidden="true">"(Ctrl+S)"</span>
                        <Show when=unsaved><span class="unsaved-dot" aria-hidden="true"></span></Show>
                    </button>
                    <button id="header-save-as" class="ghost" title="Save As (Ctrl+Shift+S)" aria-label="Save As"
                        disabled=move ||!ui.can_save_project() on:click=move |_|ui.save(true)>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M13 4H4v16h8 M8 4v6h5 M14 18l5-5 2 2-5 5h-2z"/></svg><span class="toolbar-label">"Save As"</span><span class="hotkey-hint" aria-hidden="true">"(Ctrl+Shift+S)"</span>
                    </button>
                </div>
                <div class="toolbar-group">
                    <button id="header-shortcuts" type="button" class="ghost icon-button" title="Keyboard shortcuts (F1)" aria-label="Keyboard shortcuts" aria-haspopup="dialog"
                        on:click=move |_|ui.studio.shortcuts.set(true)>"?"<span class="hotkey-hint" aria-hidden="true">"(F1)"</span></button>
                    <button id="header-theme" type="button" class="ghost theme-toggle" aria-label=move ||if ui.theme.get()=="dark"{"Switch to light theme"}else{"Switch to dark theme"}
                        aria-pressed=move ||if ui.theme.get()=="light"{"true"}else{"false"} title=move ||if ui.theme.get()=="dark"{"Switch workspace to light theme (Ctrl+Shift+T)"}else{"Switch workspace to dark theme (Ctrl+Shift+T)"} on:click=move |_|ui.toggle_theme()>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><path d=move ||if ui.theme.get()=="dark"{"M12 3v2 M12 19v2 M4.93 4.93l1.41 1.41 M17.66 17.66l1.41 1.41 M3 12h2 M19 12h2 M4.93 19.07l1.41-1.41 M17.66 6.34l1.41-1.41 M16 12a4 4 0 1 1-8 0 4 4 0 0 1 8 0z"}else{"M21 12.79A9 9 0 1 1 11.21 3a7 7 0 0 0 9.79 9.79z"}/></svg>
                        <span class="toolbar-label">{move ||if ui.theme.get()=="dark"{"Light"}else{"Dark"}}</span><span class="hotkey-hint" aria-hidden="true">"(Ctrl+Shift+T)"</span>
                    </button>
                </div>
                <button id="header-notifications" type="button" class="ghost" title="Notifications (Alt+N)"
                    aria-controls="notifications-panel" aria-expanded=move ||ui.notifications_open.get().to_string()
                    aria-label=move ||if unread()==0 {"Notifications".into()} else {format!("Notifications, {} unread", unread())}
                    on:click=move |_|ui.toggle_notifications()>
                    <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M18 8a6 6 0 0 0-12 0v4l-2 4h16l-2-4z M10 20h4"/></svg>
                    <span class="hotkey-hint" aria-hidden="true">"(Alt+N)"</span>
                    <Show when={move || unread() > 0}><span class="notif-badge" aria-hidden="true">{unread}</span></Show>
                </button>
            </div>
        </header>
    }
}
