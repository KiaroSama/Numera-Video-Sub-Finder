use super::timeline::PresentationClock;
use ffmpeg_next::Rational;

#[test]
fn source_pts_and_duration_preserve_vfr_nonzero_and_negative_origins() {
    for origin in [-90_000, 450_000] {
        let mut clock = PresentationClock::new(Some(origin), Rational(1, 90_000)).unwrap();
        assert_eq!(clock.observe(origin, 3_000).unwrap(), Some((0, 33)));
        assert_eq!(
            clock.observe(origin + 3_000, 9_000).unwrap(),
            Some((33, 100))
        );
        assert_eq!(
            clock.observe(origin + 12_000, 4_500).unwrap(),
            Some((133, 50))
        );
    }
}

#[test]
fn seek_preroll_is_discarded_without_rebasing_the_source_timeline() {
    let mut clock = PresentationClock::new(Some(5_000), Rational(1, 1_000)).unwrap();
    assert_eq!(clock.observe(4_900, 40).unwrap(), None);
    assert_eq!(clock.observe(4_960, 40).unwrap(), None);
    assert_eq!(clock.observe(5_000, 40).unwrap(), Some((0, 40)));
    assert_eq!(clock.observe(6_001, 40).unwrap(), Some((1_001, 40)));
}

#[test]
fn missing_origin_uses_first_source_pts_and_missing_duration_has_bounded_uncertainty() {
    let mut clock = PresentationClock::new(None, Rational(1, 1_000)).unwrap();
    assert_eq!(clock.observe(-30, 0).unwrap(), Some((0, 40)));
    assert_eq!(clock.observe(70, 0).unwrap(), Some((100, 100)));
    assert_eq!(clock.observe(10_070, 0).unwrap(), Some((10_100, 1_000)));
}

#[test]
fn backward_submillisecond_pts_is_not_hidden_by_millisecond_rounding() {
    let mut clock = PresentationClock::new(Some(0), Rational(1, 90_000)).unwrap();
    assert_eq!(clock.observe(90, 0).unwrap().unwrap().0, 1);
    assert!(clock.observe(89, 0).is_err());
    assert!(PresentationClock::new(None, Rational(1, 0)).is_err());
    let mut overflow = PresentationClock::new(None, Rational(i32::MAX, 1)).unwrap();
    assert!(overflow.observe(i64::MAX, 0).is_err());
}
