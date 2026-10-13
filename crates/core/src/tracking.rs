use crate::{Cue, Result};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Recognition {
    pub text: String,
    pub confidence: Option<f32>,
    pub crop: Option<String>,
}

#[derive(Default)]
pub struct AppearanceTracker {
    active: Option<Cue>,
    next_id: u64,
    track: u32,
}

impl AppearanceTracker {
    pub fn new(track: u32) -> Self {
        Self {
            track,
            next_id: u64::from(track) << 32,
            active: None,
        }
    }

    pub fn restore(track: u32, cue: Cue) -> Result<Self> {
        cue.validate()?;
        if cue.track != track {
            return Err(crate::Error::Invalid("checkpoint track mismatch".into()));
        }
        Ok(Self {
            track,
            next_id: cue.id,
            active: Some(cue),
        })
    }

    pub fn observe(
        &mut self,
        at_ms: u64,
        until_ms: u64,
        recognized: Option<Recognition>,
    ) -> Result<Option<Cue>> {
        if until_ms <= at_ms {
            return Err(crate::Error::Invalid(
                "appearance interval must be positive".into(),
            ));
        }
        let recognized = recognized.filter(|r| !r.text.trim().is_empty());
        let same = self
            .active
            .as_ref()
            .zip(recognized.as_ref())
            .is_some_and(|(c, r)| c.raw_text == r.text && at_ms <= c.end_ms.saturating_add(1));
        if same {
            if let Some(c) = &mut self.active {
                c.end_ms = c.end_ms.max(until_ms);
                if let Some(r) = recognized {
                    if r.confidence.unwrap_or(0.0) > c.confidence.unwrap_or(0.0) {
                        c.confidence = r.confidence;
                        c.crop = r.crop;
                    }
                }
            }
            return Ok(None);
        }
        let finished = self.active.take();
        if let Some(r) = recognized {
            self.next_id = self.next_id.checked_add(1).ok_or(crate::Error::Overflow)?;
            let mut cue = Cue::new(self.next_id, self.track, at_ms, until_ms, r.text)?;
            cue.confidence = r.confidence;
            cue.crop = r.crop;
            self.active = Some(cue);
        }
        Ok(finished)
    }

    pub fn finish(&mut self) -> Option<Cue> {
        self.active.take()
    }
    pub fn active(&self) -> Option<&Cue> {
        self.active.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn r(text: &str) -> Option<Recognition> {
        Some(Recognition {
            text: text.into(),
            confidence: Some(0.9),
            crop: None,
        })
    }
    #[test]
    fn repeated_appearances_and_numeric_changes_are_separate() {
        let mut t = AppearanceTracker::new(0);
        assert!(t.observe(0, 40, r("15")).unwrap().is_none());
        assert!(t.observe(40, 80, r("15")).unwrap().is_none());
        let c = t.observe(80, 120, r("16")).unwrap().unwrap();
        assert_eq!((c.start_ms, c.end_ms, c.text.as_str()), (0, 80, "15"));
        assert_eq!(t.observe(120, 160, None).unwrap().unwrap().text, "16");
        t.observe(160, 200, r("15")).unwrap();
        assert_eq!(t.finish().unwrap().start_ms, 160);
    }
}
