use super::*;
use image::{Rgb, RgbImage};

#[test]
fn detector_retains_deployed_scale_and_validates_dimensions() {
    for (source, expected) in [
        ((320, 160), (960, 480)),
        ((1920, 1080), (960, 544)),
        ((80, 112), (672, 960)),
        ((112, 80), (960, 672)),
        ((4000, 1), (960, 32)),
        ((1, 1), (960, 960)),
    ] {
        assert_eq!(detector_size(source.0, source.1).unwrap(), expected);
    }
    assert!(detector_size(0, 48).is_err());
    assert!(detector_size(48, 0).is_err());
    assert!(detector_size(8192, 8192).is_err());
}

#[test]
fn recognition_uses_floor_padding_ceil_content_and_no_minimum_eight() {
    for (source, expected) in [
        ((1, 48), (1, 320)),
        ((1, 96), (1, 320)),
        ((320, 48), (320, 320)),
        ((1001, 100), (480, 480)),
        ((10000, 48), (3200, 3200)),
    ] {
        assert_eq!(recognition_size(source.0, source.1).unwrap(), expected);
    }
    assert!(recognition_size(0, 48).is_err());
}

#[test]
fn detection_coordinates_follow_source_axes() {
    for (left, right, expected) in [
        (1, 3, vec![(0, 2, 6, 10)]),
        (7, 10, vec![(5, 2, 8, 10)]),
        (20, 23, vec![(18, 2, 8, 10)]),
    ] {
        let mut map = Array4::<f32>::zeros((1, 1, 32, 32));
        for y in 4..8 {
            for x in left..=right {
                map[[0, 0, y, x]] = 1.0;
            }
        }
        assert_eq!(
            detected_regions(map.view().into_dyn(), 32, 32, 32, 40).unwrap(),
            expected
        );
    }
}

#[test]
fn asymmetric_resize_preserves_spatial_channels_without_modifying_source() {
    let source = RgbImage::from_fn(80, 96, |x, _| {
        if x < 40 {
            Rgb([255, 0, 0])
        } else {
            Rgb([0, 0, 255])
        }
    });
    let original = source.clone();
    let tensor = detector(&source).unwrap();
    assert_eq!(tensor.shape(), [1, 3, 960, 800]);
    assert!((tensor[[0, 2, 0, 0]] - 2.64).abs() < 0.001);
    assert!((tensor[[0, 0, 959, 799]] - 2.2489083).abs() < 0.001);
    assert!((tensor[[0, 2, 959, 799]] + 1.8044444).abs() < 0.001);
    let tensor = recognition(&source).unwrap();
    assert_eq!(tensor.shape(), [1, 3, 48, 320]);
    assert_eq!(tensor[[0, 2, 0, 0]], 1.0);
    assert_eq!(tensor[[0, 0, 47, 39]], 1.0);
    assert_eq!(tensor[[0, 0, 47, 40]], 0.0);
    assert_eq!(source, original);
}

#[test]
fn tensors_preserve_bgr_channel_order_and_zero_normalized_padding() {
    let image = RgbImage::from_pixel(1, 48, Rgb([255, 0, 0]));
    let tensor = recognition(&image).unwrap();
    assert_eq!(tensor.shape(), [1, 3, 48, 320]);
    assert_eq!(tensor[[0, 0, 0, 0]], -1.0);
    assert_eq!(tensor[[0, 1, 0, 0]], -1.0);
    assert_eq!(tensor[[0, 2, 0, 0]], 1.0);
    for channel in 0..3 {
        assert_eq!(tensor[[0, channel, 0, 1]], 0.0);
        assert_eq!(tensor[[0, channel, 47, 319]], 0.0);
    }
    let tiny = RgbImage::from_pixel(1, 1, Rgb([255, 0, 0]));
    let tensor = detector(&tiny).unwrap();
    assert_eq!(tensor.shape(), [1, 3, 960, 960]);
    assert!((tensor[[0, 2, 0, 0]] - 2.64).abs() < 0.001);
    assert!((tensor[[0, 2, 959, 959]] - 2.64).abs() < 0.001);
    assert_eq!(tiny.get_pixel(0, 0).0, [255, 0, 0]);
}
