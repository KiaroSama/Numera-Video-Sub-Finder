use crate::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Default for Rect {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.7,
            width: 1.0,
            height: 0.3,
        }
    }
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Result<Self> {
        if ![x, y, width, height].iter().all(|v| v.is_finite())
            || x < 0.0
            || y < 0.0
            || width <= 0.0
            || height <= 0.0
            || x + width > 1.00000001
            || y + height > 1.00000001
        {
            return Err(Error::Invalid(
                "region must be finite, nonempty and inside the image".into(),
            ));
        }
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }

    pub fn validate(self) -> Result<Self> {
        Self::new(self.x, self.y, self.width, self.height)
    }

    pub fn pixels(self, width: u32, height: u32) -> Result<(u32, u32, u32, u32)> {
        self.validate()?;
        if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 33_177_600 {
            return Err(Error::Invalid(
                "invalid or oversized image dimensions".into(),
            ));
        }
        let x = (self.x * f64::from(width)).round() as u32;
        let y = (self.y * f64::from(height)).round() as u32;
        let w = ((self.width * f64::from(width)).round() as u32).min(width.saturating_sub(x));
        let h = ((self.height * f64::from(height)).round() as u32).min(height.saturating_sub(y));
        if w == 0 || h == 0 {
            return Err(Error::Invalid("region contains no pixels".into()));
        }
        Ok((x, y, w, h))
    }
}

impl std::str::FromStr for Rect {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        let n: Vec<f64> = s
            .split(',')
            .map(|s| {
                s.parse()
                    .map_err(|_| Error::Invalid("ROI expects x,y,width,height".into()))
            })
            .collect::<Result<_>>()?;
        if n.len() != 4 {
            return Err(Error::Invalid("ROI expects four normalized values".into()));
        }
        Self::new(n[0], n[1], n[2], n[3])
    }
}
