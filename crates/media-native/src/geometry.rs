use crate::{Result, fail};
use ffmpeg_next::{self as ffmpeg, Rational, util::frame::Video};
use image::RgbImage;

pub const MAX_PIXELS: u64 = 33_177_600;

#[derive(Clone, Copy)]
pub struct DisplayTransform {
    rotation: i32,
    mirror: bool,
}

impl DisplayTransform {
    pub fn resolve(matrix: Option<&[u8]>, legacy: Option<&str>) -> Result<Self> {
        if let Some(matrix) = matrix {
            return Self::from_matrix(matrix);
        }
        let rotation = legacy
            .unwrap_or("0")
            .parse::<i32>()
            .map_err(|_| fail("Invalid legacy display rotation"))?
            .rem_euclid(360);
        if rotation % 90 != 0 {
            return Err(fail("Nonorthogonal display rotation is unsupported"));
        }
        Ok(Self {
            rotation,
            mirror: false,
        })
    }

    pub fn from_stream(stream: &ffmpeg::Stream<'_>) -> Result<Self> {
        let matrix = stream
            .side_data()
            .find(|data| data.kind() == ffmpeg::codec::packet::side_data::Type::DisplayMatrix);
        Self::resolve(
            matrix.as_ref().map(|data| data.data()),
            stream.metadata().get("rotate"),
        )
    }

    pub fn for_frame(self, frame: &Video) -> Result<Self> {
        frame
            .side_data(ffmpeg::util::frame::side_data::Type::DisplayMatrix)
            .map_or(Ok(self), |data| Self::from_matrix(data.data()))
    }

    pub fn from_matrix(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 36 {
            return Err(fail("Invalid display matrix byte length"));
        }
        let mut matrix = [0i32; 9];
        for (value, bytes) in matrix.iter_mut().zip(bytes.chunks_exact(4)) {
            *value = i32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        if matrix[2] != 0 || matrix[5] != 0 || matrix[8] != 1 << 30 {
            return Err(fail("Perspective display geometry is unsupported"));
        }
        let axis = |value: i32| -> Result<i32> {
            [-1i32, 0, 1]
                .into_iter()
                .find(|axis| (i64::from(value) - i64::from(*axis) * 65_536).abs() <= 2)
                .ok_or_else(|| fail("Scaled or nonorthogonal display matrix is unsupported"))
        };
        let (rotation, mirror) = match (
            axis(matrix[0])?,
            axis(matrix[1])?,
            axis(matrix[3])?,
            axis(matrix[4])?,
        ) {
            (1, 0, 0, 1) => (0, false),
            (0, 1, -1, 0) => (90, false),
            (-1, 0, 0, -1) => (180, false),
            (0, -1, 1, 0) => (270, false),
            (-1, 0, 0, 1) => (0, true),
            (1, 0, 0, -1) => (180, true),
            (0, 1, 1, 0) => (270, true),
            (0, -1, -1, 0) => (90, true),
            _ => {
                return Err(fail(
                    "Singular or nonorthogonal display matrix is unsupported",
                ));
            }
        };
        Ok(Self { rotation, mirror })
    }

    pub fn rotation(self) -> i32 {
        self.rotation
    }

    pub fn dimensions(self, width: u32, height: u32) -> (u32, u32) {
        if self.rotation % 180 == 0 {
            (width, height)
        } else {
            (height, width)
        }
    }

    pub fn apply(self, mut image: RgbImage) -> RgbImage {
        if self.mirror {
            image::imageops::flip_horizontal_in_place(&mut image);
        }
        match self.rotation {
            90 => image::imageops::rotate90(&image),
            180 => image::imageops::rotate180(&image),
            270 => image::imageops::rotate270(&image),
            _ => image,
        }
    }
}

pub fn display_size(width: u32, height: u32, sar: Rational) -> Result<(u32, u32)> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(fail("Invalid or oversized decoded frame"));
    }
    let (num, den) = (sar.numerator(), sar.denominator());
    if num < 0 || den <= 0 {
        return Err(fail("Invalid sample aspect ratio"));
    }
    let scaled = if num == 0 {
        u64::from(width)
    } else {
        (u64::from(width) * num as u64 + den as u64 / 2) / den as u64
    };
    if scaled == 0 || scaled > u64::from(i32::MAX) || scaled * u64::from(height) > MAX_PIXELS {
        return Err(fail("Display aspect ratio exceeds supported pixel budget"));
    }
    Ok((scaled as u32, height))
}

pub fn packed_rgb(rgb: &Video) -> Result<RgbImage> {
    display_size(rgb.width(), rgb.height(), Rational(1, 1))?;
    let row = rgb.width() as usize * 3;
    let stride = rgb.stride(0);
    let storage = stride
        .checked_mul(rgb.height() as usize)
        .filter(|bytes| *bytes <= 268_435_456 && stride >= row)
        .ok_or_else(|| fail("Invalid native RGB frame stride"))?;
    // SAFETY: inspect the live descriptor before the wrapper constructs a borrowed data slice.
    if unsafe { (*rgb.as_ptr()).data[0].is_null() } {
        return Err(fail("Missing native RGB frame buffer"));
    }
    let data = rgb.data(0);
    if data.len() < storage {
        return Err(fail("Truncated native RGB frame"));
    }
    let mut pixels = vec![0; row * rgb.height() as usize];
    for (destination, source) in pixels.chunks_exact_mut(row).zip(data.chunks(stride)) {
        destination.copy_from_slice(&source[..row]);
    }
    RgbImage::from_raw(rgb.width(), rgb.height(), pixels)
        .ok_or_else(|| fail("Invalid RGB frame dimensions"))
}
