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
    include_preroll: bool,
    mut consume: impl FnMut(DecodedFrame) -> Result<bool>,
) -> Result<()> {
    let mut decoder = Decoder::open(path)?;
    if start_ms > 0 && decoder.origin.is_some() {
        decoder.seek(start_ms)?;
    }
    while let Some(frame) = decoder.next()? {
        if frame.at < start_ms && !include_preroll {
            continue;
        }
        if end_ms.is_some_and(|end| frame.at >= end) || !consume(decoder.rgb(&frame, 0)?)? {
            break;
        }
    }
    Ok(())
}

struct TimedFrame {
    raw: Video,
    at: u64,
    duration: u64,
}

pub struct Decoder {
    input: ffmpeg::format::context::Input,
    codec: ffmpeg::decoder::Video,
    scaler: DisplayScaler,
    clock: PresentationClock,
    origin: Option<i64>,
    time_base: ffmpeg::Rational,
    index: usize,
    transform: DisplayTransform,
    stream_sar: ffmpeg::Rational,
    codec_sar: ffmpeg::Rational,
    draining: bool,
    last: Option<TimedFrame>,
}

impl Decoder {
    pub fn open(path: &Path) -> Result<Self> {
        let input = open(path)?;
        let stream = input
            .streams()
            .best(Type::Video)
            .ok_or_else(|| fail("No video stream"))?;
        let time_base = stream.time_base();
        let start = stream.start_time();
        let origin = (start != ffmpeg::ffi::AV_NOPTS_VALUE).then_some(start);
        let index = stream.index();
        let transform = DisplayTransform::from_stream(&stream)?;
        // SAFETY: the stream borrows this input; only the rational value is copied.
        let stream_sar = unsafe { (*stream.as_ptr()).sample_aspect_ratio.into() };
        let codec = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?
            .decoder()
            .video()?;
        display_size(codec.width(), codec.height(), ffmpeg::Rational(1, 1))?;
        let codec_sar = codec.aspect_ratio();
        Ok(Self {
            input,
            codec,
            scaler: DisplayScaler::new()?,
            clock: PresentationClock::new(origin, time_base)?,
            origin,
            time_base,
            index,
            transform,
            stream_sar,
            codec_sar,
            draining: false,
            last: None,
        })
    }

    fn seek(&mut self, at: u64) -> Result<()> {
        let origin = Time::from_pts(
            self.origin
                .ok_or_else(|| fail("Source has no presentation origin"))?,
            i64::from(self.time_base.numerator()),
            i64::from(self.time_base.denominator()),
        )?;
        let timestamp = i64::try_from(at)
            .ok()
            .and_then(|offset| offset.checked_mul(1000))
            .and_then(|offset| origin.micros().checked_add(offset))
            .ok_or_else(|| fail("Seek time overflow"))?;
        self.input.seek(timestamp, ..timestamp)?;
        self.codec.flush();
        self.clock = PresentationClock::new(self.origin, self.time_base)?;
        self.draining = false;
        self.last = None;
        Ok(())
    }

    fn next(&mut self) -> Result<Option<TimedFrame>> {
        loop {
            let mut raw = Video::empty();
            match self.codec.receive_frame(&mut raw) {
                Ok(()) => {
                    let pts = raw.timestamp().or_else(|| raw.pts()).ok_or_else(|| {
                        fail("Frame has no presentation timestamp; accurate timing unavailable")
                    })?;
                    self.origin.get_or_insert(pts);
                    if let Some((at, duration)) = self.clock.observe(pts, raw.packet().duration)? {
                        return Ok(Some(TimedFrame { raw, at, duration }));
                    }
                }
                Err(ffmpeg::Error::Eof) => return Ok(None),
                Err(ffmpeg::Error::Other {
                    errno: ffmpeg::util::error::EAGAIN,
                }) => {
                    if self.draining {
                        return Ok(None);
                    }
                    loop {
                        let mut packet = ffmpeg::Packet::empty();
                        match packet.read(&mut self.input) {
                            Ok(()) if packet.stream() == self.index => {
                                self.codec.send_packet(&packet)?;
                                break;
                            }
                            Ok(()) => {}
                            Err(ffmpeg::Error::Eof) => {
                                self.codec.send_eof()?;
                                self.draining = true;
                                break;
                            }
                            Err(error) => return Err(error.into()),
                        }
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn rgb(&mut self, frame: &TimedFrame, max_width: u32) -> Result<DecodedFrame> {
        let raw = &frame.raw;
        let sar = [self.stream_sar, raw.aspect_ratio(), self.codec_sar]
            .into_iter()
            .find(|sar| sar.numerator() > 0 && sar.denominator() > 0)
            .unwrap_or(ffmpeg::Rational(1, 1));
        let (mut width, mut height) = display_size(raw.width(), raw.height(), sar)?;
        let transform = self.transform.for_frame(raw)?;
        let rotated = matches!(transform.rotation(), 90 | 270);
        if rotated {
            std::mem::swap(&mut width, &mut height);
        }
        if max_width != 0 && width > max_width {
            height = (u64::from(height) * u64::from(max_width) / u64::from(width)).max(1) as u32;
            width = max_width;
        }
        if rotated {
            std::mem::swap(&mut width, &mut height);
        }
        let image = self.scaler.convert(raw, width, height)?;
        Ok(DecodedFrame {
            image: transform.apply(image),
            time_ms: frame.at,
            estimated_duration_ms: frame.duration,
        })
    }

    pub fn preview(&mut self, target: u64, step: i8, max_width: u32) -> Result<DecodedFrame> {
        if !matches!(step, -1..=2) {
            return Err(fail("Preview step must be -1, 0, 1 or 2"));
        }
        let sequential = step == 2
            && self
                .last
                .as_ref()
                .is_some_and(|last| target.checked_sub(last.at).is_some_and(|gap| gap <= 2000));
        if !sequential && self.origin.is_some() && (target > 0 || self.last.is_some()) {
            self.seek(if step < 0 {
                target.saturating_sub(1)
            } else {
                target
            })?;
        }
        let mut last = self.last.take();
        let mut selected = None;
        // A repeated playback target can use the same decoded frame without reading another packet.
        if !(sequential && last.as_ref().is_some_and(|frame| frame.at == target)) {
            while let Some(frame) = self.next()? {
                let at = frame.at;
                let stop = match step {
                    -1 | 0 | 2 => at >= target,
                    _ => at > target,
                };
                if stop {
                    if (step == 0 && at > target) || (step == -1 && last.is_some()) {
                        selected = last.take();
                    }
                    last = Some(frame);
                    break;
                }
                last = Some(frame);
            }
        }
        let result = if let Some(frame) = selected.as_ref() {
            self.rgb(frame, max_width)
        } else if let Some(frame) = last.as_ref().filter(|frame| match step {
            0 if frame.at > target => false,
            0 | 2 => target < frame.at.saturating_add(frame.duration),
            _ => true,
        }) {
            self.rgb(frame, max_width)
        } else {
            Err(fail("No frame at the requested presentation time"))
        };
        self.last = last;
        result
    }
}
