use crate::{extract::Preview, helpers, state::Ui};
use leptos::prelude::*;
use nvsf_core::{Error, merge_cues, split_cue};
use serde_json::json;

#[component]
pub fn Review(ui: Ui) -> impl IntoView {
    let query = RwSignal::new(String::new());
    let filter = RwSignal::new("all".to_string());
    let scroll = RwSignal::new(0usize);
    let indices = Memo::new(move |_| {
        let query = query.get().to_lowercase();
        let filter = filter.get();
        ui.project.with(|p| {
            p.as_ref()
                .map(|p| {
                    p.cues
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| helpers::matches(c, &query, &filter))
                        .map(|(i, _)| i)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });
    let rows = move || {
        let range = indices.with(|i| helpers::window_range(i.len(), scroll.get()));
        let visible = indices.with(|i| i[range].to_vec());
        ui.project.with(|p| {
            visible
                .iter()
                .filter_map(|i| p.as_ref()?.cues.get(*i).cloned())
                .collect::<Vec<_>>()
        })
    };
    view! {
        <Show when=move ||ui.project.with(Option::is_some) fallback=move ||view!{
            <section class="panel empty-state"><h2>"No review project open"</h2><p>"Extract a video or open a saved project. Original OCR and corrected text stay separate."</p>
                <button class="primary" on:click={move |_|ui.request::<Option<nvsf_core::Project>>("pick_project",json!({}),move |p|if let Some(p)=p{ui.load(p);})} disabled=move ||ui.busy.get()>"Open project"</button></section>
        }>
            <div class="review-toolbar panel">
                <label class="search"><span class="sr-only">"Search cue text"</span><input type="search" placeholder="Search subtitles…" prop:value=move ||query.get() on:input=move |e|{query.set(event_target_value(&e));scroll.set(0);}/></label>
                <label class="filter"><span class="sr-only">"Filter cues"</span><select prop:value=move ||filter.get() on:change=move |e|{filter.set(event_target_value(&e));scroll.set(0);}>
                    <option value="all">"All cues"</option><option value="unreviewed">"Needs review"</option><option value="uncertain">"Uncertain / unscored"</option><option value="edited">"Edited"</option>
                </select></label>
                <span class="muted">{move ||format!("{} matches",indices.with(Vec::len))}</span>
                <button disabled=move ||{ui.revision.get();!ui.history.with_value(|h|h.can_undo())} on:click=move |_|ui.travel(false)>"Undo"</button>
                <button disabled=move ||{ui.revision.get();!ui.history.with_value(|h|h.can_redo())} on:click=move |_|ui.travel(true)>"Redo"</button>
                <button on:click=move |_|ui.persist(None,false,false) disabled=move ||ui.busy.get()||ui.preview_busy.get()>"Save project"</button>
                <span class="edit-state">{move ||if ui.dirty.get(){"Unsaved corrections"}else{"Project loaded"}}</span>
            </div>
            <div class="review-grid">
                <section class="panel cue-panel" aria-label="Subtitle cue list">
                    <div class="cue-heading"><h2>"Subtitle cues"</h2><span class="muted">"Text / source time"</span></div>
                    <div class="cue-list" tabindex="0" on:scroll=move |e|{
                        use wasm_bindgen::JsCast;
                        if let Some(target)=e.target().and_then(|t|t.dyn_into::<web_sys::HtmlElement>().ok()){scroll.set(target.scroll_top().max(0) as usize);}
                    }>
                        <div style=move ||format!("height:{}px",indices.with(|i|helpers::window_range(i.len(),scroll.get()).start)*helpers::ROW_HEIGHT) aria-hidden="true"></div>
                        <For each=rows key=|cue| (cue.id,cue.text.clone(),cue.reviewed,cue.start_ms,cue.end_ms,cue.confidence.map(f32::to_bits)) children=move |cue|{
                            let id=cue.id; let start=cue.start_ms;
                            view!{<button class="cue-row" disabled=move ||ui.busy.get()||ui.closing.get() class:selected=move ||ui.selected.get()==Some(id) aria-pressed=move ||ui.selected.get()==Some(id)
                                on:click=move |_|{if ui.draft_dirty.get_untracked(){ui.error.set(Some("Apply or discard your current cue draft before selecting another cue".into()));}else{ui.selected.set(Some(id));ui.seek(start);}}>
                                <span class="cue-number mono">{format!("{:04}",id)}</span>
                                <span class="cue-content"><span class="cue-text" dir="auto">{cue.text}</span><span class="cue-meta mono">{format!("{} → {} · Track {}",helpers::time_label(start),helpers::time_label(cue.end_ms),cue.track)}</span></span>
                                <span class="cue-tag">{if cue.reviewed{"Reviewed"}else if cue.confidence.is_none_or(|c|c<0.7){"Check"}else{"New"}}</span>
                            </button>}
                        }/>
                        <div style=move ||format!("height:{}px",indices.with(|i|i.len()-helpers::window_range(i.len(),scroll.get()).end)*helpers::ROW_HEIGHT) aria-hidden="true"></div>
                        <Show when=move ||indices.with(Vec::is_empty)><p class="empty-list">"No cues match this filter."</p></Show>
                    </div>
                    <div class="cue-navigation">
                        <button disabled=move ||ui.busy.get()||ui.closing.get() on:click=move |_|select_relative(ui,-1)>"Previous cue"</button><button disabled=move ||ui.busy.get()||ui.closing.get() on:click=move |_|select_relative(ui,1)>"Next cue"</button>
                    </div>
                </section>
                <section class="panel editor-panel">
                    <div class="panel-heading"><h2>"Review against source"</h2><button on:click=move |_|ui.pick_source() disabled=move ||ui.busy.get()>"Link video"</button></div>
                    <p class="hint">"Saved projects do not grant source-file access. Link the original video explicitly for preview."</p>
                    <Preview ui/>
                    <CueEditor ui/>
                    <Export ui/>
                </section>
            </div>
        </Show>
    }
}

fn select_relative(ui: Ui, delta: isize) {
    if ui.busy.get_untracked() || ui.closing.get_untracked() {
        return;
    }
    if ui.draft_dirty.get_untracked() {
        ui.error.set(Some(
            "Apply or discard the current draft before changing cues".into(),
        ));
        return;
    }
    let next = ui.project.with(|p| {
        let p = p.as_ref()?;
        let current = p
            .cues
            .iter()
            .position(|c| Some(c.id) == ui.selected.get_untracked())
            .unwrap_or(0);
        let index = current
            .saturating_add_signed(delta)
            .min(p.cues.len().saturating_sub(1));
        p.cues.get(index).map(|c| (c.id, c.start_ms))
    });
    if let Some((id, time)) = next {
        ui.selected.set(Some(id));
        ui.seek(time);
    }
}

#[component]
fn CueEditor(ui: Ui) -> impl IntoView {
    let text = RwSignal::new(String::new());
    let start = RwSignal::new(String::new());
    let end = RwSignal::new(String::new());
    let split = RwSignal::new(String::new());
    let proposal = RwSignal::new(None::<(u64, nvsf_core::Recognition)>);
    Effect::new(move |_| {
        ui.revision.get();
        if let Some(c) = ui.selected_cue() {
            text.set(c.text);
            start.set(helpers::time_label(c.start_ms));
            end.set(helpers::time_label(c.end_ms));
            split.set(helpers::time_label(
                c.start_ms + (c.end_ms - c.start_ms) / 2,
            ));
        }
    });
    let apply = move || {
        let Some(id) = ui.selected.get_untracked() else {
            return;
        };
        let values = (
            helpers::parse_time(&start.get_untracked()),
            helpers::parse_time(&end.get_untracked()),
        );
        match values {
            (Ok(a), Ok(b)) => {
                let value = text.get_untracked();
                ui.edit(move |cues| {
                    let cue = cues
                        .iter_mut()
                        .find(|c| c.id == id)
                        .ok_or_else(|| Error::Invalid("Cue no longer exists".into()))?;
                    cue.text = value;
                    cue.start_ms = a;
                    cue.end_ms = b;
                    cue.edited = true;
                    cue.validate()
                });
            }
            (Err(e), _) | (_, Err(e)) => ui.error.set(Some(e)),
        }
    };
    view! {
        <Show when=move ||ui.selected_cue().is_some() fallback=||view!{<p class="hint">"Select a cue to review its text and timing."</p>}>
            <div class="editor-heading"><h3>{move ||ui.selected.get().map(|id|format!("Cue {id}")).unwrap_or_default()}</h3>
                <span class="muted">{move ||ui.selected_cue().and_then(|c|c.confidence).map(|c|format!("Model score {c:.3}")).unwrap_or_else(||"Confidence not reported".into())}</span></div>
            <label for="cue-text">"Subtitle text (logical Unicode)"</label>
            <textarea id="cue-text" dir="auto" rows="4" spellcheck="false" prop:value=move ||text.get() on:input=move |e|{if !ui.closing.get_untracked(){text.set(event_target_value(&e));ui.draft_dirty.set(true);}}></textarea>
            <div class="time-fields"><label for="cue-start">"Start"<input id="cue-start" class="mono" prop:value=move ||start.get() on:input=move |e|{if !ui.closing.get_untracked(){start.set(event_target_value(&e));ui.draft_dirty.set(true);}}/></label>
                <label for="cue-end">"End"<input id="cue-end" class="mono" prop:value=move ||end.get() on:input=move |e|{if !ui.closing.get_untracked(){end.set(event_target_value(&e));ui.draft_dirty.set(true);}}/></label></div>
            <div class="button-row"><button id="cue-apply" class="primary" on:click=move |_|apply()>"Apply correction"</button>
                <button disabled=move ||!ui.draft_dirty.get() on:click=move |_|{ui.draft_dirty.set(false);ui.revision.update(|v|*v+=1);}>"Discard draft"</button>
                <button disabled=move ||ui.draft_dirty.get() on:click=move |_|if let Some(id)=ui.selected.get_untracked(){ui.edit(move |cues|{let cue=cues.iter_mut().find(|c|c.id==id).ok_or_else(||Error::Invalid("Cue not found".into()))?;cue.reviewed=true;Ok(())});}>"Accept cue"</button>
                <button disabled=move ||ui.draft_dirty.get() on:click=move |_|if let Some(id)=ui.selected.get_untracked(){ui.edit(move |cues|{let cue=cues.iter_mut().find(|c|c.id==id).ok_or_else(||Error::Invalid("Cue not found".into()))?;cue.text=cue.raw_text.clone();cue.edited=false;cue.reviewed=false;Ok(())});}>"Revert to OCR"</button>
            </div>
            <div class="button-row"><button disabled=move ||ui.busy.get()||ui.active()||ui.source.with(Option::is_none) on:click=move |_|{
                let Some(source)=ui.source.get_untracked()else{return;};let Some(cue)=ui.selected_cue()else{return;};
                let setup=ui.project.with_untracked(|p|p.as_ref().map(|p|(p.config.language.clone(),p.config.regions.get(cue.track as usize).copied())));
                if let Some((language,Some(region)))=setup{ui.request::<nvsf_core::Recognition>("recognize_cue",json!({"token":source.token,"cueId":cue.id,"atMs":cue.start_ms+(cue.end_ms-cue.start_ms)/2,"language":language,"region":region}),move |result|proposal.set(Some((cue.id,result))));}
                else{ui.error.set(Some("Cue track has no valid region".into()));}
            }>"Re-recognize original crop"</button></div>
            <Show when=move ||proposal.with(Option::is_some)><section class="message notice"><div><strong>"Review replacement proposal"</strong><p dir="auto">{move ||proposal.with(|p|p.as_ref().map(|(_,r)|r.text.clone()).unwrap_or_default())}</p>
                <p class="hint">"This replaces the selected cue's text, including user corrections, only when you explicitly accept. Undo restores the previous version."</p>
                <div class="button-row"><button class="primary" disabled=move ||ui.draft_dirty.get()||proposal.with(|p|p.as_ref().is_some_and(|(id,_)|Some(*id)!=ui.selected.get())) on:click=move |_|if let Some((id,r))=proposal.get_untracked(){ui.edit(move |cues|{let cue=cues.iter_mut().find(|c|c.id==id).ok_or_else(||Error::Invalid("Cue no longer exists".into()))?;cue.text=r.text;cue.confidence=r.confidence;cue.edited=true;cue.reviewed=false;cue.validate()});proposal.set(None);}>"Replace cue text"</button>
                    <button on:click=move |_|proposal.set(None)>"Keep existing text"</button></div></div></section></Show>
            <crate::visual_retry::VisualCue ui/>
            <details><summary>"Original OCR and structural edits"</summary>
                <p class="raw-text" dir="auto">{move ||ui.selected_cue().map(|c|c.raw_text).unwrap_or_default()}</p>
                <label for="split-time">"Split at source time"</label><div class="button-row"><input id="split-time" class="mono" prop:value=move ||split.get() on:input=move |e|split.set(event_target_value(&e))/>
                    <button disabled=move ||ui.draft_dirty.get() on:click=move |_|if let Some(id)=ui.selected.get_untracked(){match helpers::parse_time(&split.get_untracked()){Ok(at)=>ui.edit(move |c|split_cue(c,id,at)),Err(e)=>ui.error.set(Some(e))}}>"Split cue"</button></div>
                <button disabled=move ||ui.draft_dirty.get() on:click=move |_|{
                    let pair=ui.project.with(|p|{let p=p.as_ref()?;let i=p.cues.iter().position(|c|Some(c.id)==ui.selected.get_untracked())?;Some((p.cues[i].id,p.cues.get(i+1)?.id))});
                    if let Some((a,b))=pair{ui.edit(move |c|merge_cues(c,a,b));}else{ui.error.set(Some("No following cue to merge".into()));}
                }>"Merge with next cue (same track)"</button>
            </details>
        </Show>
    }
}

#[component]
fn Export(ui: Ui) -> impl IntoView {
    let format = RwSignal::new("srt".to_string());
    let bom = RwSignal::new(false);
    let crlf = RwSignal::new(false);
    view! {<div class="export-panel"><h3>"Export corrections"</h3><div class="button-row">
    <label for="export-format" class="sr-only">"Export format"</label><select id="export-format" on:change=move |e|format.set(event_target_value(&e))><option value="srt">"SRT"</option><option value="vtt">"WebVTT"</option><option value="ass">"ASS"</option><option value="json">"JSON evidence"</option></select>
    <button class="primary" disabled=move ||ui.busy.get()||ui.preview_busy.get() on:click=move |_|ui.persist(Some(format.get_untracked()),bom.get_untracked(),crlf.get_untracked())>"Export…"</button></div>
    <div class="button-row"><label class="check"><input type="checkbox" on:change=move |e|bom.set(event_target_checked(&e))/><span>"UTF-8 BOM"</span></label><label class="check"><input type="checkbox" on:change=move |e|crlf.set(event_target_checked(&e))/><span>"CRLF line endings"</span></label></div>
    <p class="hint">"Export uses a native save dialog and confirms existing-file replacement. Unsupported formats return the backend error."</p></div>}
}
