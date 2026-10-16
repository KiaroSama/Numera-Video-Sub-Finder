use crate::{Error, Rect, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cue {
    pub id: u64,
    pub track: u32,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub raw_text: String,
    pub confidence: Option<f32>,
    pub reviewed: bool,
    pub edited: bool,
    pub crop: Option<String>,
}

impl Cue {
    pub fn new(id: u64, track: u32, start_ms: u64, end_ms: u64, text: String) -> Result<Self> {
        if end_ms <= start_ms
            || text.trim().is_empty()
            || text.len() > 65_536
            || text.contains('\0')
            || text.contains("\n\n")
        {
            return Err(Error::Invalid(
                "cue needs nonempty bounded text and end > start".into(),
            ));
        }
        Ok(Self {
            id,
            track,
            start_ms,
            end_ms,
            raw_text: text.clone(),
            text,
            confidence: None,
            reviewed: false,
            edited: false,
            crop: None,
        })
    }
    pub fn validate(&self) -> Result<()> {
        if self.end_ms <= self.start_ms
            || self.text.trim().is_empty()
            || self.text.len() > 65_536
            || self.text.contains('\0')
            || self.text.contains("\n\n")
            || self.raw_text.len() > 65_536
            || self.raw_text.contains('\0')
            || self
                .confidence
                .is_some_and(|v| !v.is_finite() || !(0.0..=1.0).contains(&v))
        {
            return Err(Error::Invalid(
                "cue contains invalid interval, text or confidence".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractConfig {
    pub input: String,
    pub output: String,
    pub data_root: Option<String>,
    pub language: String,
    pub device: String,
    pub quality: String,
    pub regions: Vec<Rect>,
    pub exclusions: Vec<Rect>,
    pub start_ms: u64,
    pub end_ms: Option<u64>,
    pub threads: usize,
    pub overwrite: bool,
    pub normalize_persian: bool,
    pub min_confidence: f32,
    pub gpu_required: bool,
    pub accelerator_required: bool,
}

impl Default for ExtractConfig {
    fn default() -> Self {
        Self {
            input: String::new(),
            output: String::new(),
            data_root: None,
            language: "en".into(),
            device: "auto".into(),
            quality: "balanced".into(),
            regions: vec![Rect::default()],
            exclusions: vec![],
            start_ms: 0,
            end_ms: None,
            threads: 2,
            overwrite: false,
            normalize_persian: false,
            min_confidence: 0.7,
            gpu_required: false,
            accelerator_required: false,
        }
    }
}

impl ExtractConfig {
    pub fn validate(&self) -> Result<()> {
        if self.input.is_empty() || self.output.is_empty() || self.input == self.output {
            return Err(Error::Invalid(
                "distinct input/output paths required".into(),
            ));
        }
        if crate::languages::find(&self.language).is_none() {
            return Err(Error::Invalid(
                "choose a language from the official catalog".into(),
            ));
        }
        if !["cpu", "auto", "cuda", "directml", "coreml"].contains(&self.device.as_str()) {
            return Err(Error::Invalid("unknown device".into()));
        }
        if !["accurate", "balanced", "fast"].contains(&self.quality.as_str())
            || self.threads == 0
            || self.threads > 16
            || self.regions.is_empty()
            || self.regions.len() > 8
            || self.exclusions.len() > 32
        {
            return Err(Error::Invalid(
                "invalid quality, region count or thread budget".into(),
            ));
        }
        for r in self.regions.iter().chain(&self.exclusions) {
            r.validate()?;
        }
        if self.end_ms.is_some_and(|end| end <= self.start_ms)
            || !self.min_confidence.is_finite()
            || !(0.0..=1.0).contains(&self.min_confidence)
        {
            return Err(Error::Invalid("invalid range/confidence".into()));
        }
        if self.device == "cpu" && (self.gpu_required || self.accelerator_required) {
            return Err(Error::Invalid(
                "CPU contradicts required acceleration".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaInfo {
    pub width: u32,
    pub height: u32,
    pub duration_ms: Option<u64>,
    pub codec: String,
    pub time_base_num: i32,
    pub time_base_den: i32,
    pub subtitle_streams: usize,
    pub rotation: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Progress {
    pub stage: String,
    pub frames: u64,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub cues: usize,
    pub provider: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub schema: u32,
    pub source_hash: String,
    pub config_hash: String,
    pub model_hash: String,
    pub config: ExtractConfig,
    pub media: MediaInfo,
    pub cues: Vec<Cue>,
    pub processed_ms: u64,
    #[serde(default)]
    pub active_appearances: Vec<Cue>,
    pub completed: bool,
    pub warnings: Vec<String>,
}

impl Project {
    pub fn validate(&self) -> Result<()> {
        if self.schema != 1 || self.cues.len() > 100_000 {
            return Err(Error::Invalid("unsupported/oversized project".into()));
        }
        self.config.validate()?;
        let mut tracks = std::collections::HashSet::new();
        for cue in &self.active_appearances {
            cue.validate()?;
            if cue.track as usize >= self.config.regions.len()
                || !tracks.insert(cue.track)
                || cue.end_ms > self.processed_ms
                || self.completed
            {
                return Err(Error::Invalid("invalid checkpoint appearance".into()));
            }
        }
        let mut identities = std::collections::HashSet::with_capacity(self.cues.len());
        for cue in &self.cues {
            cue.validate()?;
            if !identities.insert(cue.id) {
                return Err(Error::Invalid("duplicate cue identity".into()));
            }
        }
        Ok(())
    }
}
