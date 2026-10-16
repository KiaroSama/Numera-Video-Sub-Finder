use crate::{helpers, state::Ui};
use leptos::prelude::*;
use nvsf_core::Rect;
use serde_json::json;

#[component]
pub fn Extract(ui: Ui) -> impl IntoView {
    view! {
        <div class="extract-grid">
            <section class="panel source-panel">
                <div class="panel-heading"><div><span class="eyebrow">"01 / SOURCE"</span><h2>"Your video, your device"</h2></div>
                    <div class="button-row"><button on:click=move |_| ui.pick_source() disabled=move || ui.busy.get()>"Open video"</button>
                    <button disabled=move ||ui.busy.get()||ui.active()||ui.source.with(Option::is_none) on:click=move |_|{
                        if let Some(source)=ui.source.get_untracked(){ui.request::<Vec<Rect>>("suggest_regions",json!({"token":source.token,"language":ui.config.with_untracked(|c|c.language.clone())}),move |regions|{
                            ui.config.update(|c|c.regions=regions);ui.notice.set("Region proposals loaded. Inspect every proposal before extraction; scene text is not accepted automatically.".into());
                        });}
                    }>"Suggest subtitle regions"</button></div>
                </div>
                <div class="source-line">
                    <strong dir="auto">{move || ui.source.with(|s| s.as_ref().map(|s| s.name.clone()).unwrap_or_else(|| "No video selected".into()))}</strong>
                    <span class="muted">{move || ui.source.with(|s| s.as_ref().map(|s| format!("{} × {} · {} · {}", s.media.width, s.media.height, s.media.codec,
                        s.media.duration_ms.map(helpers::time_label).unwrap_or_else(|| "unknown duration".into()))).unwrap_or_else(|| "Choose a local video. Nothing is uploaded.".into()))}</span>
                </div>
                <Preview ui/>
                <Regions ui/>
            </section>
            <section class="panel settings-panel">
                <span class="eyebrow">"02 / EXTRACTION"</span><h2>"Recognition setup"</h2>
                <label for="language">"Subtitle language"</label>
                <select id="language" prop:value=move || ui.config.with(|c| c.language.clone()) on:change=move |e| ui.config.update(|c| c.language=event_target_value(&e))>
                    <option value="en">"English"</option><option value="fa">"Persian / فارسی"</option><option value="ja">"Japanese / 日本語"</option>
                    {nvsf_core::languages::catalog().iter().map(|language|view!{<option value=language.code.clone()>{format!("{} ({})",language.name,language.code)}</option>}).collect_view()}
                </select>
                <label for="device">"Processing device"</label>
                <select id="device" prop:value=move || ui.config.with(|c| c.device.clone()) on:change=move |e| ui.config.update(|c| c.device=event_target_value(&e))>
                    <option value="auto">"Auto · acceleration first"</option><option value="cpu">"CPU"</option><option value="cuda">"CUDA"</option>
                    <option value="directml">"DirectML"</option><option value="coreml">"Core ML"</option>
                </select>
                <p class="hint">"Availability is checked by the backend. Unverified acceleration is never reported as working."</p>
                <label for="quality">"Sampling quality"</label>
                <select id="quality" prop:value=move || ui.config.with(|c| c.quality.clone()) on:change=move |e| ui.config.update(|c| c.quality=event_target_value(&e))>
                    <option value="accurate">"Accurate · every presented frame"</option><option value="balanced">"Balanced"</option><option value="fast">"Fast · reduced coverage"</option>
                </select>
                <details class="advanced"><summary>"Range and safety options"</summary>
                    <label for="range-start">"Start (milliseconds)"</label>
                    <input id="range-start" type="number" min="0" prop:value=move || ui.config.with(|c| c.start_ms.to_string()) on:change=move |e| match event_target_value(&e).parse() { Ok(v)=>ui.config.update(|c|c.start_ms=v),Err(_)=>ui.error.set(Some("Start must be a nonnegative integer".into())) }/>
                    <label for="range-end">"End (milliseconds, empty = entire video)"</label>
                    <input id="range-end" type="number" min="1" prop:value=move || ui.config.with(|c| c.end_ms.map(|v|v.to_string()).unwrap_or_default()) on:change=move |e| {
                        let value=event_target_value(&e); if value.is_empty(){ui.config.update(|c|c.end_ms=None);}else{match value.parse(){Ok(v)=>ui.config.update(|c|c.end_ms=Some(v)),Err(_)=>ui.error.set(Some("End must be a positive integer".into()))}}
                    }/>
                    <label class="check"><input type="checkbox" prop:checked=move || ui.config.with(|c|c.gpu_required) on:change=move |e|ui.config.update(|c|c.gpu_required=event_target_checked(&e))/><span>"Require proven GPU execution"</span></label>
                    <label class="check"><input type="checkbox" prop:checked=move || ui.config.with(|c|c.normalize_persian) on:change=move |e|ui.config.update(|c|c.normalize_persian=event_target_checked(&e))/><span>"Explicit Persian normalization"</span></label>
                    <label class="check"><input type="checkbox" prop:checked=move || ui.config.with(|c|c.overwrite) on:change=move |e|ui.config.update(|c|c.overwrite=event_target_checked(&e))/><span>"Allow replacing the selected output"</span></label>
                </details>
                <hr/><span class="eyebrow">"03 / DESTINATION"</span>
                <button class="wide" disabled=move || ui.busy.get() on:click={move |_|ui.request::<Option<crate::ipc::Destination>>("choose_output",json!({}),move |d|if let Some(d)=d{ui.output.set(Some(d));})}>"Choose SRT destination"</button>
                <p class="hint" dir="auto">{move ||ui.output.with(|d|d.as_ref().map(|d|d.name.clone()).unwrap_or_else(||"Choose where to save your subtitles.".into()))}</p>
                <div class="action-stack">
                    <button id="extract-start" class="primary wide" disabled=move ||ui.busy.get()||ui.active()||ui.source.with(Option::is_none)||ui.output.with(Option::is_none) on:click=move |_|if let Some(q)=ui.current_queue(){ui.begin(q);}>"Extract subtitles"</button>
                    <button class="wide" disabled=move ||ui.busy.get()||ui.source.with(Option::is_none)||ui.output.with(Option::is_none) on:click=move |_|{
                        ui.add_queue();
                    }>"Add to queue"</button>
                </div>
                <p class="hint">"Direct SRT plus a recoverable review project. Missing models and processing failures are shown, never replaced with sample results."</p>
            </section>
        </div>
    }
}

#[component]
pub fn Preview(ui: Ui) -> impl IntoView {
    let begin = RwSignal::new(None::<(f64, f64)>);
    let exclusion = RwSignal::new(false);
    let bounds = NodeRef::<leptos::html::Div>::new();
    let point = move |e: &web_sys::MouseEvent| {
        bounds.get().map(|element| {
            let r = element.get_bounding_client_rect();
            (
                ((f64::from(e.client_x()) - r.left()) / r.width()).clamp(0.0, 1.0),
                ((f64::from(e.client_y()) - r.top()) / r.height()).clamp(0.0, 1.0),
            )
        })
    };
    view! {
        <div class="preview-shell" aria-busy=move ||ui.preview_busy.get()>
            <Show when=move ||!ui.preview_url.get().is_empty() fallback=||view!{<div class="preview-empty"><svg viewBox="0 0 48 48" aria-hidden="true"><rect x="7" y="12" width="34" height="24" rx="3"/><path d="m20 18 10 6-10 6z"/></svg><strong>"Original frame preview"</strong><p>"Open a video to inspect subtitles and adjust regions."</p></div>}>
                <div class="frame" node_ref=bounds style=move ||ui.source.with(|s|s.as_ref().map(|s|format!("aspect-ratio:{} / {}",s.media.width,s.media.height)).unwrap_or_default())
                    on:mousedown=move |e|{if e.button()==0{begin.set(point(&e));}}
                    on:mouseup=move |e|{
                        if let (Some(a),Some(b))=(begin.get_untracked(),point(&e)){
                            match Rect::new(a.0.min(b.0),a.1.min(b.1),(a.0-b.0).abs(),(a.1-b.1).abs()){
                                Ok(r) if r.width>0.005&&r.height>0.005=>ui.config.update(|c|{
                                    if exclusion.get_untracked(){if c.exclusions.len()<32{c.exclusions.push(r);}}
                                    else if c.regions.len()<8{c.regions.push(r);}
                                }), _=>{}
                            }
                        }begin.set(None);
                    } on:mouseleave=move |_|begin.set(None)>
                    <img class="source-frame" src=move ||ui.preview_url.get() alt="Original decoded source frame; drag to add a region, or use normalized coordinates below" draggable="false"/>
                    {move ||ui.config.with(|c|c.regions.iter().copied().enumerate().map(|(i,r)|view!{<div class="roi" style=helpers::region_style(r)><span>{format!("R{}",i+1)}</span></div>}).collect_view())}
                    {move ||ui.config.with(|c|c.exclusions.iter().copied().enumerate().map(|(i,r)|view!{<div class="roi excluded" style=helpers::region_style(r)><span>{format!("Exclude {}",i+1)}</span></div>}).collect_view())}
                </div>
            </Show>
            <Show when=move ||ui.preview_busy.get()><span class="preview-loading" role="status">"Decoding source frame…"</span></Show>
        </div>
        <div class="scrubber">
            <button title="Previous 100 milliseconds" disabled=move ||ui.source.with(Option::is_none) on:click=move |_|ui.seek(ui.time.get_untracked().saturating_sub(100))>"−100 ms"</button>
            <input aria-label="Preview position in milliseconds" type="range" min="0" max=move ||ui.source.with(|s|s.as_ref().and_then(|s|s.media.duration_ms).unwrap_or(60_000).to_string()) step="1" prop:value=move ||ui.time.get().to_string()
                disabled=move ||ui.source.with(Option::is_none) on:input=move |e|if let Ok(t)=event_target_value(&e).parse(){ui.time.set(t);} on:change=move |_|ui.seek(ui.time.get_untracked())/>
            <button title="Next 100 milliseconds" disabled=move ||ui.source.with(Option::is_none) on:click=move |_|ui.seek(ui.time.get_untracked().saturating_add(100))>"+100 ms"</button>
            <output class="mono">{move ||helpers::time_label(ui.time.get())}</output>
        </div>
        <label class="check draw-mode"><input type="checkbox" prop:checked=move ||exclusion.get() on:change=move |e|exclusion.set(event_target_checked(&e))/><span>"Draw exclusion instead of subtitle region"</span></label>
    }
}

#[component]
fn Regions(ui: Ui) -> impl IntoView {
    let value = RwSignal::new("0,0.7,1,0.3".to_string());
    let exclude = RwSignal::new(false);
    view! {
        <details class="regions" open>
            <summary>"Subtitle regions and exclusions"</summary>
            <p class="hint">"Normalized x,y,width,height (0–1). The initial bottom region is a manual starting point, not a detected subtitle. Draw on the preview or edit coordinates."</p>
            <div class="region-list">{move ||ui.config.with(|c|c.regions.iter().copied().enumerate().map(|(i,r)|view!{
                <label class="region-row"><span>{format!("Region {}",i+1)}</span><input aria-label=format!("Region {} normalized coordinates",i+1) prop:value=format!("{:.4},{:.4},{:.4},{:.4}",r.x,r.y,r.width,r.height)
                    on:change={move |e|match event_target_value(&e).parse::<Rect>(){Ok(r)=>ui.config.update(|c|if let Some(old)=c.regions.get_mut(i){*old=r;}),Err(e)=>ui.error.set(Some(e.to_string()))}}/>
                    <button aria-label=format!("Remove region {}",i+1) on:click=move |_|ui.config.update(|c|if c.regions.len()>1&&i<c.regions.len(){c.regions.remove(i);})>"Remove"</button></label>
            }).collect_view())}</div>
            <div class="region-list">{move ||ui.config.with(|c|c.exclusions.iter().copied().enumerate().map(|(i,r)|view!{
                <label class="region-row"><span>{format!("Exclude {}",i+1)}</span><input aria-label=format!("Exclusion {} normalized coordinates",i+1) prop:value=format!("{:.4},{:.4},{:.4},{:.4}",r.x,r.y,r.width,r.height)
                    on:change={move |e|match event_target_value(&e).parse::<Rect>(){Ok(r)=>ui.config.update(|c|if let Some(old)=c.exclusions.get_mut(i){*old=r;}),Err(e)=>ui.error.set(Some(e.to_string()))}}/>
                    <button aria-label=format!("Remove exclusion {}",i+1) on:click=move |_|ui.config.update(|c|if i<c.exclusions.len(){c.exclusions.remove(i);})>"Remove"</button></label>
            }).collect_view())}</div>
            <div class="region-add"><label for="new-region" class="sr-only">"New normalized region coordinates"</label>
                <input id="new-region" prop:value=move ||value.get() on:input=move |e|value.set(event_target_value(&e))/>
                <label class="check"><input type="checkbox" on:change=move |e|exclude.set(event_target_checked(&e))/><span>"Exclude"</span></label>
                <button on:click={move |_|match value.get_untracked().parse::<Rect>(){
                    Ok(r)=>ui.config.update(|c|{let regions=if exclude.get_untracked(){&mut c.exclusions}else{&mut c.regions};let max=if exclude.get_untracked(){32}else{8};if regions.len()<max{regions.push(r);}else{ui.error.set(Some("Region limit reached (8 regions / 32 exclusions)".into()));}}),Err(e)=>ui.error.set(Some(e.to_string()))
                }}>"Add region"</button>
            </div>
        </details>
    }
}
