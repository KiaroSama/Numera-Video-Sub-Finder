use crate::{Result, fail, geometry::packed_rgb};
use ffmpeg_next::{self as ffmpeg, color, ffi, format::Pixel, util::frame::Video};
use image::RgbImage;
use std::ptr::NonNull;

pub struct DisplayScaler(NonNull<ffi::SwsContext>);

impl DisplayScaler {
    pub fn new() -> Result<Self> {
        // SAFETY: no borrowed arguments; the returned context is uniquely owned until Drop.
        let pointer = unsafe { ffi::sws_alloc_context() };
        let scaler =
            Self(NonNull::new(pointer).ok_or_else(|| fail("Cannot allocate display scaler"))?);
        // FFmpeg9's dynamic frame API honors color metadata; the legacy sws_scale API does not.
        for (name, value) in [(c"intent", 0), (c"threads", 1), (c"sws_flags", 2)] {
            // SAFETY: this live SwsContext implements AVOptions; all names are static C strings.
            let status =
                unsafe { ffi::av_opt_set_int(scaler.0.as_ptr().cast(), name.as_ptr(), value, 0) };
            if status < 0 {
                return Err(ffmpeg::Error::from(status).into());
            }
        }
        Ok(scaler)
    }

    pub fn convert(&mut self, source: &Video, width: u32, height: u32) -> Result<RgbImage> {
        crate::geometry::display_size(source.width(), source.height(), ffmpeg::Rational(1, 1))?;
        crate::geometry::display_size(width, height, ffmpeg::Rational(1, 1))?;
        let mut rgb = Video::empty();
        rgb.set_format(Pixel::RGB24);
        rgb.set_width(width);
        rgb.set_height(height);
        rgb.set_color_space(color::Space::RGB);
        rgb.set_color_range(color::Range::JPEG);
        rgb.set_color_primaries(color::Primaries::BT709);
        rgb.set_color_transfer_characteristic(color::TransferCharacteristic::IEC61966_2_1);
        // SAFETY: both AVFrames are live; preserve field order without copying source color tags
        // over the explicitly selected SDR destination. This is not a deinterlacing operation.
        unsafe { (*rgb.as_mut_ptr()).flags = (*source.as_ptr()).flags };
        // SAFETY: both owned AVFrames and the unique scaler live through this synchronous call.
        // sws_scale_frame allocates the destination, reports allocation/conversion errors, and
        // supports changing input format/geometry/color metadata without stale cached definitions.
        let status =
            unsafe { ffi::sws_scale_frame(self.0.as_ptr(), rgb.as_mut_ptr(), source.as_ptr()) };
        if status < 0 {
            return Err(ffmpeg::Error::from(status).into());
        }
        packed_rgb(&rgb)
    }
}

impl Drop for DisplayScaler {
    fn drop(&mut self) {
        // SAFETY: this owner releases its sole native context once; frames own their own buffers.
        unsafe { ffi::sws_freeContext(self.0.as_ptr()) };
    }
}
