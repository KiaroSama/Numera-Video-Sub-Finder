use crate::{
    ipc::{self, Model},
    state::Ui,
};
use leptos::prelude::*;
use serde_json::json;
use wasm_bindgen_futures::spawn_local;

#[component]
pub fn Models(ui: Ui) -> impl IntoView {
    let installing = RwSignal::new(false);
    let cancelling = RwSignal::new(false);
    let install = move |selected: Option<String>| {
        if ui.busy.get_untracked() || ui.active() {
            return;
        }
        ui.busy.set(true);
        installing.set(true);
        cancelling.set(false);
        ui.error.set(None);
        spawn_local(async move {
            let result = ipc::call::<()>("install_models", json!({"selected":selected})).await;
            match result {
                Ok(()) => ui.notice.set(
                    "Model provisioning completed; verification below is authoritative.".into(),
                ),
                Err(e) => ui.error.set(Some(e)),
            }
            match ipc::call::<Vec<Model>>("model_catalog", json!({})).await {
                Ok(models) => ui.models.set(models),
                Err(e) => ui.error.set(Some(e)),
            }
            installing.set(false);
            cancelling.set(false);
            ui.busy.set(false);
        });
    };
    let artifact = move |command: &'static str, id: String| {
        ui.request::<Option<String>>(command, json!({"id":id}), move |saved| {
            if let Some(name) = saved {
                ui.notice
                    .set(format!("Model artifact operation completed: {name}"));
            }
            spawn_local(async move {
                match ipc::call::<Vec<Model>>("model_catalog", json!({})).await {
                    Ok(m) => ui.models.set(m),
                    Err(e) => ui.error.set(Some(e)),
                }
            });
        });
    };
    view! {<section class="panel"><div class="panel-heading"><div><span class="eyebrow">"OFFLINE RESOURCES"</span><h2>"Recognition models"</h2></div>
        <button data-testid="models-verify" disabled=move ||ui.busy.get() on:click=move |_|ui.request::<Vec<Model>>("model_catalog",json!({}),move |models|ui.models.set(models))>"Verify installed models"</button></div>
        <p class="muted">"Checksummed language resources. Downloads occur only when you explicitly install them. Interrupted downloads resume on the next explicit install; extraction stays local."</p>
        <Show when=move ||ui.models.with(Vec::is_empty)><p class="empty-list">"No catalog loaded. Verify models to see the actual installed state."</p></Show>
        {move ||ui.models.get().into_iter().map(|model|{
            let install_id=model.id.clone();let import_id=model.id.clone();let export_id=model.id.clone();let verified=model.verified;
            view!{<article class="model-row" data-model-id=model.id.clone()><div><h3>{model.id}</h3><p class="hint">{format!("Languages: {}",model.languages.join(", "))}</p><p>{model.message}</p>
                <div class="button-row"><button data-testid="model-install" disabled=move ||ui.busy.get()||ui.active() on:click=move |_|install(Some(install_id.clone()))>"Install selected model"</button>
                    <button data-testid="model-import" disabled=move ||ui.busy.get()||ui.active() on:click=move |_|artifact("import_model",import_id.clone())>"Import ONNX…"</button>
                    <button data-testid="model-export" disabled=move ||ui.busy.get()||ui.active()||!verified on:click=move |_|artifact("export_model",export_id.clone())>"Export ONNX…"</button></div></div>
                <span class="state-chip">{if model.verified{"Hash verified"}else if model.installed{"Needs verification"}else{"Not installed"}}</span></article>}
        }).collect_view()}
        <div class="button-row"><button data-testid="models-install-defaults" class="primary" disabled=move ||ui.busy.get()||ui.active() on:click=move |_|install(None)>"Install default language models + verifier data"</button>
            <Show when=move ||installing.get()><button data-testid="models-cancel" class="danger" disabled=move ||cancelling.get() on:click=move |_|{
                cancelling.set(true);spawn_local(async move{match ipc::call::<()>("cancel_model_install",json!({})).await{
                    Ok(())=>ui.notice.set("Cancellation requested; waiting for the active transfer to stop safely.".into()),Err(e)=>{ui.error.set(Some(e));cancelling.set(false);}
                }});
            }>{move ||if cancelling.get(){"Stopping download…"}else{"Cancel download"}}</button></Show></div>
        <p class="hint">"Default provisioning includes primary models and verifier language files; the Japanese specialist requires explicit selected install. ONNX import/export moves only that checksummed model artifact, not a complete offline verifier-data archive. Existing export destinations are never overwritten."</p>
        <p class="hint">"Model mutation is blocked during extraction. Integrity/download/import errors remain visible; installed status does not certify OCR quality or acceleration."</p>
        <crate::visual_retry::VisualPack ui/>
    </section>}
}
