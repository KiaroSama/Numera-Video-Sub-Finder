use crate::{ipc, state::Ui};
use leptos::prelude::*;
use serde_json::json;
use wasm_bindgen_futures::spawn_local;

#[component]
pub(crate) fn StudioDialog<C>(
    id: &'static str,
    label: &'static str,
    role: &'static str,
    #[prop(optional)] description: Option<&'static str>,
    visible: RwSignal<bool>,
    on_cancel: C,
    children: Children,
) -> impl IntoView
where
    C: Fn() + 'static,
{
    let dialog = NodeRef::<leptos::html::Dialog>::new();
    Effect::new(move |_| {
        let visible = visible.get();
        if let Some(dialog) = dialog.get() {
            if visible && !dialog.open() {
                let _ = dialog.show_modal();
            } else if !visible && dialog.open() {
                dialog.close();
            }
        }
    });
    view! {
        <dialog id=id class="studio-dialog panel" node_ref=dialog role=role aria-modal="true" aria-labelledby=label aria-describedby=description
            on:cancel=move |e: web_sys::Event|{e.prevent_default();on_cancel();}>
            {children()}
        </dialog>
    }
}

#[component]
pub(crate) fn CloseModal(ui: Ui) -> impl IntoView {
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    Effect::new(move |_| {
        if ui.studio.close_open.get() {
            error.set(None);
        }
    });
    let respond = move |action: &'static str| {
        if pending.get_untracked() {
            return;
        }
        if action == "save" && ui.draft_dirty.get_untracked() {
            error.set(Some(
                "Cancel, then apply the current cue correction before saving.".into(),
            ));
            return;
        }
        let project = if action == "save" {
            ui.project.get_untracked()
        } else {
            None
        };
        pending.set(true);
        error.set(None);
        spawn_local(async move {
            let result = ipc::call::<()>(
                "confirm_close",
                json!({"action": action, "project": project}),
            )
            .await;
            if pending.is_disposed() {
                return;
            }
            pending.set(false);
            match result {
                Ok(()) => ui.studio.close_open.set(false),
                Err(message) if ui.closing.get_untracked() => error.set(Some(message)),
                Err(message) => {
                    ui.studio.close_open.set(false);
                    ui.error.set(Some(message));
                }
            }
        });
    };
    view! {
        <StudioDialog id="close-modal" label="close-title" description="close-description" role="alertdialog"
            visible=ui.studio.close_open on_cancel=move ||respond("cancel")>
            <h2 id="close-title">"Unsaved changes"</h2>
            <p id="close-description">"Save your review edits before closing, or exit without saving."</p>
            <Show when=move ||ui.draft_dirty.get()>
                <p class="hint">"To save an unapplied cue correction, cancel and apply it first."</p>
            </Show>
            <Show when=move ||error.with(Option::is_some)><p class="dialog-error" role="alert">{move ||error.get().unwrap_or_default()}</p></Show>
            <div class="dialog-actions">
                <button id="close-cancel" autofocus disabled=move ||pending.get() on:click=move |_|respond("cancel")>"Cancel"</button>
                <button id="close-discard" class="danger" disabled=move ||pending.get() on:click=move |_|respond("discard")>"Exit without saving"</button>
                <button id="close-save" class="primary" disabled=move ||pending.get() on:click=move |_|respond("save")>"Save & Exit"</button>
            </div>
        </StudioDialog>
    }
}
