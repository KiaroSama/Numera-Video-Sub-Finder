const MIN: u32 = 100;
const MAX: u32 = 800;
pub(crate) const PRESETS: [u32; 5] = [100, 150, 200, 300, 400];

pub(crate) fn next_zoom(current: u32, steps: i8) -> u32 {
    let levels = PRESETS.into_iter().chain([600, 800]);
    let mut zoom = current;
    for _ in 0..steps.unsigned_abs() {
        zoom = if steps > 0 {
            levels.clone().find(|&level| level > zoom).unwrap_or(MAX)
        } else {
            levels.clone().rfind(|&level| level < zoom).unwrap_or(MIN)
        };
    }
    if steps == 0 { MIN } else { zoom }
}

pub(crate) fn marquee_zoom_target(
    [x, y, w, h]: [f64; 4],
    viewport_w: f64,
    viewport_h: f64,
    frame_base_w: f64,
    frame_base_h: f64,
) -> (u32, f64, f64) {
    if ![x, y, w, h].into_iter().all(f64::is_finite)
        || ![viewport_w, viewport_h, frame_base_w, frame_base_h]
            .into_iter()
            .all(|n| n.is_finite() && n > 0.0)
    {
        return (MIN, 0.0, 0.0);
    }
    let x = x.clamp(0.0, 1.0);
    let y = y.clamp(0.0, 1.0);
    let w = w.clamp(0.0, 1.0 - x);
    let h = h.clamp(0.0, 1.0 - y);
    let scale = (viewport_w / (w * frame_base_w)).min(viewport_h / (h * frame_base_h));
    let zoom = (100.0 * scale)
        .floor()
        .clamp(f64::from(MIN), f64::from(MAX)) as u32;
    let scale = f64::from(zoom) / 100.0;
    (
        zoom,
        anchored_scroll(
            x + w / 2.0,
            viewport_w / 2.0,
            frame_base_w * scale,
            viewport_w,
        ),
        anchored_scroll(
            y + h / 2.0,
            viewport_h / 2.0,
            frame_base_h * scale,
            viewport_h,
        ),
    )
}

fn anchored_scroll(point: f64, at: f64, frame: f64, viewport: f64) -> f64 {
    (point * frame - at).clamp(0.0, (frame - viewport).max(0.0))
}

#[cfg(target_arch = "wasm32")]
mod dom {
    use super::*;
    use crate::{helpers, state::Ui};
    use leptos::{ev, prelude::*};
    use web_sys::{MouseEvent, WheelEvent};

    pub(crate) type Div = NodeRef<leptos::html::Div>;

    #[derive(Clone, Copy)]
    struct Layout {
        view: [f64; 4],
        frame: [f64; 4],
        base: (f64, f64),
    }

    impl Layout {
        fn read(viewport: Div, frame: &web_sys::Element, zoom: u32) -> Option<Self> {
            let viewport = viewport.get_untracked()?;
            let area = viewport.get_bounding_client_rect();
            let picture = frame.get_bounding_client_rect();
            let scale = f64::from(zoom) / 100.0;
            Some(Self {
                view: [
                    area.left(),
                    area.top(),
                    f64::from(viewport.client_width()),
                    f64::from(viewport.client_height()),
                ],
                frame: [
                    picture.left(),
                    picture.top(),
                    picture.width(),
                    picture.height(),
                ],
                base: (picture.width() / scale, picture.height() / scale),
            })
        }

        fn point(self, e: &MouseEvent) -> Option<(f64, f64)> {
            helpers::frame_point(f64::from(e.client_x()), f64::from(e.client_y()), self.frame)
        }

        fn click(self, e: &MouseEvent, zoom: u32) -> Option<(u32, f64, f64)> {
            let (x, y) = self.point(e)?;
            let [left, top, w, h] = self.view;
            let scale = f64::from(zoom) / 100.0;
            Some((
                zoom,
                anchored_scroll(x, f64::from(e.client_x()) - left, self.base.0 * scale, w),
                anchored_scroll(y, f64::from(e.client_y()) - top, self.base.1 * scale, h),
            ))
        }

        fn marquee(self, start: (f64, f64), end: (f64, f64)) -> ([f64; 4], (u32, f64, f64)) {
            let rect = [
                start.0.min(end.0),
                start.1.min(end.1),
                (start.0 - end.0).abs(),
                (start.1 - end.1).abs(),
            ];
            (
                rect,
                marquee_zoom_target(rect, self.view[2], self.view[3], self.base.0, self.base.1),
            )
        }
    }

    fn apply(ui: Ui, viewport: Div, (zoom, x, y): (u32, f64, f64)) {
        ui.studio.roi_drag.set(None);
        ui.studio.zoom.set(zoom);
        request_animation_frame(move || {
            if ui.studio.zoom.try_get_untracked() == Some(zoom)
                && let Some(el) = viewport.get_untracked()
            {
                el.set_scroll_left(x.round() as i32);
                el.set_scroll_top(y.round() as i32);
            }
        });
    }

    pub(crate) fn wheel(ui: Ui, viewport: Div, e: WheelEvent) {
        if !e.ctrl_key() || e.delta_y() == 0.0 {
            return;
        }
        e.prevent_default();
        let Some(frame) = viewport
            .get_untracked()
            .and_then(|el| el.query_selector(".frame").ok().flatten())
        else {
            return;
        };
        let zoom = ui.studio.zoom.get_untracked();
        let new = (f64::from(zoom) * (-e.delta_y().clamp(-100.0, 100.0) * 0.002).exp())
            .round()
            .clamp(f64::from(MIN), f64::from(MAX)) as u32;
        if new != zoom
            && let Some(target) =
                Layout::read(viewport, &frame, zoom).and_then(|layout| layout.click(&e, new))
        {
            apply(ui, viewport, target);
        }
    }

    pub(crate) fn enable_marquee(
        ui: Ui,
        viewport: Div,
        frame: Div,
    ) -> (
        impl Fn(MouseEvent) + Copy,
        impl Fn() -> Option<([f64; 4], u32)> + Copy,
    ) {
        let drag = RwSignal::new(None::<(Layout, (f64, f64), (f64, f64))>);
        let movement = window_event_listener(ev::mousemove, move |e| {
            if let Some((layout, start, _)) = drag.get_untracked()
                && let Some(end) = layout.point(&e)
            {
                drag.set(Some((layout, start, end)));
            }
        });
        let release = window_event_listener(ev::mouseup, move |e| {
            if e.button() != 0 {
                return;
            }
            if let Some((layout, start, _)) = drag.get_untracked() {
                drag.set(None);
                if !ui.studio.zoom_tool.get_untracked() || ui.closing.get_untracked() {
                    return;
                }
                let Some(end) = layout.point(&e) else {
                    return;
                };
                let (rect, target) = layout.marquee(start, end);
                if rect[2] * layout.frame[2] >= 10.0 || rect[3] * layout.frame[3] >= 10.0 {
                    apply(ui, viewport, target);
                } else if let Some(target) = layout.click(
                    &e,
                    next_zoom(
                        ui.studio.zoom.get_untracked(),
                        if e.alt_key() { -1 } else { 1 },
                    ),
                ) {
                    apply(ui, viewport, target);
                }
            }
        });
        let blur = window_event_listener(ev::blur, move |_| drag.set(None));
        Effect::new(move |_| {
            if !ui.studio.zoom_tool.get() {
                drag.set(None);
            }
        });
        on_cleanup(move || {
            movement.remove();
            release.remove();
            blur.remove();
        });
        (
            move |e: MouseEvent| {
                if let Some(layout) = frame
                    .get_untracked()
                    .and_then(|el| Layout::read(viewport, &el, ui.studio.zoom.get_untracked()))
                    && let Some(point) = layout.point(&e)
                {
                    e.prevent_default();
                    ui.studio.roi_drag.set(None);
                    drag.set(Some((layout, point, point)));
                }
            },
            move || {
                let (layout, start, end) = drag.get()?;
                let (rect, (zoom, _, _)) = layout.marquee(start, end);
                (start != end).then_some((rect, zoom))
            },
        )
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) use dom::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marquee_fits_and_centers_a_region_using_the_fitted_frame() {
        assert_eq!(
            marquee_zoom_target([0.25, 0.25, 0.5, 0.5], 800.0, 600.0, 800.0, 450.0),
            (200, 400.0, 150.0)
        );
    }

    #[test]
    fn marquee_caps_tiny_selections_and_handles_unmeasurable_frames() {
        assert_eq!(
            marquee_zoom_target([0.0, 0.0, 0.01, 0.01], 800.0, 600.0, 800.0, 450.0),
            (800, 0.0, 0.0)
        );
        assert_eq!(
            marquee_zoom_target([0.0, 0.0, 1.0, 1.0], 800.0, 600.0, 0.0, 450.0),
            (100, 0.0, 0.0)
        );
        assert_eq!(
            marquee_zoom_target([0.0, 0.0, 1.0, 1.0], 800.0, 600.0, 1600.0, 900.0),
            (100, 400.0, 150.0)
        );
    }

    #[test]
    fn click_keeps_the_same_pixel_under_the_mouse_where_scroll_allows_it() {
        assert_eq!(anchored_scroll(0.25, 200.0, 1600.0, 800.0), 200.0);
        assert_eq!(anchored_scroll(0.0, 200.0, 1600.0, 800.0), 0.0);
        assert_eq!(anchored_scroll(1.0, 200.0, 1600.0, 800.0), 800.0);
        assert_eq!(next_zoom(285, -1), 200);
        assert_eq!(next_zoom(285, 1), 300);
    }
}
