use super::{Preview, candidate_preview_timestamps, is_near_black_rgb};
use crate::media::DecodedFrame;
use image::{Rgb, RgbImage};

#[test]
fn near_black_requires_low_mean_and_low_bright_pixel_share() {
    assert!(is_near_black_rgb(&[0; 3]));
    assert!(is_near_black_rgb(&[11; 3]));
    assert!(!is_near_black_rgb(&[12; 3]));
    assert!(!is_near_black_rgb(&[160; 3]));
    assert!(!is_near_black_rgb(&[]));
    assert!(!is_near_black_rgb(&[0, 0]));

    let mut dark = [0; 3 * 20];
    dark[..3].fill(24);
    assert!(is_near_black_rgb(&dark));
    dark[57..].fill(24);
    assert!(!is_near_black_rgb(&dark));
}

#[test]
fn preview_timestamps_are_bounded_sorted_and_within_the_clip() {
    assert_eq!(
        candidate_preview_timestamps(None),
        vec![0, 1500, 4000, 10_000]
    );
    assert_eq!(candidate_preview_timestamps(Some(0)), Vec::<u64>::new());
    assert_eq!(candidate_preview_timestamps(Some(1)), vec![0]);
    assert_eq!(
        candidate_preview_timestamps(Some(2000)),
        vec![0, 200, 500, 1500]
    );
    assert_eq!(
        candidate_preview_timestamps(Some(26 * 60 * 1000)),
        vec![0, 1500, 4000, 10_000, 156_000, 390_000]
    );
}

#[test]
fn initial_preview_stops_at_the_first_visible_frame_or_decode_failure() {
    use crate::media::MediaRuntime;
    let mut seen = Vec::new();
    let time = MediaRuntime::initial_preview_timestamp(None, |time| {
        seen.push(time);
        let value = if time == 0 { 0 } else { 80 };
        Some(frame(time, 33, value).image)
    });
    assert_eq!(time, 1500);
    assert_eq!(seen, [0, 1500]);
    assert_eq!(MediaRuntime::initial_preview_timestamp(None, |_| None), 0);
    assert_eq!(
        MediaRuntime::initial_preview_timestamp(None, |time| Some(frame(time, 33, 0).image)),
        0
    );
}

fn frame(time_ms: u64, estimated_duration_ms: u64, value: u8) -> DecodedFrame {
    DecodedFrame {
        image: RgbImage::from_pixel(1, 1, Rgb([value; 3])),
        time_ms,
        estimated_duration_ms,
    }
}

#[test]
fn preview_uses_source_pts_for_variable_frame_intervals_and_final_duration() {
    for (at, expected) in [(0, 1), (32, 1), (33, 2), (199, 2), (200, 3), (249, 3)] {
        let mut preview = Preview::new(at);
        for (time, duration, value) in [(0, 33, 1), (33, 20, 2), (200, 50, 3)] {
            if !preview.observe(frame(time, duration, value)) {
                break;
            }
        }
        assert_eq!(
            preview.finish().unwrap().get_pixel(0, 0)[0],
            expected,
            "at={at}"
        );
    }
    let mut preview = Preview::new(250);
    assert!(preview.observe(frame(200, 50, 3)));
    assert!(preview.finish().is_err());
}

#[test]
fn preview_never_substitutes_a_later_frame_when_no_preceding_frame_exists() {
    let mut preview = Preview::new(49);
    assert!(!preview.observe(frame(50, 40, 1)));
    assert!(preview.finish().is_err());
    assert!(Preview::new(0).finish().is_err());
}
