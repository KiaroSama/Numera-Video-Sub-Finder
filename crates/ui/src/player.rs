use crate::{helpers, ipc, roi_editor::RoiCanvas, state::Ui, zoom_tool};
use leptos::{ev, prelude::*};
use wasm_bindgen_futures::{JsFuture, spawn_local};

fn now_ms() -> Option<u64> {
    web_sys::window()
        .and_then(|window| window.performance())
        .map(|clock| clock.now())
        .filter(|now| now.is_finite() && *now >= 0.0)
        .map(|now| now as u64)
}

impl Ui {
    pub(crate) fn duration(self) -> Option<u64> {
        self.source.with(|s| s.as_ref()?.media.duration_ms)
    }

    pub(crate) fn preview_controls_available(self) -> bool {
        !self.closing.get()
            && !self.busy.get()
            && !self.studio.initial.get()
            && self.source.with(Option::is_some)
            && (self.route.get() != "Review" || self.review_source_linked())
    }

    pub(crate) fn toggle_play(self) {
        let Some(now) = now_ms() else {
            playback_clock_failed(self);
            return;
        };
        if self.studio.playing.get_untracked() {
            self.studio.playing.set(false);
            let end = self.duration().unwrap_or(0).saturating_sub(1);
            if let Some(position) = self
                .studio
                .transport
                .try_update(|transport| {
                    transport.pause(now, end, self.studio.playback_rate.get_untracked())
                })
                .flatten()
            {
                self.time.set(position);
            }
            self.pump_preview();
        } else if self.preview_controls_available() && self.duration().is_some_and(|end| end > 1) {
            let position = self.studio.transport.try_update(|transport| {
                transport.play(
                    now,
                    self.time.get_untracked(),
                    self.config.with_untracked(|c| c.start_ms),
                    self.duration().unwrap_or(0).saturating_sub(1),
                )
            });
            if let Some(position) = position {
                self.time.set(position);
            }
            self.studio.playing.set(true);
            self.pump_preview();
        }
    }

    pub(crate) fn stop_playback(self) {
        self.seek_start();
    }

    pub(crate) fn seek_start(self) {
        self.seek(self.config.with_untracked(|c| c.start_ms));
    }

    pub(crate) fn seek_end(self) {
        if let Some(end) = self.config.with_untracked(|c| c.end_ms).or(self.duration()) {
            self.seek(end);
        }
    }

    pub(crate) fn step_frame(self, direction: i8) {
        if self.preview_controls_available() {
            self.studio.playing.set(false);
            self.studio
                .transport
                .update(|transport| transport.step(direction));
            self.pump_preview();
        }
    }

    pub(crate) fn step(self, delta: i64) {
        if self.preview_controls_available() {
            self.studio.playing.set(false);
            let position = self
                .studio
                .transport
                .with_untracked(|transport| transport.position());
            self.seek(position.saturating_add_signed(delta));
        }
    }

    pub(crate) fn mark_range(self, end: bool) {
        if self.preview_controls_available() {
            let Some(time) = self
                .studio
                .transport
                .with_untracked(|transport| transport.displayed())
            else {
                return;
            };
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
        self.studio.roi_drag.set(None);
        self.studio.zoom.set(zoom_tool::next_zoom(current, delta));
    }

    pub(crate) fn seek(self, time: u64) {
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
        self.studio.playing.set(false);
        self.studio
            .transport
            .update(|transport| transport.seek(time));
        self.pump_preview();
    }

    fn pump_preview(self) {
        if self.closing.get_untracked()
            || self.preview_busy.get_untracked()
            || self.studio.initial.get_untracked()
        {
            return;
        }
        let Some(source) = self.source.get_untracked() else {
            return;
        };
        if self.route.get_untracked() == "Review" && !self.review_source_linked() {
            return;
        }
        let Some(request) = self
            .studio
            .transport
            .try_update(|transport| transport.take_request(&source.token))
            .flatten()
        else {
            return;
        };
        self.preview_busy.set(true);
        self.preview_generation
            .update(|generation| *generation += 1);
        let generation = self.preview_generation.get_untracked();
        spawn_local(async move {
            let preview =
                ipc::preview(&request.source, request.at_ms, request.step, request.fast()).await;
            let preview = match preview {
                Ok(frame) => decode_frame(frame).await,
                Err(error) => Err(error),
            };
            if self.preview_url.is_disposed() {
                if let Ok((frame, _)) = preview {
                    let _ = web_sys::Url::revoke_object_url(&frame.url);
                }
                return;
            }
            let token = self
                .source
                .with_untracked(|source| source.as_ref().map(|source| source.token.clone()));
            let token = (self.preview_generation.get_untracked() == generation)
                .then_some(token)
                .flatten();
            let completion = self
                .studio
                .transport
                .try_update(|transport| {
                    transport.finish(
                        &request,
                        token.as_deref(),
                        preview.as_ref().ok().map(|(frame, _)| frame.time_ms),
                    )
                })
                .unwrap_or_default();
            match preview {
                Ok((frame, decoded)) if completion.publish => {
                    let old = self.preview_url.get_untracked();
                    self.preview_url.set(frame.url);
                    // Keep the decoded image alive until the reactive src update paints.
                    request_animation_frame(move || drop(decoded));
                    self.studio.frame_duration.set(Some(frame.duration_ms));
                    if !self.studio.playing.get_untracked() {
                        self.time.set(frame.time_ms);
                    }
                    if !old.is_empty() {
                        let _ = web_sys::Url::revoke_object_url(&old);
                    }
                }
                Ok((frame, _)) => {
                    let _ = web_sys::Url::revoke_object_url(&frame.url);
                }
                Err(error) if completion.current => {
                    self.studio.playing.set(false);
                    self.studio
                        .transport
                        .update(|transport| transport.stop_clock());
                    self.error.set(Some(error));
                }
                Err(_) => {}
            }
            if completion.released {
                self.preview_busy.set(false);
                if self.studio.initial.get_untracked() {
                    self.request_initial_frame();
                } else if self
                    .studio
                    .transport
                    .with_untracked(|transport| transport.has_pending())
                {
                    self.pump_preview();
                }
            }
        });
    }
}

async fn decode_frame(
    frame: ipc::SourceFrame,
) -> Result<(ipc::SourceFrame, web_sys::HtmlImageElement), String> {
    if let Ok(image) = web_sys::HtmlImageElement::new() {
        image.set_src(&frame.url);
        if JsFuture::from(image.decode()).await.is_ok() {
            return Ok((frame, image));
        }
    }
    let _ = web_sys::Url::revoke_object_url(&frame.url);
    Err("Could not decode the preview image".into())
}

type FrameRequest = StoredValue<Option<AnimationFrameRequestHandle>>;

pub(crate) fn watch_playback(ui: Ui) {
    let frame = StoredValue::new(None::<AnimationFrameRequestHandle>);
    Effect::new(move |_| {
        if let Some(handle) = frame.try_update_value(Option::take).flatten() {
            handle.cancel();
        }
        if ui.studio.playing.get() {
            schedule_playback(ui, frame);
        }
    });
    on_cleanup(move || {
        if let Some(handle) = frame.try_update_value(Option::take).flatten() {
            handle.cancel();
        }
    });
}

fn schedule_playback(ui: Ui, frame: FrameRequest) {
    match request_animation_frame_with_handle(move || {
        frame.set_value(None);
        if !ui.studio.playing.get_untracked() {
            return;
        }
        if !matches!(ui.route.get_untracked().as_str(), "Extract" | "Review")
            || !ui.preview_controls_available()
        {
            ui.studio.playing.set(false);
            ui.studio
                .transport
                .update(|transport| transport.stop_clock());
            return;
        }
        let Some(now) = now_ms() else {
            playback_clock_failed(ui);
            return;
        };
        let end = ui.duration().unwrap_or(0).saturating_sub(1);
        let rate = ui.studio.playback_rate.get_untracked();
        if let Some((position, ended)) = ui
            .studio
            .transport
            .try_update(|transport| transport.tick(now, end, rate))
            .flatten()
        {
            ui.time.set(position);
            if ended {
                ui.studio.playing.set(false);
            }
            ui.pump_preview();
        }
        if ui.studio.playing.get_untracked() {
            schedule_playback(ui, frame);
        }
    }) {
        Ok(handle) => frame.set_value(Some(handle)),
        Err(_) => playback_clock_failed(ui),
    }
}

fn playback_clock_failed(ui: Ui) {
    ui.studio.playing.set(false);
    ui.studio
        .transport
        .update(|transport| transport.stop_clock());
    ui.error
        .set(Some("Monotonic playback clock is unavailable".into()));
}

fn enable_pan(ui: Ui, viewport: zoom_tool::Div) -> impl Fn(web_sys::MouseEvent) + Copy {
    let drag = RwSignal::new(None::<(i32, i32, i32, i32)>);
    let movement = window_event_listener(ev::mousemove, move |e| {
        if let Some((x, y, left, top)) = drag.get_untracked()
            && let Some(el) = viewport.get_untracked()
        {
            el.set_scroll_left(left + x - e.client_x());
            el.set_scroll_top(top + y - e.client_y());
        }
    });
    let release = window_event_listener(ev::mouseup, move |e| {
        if e.button() == 1 {
            drag.set(None);
        }
    });
    let blur = window_event_listener(ev::blur, move |_| drag.set(None));
    on_cleanup(move || {
        movement.remove();
        release.remove();
        blur.remove();
    });
    move |e: web_sys::MouseEvent| {
        if e.button() == 1
            && ui.studio.zoom.get_untracked() > 100
            && let Some(el) = viewport.get_untracked()
        {
            e.prevent_default();
            ui.studio.roi_drag.set(None);
            drag.set(Some((
                e.client_x(),
                e.client_y(),
                el.scroll_left(),
                el.scroll_top(),
            )));
        }
    }
}

#[component]
pub(crate) fn Preview(ui: Ui, #[prop(optional)] editor: bool) -> impl IntoView {
    let viewport = NodeRef::<leptos::html::Div>::new();
    let begin_pan = enable_pan(ui, viewport);
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
                "--playhead:{}%;--range-in:{}%;--range-out:{}%",
                percent(ui.time.get()),
                percent(c.start_ms),
                percent(c.end_ms.unwrap_or(duration))
            )
        })
    };
    let disabled = move || !ui.preview_controls_available();
    let stage = view! {
        <div class="preview-shell" dir="ltr" aria-busy=move ||if ui.preview_busy.get()||ui.studio.initial.get(){"true"}else{"false"}
            data-displayed-time-ms=move || ui.studio.transport.with(|transport| transport.displayed()).map(|time| time.to_string())
            data-frame-duration-ms=move || ui.studio.frame_duration.get().map(|duration| duration.to_string())>
            <Show when=move || (editor||ui.review_source_linked())&&!ui.preview_url.get().is_empty() fallback=move ||view! {
                <div class="preview-empty">
                    <svg viewBox="0 0 48 48" aria-hidden="true"><rect x="7" y="12" width="34" height="24" rx="3"/><path d="m20 18 10 6-10 6z"/></svg>
                    <strong>"No frame to display"</strong><p>{if editor {"Open or drop a video to preview it."} else {"Link the original video to preview it."}}</p>
                </div>
            }>
                <div class="preview-viewport" class:zoomed={move || ui.studio.zoom.get() > 100} node_ref=viewport tabindex="0" role="region" aria-label="Source frame viewport"
                    on:wheel=move |e|zoom_tool::wheel(ui,viewport,e) on:mousedown=begin_pan
                    on:auxclick=move |e|if e.button()==1 && ui.studio.zoom.get_untracked()>100 {e.prevent_default();}>
                    <RoiCanvas ui editor viewport/>
                </div>
            </Show>
            <Show when=move ||!ui.studio.loading_video.get() && (ui.studio.initial.get() || ui.preview_busy.get() && !ui.studio.playing.get())>
                <span class="preview-loading" role="status">"Decoding frame…"</span>
            </Show>
            {editor.then(||view! { <crate::extract::FocusHud ui/> })}
        </div>
    };
    view! {
        {if editor {view! {<crate::extract::VideoStage>{stage}</crate::extract::VideoStage>}.into_any()} else {stage.into_any()}}
        {editor.then(||view! {
            <Show when=move ||ui.studio.loading_video.get()>
                <div id="video-drop-loader" class="drop-spinner-bar" role="status" aria-live="polite">
                    <span class="drop-spinner" aria-hidden="true">
                        {(0..8).map(|_|view!{<span class="spinner-dot"></span>}).collect_view()}
                    </span>
                    "Loading video and decoding preview frame…"
                </div>
            </Show>
        })}
        <div class="transport" role="group" aria-label="Playback">
            <div class="transport-scrub">
                <input id={editor.then_some("preview-timeline")} aria-label="Preview position in milliseconds" type="range" min="0"
                    max=move ||ui.duration().unwrap_or(60_000).saturating_sub(1).to_string() step="1"
                    style=timeline_style prop:value=move ||ui.time.get().to_string() disabled=disabled
                    aria-valuetext=move ||helpers::time_label(ui.time.get())
                    on:input=move |e|if let Ok(time)=event_target_value(&e).parse(){ui.studio.playing.set(false);ui.seek(time);}/>
                <output class="mono">{move ||format!("{} / {}",helpers::time_label(ui.time.get()),ui.duration().map(helpers::time_label).unwrap_or_else(||"--:--:--.---".into()))}</output>
            </div>
            <div class="button-row transport-keys">
                <button id={editor.then_some("preview-play")} title="Play or pause (Space)" aria-pressed=move ||ui.studio.playing.get().to_string() disabled=move ||disabled()||ui.duration().is_none_or(|duration|duration<2)
                    on:click=move |_|ui.toggle_play()>{move ||if ui.studio.playing.get(){"Pause"}else{"Play"}}" "<span class="hotkey-hint">"(Space)"</span></button>
                <button id={editor.then_some("preview-stop")} disabled=disabled on:click=move |_|ui.stop_playback()>"Stop "<span class="hotkey-hint">"(S)"</span></button>
                <button disabled=disabled on:click=move |_|ui.step(-10000)>"−10s "<span class="hotkey-hint">"(J)"</span></button>
                <button title="Jump back one second (Shift+Left)" disabled=disabled on:click=move |_|ui.step(-1000)>"−1s "<span class="hotkey-hint">"(Shift+←)"</span></button>
                <button data-testid="previous-frame" title="Previous frame (Left)" disabled=disabled on:click=move |_|ui.step_frame(-1)>"Prev "<span class="hotkey-hint">"(←)"</span></button>
                <button data-testid="next-frame" title="Next frame (Right)" disabled=disabled on:click=move |_|ui.step_frame(1)>"Next "<span class="hotkey-hint">"(→)"</span></button>
                <button title="Jump forward one second (Shift+Right)" disabled=disabled on:click=move |_|ui.step(1000)>"+1s "<span class="hotkey-hint">"(Shift+→)"</span></button>
                <button disabled=disabled on:click=move |_|ui.step(10000)>"+10s "<span class="hotkey-hint">"(L)"</span></button>
                {editor.then(||view! {
                    <button id="set-start" title="Set start (I)" disabled=disabled on:click=move |_|ui.mark_range(false)>"Set start "<span class="hotkey-hint">"(I)"</span></button>
                    <button id="set-end" title="Set end (O)" disabled=disabled on:click=move |_|ui.mark_range(true)>"Set end "<span class="hotkey-hint">"(O)"</span></button>
                })}
                <select id={editor.then_some("playback-rate")} aria-label="Playback speed" disabled=disabled
                    prop:value=move ||ui.studio.playback_rate.get().to_string()
                    on:change=move |e|if let Ok(rate)=event_target_value(&e).parse(){ui.set_playback_rate(rate);}>
                    <option value="0.5">"0.5×"</option><option value="1">"1×"</option>
                    <option value="1.5">"1.5×"</option><option value="2">"2×"</option>
                </select>
            </div>
        </div>
        <div class="stage-options">
        <div class="button-row preview-controls" role="group" aria-label="Preview navigation">
            <label>"Zoom"<select aria-label="Preview zoom" prop:value=move ||ui.studio.zoom.get().to_string() on:change=move |e| {
                if let Ok(value)=event_target_value(&e).parse::<u32>() && (100..=800).contains(&value) {
                    ui.studio.roi_drag.set(None);ui.studio.zoom.set(value);
                }
            }><option value="100">"Fit · 100%"</option><option value="150">"150%"</option><option value="200">"200%"</option><option value="300">"300%"</option><option value="400">"400%"</option>
            {move || {
                let value = ui.studio.zoom.get();
                (!zoom_tool::PRESETS.contains(&value)).then(||view! {
                    <option value=value.to_string() selected=true>{format!("{value}%")}</option>
                })
            }}
            </select></label>
            <button id={editor.then_some("zoom-tool-toggle")} aria-pressed=move ||ui.studio.zoom_tool.get().to_string()
                title="Zoom tool: drag on video to zoom (Z)" on:click=move |_|ui.toggle_zoom_tool()>"Zoom tool "<span class="hotkey-hint">"(Z)"</span></button>
            <button aria-label="Pan preview left" disabled=move ||ui.studio.zoom.get()==100 on:click=move |_|pan(-120,0)>"Left"</button>
            <button aria-label="Pan preview right" disabled=move ||ui.studio.zoom.get()==100 on:click=move |_|pan(120,0)>"Right"</button>
            <button aria-label="Pan preview up" disabled=move ||ui.studio.zoom.get()==100 on:click=move |_|pan(0,-120)>"Up"</button>
            <button aria-label="Pan preview down" disabled=move ||ui.studio.zoom.get()==100 on:click=move |_|pan(0,120)>"Down"</button>
            <button title="Reset zoom (Ctrl+0)" on:click=move |_|{ui.change_zoom(0);pan(-i32::MAX,-i32::MAX);}>"Reset view "<span class="hotkey-hint">"(Ctrl+0)"</span></button>
            {editor.then(||view! {
                <button id="focus-mode-toggle" aria-pressed=move ||ui.studio.focus_mode.get().to_string()
                    title="Fullscreen focus mode (`)" on:click=move |_|ui.toggle_focus_mode()>"Focus "<span class="hotkey-hint">"(`)"</span></button>
            })}
        </div>
        {editor.then(||view! {<div class="canvas-options">
            <label class="check draw-mode"><input type="checkbox" prop:checked=move ||ui.studio.exclusion_mode.get() on:change=move |_|ui.toggle_exclusion_mode()/><span>"Draw exclusion instead of subtitle region "<span class="hotkey-hint">"(E)"</span></span></label>
            <label class="check guides-toggle"><input type="checkbox" prop:checked=move ||ui.studio.guides.get() on:change=move |_|ui.toggle_guides()/><span>"Guides "<span class="hotkey-hint">"(G)"</span></span></label>
        </div>})}
        </div>
    }
}
