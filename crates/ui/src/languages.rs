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
    let status = RwSignal::new(None::<Vec<Status>>);
    let refreshing = RwSignal::new(false);
    let refresh_error = RwSignal::new(false);
    let installing = RwSignal::new(false);
    let refresh = move || {
        if refreshing.get_untracked() || ui.closing.get_untracked() {
            return;
        }
        refreshing.set(true);
        refresh_error.set(false);
        spawn_local(async move {
            let result = ipc::call::<Vec<Status>>("language_catalog", json!({})).await;
            if status.is_disposed() {
                return;
            }
            refreshing.set(false);
            match result {
                Ok(rows) => status.set(Some(rows)),
                Err(error) => {
                    refresh_error.set(true);
                    if ui.route.get_untracked() == "Models" {
                        ui.error.set(Some(error));
                    }
                }
            }
        });
    };
    Effect::new(move |_| refresh());
    let install = move |_| {
        if ui.busy.get_untracked() || ui.active() {
            return;
        }
        ui.busy.set(true);
        installing.set(true);
        spawn_local(async move {
            match ipc::call::<()>("install_language", json!({"language":selected.get_untracked(),"consent":true})).await {
                Ok(()) => ui.push_notice("Language pack installed and verified. This check does not measure subtitle accuracy."),
                Err(error) => ui.error.set(Some(error)),
            }
            installing.set(false);
            ui.busy.set(false);
            refresh();
        });
    };
    let artifact = move |command: &'static str| {
        ui.request::<Option<String>>(
            command,
            json!({"language":selected.get_untracked()}),
            move |name| {
                if let Some(name) = name {
                    ui.push_notice(format!("Language pack operation completed: {name}"));
                    refresh();
                }
            },
        );
    };
    let catalog = nvsf_core::languages::catalog();
    view! {<section class="panel language-packs-panel">
        <header class="language-packs-heading">
            <h2>"Additional language packs"</h2>
            <p class="hint">"Choose an optional language or vertical writing pack to download. Verification checks file integrity, not subtitle accuracy."</p>
        </header>
        <section data-testid="installed-languages-list" aria-labelledby="installed-languages-title">
            <h3 id="installed-languages-title">"Installed language packs"</h3>
            {move || {
                let rows: Vec<Status> = status.with(|rows| rows.iter().flatten().filter(|row| row.installed).cloned().collect());
                if refreshing.get() && status.with(Option::is_none) {
                    return view! {<p role="status" class="hint">"Checking installed language packs…"</p>}.into_any();
                }
                if refresh_error.get() && status.with(Option::is_none) {
                    return view! {<p role="status" class="hint">"Could not read the installed packs. Select Verify language packs to retry."</p>}.into_any();
                }
                let summary = format!("Installed: {} of {} packs", rows.len(), catalog.len());
                view! {
                    <p class="hint" role="status">{summary}</p>
                    {if rows.is_empty() {
                        view! {<p class="hint">"No optional packs are installed. Default language models are listed above."</p>}.into_any()
                    } else {
                        view! {<ul class="installed-languages-grid">{rows.into_iter().map(|row| {
                            let name = nvsf_core::languages::find(&row.code).map_or_else(|| row.code.clone(), |language| language.name.clone());
                            view! {<li><span>{format!("{name} ({})", row.code)}</span><small>{pack_size(row.bytes)}</small>
                                <span class="state-chip">{if row.verified {"Verified"} else {"Invalid"}}</span></li>}
                        }).collect_view()}</ul>}.into_any()
                    }}
                }.into_any()
            }}
        </section>
        <div class="language-pack-picker">
            <label for="language-pack">"Language pack"</label>
            <select id="language-pack" prop:value=move ||selected.get() on:change=move |event|selected.set(event_target_value(&event))>
                {catalog.iter().map(|language|view!{<option value=language.code.clone()>{format!("{} ({})",language.name,language.code)}</option>}).collect_view()}
            </select>
        </div>
        <div class="button-row"><button data-testid="languages-verify" disabled=move ||ui.busy.get()||refreshing.get() on:click=move |_|refresh()>"Verify language packs"</button>
            <button class="primary" disabled=move ||ui.busy.get()||ui.active()||refreshing.get() on:click=install>"Download selected language pack"</button>
            <button disabled=move ||ui.busy.get()||ui.active() on:click=move |_|artifact("import_language")>"Import language…"</button>
            <button disabled=move ||ui.busy.get()||ui.active() on:click=move |_|artifact("export_language")>"Export language…"</button>
            <Show when=move ||installing.get()><button class="danger" on:click=move |_|spawn_local(async move{if let Err(error)=ipc::call::<()>("cancel_model_install",json!({})).await{ui.error.set(Some(error));}})>"Cancel language download"</button></Show>
        </div>
        <p class="language-pack-status" role="status" data-testid="language-pack-status">{move ||status.with(|rows|rows.iter().flatten().find(|row|row.code==selected.get()).map(|row|format!("{} · {} bytes · {} · {}",row.primary,row.bytes,if row.verified{"Hash verified"}else if row.installed{"Invalid pack"}else{"Not installed"},row.message)).unwrap_or_else(||if refreshing.get() {"Checking language packs…".into()} else {"Language pack status is unavailable.".into()}))}</p>
    </section>}
}

fn pack_size(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else {
        format!("{:.1} KB", bytes as f64 / 1_024.0)
    }
}
