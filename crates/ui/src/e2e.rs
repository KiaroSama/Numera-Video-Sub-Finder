//! Only included when CI builds the frontend with --features e2e.
use crate::{
    ipc::{Destination, Source},
    state::Ui,
};
use leptos::prelude::*;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct Grant {
    source: Source,
    output: Destination,
}

#[component]
pub fn FixtureControls(ui: Ui) -> impl IntoView {
    view! {<section class="panel e2e-controls">
        <h2>"CI fixture admission (test artifact only)"</h2>
        <button id="e2e-load" data-testid="e2e-load-fixture" on:click=move |_|ui.request::<Grant>("e2e_fixture",json!({"fixture":"en.mp4","output":"ui-en.srt"}),move |grant|{
            ui.source.set(Some(grant.source));ui.output.set(Some(grant.output));ui.seek(0);
        }) disabled=move ||ui.busy.get()>"Admit English CI fixture"</button>
        <button data-testid="e2e-save-edits" disabled=move ||ui.busy.get()||ui.project.with(Option::is_none)||ui.draft_dirty.get() on:click=move |_|{
            let Some(project)=ui.project.get_untracked()else{return;};let Some(output)=ui.output.get_untracked()else{return;};
            if ui.busy.get_untracked(){return;}ui.busy.set(true);
            wasm_bindgen_futures::spawn_local(async move{
                let result=async{
                    crate::ipc::call::<()>("replace_project",json!({"project":project})).await?;
                    crate::ipc::call::<String>("e2e_save_edits",json!({"outputToken":output.token})).await
                }.await;
                match result{Ok(name)=>{ui.notice.set(format!("Saved {name}"));ui.dirty.set(false);},Err(e)=>ui.error.set(Some(e))}ui.busy.set(false);
            });
        }>"Save edited SRT to CI fixture destination"</button>
    </section>}
}
