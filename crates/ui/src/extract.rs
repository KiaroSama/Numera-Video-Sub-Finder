use crate::{
    helpers,
    language_search::{KeyAction, LanguageSearch, current_language_option},
    player::Preview,
    roi_editor::Regions,
    state::Ui,
};
use leptos::ev;
use leptos::prelude::*;
use nvsf_core::Rect;
use serde_json::json;
use wasm_bindgen::JsCast;

#[component]
pub fn Extract(ui: Ui) -> impl IntoView {
    let language_search = RwSignal::new(LanguageSearch::default());
    let language_matches = Memo::new(move |_| language_search.with(LanguageSearch::options));
    let language_input = NodeRef::<leptos::html::Input>::new();
    Effect::new(move |_| {
        let Some(code) = language_search.with(LanguageSearch::active_code) else {
            return;
        };
        let document = document();
        if let (Some(list), Some(option)) = (
            document.get_element_by_id("language-results"),
            document.get_element_by_id(&format!("language-option-{code}")),
        ) {
            let bounds = list.get_bounding_client_rect();
            let item = option.get_bounding_client_rect();
            let top = bounds.top() + f64::from(list.client_top());
            let bottom = top + f64::from(list.client_height());
            let delta = if item.top() < top {
                item.top() - top
            } else {
                (item.bottom() - bottom).max(0.0)
            };
            list.set_scroll_top(list.scroll_top() + delta.round() as i32);
        }
    });
    let split = NodeRef::<leptos::html::Div>::new();
    let sidebar = RwSignal::new(None::<i32>);
    let inspector = RwSignal::new(Some(320));
    let viewport_width = RwSignal::new(window_width());
    let resize = window_event_listener(ev::resize, move |_| viewport_width.set(window_width()));
    on_cleanup(move || resize.remove());
    let inspector_max = Signal::derive(move || {
        (viewport_width.get() - sidebar.get().unwrap_or(168) - 346).clamp(240, 520)
    });
    let sidebar_max = Signal::derive(move || {
        (viewport_width.get() - inspector.get().unwrap_or(320).min(inspector_max.get()) - 346)
            .clamp(56, 260)
    });
    Effect::new(move |_| {
        let width = sidebar.get();
        if let Some(shell) = split
            .get()
            .and_then(|el| el.closest(".shell").ok().flatten())
            .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
        {
            if let Some(width) = width {
                let _ = shell
                    .style()
                    .set_property("--sidebar-w", &format!("{width}px"));
            } else {
                let _ = shell.style().remove_property("--sidebar-w");
            }
        }
    });
    view! {
        <div class="extract-grid" node_ref=split style=move ||format!("--inspector-w:{}px",inspector.get().unwrap_or(320).min(inspector_max.get()))>
            <ResizeHandle edge=Edge::Left size=sidebar max=sidebar_max/>
            <section class="panel source-panel" aria-label="Video stage">
                <div class="source-line">
                    <strong dir="auto" title=move ||ui.source.with(|s|s.as_ref().map(|s|s.name.clone()).unwrap_or_default())>{move ||ui.source.with(|s|s.as_ref().map(|s|s.name.clone()).unwrap_or_else(||"No video selected".into()))}</strong>
                    <span class="muted mono">{move ||ui.source.with(|s|s.as_ref().map(|s|format!("{} × {} · {}",s.media.width,s.media.height,s.media.codec)).unwrap_or_default())}</span>
                </div>
                <Preview ui editor=true/>
            </section>
            <ResizeHandle edge=Edge::Right size=inspector max=inspector_max/>
            <section id="extract-inspector" class="panel settings-panel" aria-label="Extraction inspector">
                <div class="panel-heading"><h2>"Recognition"</h2></div>
                <div class="inspector-section">
                <label for="language-search">"Subtitle language"</label>
                <div class="language-picker">
                    <div class="language-search-row">
                        <input
                            id="language-search"
                            node_ref=language_input
                            type="search"
                            autocomplete="off"
                            placeholder="Search language by name or code…"
                            aria-label="Search subtitle language"
                            role="combobox"
                            aria-autocomplete="list"
                            aria-controls="language-results"
                            aria-expanded=move || language_search.with(LanguageSearch::is_open).to_string()
                            aria-activedescendant=move || language_search.with(LanguageSearch::active_code).map(|code|format!("language-option-{code}"))
                            prop:value=move || language_search.with(|search|search.query().to_owned())
                            on:input=move |e| language_search.update(|search|search.set_query(event_target_value(&e)))
                            on:focus=move |_| language_search.update(LanguageSearch::open)
                            on:blur=move |_| language_search.update(LanguageSearch::close)
                            on:keydown=move |e: web_sys::KeyboardEvent| {
                                let mut action = KeyAction::Ignored;
                                language_search.update(|search|action=search.key(&e.key(),e.is_composing()||e.key_code()==229));
                                if action != KeyAction::Ignored {
                                    e.prevent_default();
                                }
                                if let KeyAction::Selected(code)=action {
                                    ui.config.update(|c|c.language=code);
                                }
                            }
                        />
                        <Show when=move || language_search.with(|search|!search.query().is_empty())>
                            <button
                                type="button"
                                class="clear-search"
                                aria-label="Clear language search"
                                on:mousedown=move |e|e.prevent_default()
                                on:click=move |_| {
                                    language_search.update(LanguageSearch::clear);
                                    if let Some(input)=language_input.get_untracked(){let _=input.focus();}
                                }
                            >"×"</button>
                        </Show>
                    </div>
                    <Show when=move || language_search.with(LanguageSearch::is_open)>
                        <div id="language-results" class="language-matches" role="listbox" aria-label="Matching subtitle languages">
                            {move || {
                                    language_matches.get()
                                        .into_iter()
                                        .map(|lang| {
                                            let code = lang.code.clone();
                                            let active_code = lang.code.clone();
                                            let selected_code = lang.code.clone();
                                            view! {
                                                <button
                                                    id=format!("language-option-{}",lang.code)
                                                    type="button"
                                                    role="option"
                                                    tabindex="-1"
                                                    class="language-option"
                                                    class:active=move || language_search.with(|search|search.active_code().as_ref()==Some(&active_code))
                                                    aria-selected=move || language_search.with(|search|search.active_code().as_ref()==Some(&selected_code)).to_string()
                                                    on:mousedown=move |e|e.prevent_default()
                                                    on:click=move |_| {
                                                        let mut selected=None;
                                                        language_search.update(|search|selected=search.choose(&code));
                                                        if let Some(code)=selected {ui.config.update(|c|c.language=code);}
                                                        if let Some(input)=language_input.get_untracked(){let _=input.focus();}
                                                    }
                                                >{lang.label}</button>
                                            }
                                        })
                                        .collect_view()
                            }}
                        </div>
                        <Show when=move ||language_matches.with(Vec::is_empty)>
                            <div class="language-empty muted" role="status">"No matching language"</div>
                        </Show>
                    </Show>
                    <select id="language" aria-label="Selected subtitle language" prop:value=move || ui.config.with(|c| c.language.clone()) on:change=move |e| {
                        ui.config.update(|c|c.language=event_target_value(&e));language_search.update(LanguageSearch::clear);
                    }>
                        {
                            helpers::language_options("")
                                .into_iter()
                                .map(|language| view! { <option value=language.code>{language.label}</option> })
                                .collect_view()
                        }
                        {move ||ui.config.with(|config|current_language_option(&config.language)).map(|language|view!{
                            <option value=language.code selected=true>{language.label}</option>
                        })}
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
                    <button id="extract-start" class="primary wide" title="Extract subtitles (Ctrl+Enter)" disabled=move ||!ui.can_extract() on:click=move |_|ui.start_extraction()>"Extract subtitles "<span class="hotkey-hint">"(Ctrl+Enter)"</span></button>
                    <button id="extract-queue" class="wide" title="Add to queue (Ctrl+Q)" disabled=move ||ui.busy.get()||ui.source.with(Option::is_none)||ui.output.with(Option::is_none) on:click=move |_|ui.add_queue()>"Add to queue "<span class="hotkey-hint">"(Ctrl+Q)"</span></button>
                </div>
            </section>
        </div>
    }
}

fn window_width() -> i32 {
    document()
        .document_element()
        .map_or(1280, |root| root.client_width())
}

#[derive(Clone, Copy)]
enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

#[component]
fn ResizeHandle(
    edge: Edge,
    size: RwSignal<Option<i32>>,
    max: Signal<i32>,
    #[prop(optional)] stage: Option<NodeRef<leptos::html::Div>>,
) -> impl IntoView {
    let (class, label, controls, min, default, horizontal, direction) = match edge {
        Edge::Left => (
            "stage-resizer-left",
            "Resize sidebar width",
            "workspace-sidebar",
            56,
            Some(168),
            false,
            1,
        ),
        Edge::Right => (
            "inspector-splitter",
            "Resize inspector",
            "extract-inspector",
            240,
            Some(320),
            false,
            -1,
        ),
        Edge::Top => (
            "stage-resizer-top",
            "Resize video stage top edge",
            "extract-video-stage",
            0,
            Some(0),
            true,
            1,
        ),
        Edge::Bottom => (
            "stage-resizer-bottom",
            "Resize video stage bottom edge",
            "extract-video-stage",
            180,
            None,
            true,
            1,
        ),
    };
    let value = move || {
        size.get()
            .unwrap_or_else(|| {
                stage
                    .and_then(|node| node.get())
                    .map_or(min, |node| node.client_height())
            })
            .clamp(min, max.get())
    };
    let drag = RwSignal::new(None::<(i32, i32, i32)>);
    let position = move |e: &web_sys::PointerEvent| {
        if horizontal {
            e.client_y()
        } else {
            e.client_x()
        }
    };
    view! {
        <div id={matches!(edge, Edge::Right).then_some("inspector-splitter")} class=class role="separator"
            aria-label=label aria-controls=controls tabindex="0"
            aria-orientation={if horizontal {"horizontal"} else {"vertical"}}
            aria-valuemin=min.to_string() aria-valuemax=move ||max.get().to_string()
            aria-valuenow=move ||value().to_string()
            on:pointerdown=move |e| {
                if e.button()!=0 {return;}
                if let Some(el)=e.current_target().and_then(|el|el.dyn_into::<web_sys::HtmlElement>().ok()) {
                    e.prevent_default();
                    let _=el.focus();
                    if el.set_pointer_capture(e.pointer_id()).is_ok() {
                        drag.set(Some((e.pointer_id(),position(&e),untrack(value))));
                    }
                }
            }
            on:pointermove=move |e| {
                if let Some((_,start,width))=drag.get_untracked().filter(|(id,_,_)|*id==e.pointer_id()) {
                    size.set(Some((width+direction*(position(&e)-start)).clamp(min,max.get_untracked())));
                }
            }
            on:lostpointercapture=move |_|drag.set(None)
            on:dblclick=move |_| size.set(default)
            on:keydown=move |e| {
                let value=untrack(value);
                let next=match e.key().as_str() {
                    "ArrowLeft" if !horizontal=>Some(value-16*direction),
                    "ArrowRight" if !horizontal=>Some(value+16*direction),
                    "ArrowUp" if horizontal=>Some(value-16),
                    "ArrowDown" if horizontal=>Some(value+16),
                    "Home"=>Some(min), "End"=>Some(max.get_untracked()),
                    "Enter"|" "=>default, _=>return,
                };
                e.prevent_default();
                size.set(next.map(|value|value.clamp(min,max.get_untracked())));
            }
        ></div>
    }
}

#[component]
pub(crate) fn VideoStage(children: Children) -> impl IntoView {
    let stage = NodeRef::<leptos::html::Div>::new();
    let top = RwSignal::new(Some(0));
    let height = RwSignal::new(None);
    let max_height = RwSignal::new(stage_height_limit());
    let resize = window_event_listener(ev::resize, move |_| max_height.set(stage_height_limit()));
    on_cleanup(move || resize.remove());
    view! {
        <div id="extract-video-stage" class="stage-frame" node_ref=stage
            style=move || {
                let top=top.get().unwrap_or_default();
                height.get().map_or_else(||format!("--stage-top-pad:{top}px"),
                    |height:i32|format!("--stage-top-pad:{top}px;--stage-height:{}px",height.min(max_height.get())))
            }>
            <ResizeHandle edge=Edge::Top size=top max=Signal::derive(||160)/>
            {children()}
            <ResizeHandle edge=Edge::Bottom size=height max=Signal::derive(move ||max_height.get()) stage=stage/>
        </div>
    }
}

fn stage_height_limit() -> i32 {
    document().document_element().map_or(600, |root| {
        (f64::from(root.client_height()) * 0.82).max(180.0) as i32
    })
}

#[component]
pub(crate) fn FocusHud(ui: Ui) -> impl IntoView {
    view! {
        <Show when=move ||ui.studio.focus_mode.get()>
            <div class="focus-hud" role="group" aria-label="Focus mode controls">
                <button type="button" disabled=move ||!ui.preview_controls_available() || ui.duration().is_none_or(|d|d<2)
                    on:click=move |_|ui.toggle_play()>{move ||if ui.studio.playing.get(){"Pause"}else{"Play"}}
                    " "<span class="hotkey-hint">"(Space)"</span></button>
                <button type="button" disabled=move ||!ui.preview_controls_available() on:click=move |_|ui.step_frame(-1)>"Prev "<span class="hotkey-hint">"(←)"</span></button>
                <button type="button" disabled=move ||!ui.preview_controls_available() on:click=move |_|ui.step_frame(1)>"Next "<span class="hotkey-hint">"(→)"</span></button>
                <button type="button" aria-pressed=move ||ui.studio.zoom_tool.get().to_string() on:click=move |_|ui.toggle_zoom_tool()>"Zoom tool "<span class="hotkey-hint">"(Z)"</span></button>
                <button type="button" aria-pressed=move ||ui.studio.exclusion_mode.get().to_string() on:click=move |_|ui.toggle_exclusion_mode()>"Exclude "<span class="hotkey-hint">"(E)"</span></button>
                <button type="button" aria-pressed=move ||ui.studio.guides.get().to_string() on:click=move |_|ui.toggle_guides()>"Guides "<span class="hotkey-hint">"(G)"</span></button>
                <button type="button" disabled=move ||!ui.can_undo() on:click=move |_|ui.undo()>"Undo "<span class="hotkey-hint">"(Ctrl+Z)"</span></button>
                <button type="button" on:click=move |_|ui.toggle_focus_mode()>"Exit Focus "<span class="hotkey-hint">"(`)"</span></button>
            </div>
        </Show>
    }
}
