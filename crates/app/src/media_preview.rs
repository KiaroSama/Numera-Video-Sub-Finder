use super::DecodedFrame;
use crate::{Result, fail};
use image::RgbImage;

pub(super) struct Preview {
    at_ms: u64,
    preceding: Option<DecodedFrame>,
    observed_next: bool,
}

impl Preview {
    pub(super) fn new(at_ms: u64) -> Self {
        Self {
            at_ms,
            preceding: None,
            observed_next: false,
        }
    }

    pub(super) fn observe(&mut self, frame: DecodedFrame) -> bool {
        if frame.time_ms > self.at_ms {
            self.observed_next = true;
            return false;
        }
        let exact = frame.time_ms == self.at_ms;
        self.preceding = Some(frame);
        !exact
    }

    pub(super) fn finish(self) -> Result<RgbImage> {
        if let Some(frame) = self.preceding
            && (self.observed_next
                || self.at_ms < frame.time_ms.saturating_add(frame.estimated_duration_ms))
        {
            return Ok(frame.image);
        }
        Err(fail("No frame at the requested presentation time"))
    }
}

#[cfg(test)]
#[path = "media_preview_tests.rs"]
mod tests;
