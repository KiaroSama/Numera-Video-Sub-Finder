use crate::{Result, fail};
use ffmpeg_next::Rational;
use nvsf_core::{Error, Time};

pub struct PresentationClock {
    time_base: Rational,
    origin: Option<i64>,
    previous_pts: Option<i64>,
    previous_time: Option<Time>,
    last_delta_ms: u64,
}

impl PresentationClock {
    pub fn new(origin: Option<i64>, time_base: Rational) -> Result<Self> {
        Time::from_pts(
            0,
            i64::from(time_base.numerator()),
            i64::from(time_base.denominator()),
        )?;
        if let Some(pts) = origin {
            Self::time(pts, time_base)?;
        }
        Ok(Self {
            time_base,
            origin,
            previous_pts: None,
            previous_time: None,
            last_delta_ms: 40,
        })
    }

    fn time(pts: i64, time_base: Rational) -> Result<Time> {
        Ok(Time::from_pts(
            pts,
            i64::from(time_base.numerator()),
            i64::from(time_base.denominator()),
        )?)
    }

    fn normalized_ms(&self, pts: i64, origin: i64) -> Result<u64> {
        // Subtract raw PTS before scaling to preserve fractional negative origins.
        let ticks = i128::from(pts) - i128::from(origin);
        if ticks < 0 {
            return Err(fail("Presentation time precedes source origin"));
        }
        let micros = ticks
            .checked_mul(i128::from(self.time_base.numerator()))
            .and_then(|value| value.checked_mul(1_000_000))
            .ok_or(Error::Overflow)?
            / i128::from(self.time_base.denominator());
        let micros = i64::try_from(micros).map_err(|_| Error::Overflow)?;
        Ok(u64::try_from(micros / 1_000).map_err(|_| Error::Overflow)?)
    }

    pub fn observe(&mut self, pts: i64, duration: i64) -> Result<Option<(u64, u64)>> {
        if self.previous_pts.is_some_and(|previous| pts < previous) {
            return Err(fail("Discontinuous presentation timestamps require review"));
        }
        let time = Self::time(pts, self.time_base)?;
        if let Some(previous) = self.previous_time {
            let delta = time.relative_ms(previous)?;
            if delta > 0 {
                self.last_delta_ms = delta.clamp(1, 1_000);
            }
        }
        self.previous_pts = Some(pts);
        self.previous_time = Some(time);
        let origin = *self.origin.get_or_insert(pts);
        if pts < origin {
            return Ok(None);
        }
        let time_ms = self.normalized_ms(pts, origin)?;
        let duration_ms = if duration > 0 {
            let end_pts = pts
                .checked_add(duration)
                .ok_or_else(|| fail("Source frame end timestamp overflow"))?;
            Self::time(end_pts, self.time_base)?;
            let end_ms = self.normalized_ms(end_pts, origin)?;
            // Keep fractional-frame endpoints aligned after millisecond quantization.
            end_ms
                .checked_sub(time_ms)
                .ok_or_else(|| fail("Invalid source frame duration"))?
                .max(1)
        } else {
            self.last_delta_ms
        };
        Ok(Some((time_ms, duration_ms)))
    }
}
