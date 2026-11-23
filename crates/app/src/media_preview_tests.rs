use super::Preview;
use crate::media::DecodedFrame;
use image::{Rgb, RgbImage};

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
