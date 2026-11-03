use super::*;

fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect::new(x, y, width, height).unwrap()
}

fn contains(outer: Rect, inner: Rect) -> bool {
    outer.x <= inner.x
        && outer.y <= inner.y
        && outer.x + outer.width >= inner.x + inner.width
        && outer.y + outer.height >= inner.y + inner.height
}

#[test]
fn full_frame_proposals_preserve_middle_and_disconnected_regions() {
    let locations = vec![
        rect(0.1, 0.08, 0.3, 0.05),
        rect(0.1, 0.45, 0.2, 0.06),
        rect(0.65, 0.45, 0.2, 0.06),
        rect(0.4, 0.85, 0.4, 0.05),
        rect(0.92, 0.3, 0.04, 0.3),
    ];
    let result = proposals(&[locations.clone(), locations.clone()]).unwrap();
    assert_eq!(result.len(), 5);
    for location in locations {
        assert_eq!(result.iter().filter(|&&r| contains(r, location)).count(), 1);
    }
}

#[test]
fn repeated_jitter_is_one_proposal_with_all_observed_bounds() {
    let a = rect(0.2, 0.45, 0.3, 0.05);
    let b = rect(0.21, 0.455, 0.3, 0.05);
    let result = proposals(&[vec![a, a], vec![b]]).unwrap();
    assert_eq!(result.len(), 1);
    assert!(contains(result[0], a));
    assert!(contains(result[0], b));
}

#[test]
fn proposals_are_order_independent_and_fixed_seeds_do_not_bridge_regions() {
    let a = rect(0.1, 0.45, 0.2, 0.06);
    let b = rect(0.16, 0.45, 0.2, 0.06);
    let c = rect(0.24, 0.45, 0.2, 0.06);
    let moments = vec![vec![a, b, c], vec![a, b, c]];
    let result = proposals(&moments).unwrap();
    assert_eq!(result.len(), 2);
    let reversed = moments
        .into_iter()
        .rev()
        .map(|mut m| {
            m.reverse();
            m
        })
        .collect::<Vec<_>>();
    assert_eq!(proposals(&reversed).unwrap(), result);
}

#[test]
fn repeated_moments_rank_above_single_frame_duplicates_at_eight_region_limit() {
    let transient = (0..8).map(|i| rect(i as f64 * 0.12, 0.1, 0.06, 0.04));
    let repeated = rect(0.4, 0.8, 0.2, 0.05);
    let mut first = transient.collect::<Vec<_>>();
    first.extend(std::iter::repeat_n(first[0], 30));
    first.push(repeated);
    let result = proposals(&[first, vec![repeated]]).unwrap();
    assert_eq!(result.len(), 8);
    assert!(result.iter().any(|&r| contains(r, repeated)));
    assert!(
        !result
            .iter()
            .any(|&r| contains(r, rect(0.84, 0.1, 0.06, 0.04)))
    );
}

#[test]
fn proposals_clip_padding_and_fail_without_inventing_bottom_regions() {
    let result = proposals(&[vec![rect(0.0, 0.0, 0.1, 0.05), rect(0.9, 0.95, 0.1, 0.05)]]).unwrap();
    assert_eq!(result.len(), 2);
    for &r in &result {
        r.validate().unwrap();
    }
    assert_eq!((result[0].x, result[0].y), (0.0, 0.0));
    assert_eq!(result[1].x + result[1].width, 1.0);
    assert_eq!(result[1].y + result[1].height, 1.0);
    for empty in [vec![], vec![vec![]], vec![vec![], vec![]]] {
        assert!(proposals(&empty).is_err());
    }
    assert!(proposals(&vec![vec![]; 6]).is_err());
    assert!(proposals(&[vec![Rect::default(); 1001]]).is_err());
    let invalid = Rect {
        x: f64::NAN,
        ..Rect::default()
    };
    assert!(proposals(&[vec![invalid]]).is_err());
}
