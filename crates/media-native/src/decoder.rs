use crate::{
    Result,
    color::DisplayScaler,
    fail,
    geometry::{DisplayTransform, display_size},
    timeline::PresentationClock,
};
use ffmpeg::{media::Type, util::frame::Video};
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
    let transform = DisplayTransform::from_stream(&stream)?;
    // SAFETY: stream and input are live; a null frame requests stream/codec SAR only.
    let sar = unsafe {
        ffmpeg::ffi::av_guess_sample_aspect_ratio(
            input.as_ptr().cast_mut(),
            stream.as_ptr().cast_mut(),
            std::ptr::null_mut(),
        )
    };
    let (width, height) = display_size(codec.width(), codec.height(), sar.into())?;
    let (width, height) = transform.dimensions(width, height);
    Ok(MediaInfo {
        width,
        height,
        duration_ms,
        codec: codec.id().name().into(),
        time_base_num: tb.numerator(),
        time_base_den: tb.denominator(),
        subtitle_streams: input
            .streams()
            .filter(|s| s.parameters().medium() == Type::Subtitle)
            .count(),
        rotation: transform.rotation(),
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
    let transform = DisplayTransform::from_stream(&stream)?;
    // SAFETY: stream is borrowed from this input; only the rational value is copied.
    let stream_sar: ffmpeg::Rational = unsafe { (*stream.as_ptr()).sample_aspect_ratio.into() };
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?
        .decoder()
        .video()?;
    display_size(decoder.width(), decoder.height(), ffmpeg::Rational(1, 1))?;
    let codec_sar = decoder.aspect_ratio();
    let mut scaler = DisplayScaler::new()?;
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
    let mut clock = PresentationClock::new(
        (start_pts != ffmpeg::ffi::AV_NOPTS_VALUE).then_some(start_pts),
        tb,
    )?;
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
            let Some((at, duration_ms)) = clock.observe(pts, frame.packet().duration)? else {
                continue;
            };
            if at < start_ms {
                continue;
            }
            if end_ms.is_some_and(|end| at >= end) {
                return Ok(false);
            }
            let frame_sar = frame.aspect_ratio();
            let sar = [stream_sar, frame_sar, codec_sar]
                .into_iter()
                .find(|sar| sar.numerator() > 0 && sar.denominator() > 0)
                .unwrap_or(ffmpeg::Rational(1, 1));
            let (width, height) = display_size(frame.width(), frame.height(), sar)?;
            let image = scaler.convert(&frame, width, height)?;
            let image = transform.for_frame(&frame)?.apply(image);
            if !consume(DecodedFrame {
                image,
                time_ms: at,
                estimated_duration_ms: duration_ms,
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
