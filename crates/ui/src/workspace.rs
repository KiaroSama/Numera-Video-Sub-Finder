use crate::{
    helpers,
    ipc::{Doctor, HistoryEntry, QueueEntry},
    state::Ui,
};
use leptos::prelude::*;
use serde_json::json;

#[component]
pub fn Queue(ui: Ui) -> impl IntoView {
    view! {<div class="workspace-column">
        <section class="panel job-panel"><div class="panel-heading"><div><span class="eyebrow">"JOB CONTROL"</span><h2>"Extraction queue"</h2></div><span class="state-chip">{move ||ui.status.with(|s|s.state.clone())}</span></div>
            <Show when=move ||!ui.status.with(|s|s.id.is_empty()) fallback=||view!{<p class="muted">"No job started. Add a video from Extract, then start it here."</p>}>
                <p class="mono">{move ||ui.status.with(|s|s.id.clone())}</p>
                <Show when=move ||ui.status.with(|s|s.progress.is_some()) fallback=move ||view!{<p class="hint">{move ||if ui.active(){"Waiting for the backend’s first progress event…"}else{"No progress event was reported."}}</p>}>
                    <div class="progress-detail"><strong>{move ||ui.status.with(|s|s.progress.as_ref().map(|p|p.stage.clone()).unwrap_or_default())}</strong>
                        <span class="mono">{move ||ui.status.with(|s|s.progress.as_ref().map(|p|format!("{} / {}",helpers::time_label(p.position_ms),p.duration_ms.map(helpers::time_label).unwrap_or_else(||"unknown".into()))).unwrap_or_default())}</span></div>
                    <progress aria-label="Source processing progress" max=move ||ui.status.with(|s|s.progress.as_ref().and_then(|p|p.duration_ms).unwrap_or(1) as f64)
                        value=move ||ui.status.with(|s|s.progress.as_ref().filter(|p|p.duration_ms.is_some()).map(|p|p.position_ms as f64))></progress>
                    <p class="muted">{move ||ui.status.with(|s|s.progress.as_ref().map(|p|format!("{} frames · {} cues · provider: {}",p.frames,p.cues,p.provider)).unwrap_or_default())}</p>
                    <p role="status">{move ||ui.status.with(|s|s.progress.as_ref().map(|p|p.message.clone()).unwrap_or_default())}</p>
                </Show>
                <div class="button-row"><button disabled=move ||!ui.active()||ui.busy.get()||ui.status.with(|s|s.state=="cancelling") on:click=move |_|{
                    let id=ui.status.with_untracked(|s|s.id.clone());let action=if ui.status.with_untracked(|s|s.state=="paused"){"resume"}else{"pause"};ui.request::<()>("control_job",json!({"id":id,"action":action}),|_|{});
                }>{move ||if ui.status.with(|s|s.state=="paused"){"Resume job"}else{"Pause job"}}</button>
                    <button class="danger" disabled=move ||!ui.active()||ui.busy.get()||ui.status.with(|s|s.state=="cancelling") on:click=move |_|ui.request::<()>("control_job",json!({"id":ui.status.with_untracked(|s|s.id.clone()),"action":"cancel"}),|_|{})>"Cancel job"</button></div>
                <p class="hint">"Pause stops work at the next safe boundary. Cancel preserves completed checkpoints; wait for cancellation to finish before closing."</p>
            </Show>
        </section>
        <section class="panel"><div class="panel-heading"><h2>"Durable queue"</h2><div class="button-row">
            <button data-testid="queue-refresh" disabled=move ||ui.busy.get() on:click=move |_|ui.request::<Vec<QueueEntry>>("queue_list",json!({}),move |entries|ui.queue.set(entries))>"Refresh saved queue"</button>
            <button data-testid="resume-checkpoint" disabled=move ||ui.busy.get()||ui.active() on:click=move |_|ui.resume_picker()>"Resume checkpoint…"</button></div></div>
            <p class="hint">"Queue records survive restart. Restored source/output access must be regranted with native pickers and the original source fingerprint. Nothing starts automatically. Checkpoint resumes preserve corrections stored in that checkpoint, not a different review file. Save other corrections separately first. Changed source/model/configuration is rejected."</p>
            <Show when=move ||ui.queue.with(Vec::is_empty)><p class="muted">"No saved videos. Choose source/output in Extract and add to queue, or select an existing checkpoint."</p></Show>
            {move ||ui.queue.get().into_iter().enumerate().map(|(i,q)|{
                let start=q.id.clone();let regrant=q.id.clone();let remove=q.id.clone();let needs_regrant=q.needs_regrant;let completed=q.state=="completed";
                view!{<div class="queue-row" data-queue-id=q.id><span class="queue-order mono">{format!("{:02}",i+1)}</span><div><strong dir="auto">{q.name}</strong><p class="hint">{format!("{} · {} · {} → {} · {}",q.language,q.device,q.quality,q.output_name,q.state)}</p>
                    <p class="hint">{if needs_regrant{"Source/output regrant required after restart"}else{"Source/output granted in this session"}}</p>
                    {q.error.map(|e|view!{<p class="hint" role="status">{e}</p>})}</div>
                    <button data-testid="queue-regrant" disabled=move ||ui.active()||ui.busy.get() on:click=move |_|ui.request::<Option<Vec<QueueEntry>>>("queue_regrant",json!({"id":regrant.clone()}),move |entries|if let Some(entries)=entries{ui.queue.set(entries);})>"Regrant source/output"</button>
                    <button class="primary" data-testid="queue-start" disabled=move ||ui.active()||ui.busy.get()||needs_regrant||completed on:click=move |_|ui.start_saved(start.clone())>{if q.resumable{"Resume saved work"}else{"Start"}}</button>
                    <button data-testid="queue-remove" disabled=move ||ui.busy.get()||ui.active() aria-label=format!("Remove saved queue entry {}",i+1) on:click=move |_|ui.request::<Vec<QueueEntry>>("queue_remove",json!({"id":remove.clone()}),move |entries|ui.queue.set(entries))>"Remove record"</button></div>}
            }).collect_view()}
        </section>
    </div>}
}

#[component]
pub fn History(ui: Ui) -> impl IntoView {
    view! {<section class="panel"><div class="panel-heading"><div><span class="eyebrow">"SAVED WORK"</span><h2>"Project history"</h2></div>
        <div class="button-row"><button disabled=move ||ui.busy.get() on:click=move |_|ui.request::<Vec<HistoryEntry>>("list_history",json!({}),move |r|ui.recent.set(r))>"Refresh history"</button>
            <button disabled=move ||ui.busy.get() on:click=move |_|ui.request::<Option<nvsf_core::Project>>("pick_project",json!({}),move |p|if let Some(p)=p{ui.load(p);})>"Open project…"</button></div></div>
        <p class="muted">"Recent completed projects from the application data root. Open an external review project with the native picker."</p>
        <Show when=move ||ui.recent.with(Vec::is_empty)><p class="empty-list">"No history loaded. Refresh to read actual saved projects."</p></Show>
        {move ||ui.recent.get().into_iter().map(|entry|{
            let token=entry.token.clone();view!{<div class="history-row"><div><strong dir="auto">{entry.name}</strong><p class="hint mono">{entry.modified.map(|t|format!("Modified UTC epoch {t}")).unwrap_or_else(||"Modified time unavailable".into())}</p></div>
                <button disabled=move ||ui.busy.get()||ui.active() on:click=move |_|ui.request::<nvsf_core::Project>("open_history",json!({"token":token.clone()}),move |p|ui.load(p))>"Open review"</button></div>}
        }).collect_view()}
    </section>}
}

#[component]
pub fn Settings(ui: Ui) -> impl IntoView {
    view! {<div class="workspace-column"><section class="panel"><span class="eyebrow">"WORKSPACE PREFERENCES"</span><h2>"Appearance"</h2>
        <label for="theme">"Color theme"</label><select id="theme" prop:value=move ||ui.theme.get() on:change=move |e|ui.set_theme(event_target_value(&e))>
            <option value="system">"System · follows OS changes"</option><option value="dark">"Dark"</option><option value="light">"Light"</option></select>
        <p class="hint">"The saved preference is applied before this window appears. Reduced motion follows the operating system; original frame pixels are never recolored."</p>
        <h3>"Keyboard and text"</h3><dl class="shortcut-list"><dt>"Ctrl/Cmd + S"</dt><dd>"Save current review project"</dd><dt>"Ctrl/Cmd + Z / Shift + Z"</dt><dd>"Undo / redo cue edits (outside text fields)"</dd><dt>"Tab / Shift + Tab"</dt><dd>"Navigate every control"</dd></dl>
        <p class="hint">"Persian text is kept in logical Unicode with direction determined by its content. Japanese composition is not intercepted. Timestamps use ASCII digits."</p>
    </section><section class="panel"><div class="panel-heading"><h2>"Resources and diagnostics"</h2><button disabled=move ||ui.busy.get() on:click=move |_|ui.request::<Doctor>("doctor",json!({}),move |d|ui.diagnostics.set(Some(d)))>"Run resource checks"</button></div>
        <Show when=move ||ui.diagnostics.with(Option::is_some) fallback=||view!{<p class="muted">"Check actual data paths, model status and diagnostic logging. Directory presence is not proof of a functioning acceleration provider."</p>}>
            {move ||ui.diagnostics.get().map(|d|view!{<dl class="diagnostics"><dt>"Platform"</dt><dd>{d.platform}</dd><dt>"Data root"</dt><dd class="mono" dir="auto">{d.data_root}</dd><dt>"Models"</dt><dd class="mono" dir="auto">{d.models_root}</dd><dt>"Packaged runtime directory"</dt><dd>{if d.runtimes_present{"Present (not execution-verified)"}else{"Missing"}}</dd><dt>"Diagnostic file log"</dt><dd>{if d.log_available{"Initialized"}else{"Unavailable; console fallback"}}</dd><dt>"Catalog entries"</dt><dd>{d.models.len()}</dd></dl><p class="hint">{d.message}</p>})}
        </Show>
    </section></div>}
}
