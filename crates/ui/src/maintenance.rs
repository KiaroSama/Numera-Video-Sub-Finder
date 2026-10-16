use crate::{ipc, state::Ui};
use leptos::prelude::*;
use serde::Deserialize;
use serde_json::json;
use wasm_bindgen_futures::spawn_local;

#[derive(Clone, Default, Deserialize)]
pub struct Status {
    pub enabled: bool,
    pub state: String,
    pub token: Option<String>,
    pub version: Option<String>,
    pub message: String,
    pub downloaded: u64,
    pub total: u64,
    pub automatic_install: bool,
}

pub fn watch(ui: Ui) {
    spawn_local(async move {
        loop {
            if ui.maintenance.is_disposed() {
                break;
            }
            if let Ok(status) = ipc::call::<Status>("update_status", json!({})).await {
                if ui.maintenance.is_disposed() {
                    break;
                }
                ui.maintenance.set(status);
            }
            gloo_timers::future::TimeoutFuture::new(2000).await;
        }
    });
}

#[component]
pub fn Updates(ui: Ui) -> impl IntoView {
    let download_approved = RwSignal::new(false);
    let install_approved = RwSignal::new(false);
    let approved_offer = RwSignal::new(None::<String>);
    Effect::new(move |_| {
        let token = ui.maintenance.with(|s| s.token.clone());
        if approved_offer.get_untracked() != token {
            download_approved.set(false);
            install_approved.set(false);
            approved_offer.set(token);
        }
    });
    let action = move |command: &'static str| {
        let token = ui.maintenance.with_untracked(|s| s.token.clone());
        let Some(token) = token else {
            return;
        };
        if approved_offer.get_untracked().as_ref() != Some(&token) {
            return;
        }
        if command == "download_update" {
            install_approved.set(false);
            ui.request::<Status>(
                command,
                json!({"token":token,"consent":download_approved.get_untracked()}),
                move |s| ui.maintenance.set(s),
            );
        } else if command == "install_update" {
            ui.request::<()>(command, json!({"token":token,"consent":install_approved.get_untracked()}), move |_|ui.notice.set("Installation accepted. Follow the native installer or restart when instructed.".into()));
        } else {
            ui.request::<Option<String>>(command, json!({"token":token}), move |name| {
                if let Some(name) = name {
                    ui.notice.set(format!(
                        "Verified download saved: {name}. Follow the installation guide."
                    ));
                }
            });
        }
    };
    view! {<section class="panel"><div class="panel-heading"><h2>"Application updates"</h2>
        <button disabled=move ||ui.busy.get() on:click={move |_|ui.request::<Status>("check_updates",json!({}),move |s|ui.maintenance.set(s))}>"Check updates"</button></div>
        <label class="check"><input data-testid="automatic-update-checks" type="checkbox" disabled=move ||ui.busy.get() prop:checked=move ||ui.maintenance.with(|s|s.enabled) on:change={move |event|ui.request::<Status>("set_update_checks",json!({"enabled":event_target_checked(&event)}),move |s|ui.maintenance.set(s))}/><span>"Check official stable releases automatically (startup and at most daily)"</span></label>
        <p class="hint">"Metadata checks contact the official repository. Video, subtitles and models are not uploaded. Downloads and installation each require your approval. Disabling checks does not disable manual checks."</p>
        <p role="status">{move ||ui.maintenance.with(|s|format!("{}{} · {}",s.state,s.version.as_ref().map(|v|format!(" {v}")).unwrap_or_default(),s.message))}</p>
        <Show when=move ||ui.maintenance.with(|s|s.token.is_some())>
            <label class="check"><input type="checkbox" prop:checked=move ||download_approved.get() on:change=move |e|download_approved.set(event_target_checked(&e))/><span>"I approve downloading this offered update"</span></label>
            <div class="button-row"><button disabled=move ||ui.busy.get()||!download_approved.get() on:click=move |_|action("download_update")>"Download and verify"</button>
                <button disabled=move ||!ui.maintenance.with(|s|s.state=="downloading") on:click=move |_|spawn_local(async move{let _=ipc::call::<()>("cancel_update",json!({})).await;})>"Cancel download"</button></div>
            <p class="mono">{move ||ui.maintenance.with(|s|format!("{} / {} bytes",s.downloaded,s.total))}</p>
            <Show when=move ||ui.maintenance.with(|s|s.state=="verified")>
                <button disabled=move ||ui.busy.get() on:click=move |_|action("save_update")>"Save verified download…"</button>
                <Show when=move ||ui.maintenance.with(|s|s.automatic_install)>
                    <label class="check"><input type="checkbox" prop:checked=move ||install_approved.get() on:change=move |e|install_approved.set(event_target_checked(&e))/><span>"I approve installing this verified update; my review edits are saved"</span></label>
                    <button class="primary" disabled=move ||ui.busy.get()||ui.active()||ui.dirty.get()||ui.draft_dirty.get()||!install_approved.get() on:click=move |_|action("install_update")>"Install approved update"</button>
                </Show>
            </Show>
        </Show>
    </section>}
}
