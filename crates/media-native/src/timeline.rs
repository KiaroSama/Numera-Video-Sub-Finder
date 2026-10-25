use crate::{Result, fail};
use ffmpeg_next::Rational;
use nvsf_core::Time;

pub struct PresentationClock {
    time_base: Rational,
    origin: Option<Time>,
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
        Ok(Self {
            time_base,
            origin: origin.map(|pts| Self::time(pts, time_base)).transpose()?,
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
        let origin = *self.origin.get_or_insert(time);
        if time < origin {
            return Ok(None);
        }
        let duration_ms = if duration > 0 {
            let micros = Self::time(duration, self.time_base)?.micros();
            u64::try_from(micros / 1_000)
                .map_err(|_| fail("Invalid source frame duration"))?
                .max(1)
        } else {
            self.last_delta_ms
        };
        Ok(Some((time.relative_ms(origin)?, duration_ms)))
    }
}
