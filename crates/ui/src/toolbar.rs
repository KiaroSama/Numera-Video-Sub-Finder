use crate::state::Ui;
use leptos::prelude::*;

#[component]
pub(crate) fn Topbar(ui: Ui) -> impl IntoView {
    let project_busy =
        move || ui.busy.get() || ui.active() || ui.review_loading.get() || ui.closing.get();
    view! {
        <header class="topbar">
            <h1>{move ||ui.route.get()}</h1>
            <div class="topbar-actions" role="group" aria-label="Workspace commands">
                <div class="toolbar-group">
                    <button id="header-undo" class="ghost icon-button" title="Undo (Ctrl+Z)" aria-label="Undo" disabled=move ||!ui.can_undo() on:click=move |_|ui.undo()>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M9 5 4 10l5 5 M4 10h10a5 5 0 0 1 0 10h-2"/></svg>
                    </button>
                    <button id="header-redo" class="ghost icon-button" title="Redo (Ctrl+Y / Ctrl+Shift+Z)" aria-label="Redo" disabled=move ||!ui.can_redo() on:click=move |_|ui.redo()>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m15 5 5 5-5 5 M20 10H10a5 5 0 0 0 0 10h2"/></svg>
                    </button>
                </div>
                <div class="toolbar-group">
                    <button id="header-open-video" class="ghost" title="Open video (Ctrl+O)" aria-label="Open video"
                        disabled=move ||ui.busy.get()||ui.closing.get() on:click=move |_|ui.open_video()>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><rect x="3" y="5" width="18" height="14" rx="2"/><path d="m10 9 5 3-5 3z"/></svg><span class="toolbar-label">"Open video"</span>
                    </button>
                    <button id="header-open-project" class="ghost" title="Open project (Ctrl+Shift+O)" aria-label="Open project"
                        disabled=project_busy on:click=move |_|ui.open_project()>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M3 7V5h6l2 3h10v11H3z"/></svg><span class="toolbar-label">"Open project"</span>
                    </button>
                </div>
                <div class="toolbar-group">
                    <button id="header-save" class="ghost" title="Save (Ctrl+S)" aria-label=move ||if ui.dirty.get(){"Save unsaved changes"}else{"Save"}
                        disabled=move ||!ui.can_save() on:click=move |_|ui.save(false)>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 4h13l3 3v13H4z M8 4v6h8V4 M8 20v-6h8v6"/></svg><span class="toolbar-label">"Save"</span>
                        <Show when=move ||ui.dirty.get()><span class="unsaved-dot" aria-hidden="true"></span></Show>
                    </button>
                    <button id="header-save-as" class="ghost" title="Save As (Ctrl+Shift+S)" aria-label="Save As"
                        disabled=move ||!ui.can_save() on:click=move |_|ui.save(true)>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M13 4H4v16h8 M8 4v6h5 M14 18l5-5 2 2-5 5h-2z"/></svg><span class="toolbar-label">"Save As"</span>
                    </button>
                </div>
                <div class="toolbar-group">
                    <button id="header-shortcuts" type="button" class="ghost icon-button" title="Keyboard shortcuts (F1)" aria-label="Keyboard shortcuts" aria-haspopup="dialog"
                        on:click=move |_|ui.studio.shortcuts.set(true)>"?"</button>
                    <button id="header-theme" type="button" class="ghost theme-toggle" aria-label=move ||if ui.theme.get()=="dark"{"Switch to light theme"}else{"Switch to dark theme"}
                        aria-pressed=move ||if ui.theme.get()=="light"{"true"}else{"false"} title=move ||if ui.theme.get()=="dark"{"Switch workspace to light theme"}else{"Switch workspace to dark theme"} on:click=move |_|ui.toggle_theme()>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><path d=move ||if ui.theme.get()=="dark"{"M12 3v2 M12 19v2 M4.93 4.93l1.41 1.41 M17.66 17.66l1.41 1.41 M3 12h2 M19 12h2 M4.93 19.07l1.41-1.41 M17.66 6.34l1.41-1.41 M16 12a4 4 0 1 1-8 0 4 4 0 0 1 8 0z"}else{"M21 12.79A9 9 0 1 1 11.21 3a7 7 0 0 0 9.79 9.79z"}/></svg>
                        <span class="toolbar-label">{move ||if ui.theme.get()=="dark"{"Light"}else{"Dark"}}</span>
                    </button>
                </div>
            </div>
        </header>
    }
}
