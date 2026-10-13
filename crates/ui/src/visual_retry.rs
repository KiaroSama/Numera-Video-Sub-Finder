use crate::{helpers, ipc, state::Ui};
use leptos::prelude::*;
use serde::Deserialize;
use serde_json::json;
use wasm_bindgen_futures::spawn_local;

#[derive(Clone, Deserialize)]
struct Proposal {
    text: Option<String>,
    unreadable: bool,
    requires_review: bool,
    provider: String,
    warning: String,
}

#[component]
pub fn VisualPack(ui: Ui) -> impl IntoView {
    let consent = RwSignal::new(false);
    let active = RwSignal::new(false);
    let cancelling = RwSignal::new(false);
    view! {<section class="export-panel"><h3>"Optional local visual retry"</h3>
        <p class="hint">"Separate model + projector: 1,817,539,616 bytes (about 1.82 GB), plus the packaged native runtime. Not part of default OCR provisioning. Local CPU fallback profile; execution/quality is not qualified until validated. Nothing installs or runs automatically."</p>
        <label class="check"><input data-testid="visual-pack-opt-in" type="checkbox" prop:checked=move ||consent.get() disabled=move ||active.get() on:change=move |e|consent.set(event_target_checked(&e))/><span>"I explicitly approve downloading the optional 1.82 GB model pair."</span></label>
        <div class="button-row"><button data-testid="visual-pack-install" disabled=move ||!consent.get()||ui.busy.get()||ui.active() on:click=move |_|{
            if !consent.get_untracked()||ui.busy.get_untracked(){return;}ui.busy.set(true);active.set(true);cancelling.set(false);ui.error.set(None);
            spawn_local(async move{
                match ipc::call::<()>("install_visual_retry",json!({"enabled":true})).await{
                    Ok(())=>ui.notice.set("Optional visual model pair provisioned with integrity checks. Native runtime and transcription quality remain separately validated.".into()),Err(e)=>ui.error.set(Some(e))
                }
                active.set(false);cancelling.set(false);ui.busy.set(false);
            });
        }>"Install optional visual model pair"</button>
        <Show when=move ||active.get()><button data-testid="visual-pack-cancel" class="danger" disabled=move ||cancelling.get() on:click=move |_|{
            cancelling.set(true);spawn_local(async move{match ipc::call::<()>("cancel_visual_retry",json!({})).await{Ok(())=>ui.notice.set("Optional model cancellation requested; waiting for a safe stop.".into()),Err(e)=>{ui.error.set(Some(e));cancelling.set(false);}}});
        }>{move ||if cancelling.get(){"Stopping provisioning…"}else{"Cancel optional download"}}</button></Show></div>
    </section>}
}

#[component]
pub fn VisualCue(ui: Ui) -> impl IntoView {
    let enabled = RwSignal::new(false);
    let active = RwSignal::new(false);
    let cancelling = RwSignal::new(false);
    let proposal = RwSignal::new(None::<(u64, u64, Proposal)>);
    Effect::new(move |_| {
        ui.selected.get();
        enabled.set(false);
        proposal.set(None);
    });
    view! {<details><summary>"Optional local visual crop retry"</summary>
        <p class="hint">"Uses only the original selected cue crop on this device. No translation, timestamps, tools or cloud service. Model/runtime must already be explicitly provisioned. Budget: 256 output tokens, 4096 context, 1,003,520 crop pixels, 120 s wall / 60 s idle. A proposal is not accepted dialogue."</p>
        <label class="check"><input type="checkbox" data-testid="visual-cue-opt-in" disabled=move ||active.get() prop:checked=move ||enabled.get() on:change=move |e|enabled.set(event_target_checked(&e))/><span>"Enable a local visual proposal for this cue"</span></label>
        <div class="button-row"><button data-testid="visual-cue-run" disabled=move ||!enabled.get()||ui.busy.get()||ui.active()||ui.draft_dirty.get()||ui.source.with(Option::is_none) on:click=move |_|{
            if !enabled.get_untracked()||ui.busy.get_untracked(){return;}
            let Some(source)=ui.source.get_untracked()else{return;};let Some(cue)=ui.selected_cue()else{return;};
            let region=ui.project.with_untracked(|p|p.as_ref().and_then(|p|p.config.regions.get(cue.track as usize).copied()));
            let Some(region)=region else{ui.error.set(Some("Cue track has no original crop region".into()));return;};
            let revision=ui.revision.get_untracked();ui.busy.set(true);active.set(true);cancelling.set(false);proposal.set(None);ui.error.set(None);
            spawn_local(async move{
                match ipc::call::<Proposal>("propose_visual_retry",json!({"token":source.token,"cueId":cue.id,"atMs":cue.start_ms+(cue.end_ms-cue.start_ms)/2,"region":region,"enabled":true})).await{
                    Ok(result)=>proposal.set(Some((cue.id,revision,result))),Err(e)=>ui.error.set(Some(e))
                }
                active.set(false);cancelling.set(false);ui.busy.set(false);
            });
        }>"Propose text from original crop"</button>
        <Show when=move ||active.get()><button data-testid="visual-cue-cancel" class="danger" disabled=move ||cancelling.get() on:click=move |_|{
            cancelling.set(true);spawn_local(async move{match ipc::call::<()>("cancel_visual_retry",json!({})).await{Ok(())=>ui.notice.set("Visual retry cancellation requested; existing cue is unchanged.".into()),Err(e)=>{ui.error.set(Some(e));cancelling.set(false);}}});
        }>{move ||if cancelling.get(){"Stopping retry…"}else{"Cancel crop retry"}}</button></Show></div>
        <Show when=move ||proposal.with(Option::is_some)><section class="message notice"><div><h3>"Review-only visual proposal"</h3>
            <p class="hint">{move ||proposal.with(|p|p.as_ref().map(|(_,_,p)|format!("Provider: {} · {}",p.provider,p.warning)).unwrap_or_default())}</p>
            <p dir="auto" data-testid="visual-proposal-text">{move ||proposal.with(|p|p.as_ref().map(|(_,_,p)|if p.unreadable{"Unreadable — current cue retained".into()}else{p.text.clone().unwrap_or_else(||"No readable proposal returned".into())}).unwrap_or_default())}</p>
            <p class="hint">"Replacing text also replaces user corrections in this cue. Raw OCR, source timing, crop and confidence stay intact. Undo restores the previous revision."</p>
            <div class="button-row"><button data-testid="visual-proposal-accept" class="primary" disabled=move ||ui.busy.get()||ui.draft_dirty.get()||proposal.with(|p|p.as_ref().is_none_or(|(id,revision,p)|Some(*id)!=ui.selected.get()||*revision!=ui.revision.get()||p.unreadable||!p.requires_review||p.text.as_ref().is_none_or(|text|text.trim().is_empty()))) on:click=move |_|{
                if let Some((id,revision,result))=proposal.get_untracked(){if revision!=ui.revision.get_untracked()||Some(id)!=ui.selected.get_untracked()||result.unreadable||!result.requires_review{return;}
                    if let Some(text)=result.text{ui.edit(move |cues|helpers::replace_proposal_text(cues,id,text));proposal.set(None);}
                }
            }>"Replace cue text after review"</button><button data-testid="visual-proposal-reject" on:click=move |_|proposal.set(None)>"Keep current cue"</button></div></div>
        </section></Show>
    </details>}
}
