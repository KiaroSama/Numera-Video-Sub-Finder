use crate::{ipc, state::Ui};
use leptos::prelude::*;
use serde::Deserialize;
use serde_json::json;
use wasm_bindgen_futures::spawn_local;

#[derive(Clone, Deserialize)]
struct Status {
    code: String,
    installed: bool,
    verified: bool,
    bytes: u64,
    primary: String,
    message: String,
}

#[component]
pub fn LanguagePacks(ui: Ui) -> impl IntoView {
    let selected = RwSignal::new("fra".to_string());
    let status = RwSignal::new(Vec::<Status>::new());
    let installing = RwSignal::new(false);
    let refresh = move || {
        ui.request::<Vec<Status>>("language_catalog", json!({}), move |rows| status.set(rows));
    };
    let install = move |_| {
        if ui.busy.get_untracked() || ui.active() {
            return;
        }
        ui.busy.set(true);
        installing.set(true);
        spawn_local(async move {
            match ipc::call::<()>("install_language", json!({"language":selected.get_untracked(),"consent":true})).await {
                Ok(()) => ui.notice.set("Selected language installed and hash verified. Subtitle accuracy is not certified.".into()),
                Err(error) => ui.error.set(Some(error)),
            }
            match ipc::call::<Vec<Status>>("language_catalog", json!({})).await {
                Ok(rows) => status.set(rows),
                Err(error) => ui.error.set(Some(error)),
            }
            installing.set(false);
            ui.busy.set(false);
        });
    };
    let artifact = move |command: &'static str| {
        ui.request::<Option<String>>(
            command,
            json!({"language":selected.get_untracked()}),
            move |name| {
                if let Some(name) = name {
                    ui.notice
                        .set(format!("Language artifact operation completed: {name}"));
                }
            },
        );
    };
    view! {<section class="panel"><h2>"Additional language packs"</h2>
        <p class="hint">"123 official language and vertical variants. Choose a pack to install explicitly; other packs are not downloaded. Availability is not measured subtitle accuracy."</p>
        <label for="language-pack">"Language pack"</label>
        <select id="language-pack" prop:value=move ||selected.get() on:change=move |event|selected.set(event_target_value(&event))>
            {nvsf_core::languages::catalog().iter().map(|language|view!{<option value=language.code.clone()>{format!("{} ({})",language.name,language.code)}</option>}).collect_view()}
        </select>
        <div class="button-row"><button data-testid="languages-verify" disabled=move ||ui.busy.get() on:click=move |_|refresh()>"Verify language packs"</button>
            <button class="primary" disabled=move ||ui.busy.get()||ui.active() on:click=install>"Download selected language pack"</button>
            <button disabled=move ||ui.busy.get()||ui.active() on:click=move |_|artifact("import_language")>"Import language…"</button>
            <button disabled=move ||ui.busy.get()||ui.active() on:click=move |_|artifact("export_language")>"Export language…"</button>
            <Show when=move ||installing.get()><button class="danger" on:click=move |_|spawn_local(async move{if let Err(error)=ipc::call::<()>("cancel_model_install",json!({})).await{ui.error.set(Some(error));}})>"Cancel language download"</button></Show>
        </div>
        <p role="status" data-testid="language-pack-status">{move ||status.with(|rows|rows.iter().find(|row|row.code==selected.get()).map(|row|format!("{} · {} bytes · {} · {}",row.primary,row.bytes,if row.verified{"Hash verified"}else if row.installed{"Invalid pack"}else{"Not installed"},row.message)).unwrap_or_else(||"Verify language packs to read actual installed status.".into()))}</p>
    </section>}
}
