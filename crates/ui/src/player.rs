use crate::{helpers, ipc, roi_editor::RoiCanvas, state::Ui};
use gloo_timers::callback::Interval;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

const ZOOM: [u32; 5] = [100, 150, 200, 300, 400];

impl Ui {
    fn duration(self) -> Option<u64> {
        self.source.with(|s| s.as_ref()?.media.duration_ms)
    }

    fn preview_controls_available(self) -> bool {
        !self.closing.get()
            && !self.busy.get()
            && !self.studio.initial.get()
            && self.source.with(Option::is_some)
            && (self.route.get() != "Review" || self.review_source_linked())
    }

    pub(crate) fn toggle_play(self) {
        if self.studio.playing.get_untracked() {
            self.studio.playing.set(false);
        } else if self.preview_controls_available() && self.duration().is_some_and(|end| end > 1) {
            if self.time.get_untracked() >= self.duration().unwrap_or(0).saturating_sub(1) {
                self.seek(self.config.with_untracked(|c| c.start_ms));
            }
            self.studio.playing.set(true);
        }
    }

    pub(crate) fn step(self, delta: i64) {
        if self.preview_controls_available() {
            self.studio.playing.set(false);
            self.seek(self.time.get_untracked().saturating_add_signed(delta));
        }
    }

    pub(crate) fn mark_range(self, end: bool) {
        if self.preview_controls_available() {
            let time = self.time.get_untracked();
            if end {
                self.set_end(Some(time));
            } else {
                self.set_start(time);
            }
        }
    }

    pub(crate) fn set_start(self, start: u64) {
        if self
            .config
            .with_untracked(|c| c.end_ms)
            .is_some_and(|end| start >= end)
            || self.duration().is_some_and(|duration| start >= duration)
        {
            self.error
                .set(Some("Start must be before the end of the range".into()));
            return;
        }
        self.edit_extract(|c| c.start_ms = start);
    }

    pub(crate) fn set_end(self, end: Option<u64>) {
        if end.is_some_and(|end| {
            end <= self.config.with_untracked(|c| c.start_ms)
                || self.duration().is_some_and(|duration| end > duration)
        }) {
            self.error
                .set(Some("End must be after Start and within the video".into()));
            return;
        }
        self.edit_extract(|c| c.end_ms = end);
    }

    pub(crate) fn change_zoom(self, delta: i8) {
        let current = self.studio.zoom.get_untracked();
        let index = ZOOM.iter().position(|&value| value == current).unwrap_or(0);
        let next = if delta == 0 {
            0
        } else {
            index
                .saturating_add_signed(isize::from(delta))
                .min(ZOOM.len() - 1)
        };
        self.studio.roi_drag.set(None);
        self.studio.zoom.set(ZOOM[next]);
    }

    pub fn seek(self, time: u64) {
        if self.closing.get_untracked()
            || (self.route.get_untracked() == "Review" && !self.review_source_linked())
        {
            return;
        }
        let Some(source) = self.source.get_untracked() else {
            return;
        };
        let time = source
            .media
            .duration_ms
            .map_or(time, |duration| time.min(duration.saturating_sub(1)));
        self.time.set(time);
        if self.preview_busy.get_untracked() || self.studio.initial.get_untracked() {
            return;
        }
        self.preview_busy.set(true);
        self.preview_generation
            .update(|generation| *generation += 1);
        let generation = self.preview_generation.get_untracked();
        spawn_local(async move {
            let preview = ipc::preview(&source.token, time).await;
            let alive = !self.preview_url.is_disposed();
            let current = alive
                && self.preview_generation.get_untracked() == generation
                && self
                    .source
                    .with_untracked(|s| s.as_ref().is_some_and(|s| s.token == source.token));
            match preview {
                Ok(url) if current => {
                    let old = self.preview_url.get_untracked();
                    self.preview_url.set(url);
                    if !old.is_empty() {
                        let _ = web_sys::Url::revoke_object_url(&old);
                    }
                }
                Ok(url) => {
                    let _ = web_sys::Url::revoke_object_url(&url);
                }
                Err(error) if current && self.time.get_untracked() == time => {
                    self.studio.playing.set(false);
                    self.error.set(Some(error));
                }
                Err(_) => {}
            }
            if alive {
                self.preview_busy.set(false);
                if self.studio.initial.get_untracked() {
                    self.request_initial_frame();
                } else if !current || self.time.get_untracked() != time {
                    self.seek(self.time.get_untracked());
                }
            }
        });
    }
}

pub(crate) fn watch_playback(ui: Ui) {
    let interval = Interval::new(250, move || {
        if !ui.studio.playing.get_untracked() {
            return;
        }
        if !matches!(ui.route.get_untracked().as_str(), "Extract" | "Review")
            || !ui.preview_controls_available()
        {
            ui.studio.playing.set(false);
            return;
        }
        if ui.preview_busy.get_untracked() {
            return;
        }
        let end = ui.duration().unwrap_or(0).saturating_sub(1);
        let next = ui.time.get_untracked().saturating_add(250).min(end);
        ui.seek(next);
        if next >= end {
            ui.studio.playing.set(false);
        }
    });
    let interval = StoredValue::new_local(Some(interval));
    on_cleanup(move || {
        interval.update_value(|interval| {
            interval.take();
        })
    });
}

#[component]
pub(crate) fn Preview(ui: Ui, #[prop(optional)] editor: bool) -> impl IntoView {
    let viewport = NodeRef::<leptos::html::Div>::new();
    let exclusion = RwSignal::new(false);
    let guides = RwSignal::new(true);
    let pan = move |x: i32, y: i32| {
        ui.studio.roi_drag.set(None);
        if let Some(element) = viewport.get_untracked() {
            element.set_scroll_left(element.scroll_left().saturating_add(x));
            element.set_scroll_top(element.scroll_top().saturating_add(y));
        }
    };
    Effect::new(move |_| {
        if ui.studio.zoom.get() == 100
            && let Some(element) = viewport.get()
        {
            element.set_scroll_left(0);
            element.set_scroll_top(0);
        }
    });
    let timeline_style = move || {
        let duration = ui.duration().unwrap_or(60_000).max(1);
        let percent = |position: u64| position.min(duration) as f64 / duration as f64 * 100.0;
        ui.config.with(|c| {
            format!(
                "--range-in:{}%;--range-out:{}%",
                percent(c.start_ms),
                percent(c.end_ms.unwrap_or(duration))
            )
        })
    };
    let disabled = move || !ui.preview_controls_available();
    view! {
        <div class="preview-shell" aria-busy=move ||ui.preview_busy.get()||ui.studio.initial.get()>
            <Show when=move || (editor||ui.review_source_linked())&&!ui.preview_url.get().is_empty() fallback=move ||view! {
                <div class="preview-empty">
                    <svg viewBox="0 0 48 48" aria-hidden="true"><rect x="7" y="12" width="34" height="24" rx="3"/><path d="m20 18 10 6-10 6z"/></svg>
                    <strong>"No frame to display"</strong><p>{if editor {"Open or drop a video to preview it."} else {"Link the original video to preview it."}}</p>
                </div>
            }>
                <div class="preview-viewport" class:zoomed={move || ui.studio.zoom.get() > 100} node_ref=viewport tabindex="0" role="region" aria-label="Source frame viewport">
                    <RoiCanvas ui editor exclusion guides/>
                </div>
            </Show>
            <Show when=move ||ui.studio.initial.get()||ui.preview_busy.get()&&!ui.studio.playing.get()>
                <span class="preview-loading" role="status">"Decoding frame…"</span>
            </Show>
        </div>
        <div class="transport" role="group" aria-label="Playback">
            <div class="transport-scrub">
                <input id={editor.then_some("preview-timeline")} aria-label="Preview position in milliseconds" type="range" min="0"
                    max=move ||ui.duration().unwrap_or(60_000).saturating_sub(1).to_string() step="1"
                    style=timeline_style prop:value=move ||ui.time.get().to_string() disabled=disabled
                    on:input=move |e|if let Ok(time)=event_target_value(&e).parse(){ui.studio.playing.set(false);ui.seek(time);}/>
                <output class="mono">{move ||format!("{} / {}",helpers::time_label(ui.time.get()),ui.duration().map(helpers::time_label).unwrap_or_else(||"--:--:--.---".into()))}</output>
            </div>
            <div class="button-row transport-keys">
                <button id={editor.then_some("preview-play")} title="Play or pause (Space)" aria-pressed=move ||ui.studio.playing.get().to_string() disabled=move ||disabled()||ui.duration().is_none_or(|duration|duration<2)
                    on:click=move |_|ui.toggle_play()>{move ||if ui.studio.playing.get(){"Pause"}else{"Play"}}</button>
                <button id={editor.then_some("preview-stop")} disabled=disabled on:click=move |_|{ui.studio.playing.set(false);ui.seek(ui.config.with_untracked(|c|c.start_ms));}>"Stop"</button>
                <button title="Jump back one second (Shift+Left)" disabled=disabled on:click=move |_|ui.step(-1000)>"−1s"</button>
                <button title="Previous frame (Left)" disabled=disabled on:click=move |_|ui.step(-40)>"Prev frame"</button>
                <button title="Next frame (Right)" disabled=disabled on:click=move |_|ui.step(40)>"Next frame"</button>
                <button title="Jump forward one second (Shift+Right)" disabled=disabled on:click=move |_|ui.step(1000)>"+1s"</button>
                {editor.then(||view! {
                    <button id="set-start" title="Set start (I)" disabled=disabled on:click=move |_|ui.mark_range(false)>"Set start"</button>
                    <button id="set-end" title="Set end (O)" disabled=disabled on:click=move |_|ui.mark_range(true)>"Set end"</button>
                })}
            </div>
        </div>
        <div class="stage-options">
        <div class="button-row preview-controls" role="group" aria-label="Preview navigation">
            <label>"Zoom"<select aria-label="Preview zoom" prop:value=move ||ui.studio.zoom.get().to_string() on:change=move |e| {
                if let Ok(value)=event_target_value(&e).parse::<u32>() && ZOOM.contains(&value) {
                    ui.studio.roi_drag.set(None);ui.studio.zoom.set(value);
                }
            }><option value="100">"Fit · 100%"</option><option value="150">"150%"</option><option value="200">"200%"</option><option value="300">"300%"</option><option value="400">"400%"</option></select></label>
            <button aria-label="Pan preview left" disabled=move ||ui.studio.zoom.get()==100 on:click=move |_|pan(-120,0)>"Left"</button>
            <button aria-label="Pan preview right" disabled=move ||ui.studio.zoom.get()==100 on:click=move |_|pan(120,0)>"Right"</button>
            <button aria-label="Pan preview up" disabled=move ||ui.studio.zoom.get()==100 on:click=move |_|pan(0,-120)>"Up"</button>
            <button aria-label="Pan preview down" disabled=move ||ui.studio.zoom.get()==100 on:click=move |_|pan(0,120)>"Down"</button>
            <button title="Reset zoom (Ctrl+0)" on:click=move |_|{ui.change_zoom(0);pan(-i32::MAX,-i32::MAX);}>"Reset view"</button>
        </div>
        {editor.then(||view! {<div class="canvas-options">
            <label class="check draw-mode"><input type="checkbox" prop:checked=move ||exclusion.get() on:change=move |e|exclusion.set(event_target_checked(&e))/><span>"Draw exclusion instead of subtitle region"</span></label>
            <label class="check guides-toggle"><input type="checkbox" prop:checked=move ||guides.get() on:change=move |e|guides.set(event_target_checked(&e))/><span>"Guides"</span></label>
        </div>})}
        </div>
    }
}
