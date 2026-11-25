use crate::{helpers, roi_geometry, state::Ui, undo::RegionId};
use leptos::{ev, prelude::*};
use nvsf_core::Rect;
use web_sys::MouseEvent;

const HANDLES: [(&str, (i8, i8)); 8] = [
    ("nw", (-1, -1)),
    ("n", (0, -1)),
    ("ne", (1, -1)),
    ("e", (1, 0)),
    ("se", (1, 1)),
    ("s", (0, 1)),
    ("sw", (-1, 1)),
    ("w", (-1, 0)),
];
const GUIDES: [(&str, (i8, i8)); 4] = [
    ("HSL1", (0, -1)),
    ("HSL2", (0, 1)),
    ("VSL1", (-1, 0)),
    ("VSL2", (1, 0)),
];

#[derive(Clone, Copy)]
enum Gesture {
    Draw(bool),
    Edit(RegionId, Rect, (i8, i8)),
}

#[derive(Clone, Copy)]
pub(crate) struct Drag {
    gesture: Gesture,
    start: (f64, f64),
    current: (f64, f64),
}

impl Drag {
    fn rect(self) -> Option<Rect> {
        match self.gesture {
            Gesture::Draw(_) => roi_geometry::rubber_band(self.start, self.current),
            Gesture::Edit(_, original, edges) => roi_geometry::transform(
                original,
                (self.current.0 - self.start.0, self.current.1 - self.start.1),
                edges,
            ),
        }
    }
}

fn begin_edit(
    ui: Ui,
    bounds: NodeRef<leptos::html::Div>,
    e: &MouseEvent,
    id: RegionId,
    edges: (i8, i8),
) {
    if e.button() != 0 || ui.closing.get_untracked() {
        return;
    }
    e.stop_propagation();
    let Some(start) = frame_point(bounds, e) else {
        return;
    };
    let Some(rect) = ui
        .config
        .with_untracked(|c| id.list(c).get(id.index).copied())
    else {
        return;
    };
    ui.studio.roi.set(Some(id));
    ui.studio.roi_drag.set(Some(Drag {
        gesture: Gesture::Edit(id, rect, edges),
        start,
        current: start,
    }));
}

fn frame_point(bounds: NodeRef<leptos::html::Div>, e: &MouseEvent) -> Option<(f64, f64)> {
    let rect = bounds.get_untracked()?.get_bounding_client_rect();
    helpers::frame_point(
        f64::from(e.client_x()),
        f64::from(e.client_y()),
        [rect.left(), rect.top(), rect.width(), rect.height()],
    )
}

fn displayed(ui: Ui, id: RegionId, original: Rect) -> Rect {
    ui.studio.roi_drag.with(|drag| {
        drag.filter(|drag| matches!(drag.gesture, Gesture::Edit(current, _, _) if current==id))
            .and_then(Drag::rect)
            .unwrap_or(original)
    })
}

fn guide_style(rect: Rect, edges: (i8, i8)) -> String {
    let (axis, offset) = match edges {
        (0, -1) => ("top", rect.y),
        (0, 1) => ("top", rect.y + rect.height),
        (-1, 0) => ("left", rect.x),
        _ => ("left", rect.x + rect.width),
    };
    format!("{axis}:{}%", offset * 100.0)
}

#[component]
pub(crate) fn RoiCanvas(
    ui: Ui,
    editor: bool,
    exclusion: RwSignal<bool>,
    guides: RwSignal<bool>,
) -> impl IntoView {
    let bounds = NodeRef::<leptos::html::Div>::new();
    if editor {
        let movement = window_event_listener(ev::mousemove, move |e| {
            if let Some(mut drag) = ui.studio.roi_drag.get_untracked()
                && let Some(point) = frame_point(bounds, &e)
            {
                drag.current = point;
                ui.studio.roi_drag.set(Some(drag));
            }
        });
        let release = window_event_listener(ev::mouseup, move |e| {
            if e.button() != 0 {
                return;
            }
            if let Some(mut drag) = ui.studio.roi_drag.get_untracked() {
                ui.studio.roi_drag.set(None);
                if let Some(point) = frame_point(bounds, &e)
                    && ui.route.get_untracked() == "Extract"
                    && !ui.closing.get_untracked()
                {
                    drag.current = point;
                    match drag.gesture {
                        Gesture::Draw(excluded) => {
                            if let Some(rect) = roi_geometry::draw(drag.start, drag.current) {
                                ui.add_region(excluded, rect);
                            }
                        }
                        Gesture::Edit(id, _, _) if drag.current != drag.start => {
                            if let Some(rect) = drag.rect() {
                                ui.update_region(id, rect);
                            }
                        }
                        Gesture::Edit(..) => {}
                    }
                }
            }
        });
        let blur = window_event_listener(ev::blur, move |_| ui.studio.roi_drag.set(None));
        on_cleanup(move || {
            movement.remove();
            release.remove();
            blur.remove();
        });
    }
    let regions = move |excluded| {
        let id = RegionId { excluded, index: 0 };
        let rectangles = if editor {
            ui.config.with(|c| id.list(c).to_vec())
        } else {
            ui.project.with(|project| {
                project
                    .as_ref()
                    .map(|p| id.list(&p.config).to_vec())
                    .unwrap_or_default()
            })
        };
        rectangles.into_iter().enumerate().map(|(index,rect)| {
            let id = RegionId {excluded, index};
            let label = if excluded {format!("Exclude {}", index+1)} else {format!("R{}",index+1)};
            view! {
                <div class="roi" class:excluded=excluded class:editable=editor
                    class:selected=move ||editor&&ui.studio.roi.get()==Some(id)
                    style=move ||helpers::region_style(if editor {displayed(ui,id,rect)} else {rect})
                    on:mousedown=move |e|if editor {begin_edit(ui,bounds,&e,id,(0,0))}>
                    {if editor {
                        let name = if excluded {format!("Select exclusion {}",index+1)} else {format!("Select region {}",index+1)};
                        view! {<button type="button" class="roi-pick" aria-label=name aria-pressed=move ||if ui.studio.roi.get()==Some(id){"true"}else{"false"}
                            on:click=move |_|ui.studio.roi.set(Some(id))>{label}</button>}.into_any()
                    } else {view! {<span class="roi-label">{label}</span>}.into_any()}}
                    {editor.then(||HANDLES.into_iter().map(|(name,edges)|view! {
                        <div class="roi-handle" data-handle=name aria-hidden="true"
                            on:mousedown=move |e|begin_edit(ui,bounds,&e,id,edges)></div>
                    }).collect_view())}
                </div>
            }
        }).collect_view()
    };
    view! {
        <div class="frame" class:editable=editor node_ref=bounds
            style=move ||ui.source.with(|s|s.as_ref().map(|s|format!("--frame-ratio:{};--zoom-width:{}%", f64::from(s.media.width.max(1))/f64::from(s.media.height.max(1)),ui.studio.zoom.get())).unwrap_or_default())
            on:mousedown=move |e| {
                if editor && e.button()==0 && !ui.closing.get_untracked() && let Some(point)=frame_point(bounds,&e) {
                    ui.studio.roi.set(None);
                    ui.studio.roi_drag.set(Some(Drag {gesture:Gesture::Draw(exclusion.get_untracked()), start:point,current:point}));
                }
            }>
            <img class="source-frame" src=move ||ui.preview_url.get() alt="Original decoded source frame" draggable="false"/>
            {move ||regions(false)}{move ||regions(true)}
            {editor.then(||view! {
                {move || {
                    let id = ui.studio.roi.get().filter(|id| !id.excluded && guides.get())?;
                    let rect = ui.config.with(|c| id.list(c).get(id.index).copied())?;
                    Some(GUIDES.into_iter().map(|(name,edges)|view! {
                        <div class="roi-guide" class:vertical={edges.0 != 0} data-guide=name aria-hidden="true"
                            style=move ||guide_style(displayed(ui,id,rect),edges)
                            on:mousedown=move |e|begin_edit(ui,bounds,&e,id,edges)></div>
                    }).collect_view())
                }}
                {move || {
                    let drag = ui.studio.roi_drag.get()?;
                    let Gesture::Draw(excluded)=drag.gesture else {return None;};
                    let rect = drag.rect()?;
                    Some(view! {<div class="roi-draft" class:excluded=excluded style=helpers::region_style(rect)>
                        <span class="roi-label mono">{format!("{:.2} × {:.2}",rect.width,rect.height)}</span>
                    </div>})
                }}
            })}
        </div>
    }
}

#[component]
pub(crate) fn Regions(ui: Ui) -> impl IntoView {
    let value = RwSignal::new("0,0.7,1,0.3".to_string());
    let exclude = RwSignal::new(false);
    let rows = move |excluded| {
        ui.config.with(|c| {
        RegionId {excluded,index:0}.list(c).iter().copied().enumerate().map(|(index,rect)| {
            let id = RegionId {excluded,index};
            let number = index+1;
            let kind = if excluded { "Exclusion" } else { "Region" };
            let field = format!("{kind} {number} normalized coordinates");
            let remove = format!("Remove {} {number}",kind.to_ascii_lowercase());
            let remove_disabled = !excluded && c.regions.len() == 1;
            view! {
                <div class="region-row" class:selected=move ||ui.studio.roi.get()==Some(id)>
                    <button class="region-select" type="button" aria-pressed=move ||if ui.studio.roi.get()==Some(id){"true"}else{"false"}
                        on:click=move |_|ui.studio.roi.set(Some(id))>{format!("{kind} {number}")}</button>
                    <input aria-label=field prop:value=format!("{:.4},{:.4},{:.4},{:.4}",rect.x,rect.y,rect.width,rect.height)
                        on:focus=move |_|ui.studio.roi.set(Some(id))
                        on:change={move |e|match event_target_value(&e).parse::<Rect>() {
                            Ok(rect)=>ui.update_region(id,rect),
                            Err(error)=>ui.error.set(Some(error.to_string())),
                        }}/>
                    <button type="button" aria-label=remove disabled=remove_disabled on:click=move |_|{ui.remove_region(id);}>"Remove"</button>
                </div>
            }
        }).collect_view()
    })
    };
    view! {
        <details class="regions" open>
            <summary>"Regions and exclusions"</summary>
            <p class="hint">"Coordinates: x, y, width, height, each from 0 to 1."</p>
            <div class="region-list">{move ||rows(false)}</div>
            <div class="region-list">{move ||rows(true)}</div>
            <div class="region-add">
                <label for="new-region" class="sr-only">"New normalized region coordinates"</label>
                <input id="new-region" prop:value=move ||value.get() on:input=move |e|value.set(event_target_value(&e))/>
                <label class="check"><input type="checkbox" on:change=move |e|exclude.set(event_target_checked(&e))/><span>"Exclude"</span></label>
                <button type="button" on:click={move |_|match value.get_untracked().parse::<Rect>() {
                    Ok(rect)=>ui.add_region(exclude.get_untracked(),rect),
                    Err(error)=>ui.error.set(Some(error.to_string())),
                }}>"Add region"</button>
            </div>
        </details>
    }
}
