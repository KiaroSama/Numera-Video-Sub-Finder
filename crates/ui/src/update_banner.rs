use crate::{ipc, maintenance::Status, state::Ui};
use leptos::prelude::*;
use serde_json::json;
use wasm_bindgen_futures::spawn_local;

pub(crate) fn safe_to_update(ui: Ui) -> bool {
    !ui.busy.get()
        && !ui.preview_busy.get()
        && !ui.studio.initial.get()
        && !ui.active()
        && !ui.review_loading.get()
        && !ui.closing.get()
        && !ui.dirty.get()
        && !ui.draft_dirty.get()
}

impl Ui {
    pub(crate) fn restart_after_update(self) {
        if !safe_to_update(self) || !self.maintenance.with_untracked(Status::needs_restart) {
            return;
        }
        self.error.set(None);
        spawn_local(async move {
            if let Err(message) = ipc::call::<()>("restart_app", json!({})).await {
                self.error.set(Some(message));
            }
        });
    }
}

#[component]
pub(crate) fn UpdateBanner(ui: Ui) -> impl IntoView {
    let dismissed = RwSignal::new(None::<String>);
    let installing = RwSignal::new(false);
    let download = move || {
        if !safe_to_update(ui) {
            return;
        }
        let Some((token, automatic)) = ui.maintenance.with_untracked(|status| {
            if status.state != "available" {
                return None;
            }
            Some((status.token.clone()?, status.automatic_install))
        }) else {
            return;
        };
        if !automatic {
            ui.route.set("Settings".into());
            return;
        }
        ui.busy.set(true);
        installing.set(true);
        ui.error.set(None);
        spawn_local(async move {
            let result = async {
                let status =
                    ipc::call::<Status>("download_update", json!({"token":token,"consent":true}))
                        .await?;
                let verified = status.state == "verified" && status.token.as_ref() == Some(&token);
                ui.maintenance.set(status);
                if verified && !ui.closing.get_untracked() {
                    ipc::call::<()>("install_update", json!({"token":token,"consent":true}))
                        .await?;
                }
                ipc::call::<Status>("update_status", json!({})).await
            }
            .await;
            match result {
                Ok(status) => ui.maintenance.set(status),
                Err(message) => ui.error.set(Some(message)),
            }
            ui.busy.set(false);
            installing.set(false);
        });
    };
    view! {
        <Show when=move ||ui.maintenance.with(Status::needs_restart)>
            <div class="update-banner">
                <span role="status">"Update installed. Restart to apply."</span>
                <button class="primary" disabled=move ||!safe_to_update(ui) on:click=move |_|ui.restart_after_update()>"Restart now"</button>
            </div>
        </Show>
        <Show when=move || !ui.maintenance.with(Status::needs_restart) && (installing.get() || (ui.route.get()!="Settings" && ui.maintenance.with(|status| {
            status.state == "available" && status.version.is_some() && status.version != dismissed.get()
        })))>
            <div class="update-banner">
                <span role="status">{move || if installing.get() {
                    "Downloading and verifying update…".to_string()
                } else {
                    ui.maintenance.with(|status|format!("Update available: v{} (current v{})", status.version.as_deref().unwrap_or_default().trim_start_matches('v'), env!("CARGO_PKG_VERSION")))
                }}</span>
                <div class="button-row">
                    <button class="primary" disabled=move ||!safe_to_update(ui)||ui.maintenance.with(|status|status.token.is_none())
                        on:click=move |_|download()>{move ||if ui.maintenance.with(|status|status.automatic_install){"Download & Install"}else{"Review download"}}</button>
                    <button disabled=move ||installing.get() on:click=move |_|dismissed.set(ui.maintenance.with_untracked(|status|status.version.clone()))>"Dismiss"</button>
                </div>
            </div>
        </Show>
    }
}
