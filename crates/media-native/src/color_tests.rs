use super::color::DisplayScaler;
use ffmpeg_next::{self as ffmpeg, color, format::Pixel, util::frame::Video};

#[test]
fn frame_scaler_preserves_sdr_rgb_pixels_and_accepts_changed_frame_dimensions() {
    let mut scaler = DisplayScaler::new().unwrap();
    for (width, height) in [(3, 2), (2, 5), (3, 2)] {
        let mut frame = Video::new(Pixel::RGB24, width, height);
        frame.set_color_primaries(color::Primaries::BT709);
        frame.set_color_transfer_characteristic(color::TransferCharacteristic::IEC61966_2_1);
        frame.set_color_range(color::Range::JPEG);
        let stride = frame.stride(0);
        for y in 0..height as usize {
            for x in 0..width as usize {
                frame.data_mut(0)[y * stride + x * 3..y * stride + x * 3 + 3]
                    .copy_from_slice(&[20, 70, 120]);
            }
        }
        let rgb = scaler.convert(&frame, width, height).unwrap();
        assert_eq!(rgb.dimensions(), (width, height));
        assert!(rgb.pixels().all(|p| p.0 == [20, 70, 120]));
    }
}

fn hdr_frame(transfer: color::TransferCharacteristic) -> Video {
    let mut frame = Video::new(Pixel::YUV420P10LE, 4, 2);
    frame.set_color_space(color::Space::BT2020NCL);
    frame.set_color_primaries(color::Primaries::BT2020);
    frame.set_color_transfer_characteristic(transfer);
    frame.set_color_range(color::Range::MPEG);
    for y in 0..2 {
        let stride = frame.stride(0);
        for (x, value) in [64u16, 256, 512, 940].into_iter().enumerate() {
            frame.data_mut(0)[y * stride + x * 2..y * stride + x * 2 + 2]
                .copy_from_slice(&value.to_le_bytes());
        }
    }
    for plane in [1, 2] {
        for x in 0..2 {
            frame.data_mut(plane)[x * 2..x * 2 + 2].copy_from_slice(&512u16.to_le_bytes());
        }
    }
    frame
}

#[test]
fn pq_and_hlg_use_color_managed_tone_mapping_without_destroying_luma_order() {
    let mut scaler = DisplayScaler::new().unwrap();
    let mut results = Vec::new();
    for transfer in [
        color::TransferCharacteristic::SMPTE2084,
        color::TransferCharacteristic::ARIB_STD_B67,
    ] {
        let frame = hdr_frame(transfer);
        let original = frame.data(0).to_vec();
        let rgb = scaler.convert(&frame, 4, 2).unwrap();
        let values: Vec<_> = (0..4).map(|x| rgb.get_pixel(x, 0)[0]).collect();
        assert!(values.windows(2).all(|p| p[0] <= p[1]), "{values:?}");
        assert!(values[0] < 10 && values[3] > 200, "{values:?}");
        assert!(values[1] > values[0] && values[2] > values[1], "{values:?}");
        assert_eq!(
            frame.data(0),
            original,
            "tone mapping mutated source evidence"
        );
        results.push(values);
    }
    assert_ne!(results[0], results[1], "transfer metadata was ignored");
}
