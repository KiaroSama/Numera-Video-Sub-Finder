use crate::{Error, Result};

/// Refines a subtitle appearance interval using adjacent non-subtitle frame timestamps.
///
/// When neighboring absent frame timestamps bracket the observed interval, the midpoint
/// narrows boundary uncertainty without mutating source presentation timestamps.
pub fn refine_boundary_ms(
    previous_absent_ms: Option<u64>,
    first_present_ms: u64,
    last_present_ms: u64,
    next_absent_ms: Option<u64>,
) -> Result<(u64, u64)> {
    if last_present_ms <= first_present_ms {
        return Err(Error::Invalid(
            "Cue end must be greater than cue start".into(),
        ));
    }
    let start = match previous_absent_ms {
        Some(prev) if prev < first_present_ms => prev + (first_present_ms - prev) / 2,
        Some(_) => {
            return Err(Error::Invalid(
                "Previous absent timestamp must precede first present frame".into(),
            ));
        }
        None => first_present_ms,
    };
    let end = match next_absent_ms {
        Some(next) if next > last_present_ms => last_present_ms + (next - last_present_ms) / 2,
        Some(_) => {
            return Err(Error::Invalid(
                "Next absent timestamp must follow last present frame".into(),
            ));
        }
        None => last_present_ms,
    };
    Ok((start, end))
}

/// Deterministic sampling and recognition coverage summary for an extraction pass.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CoverageSummary {
    pub sampled_frames: u64,
    pub recognized_frames: u64,
    pub emitted_cues: u64,
    pub declared_coverage_ppm: u32,
}

impl CoverageSummary {
    pub fn from_counts(
        sampled_frames: u64,
        recognized_frames: u64,
        emitted_cues: u64,
    ) -> Result<Self> {
        if recognized_frames > sampled_frames {
            return Err(Error::Invalid(
                "Recognized frame count cannot exceed sampled frames".into(),
            ));
        }
        let declared_coverage_ppm = if sampled_frames == 0 {
            0
        } else {
            ((u128::from(recognized_frames) * 1_000_000) / u128::from(sampled_frames)) as u32
        };
        Ok(Self {
            sampled_frames,
            recognized_frames,
            emitted_cues,
            declared_coverage_ppm,
        })
    }
}
