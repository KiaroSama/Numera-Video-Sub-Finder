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
        self.finish_frame().map(|frame| frame.image)
    }

    pub(super) fn finish_frame(self) -> Result<DecodedFrame> {
        if let Some(frame) = self.preceding
            && (self.observed_next
                || self.at_ms < frame.time_ms.saturating_add(frame.estimated_duration_ms))
        {
            return Ok(frame);
        }
        Err(fail("No frame at the requested presentation time"))
    }
}

pub(super) struct Adjacent {
    at_ms: u64,
    direction: i8,
    selected: Option<DecodedFrame>,
}

impl Adjacent {
    pub(super) fn new(at_ms: u64, direction: i8) -> Self {
        Self {
            at_ms,
            direction,
            selected: None,
        }
    }

    pub(super) fn observe(&mut self, frame: DecodedFrame) -> bool {
        if self.direction < 0 {
            if frame.time_ms >= self.at_ms {
                // At the first source frame there is no predecessor: retain that frame.
                if self.selected.is_none() {
                    self.selected = Some(frame);
                }
                return false;
            }
            self.selected = Some(frame);
            true
        } else {
            let following = frame.time_ms > self.at_ms;
            self.selected = Some(frame);
            !following
        }
    }

    pub(super) fn finish(self) -> Result<DecodedFrame> {
        self.selected
            .ok_or_else(|| fail("No frame available for adjacent-frame preview"))
    }
}

pub fn is_near_black_rgb(rgb: &[u8]) -> bool {
    let pixels = rgb.len() / 3;
    if pixels == 0 || !rgb.len().is_multiple_of(3) {
        return false;
    }
    let samples = pixels.min(4096);
    let mut total = 0;
    let mut bright = 0;
    for sample in 0..samples {
        let index = (pixels / samples * sample + pixels % samples * sample / samples) * 3;
        let luma = 2126 * u32::from(rgb[index])
            + 7152 * u32::from(rgb[index + 1])
            + 722 * u32::from(rgb[index + 2]);
        total += u64::from(luma);
        bright += usize::from(luma >= 240_000);
    }
    // Counting bright samples is enough to test the 95th percentile without sorting.
    total < 120_000 * samples as u64 && bright <= samples / 20
}

pub fn candidate_preview_timestamps(duration_ms: Option<u64>) -> Vec<u64> {
    let mut candidates = vec![0, 1500, 4000, 10_000];
    if let Some(duration) = duration_ms {
        candidates.extend([duration / 10, duration / 4]);
        candidates.retain(|&time| time < duration);
        candidates.sort_unstable();
        candidates.dedup();
    }
    candidates
}

impl super::MediaRuntime {
    pub fn initial_preview_timestamp(
        duration_ms: Option<u64>,
        mut decode: impl FnMut(u64) -> Option<RgbImage>,
    ) -> u64 {
        for time in candidate_preview_timestamps(duration_ms) {
            let Some(frame) = decode(time) else { break };
            if !frame.is_empty() && !is_near_black_rgb(frame.as_raw()) {
                return time;
            }
        }
        0
    }
}

#[cfg(test)]
#[path = "media_preview_tests.rs"]
mod tests;
