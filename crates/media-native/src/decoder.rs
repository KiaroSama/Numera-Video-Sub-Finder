use crate::{Result, fail};
use ffmpeg::{
    format::Pixel,
    media::Type,
    software::scaling::{context::Context, flag::Flags},
    util::frame::Video,
};
use ffmpeg_next as ffmpeg;
use image::RgbImage;
use nvsf_core::{MediaInfo, Time};
use std::path::Path;

pub struct DecodedFrame {
    pub image: RgbImage,
    pub time_ms: u64,
    pub estimated_duration_ms: u64,
}

fn open(path: &Path) -> Result<ffmpeg::format::context::Input> {
    if !path.is_file() {
        return Err(fail("Input must be an existing local file"));
    }
    ffmpeg::init()?;
    let mut opts = ffmpeg::Dictionary::new();
    opts.set("protocol_whitelist", "file");
    opts.set("max_alloc", "268435456");
    Ok(ffmpeg::format::input_with_dictionary(path, opts)?)
}

pub fn probe(path: &Path) -> Result<MediaInfo> {
    let input = open(path)?;
    let stream = input
        .streams()
        .best(Type::Video)
        .ok_or_else(|| fail("Input has no decodable video stream"))?;
    let codec = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?
        .decoder()
        .video()?;
    let tb = stream.time_base();
    let duration_ms = if input.duration() > 0 {
        Some(input.duration() as u64 / 1000)
    } else {
        None
    };
    let rotation = stream
        .metadata()
        .get("rotate")
        .and_then(|r| r.parse::<i32>().ok())
        .unwrap_or(0);
    if u64::from(codec.width()) * u64::from(codec.height()) > 33_177_600 {
        return Err(fail("Video exceeds supported decoded pixel budget"));
    }
    Ok(MediaInfo {
        width: codec.width(),
        height: codec.height(),
        duration_ms,
        codec: codec.id().name().into(),
        time_base_num: tb.numerator(),
        time_base_den: tb.denominator(),
        subtitle_streams: input
            .streams()
            .filter(|s| s.parameters().medium() == Type::Subtitle)
            .count(),
        rotation,
    })
}

pub fn decode(
    path: &Path,
    start_ms: u64,
    end_ms: Option<u64>,
    mut consume: impl FnMut(DecodedFrame) -> Result<bool>,
) -> Result<()> {
    let mut input = open(path)?;
    let stream = input
        .streams()
        .best(Type::Video)
        .ok_or_else(|| fail("No video stream"))?;
    let index = stream.index();
    let tb = stream.time_base();
    let start_pts = stream.start_time();
    let origin = if start_pts != ffmpeg::ffi::AV_NOPTS_VALUE {
        Some(Time::from_pts(
            start_pts,
            i64::from(tb.numerator()),
            i64::from(tb.denominator()),
        )?)
    } else {
        None
    };
    let rotation = stream
        .metadata()
        .get("rotate")
        .and_then(|r| r.parse::<i32>().ok())
        .unwrap_or(0);
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?
        .decoder()
        .video()?;
    if decoder.width() == 0
        || decoder.height() == 0
        || u64::from(decoder.width()) * u64::from(decoder.height()) > 33_177_600
    {
        return Err(fail("Invalid or oversized decoded frame"));
    }
    let mut scaler = Context::get(
        decoder.format(),
        decoder.width(),
        decoder.height(),
        Pixel::RGB24,
        decoder.width(),
        decoder.height(),
        Flags::BILINEAR,
    )?;
    if start_ms > 0
        && let Some(origin) = origin
    {
        let offset = i64::try_from(start_ms)
            .map_err(|_| fail("Seek time overflow"))?
            .checked_mul(1000)
            .ok_or_else(|| fail("Seek time overflow"))?;
        let timestamp = origin
            .micros()
            .checked_add(offset)
            .ok_or_else(|| fail("Seek time overflow"))?;
        input.seek(timestamp, ..timestamp)?;
        decoder.flush();
    }
    let mut origin = origin;
    let mut last = None;
    let mut last_delta = 40u64;
    let mut receive = |decoder: &mut ffmpeg::decoder::Video| -> Result<bool> {
        loop {
            let mut frame = Video::empty();
            match decoder.receive_frame(&mut frame) {
                Ok(()) => {}
                Err(ffmpeg::Error::Other {
                    errno: ffmpeg::util::error::EAGAIN,
                }) => break,
                Err(ffmpeg::Error::Eof) => break,
                Err(e) => return Err(e.into()),
            }
            let pts = frame.timestamp().or_else(|| frame.pts()).ok_or_else(|| {
                fail("Frame has no presentation timestamp; accurate timing unavailable")
            })?;
            let time = Time::from_pts(pts, i64::from(tb.numerator()), i64::from(tb.denominator()))?;
            let origin = *origin.get_or_insert(time);
            let at = time.relative_ms(origin)?;
            if let Some(previous) = last {
                if at < previous {
                    return Err(fail("Discontinuous presentation timestamps require review"));
                }
                if at > previous {
                    last_delta = at - previous;
                }
            }
            last = Some(at);
            if at < start_ms {
                continue;
            }
            if end_ms.is_some_and(|end| at >= end) {
                return Ok(false);
            }
            let mut rgb = Video::empty();
            scaler.run(&frame, &mut rgb)?;
            let row = rgb.width() as usize * 3;
            let mut pixels = vec![0; row * rgb.height() as usize];
            let data = rgb.data(0);
            for y in 0..rgb.height() as usize {
                let from = y
                    .checked_mul(rgb.stride(0))
                    .ok_or_else(|| fail("Frame stride overflow"))?;
                let source = data
                    .get(from..from + row)
                    .ok_or_else(|| fail("Invalid native frame stride"))?;
                pixels[y * row..(y + 1) * row].copy_from_slice(source);
            }
            let image = RgbImage::from_raw(rgb.width(), rgb.height(), pixels)
                .ok_or_else(|| fail("Invalid RGB frame dimensions"))?;
            let image = match rotation.rem_euclid(360) {
                90 => image::imageops::rotate90(&image),
                180 => image::imageops::rotate180(&image),
                270 => image::imageops::rotate270(&image),
                _ => image,
            };
            if !consume(DecodedFrame {
                image,
                time_ms: at,
                estimated_duration_ms: last_delta,
            })? {
                return Ok(false);
            }
        }
        Ok(true)
    };
    loop {
        let mut packet = ffmpeg::Packet::empty();
        match packet.read(&mut input) {
            Ok(()) => {}
            Err(ffmpeg::Error::Eof) => break,
            Err(error) => return Err(error.into()),
        }
        if packet.stream() == index {
            decoder.send_packet(&packet)?;
            if !receive(&mut decoder)? {
                return Ok(());
            }
        }
    }
    decoder.send_eof()?;
    receive(&mut decoder)?;
    Ok(())
}
