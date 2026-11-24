use crate::{helpers, player::Preview, roi_editor::Regions, state::Ui};
use leptos::ev;
use leptos::prelude::*;
use nvsf_core::Rect;
use serde_json::json;

#[component]
pub fn Extract(ui: Ui) -> impl IntoView {
    let lang_query = RwSignal::new(String::new());
    let split = NodeRef::<leptos::html::Div>::new();
    let width = RwSignal::new(340i32);
    let drag = RwSignal::new(None::<(i32, i32)>);
    let max_width = move || {
        split
            .get_untracked()
            .map_or(620, |el| (el.client_width() - 486).clamp(260, 620))
    };
    let movement = window_event_listener(ev::mousemove, move |event| {
        if let Some((x, initial)) = drag.get_untracked() {
            width.set((initial + x - event.client_x()).clamp(260, max_width()));
        }
    });
    let release = window_event_listener(ev::mouseup, move |_| drag.set(None));
    let blur = window_event_listener(ev::blur, move |_| drag.set(None));
    on_cleanup(move || {
        movement.remove();
        release.remove();
        blur.remove();
    });
    view! {
        <div class="extract-grid" node_ref=split style=move ||format!("--inspector-w:{}px",width.get())>
            <section class="panel source-panel" aria-label="Video stage">
                <div class="source-line">
                    <strong dir="auto" title=move ||ui.source.with(|s|s.as_ref().map(|s|s.name.clone()).unwrap_or_default())>{move ||ui.source.with(|s|s.as_ref().map(|s|s.name.clone()).unwrap_or_else(||"No video selected".into()))}</strong>
                    <span class="muted mono">{move ||ui.source.with(|s|s.as_ref().map(|s|format!("{} × {} · {}",s.media.width,s.media.height,s.media.codec)).unwrap_or_default())}</span>
                </div>
                <Preview ui editor=true/>
            </section>
            <div id="inspector-splitter" class="inspector-splitter" role="separator" aria-label="Resize inspector" tabindex="0"
                aria-orientation="vertical" aria-controls="extract-inspector" aria-valuemin="260" aria-valuemax="620"
                aria-valuenow=move ||width.get().to_string()
                on:mousedown=move |event|if event.button()==0 {event.prevent_default();drag.set(Some((event.client_x(),width.get_untracked().min(max_width()))));}
                on:keydown=move |event| {
                    let next=match event.key().as_str() {
                        "ArrowLeft"=>width.get_untracked()+24,"ArrowRight"=>width.get_untracked()-24,
                        "Home"=>260,"End"=>max_width(),_=>return,
                    };
                    event.prevent_default();width.set(next.clamp(260,max_width()));
                }></div>
            <section id="extract-inspector" class="panel settings-panel" aria-label="Extraction inspector">
                <div class="panel-heading"><h2>"Inspector"</h2></div>
                <div class="inspector-section">
                <label for="language-search">"Subtitle language"</label>
                <div class="language-picker">
                    <div class="language-search-row">
                        <input
                            id="language-search"
                            type="search"
                            autocomplete="off"
                            placeholder="Search language by name or code…"
                            aria-label="Search subtitle language"
                            prop:value=move || lang_query.get()
                            on:input=move |e| lang_query.set(event_target_value(&e))
                            on:keydown=move |e: web_sys::KeyboardEvent| {
                                if e.is_composing() || e.key_code() == 229 {return;}
                                if e.key() == "Escape" && !lang_query.get_untracked().is_empty() {
                                    e.prevent_default();
                                    lang_query.set(String::new());
                                } else if e.key() == "Enter" {
                                    let matches = helpers::language_options(&lang_query.get_untracked());
                                    if let Some(first) = matches.first() {
                                        ui.config.update(|c| c.language = first.code.clone());
                                        lang_query.set(String::new());
                                        e.prevent_default();
                                    }
                                }
                            }
                        />
                        <Show when=move || !lang_query.get().is_empty()>
                            <button
                                type="button"
                                class="clear-search"
                                aria-label="Clear language search"
                                on:click=move |_| lang_query.set(String::new())
                            >"×"</button>
                        </Show>
                    </div>
                    <Show when=move || !lang_query.get().trim().is_empty()>
                        <div class="language-matches" role="listbox" aria-label="Matching subtitle languages">
                            {move || {
                                let matches = helpers::language_options(&lang_query.get());
                                if matches.is_empty() {
                                    view! { <div class="language-empty muted">"No matching language"</div> }.into_any()
                                } else {
                                    matches
                                        .into_iter()
                                        .take(30)
                                        .map(|lang| {
                                            let code = lang.code.clone();
                                            let active_code = lang.code.clone();
                                            view! {
                                                <button
                                                    type="button"
                                                    role="option"
                                                    class="language-option"
                                                    class:active=move || ui.config.with(|c| c.language == active_code)
                                                    on:click=move |_| {
                                                        ui.config.update(|c| c.language = code.clone());
                                                        lang_query.set(String::new());
                                                    }
                                                >{lang.label}</button>
                                            }
                                        })
                                        .collect_view()
                                        .into_any()
                                }
                            }}
                        </div>
                    </Show>
                    <select id="language" aria-label="Selected subtitle language" prop:value=move || ui.config.with(|c| c.language.clone()) on:change=move |e| ui.config.update(|c| c.language = event_target_value(&e))>
                        {move || {
                            helpers::language_options(&lang_query.get())
                                .into_iter()
                                .map(|language| view! { <option value=language.code>{language.label}</option> })
                                .collect_view()
                        }}
                    </select>
                </div>
                <label for="device">"Processing device"</label>
                <select id="device" prop:value=move || ui.config.with(|c| c.device.clone()) on:change=move |e| ui.config.update(|c| c.device=event_target_value(&e))>
                    <option value="auto">"Auto · acceleration first"</option><option value="cpu">"CPU"</option><option value="cuda">"CUDA"</option>
                    <option value="directml">"DirectML"</option>
                </select>
                <label for="quality">"Sampling quality"</label>
                <select id="quality" prop:value=move || ui.config.with(|c| c.quality.clone()) on:change=move |e| ui.config.update(|c| c.quality=event_target_value(&e))>
                    <option value="accurate">"Accurate · every frame"</option><option value="balanced">"Balanced"</option><option value="fast">"Fast · reduced coverage"</option>
                </select>
                </div>
                <div class="inspector-section">
                    <Regions ui/>
                    <button class="wide" disabled=move ||ui.busy.get()||ui.closing.get()||ui.active()||ui.source.with(Option::is_none) on:click=move |_|{
                        if let Some(source)=ui.source.get_untracked(){ui.request::<Vec<Rect>>("suggest_regions",json!({"token":source.token,"language":ui.config.with_untracked(|c|c.language.clone())}),move |regions|{
                            if !regions.is_empty() {
                                ui.studio.roi_drag.set(None);ui.studio.roi.set(None);
                                ui.edit_extract(|c|c.regions=regions);
                                ui.notice.set("Region suggestions loaded. Check them before extracting.".into());
                            } else {ui.notice.set("No subtitle regions suggested.".into());}
                        });}
                    }>"Suggest subtitle regions"</button>
                </div>
                <details class="advanced"><summary>"Range and safety options"</summary>
                    <label for="range-start">"Start (milliseconds)"</label>
                    <input id="range-start" type="number" min="0" prop:value=move || ui.config.with(|c| c.start_ms.to_string()) on:change=move |e| match event_target_value(&e).parse() { Ok(v)=>ui.set_start(v),Err(_)=>ui.error.set(Some("Start must be a nonnegative integer".into())) }/>
                    <label for="range-end">"End (milliseconds, empty = entire video)"</label>
                    <input id="range-end" type="number" min="1" prop:value=move || ui.config.with(|c| c.end_ms.map(|v|v.to_string()).unwrap_or_default()) on:change=move |e| {
                        let value=event_target_value(&e); if value.is_empty(){ui.set_end(None);}else{match value.parse(){Ok(v)=>ui.set_end(Some(v)),Err(_)=>ui.error.set(Some("End must be a positive integer".into()))}}
                    }/>
                    <label class="check"><input type="checkbox" prop:checked=move || ui.config.with(|c|c.gpu_required) on:change=move |e|ui.config.update(|c|c.gpu_required=event_target_checked(&e))/><span>"Require proven GPU execution"</span></label>
                    <label class="check"><input type="checkbox" prop:checked=move || ui.config.with(|c|c.normalize_persian) on:change=move |e|ui.config.update(|c|c.normalize_persian=event_target_checked(&e))/><span>"Explicit Persian normalization"</span></label>
                    <label class="check"><input type="checkbox" prop:checked=move || ui.config.with(|c|c.overwrite) on:change=move |e|ui.config.update(|c|c.overwrite=event_target_checked(&e))/><span>"Allow replacing the selected output"</span></label>
                </details>
                <hr/><h3>"Output"</h3>
                <button class="wide" disabled=move || ui.busy.get() on:click={move |_|ui.request::<Option<crate::ipc::Destination>>("choose_output",json!({}),move |d|if let Some(d)=d{ui.output.set(Some(d));})}>"Choose SRT destination"</button>
                <p class="hint" dir="auto">{move ||ui.output.with(|d|d.as_ref().map(|d|d.name.clone()).unwrap_or_else(||"Choose where to save your subtitles.".into()))}</p>
                <div class="action-stack">
                    <button id="extract-start" class="primary wide" title="Extract subtitles (Ctrl+Enter)" disabled=move ||!ui.can_extract() on:click=move |_|ui.start_extraction()>"Extract subtitles"</button>
                    <button class="wide" disabled=move ||ui.busy.get()||ui.source.with(Option::is_none)||ui.output.with(Option::is_none) on:click=move |_|{
                        ui.add_queue();
                    }>"Add to queue"</button>
                </div>
            </section>
        </div>
    }
}
