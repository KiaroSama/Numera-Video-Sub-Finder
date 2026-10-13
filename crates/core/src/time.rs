use crate::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Time(i64);

impl Time {
    pub fn from_pts(pts: i64, numerator: i64, denominator: i64) -> Result<Self> {
        if numerator <= 0 || denominator <= 0 {
            return Err(Error::Invalid("time base must be positive".into()));
        }
        let value = i128::from(pts)
            .checked_mul(i128::from(numerator))
            .and_then(|v| v.checked_mul(1_000_000))
            .ok_or(Error::Overflow)?
            / i128::from(denominator);
        Ok(Self(i64::try_from(value).map_err(|_| Error::Overflow)?))
    }

    pub fn micros(self) -> i64 {
        self.0
    }

    pub fn relative_ms(self, origin: Self) -> Result<u64> {
        let delta = self.0.checked_sub(origin.0).ok_or(Error::Overflow)?;
        if delta < 0 {
            return Err(Error::Invalid("presentation time precedes origin".into()));
        }
        u64::try_from(delta / 1000).map_err(|_| Error::Overflow)
    }
}

pub fn format_time(ms: u64) -> String {
    format!(
        "{:02}:{:02}:{:02},{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

pub fn parse_time(value: &str) -> Result<u64> {
    let values: Vec<_> = value.split([':', ',']).collect();
    if values.len() != 4
        || values[3].len() != 3
        || values
            .iter()
            .any(|v| v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(Error::Invalid("invalid SRT timecode".into()));
    }
    let n: Vec<u64> = values
        .iter()
        .map(|v| v.parse::<u64>().map_err(|_| Error::Overflow))
        .collect::<Result<_>>()?;
    if n[1] >= 60 || n[2] >= 60 || n[3] >= 1000 {
        return Err(Error::Invalid("timecode component out of range".into()));
    }
    n[0].checked_mul(3_600_000)
        .and_then(|v| v.checked_add(n[1] * 60_000 + n[2] * 1000 + n[3]))
        .ok_or(Error::Overflow)
}
