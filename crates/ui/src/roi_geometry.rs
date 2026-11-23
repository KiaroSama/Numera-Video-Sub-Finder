use nvsf_core::Rect;

const MIN_SIZE: f64 = 0.02;

pub(crate) fn rubber_band(a: (f64, f64), b: (f64, f64)) -> Option<Rect> {
    Rect::new(
        a.0.min(b.0),
        a.1.min(b.1),
        (b.0 - a.0).abs(),
        (b.1 - a.1).abs(),
    )
    .ok()
}

pub(crate) fn draw(a: (f64, f64), b: (f64, f64)) -> Option<Rect> {
    rubber_band(a, b).filter(|rect| rect.width >= MIN_SIZE && rect.height >= MIN_SIZE)
}

pub(crate) fn transform(rect: Rect, delta: (f64, f64), edges: (i8, i8)) -> Option<Rect> {
    if edges == (0, 0) {
        return Rect::new(
            (rect.x + delta.0).clamp(0.0, (1.0 - rect.width).max(0.0)),
            (rect.y + delta.1).clamp(0.0, (1.0 - rect.height).max(0.0)),
            rect.width,
            rect.height,
        )
        .ok();
    }
    let resize = |lo: f64, hi: f64, shift: f64, edge: i8| match edge {
        -1 => ((lo + shift).clamp(0.0, (hi - MIN_SIZE).max(0.0)), hi),
        1 => (lo, (hi + shift).clamp((lo + MIN_SIZE).min(1.0), 1.0)),
        _ => (lo, hi),
    };
    let (left, right) = resize(rect.x, (rect.x + rect.width).min(1.0), delta.0, edges.0);
    let (top, bottom) = resize(rect.y, (rect.y + rect.height).min(1.0), delta.1, edges.1);
    Rect::new(left, top, right - left, bottom - top).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_drag_produces_top_left_and_positive_size() {
        let r = draw((0.75, 0.75), (0.25, 0.5)).unwrap();
        assert_eq!(r, Rect::new(0.25, 0.5, 0.5, 0.25).unwrap());
        assert!(rubber_band((0.25, 0.5), (0.26, 0.51)).is_some());
        assert!(draw((0.25, 0.5), (0.26, 0.51)).is_none());
    }

    #[test]
    fn moving_clamps_to_frame_without_changing_size() {
        let r = Rect::new(0.25, 0.25, 0.5, 0.5).unwrap();
        assert_eq!(
            transform(r, (1.0, -1.0), (0, 0)),
            Rect::new(0.5, 0.0, 0.5, 0.5).ok()
        );
    }

    #[test]
    fn resizing_stops_at_minimum_size_and_leaves_opposite_edges_fixed() {
        let r = Rect::new(0.25, 0.25, 0.5, 0.5).unwrap();
        let changed = transform(r, (1.0, 1.0), (-1, -1)).unwrap();
        assert!((changed.width - MIN_SIZE).abs() < 1e-9);
        assert!((changed.height - MIN_SIZE).abs() < 1e-9);
        assert_eq!(changed.x + changed.width, 0.75);
        assert_eq!(changed.y + changed.height, 0.75);
    }
}
