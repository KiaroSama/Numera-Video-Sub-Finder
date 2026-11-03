use crate::{helpers, ipc, state::Ui};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

#[component]
pub fn Evidence(ui: Ui) -> impl IntoView {
    let original = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let loading = RwSignal::new(false);
    let generation = RwSignal::new(0u64);
    let clear = move || {
        generation.update(|n| *n += 1);
        let old = original.get_untracked();
        original.set(String::new());
        if !old.is_empty() {
            let _ = web_sys::Url::revoke_object_url(&old);
        }
        error.set(None);
        loading.set(false);
    };
    Effect::new(move |_| {
        let _ = ui.selected.get();
        let _ = ui
            .project
            .with(|p| p.as_ref().map(|p| p.source_hash.clone()));
        clear();
    });
    on_cleanup(move || {
        let old = original.get_untracked();
        if !old.is_empty() {
            let _ = web_sys::Url::revoke_object_url(&old);
        }
    });
    let region = move || {
        ui.selected_cue().and_then(|cue| {
            ui.project
                .with(|p| p.as_ref()?.config.regions.get(cue.track as usize).copied())
        })
    };
    view! {
        <details class="crop-evidence">
            <summary>"Compare original and selected-frame crops"</summary>
            <p class="hint">"The original is the retained extraction crop. The candidate is an unmasked region of the currently selected source frame; changing time never rewrites evidence or cue text."</p>
            <button data-testid="load-cue-evidence" disabled=move ||loading.get()||ui.busy.get()||ui.closing.get()||ui.active()||ui.preview_busy.get() on:click=move |_|{
                let Some(id)=ui.selected.get_untracked()else{return;};
                loading.set(true);error.set(None);
                let request=generation.get_untracked();
                spawn_local(async move {
                    let result=ipc::cue_evidence(id).await;
                    if original.is_disposed() || generation.get_untracked()!=request || ui.selected.get_untracked()!=Some(id){
                        if let Ok(url)=result{let _=web_sys::Url::revoke_object_url(&url);}return;
                    }
                    match result {
                        Ok(url)=>{let old=original.get_untracked();original.set(url);if !old.is_empty(){let _=web_sys::Url::revoke_object_url(&old);}}
                        Err(message)=>error.set(Some(message)),
                    }
                    loading.set(false);
                });
            }>"Load original crop"</button>
            <Show when=move ||loading.get()><p role="status">"Loading retained evidence…"</p></Show>
            <Show when=move ||error.with(Option::is_some)><p class="hint" role="status">{move ||error.get().unwrap_or_default()}</p></Show>
            <div class="crop-comparison">
                <figure><figcaption>"Original extraction crop"</figcaption>
                    <Show when=move ||!original.get().is_empty() fallback=||view!{<p class="hint">"Load the retained crop. A missing or relocated cache is reported, not replaced with a different image."</p>}>
                        <img class="original-crop" src=move ||original.get() alt="Content-verified original crop retained during extraction"/>
                    </Show>
                </figure>
                <figure><figcaption>{move ||format!("Candidate source region · {}",helpers::time_label(ui.time.get()))}</figcaption>
                    <Show when=move ||region().is_some()&&!ui.preview_url.get().is_empty() fallback=||view!{<p class="hint">"Link the original video and seek to a frame to compare."</p>}>
                        <div class="candidate-crop" style=move ||region().and_then(|r|ui.source.with(|s|s.as_ref().map(|s|format!("aspect-ratio:{} / {}",r.width*f64::from(s.media.width),r.height*f64::from(s.media.height))))).unwrap_or_default()>
                            <img src=move ||ui.preview_url.get() alt="Unmasked candidate region from the selected source frame" style=move ||region().map(|r|format!("width:{}%;left:{}%;top:{}%",100.0/r.width,-100.0*r.x/r.width,-100.0*r.y/r.height)).unwrap_or_default()/>
                        </div>
                    </Show>
                </figure>
            </div>
        </details>
    }
}
