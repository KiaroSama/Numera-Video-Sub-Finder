use super::geometry::{DisplayTransform, display_size, packed_rgb};
use ffmpeg_next as ffmpeg;
use image::{Rgb, RgbImage};

fn matrix(a: i32, b: i32, c: i32, d: i32) -> Vec<u8> {
    [a << 16, b << 16, 0, c << 16, d << 16, 0, 0, 0, 1 << 30]
        .into_iter()
        .flat_map(i32::to_ne_bytes)
        .collect()
}

fn labels(image: &RgbImage) -> Vec<u8> {
    image.pixels().map(|p| p[0]).collect()
}

#[test]
fn display_matrix_preserves_all_eight_orthogonal_orientations_and_vertical_layout() {
    let image = RgbImage::from_fn(3, 2, |x, y| Rgb([(y * 3 + x + 1) as u8, 0, 0]));
    for (coefficients, dimensions, expected) in [
        ((1, 0, 0, 1), (3, 2), vec![1, 2, 3, 4, 5, 6]),
        ((0, 1, -1, 0), (2, 3), vec![4, 1, 5, 2, 6, 3]),
        ((-1, 0, 0, -1), (3, 2), vec![6, 5, 4, 3, 2, 1]),
        ((0, -1, 1, 0), (2, 3), vec![3, 6, 2, 5, 1, 4]),
        ((-1, 0, 0, 1), (3, 2), vec![3, 2, 1, 6, 5, 4]),
        ((1, 0, 0, -1), (3, 2), vec![4, 5, 6, 1, 2, 3]),
        ((0, 1, 1, 0), (2, 3), vec![1, 4, 2, 5, 3, 6]),
        ((0, -1, -1, 0), (2, 3), vec![6, 3, 5, 2, 4, 1]),
    ] {
        let (a, b, c, d) = coefficients;
        let transform = DisplayTransform::from_matrix(&matrix(a, b, c, d)).unwrap();
        let output = transform.apply(image.clone());
        assert_eq!(output.dimensions(), dimensions);
        assert_eq!(labels(&output), expected, "matrix={coefficients:?}");
        assert_eq!(transform.dimensions(3, 2), dimensions);
    }
}

#[test]
fn display_matrix_is_not_replaced_by_a_stale_legacy_rotation_tag() {
    let clockwise = matrix(0, 1, -1, 0);
    let transform = DisplayTransform::resolve(Some(&clockwise), Some("180")).unwrap();
    assert_eq!(transform.rotation(), 90);
    assert_eq!(
        DisplayTransform::resolve(None, Some("-90"))
            .unwrap()
            .rotation(),
        270
    );
    assert_eq!(DisplayTransform::resolve(None, None).unwrap().rotation(), 0);
    for tag in ["not a rotation", "45", "90.5"] {
        assert!(DisplayTransform::resolve(None, Some(tag)).is_err(), "{tag}");
    }
}

#[test]
fn frame_display_matrix_overrides_container_transform_without_changing_source_bytes() {
    let stream = DisplayTransform::resolve(None, Some("180")).unwrap();
    let mut frame = ffmpeg::util::frame::Video::new(ffmpeg::format::Pixel::RGB24, 2, 3);
    assert_eq!(stream.for_frame(&frame).unwrap().rotation(), 180);
    let clockwise = matrix(0, 1, -1, 0);
    let mut data = frame
        .new_side_data(ffmpeg::util::frame::side_data::Type::DisplayMatrix, 36)
        .unwrap();
    // SAFETY: new_side_data owns36 writable bytes in this live frame; source is separate bytes.
    unsafe { std::ptr::copy_nonoverlapping(clockwise.as_ptr(), (*data.as_mut_ptr()).data, 36) };
    let transform = stream.for_frame(&frame).unwrap();
    assert_eq!(transform.rotation(), 90);
    assert_eq!(transform.dimensions(2, 3), (3, 2));
    assert_eq!(
        frame
            .side_data(ffmpeg::util::frame::side_data::Type::DisplayMatrix)
            .unwrap()
            .data(),
        clockwise
    );
}

#[test]
fn malformed_perspective_and_nonorthogonal_matrices_fail_instead_of_lying_about_geometry() {
    for bytes in [
        vec![],
        vec![0; 35],
        vec![0; 36],
        vec![0; 37],
        matrix(1, 1, 0, 1),
    ] {
        assert!(DisplayTransform::from_matrix(&bytes).is_err());
    }
    let mut perspective = matrix(1, 0, 0, 1);
    perspective[8..12].copy_from_slice(&1i32.to_ne_bytes());
    assert!(DisplayTransform::from_matrix(&perspective).is_err());
}

#[test]
fn sample_aspect_is_applied_before_rotation_with_checked_display_pixel_budget() {
    assert_eq!(
        display_size(720, 576, ffmpeg::Rational(16, 15)).unwrap(),
        (768, 576)
    );
    assert_eq!(
        display_size(720, 480, ffmpeg::Rational(8, 9)).unwrap(),
        (640, 480)
    );
    assert_eq!(display_size(3, 5, ffmpeg::Rational(3, 2)).unwrap(), (5, 5));
    assert_eq!(display_size(3, 5, ffmpeg::Rational(0, 1)).unwrap(), (3, 5));
    let transform = DisplayTransform::resolve(None, Some("90")).unwrap();
    let (width, height) = display_size(720, 576, ffmpeg::Rational(16, 15)).unwrap();
    assert_eq!(transform.dimensions(width, height), (576, 768));
    for (width, height, sar) in [
        (0, 10, ffmpeg::Rational(1, 1)),
        (u32::MAX, u32::MAX, ffmpeg::Rational(1, 1)),
        (7680, 4320, ffmpeg::Rational(2, 1)),
        (2, 2, ffmpeg::Rational(-1, 1)),
        (2, 2, ffmpeg::Rational(1, 0)),
    ] {
        assert!(display_size(width, height, sar).is_err());
    }
}

#[test]
fn packed_rgb_excludes_native_row_padding_and_rejects_short_or_overflowing_strides() {
    let mut rgb = ffmpeg::util::frame::Video::new(ffmpeg::format::Pixel::RGB24, 3, 2);
    let stride = rgb.stride(0);
    rgb.data_mut(0).fill(199);
    rgb.data_mut(0)[..9].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9]);
    rgb.data_mut(0)[stride..stride + 9].copy_from_slice(&[10, 11, 12, 13, 14, 15, 16, 17, 18]);
    let image = packed_rgb(&rgb).unwrap();
    assert_eq!(image.dimensions(), (3, 2));
    assert_eq!(image.as_raw(), &(1..=18).collect::<Vec<u8>>());
    assert_eq!(rgb.data(0)[9], 199, "source padding must not be modified");
    // SAFETY: only descriptor metadata changes; the allocated buffer remains owned by rgb.
    unsafe { (*rgb.as_mut_ptr()).linesize[0] = 1 };
    assert!(packed_rgb(&rgb).is_err());
    // SAFETY: exercise arithmetic admission without resizing or dereferencing the native buffer.
    unsafe { (*rgb.as_mut_ptr()).linesize[0] = i32::MAX };
    assert!(packed_rgb(&rgb).is_err());
}
