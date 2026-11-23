use nvsf_core::{Cue, Rect};

pub const ROW_HEIGHT: usize = 72;
pub const WINDOW_ROWS: usize = 40;

pub fn window_range(total: usize, scroll_px: usize) -> std::ops::Range<usize> {
    let start = (scroll_px / ROW_HEIGHT).saturating_sub(4).min(total);
    start..start.saturating_add(WINDOW_ROWS).min(total)
}

pub fn time_label(ms: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

pub fn parse_time(text: &str) -> Result<u64, String> {
    let values: Vec<&str> = text.trim().split([':', '.', ',']).collect();
    if values.len() != 4
        || values
            .iter()
            .any(|s| s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()))
    {
        return Err("Use ASCII hh:mm:ss.mmm timestamps".into());
    }
    let v: Vec<u64> = values
        .iter()
        .map(|s| {
            s.parse::<u64>()
                .map_err(|_| "Timestamp is too large".to_string())
        })
        .collect::<Result<_, _>>()?;
    if v[1] >= 60 || v[2] >= 60 || values[3].len() != 3 {
        return Err("Minutes/seconds must be below 60, with three millisecond digits".into());
    }
    v[0].checked_mul(3_600_000)
        .and_then(|n| n.checked_add(v[1] * 60_000 + v[2] * 1000 + v[3]))
        .ok_or_else(|| "Timestamp overflow".into())
}

pub fn matches(cue: &Cue, query: &str, filter: &str) -> bool {
    let included = match filter {
        "unreviewed" => !cue.reviewed,
        "uncertain" => cue.confidence.is_none_or(|c| c < 0.7),
        "edited" => cue.edited,
        _ => true,
    };
    included && (query.is_empty() || cue.text.to_lowercase().contains(query))
}

pub fn region_style(rect: Rect) -> String {
    format!(
        "left:{}%;top:{}%;width:{}%;height:{}%",
        rect.x * 100.0,
        rect.y * 100.0,
        rect.width * 100.0,
        rect.height * 100.0
    )
}

pub fn frame_point(x: f64, y: f64, bounds: [f64; 4]) -> Option<(f64, f64)> {
    let [left, top, width, height] = bounds;
    if ![x, y, left, top, width, height]
        .iter()
        .all(|v| v.is_finite())
        || width <= 0.0
        || height <= 0.0
    {
        return None;
    }
    Some((
        ((x - left) / width).clamp(0.0, 1.0),
        ((y - top) / height).clamp(0.0, 1.0),
    ))
}

pub fn route_value(value: &str) -> Option<&'static str> {
    match value.trim_start_matches('#') {
        "extract" => Some("Extract"),
        "queue" => Some("Queue"),
        "review" => Some("Review"),
        "models" => Some("Models"),
        "history" => Some("History"),
        "settings" => Some("Settings"),
        _ => None,
    }
}

pub fn review_mutation_allowed(
    project_open: bool,
    job_state: &str,
    busy: bool,
    closing: bool,
) -> bool {
    project_open
        && !busy
        && !closing
        && matches!(job_state, "idle" | "completed" | "cancelled" | "failed")
}

#[derive(Clone)]
pub struct OcrProposal {
    pub cue_id: u64,
    pub revision: u64,
    pub recognition: nvsf_core::Recognition,
}

impl OcrProposal {
    pub fn is_current(&self, selected: Option<u64>, revision: u64) -> bool {
        selected == Some(self.cue_id) && revision == self.revision
    }

    pub fn apply(
        &self,
        cues: &mut [Cue],
        selected: Option<u64>,
        revision: u64,
    ) -> nvsf_core::Result<()> {
        if !self.is_current(selected, revision) {
            return Err(nvsf_core::Error::Invalid(
                "The review changed; request a new OCR proposal before replacing cue text".into(),
            ));
        }
        let cue = cues
            .iter_mut()
            .find(|cue| cue.id == self.cue_id)
            .ok_or_else(|| nvsf_core::Error::Invalid("Cue no longer exists".into()))?;
        let mut revised = cue.clone();
        revised.text = self.recognition.text.clone();
        revised.confidence = self.recognition.confidence;
        revised.edited = true;
        revised.reviewed = false;
        revised.validate()?;
        *cue = revised;
        Ok(())
    }
}

pub fn replace_proposal_text(cues: &mut [Cue], id: u64, text: String) -> nvsf_core::Result<()> {
    let cue = cues
        .iter_mut()
        .find(|cue| cue.id == id)
        .ok_or_else(|| nvsf_core::Error::Invalid("Cue no longer exists".into()))?;
    let mut revised = cue.clone();
    revised.text = text;
    revised.edited = true;
    revised.reviewed = false;
    revised.validate()?;
    *cue = revised;
    Ok(())
}

pub fn parse_saved_theme(value: Option<&str>) -> Option<&'static str> {
    match value {
        Some("light") => Some("light"),
        Some("dark") => Some("dark"),
        _ => None,
    }
}

pub fn resolve_theme(saved: Option<&str>, os_prefers_dark: bool) -> &'static str {
    parse_saved_theme(saved).unwrap_or(if os_prefers_dark { "dark" } else { "light" })
}

pub fn opposite_theme(current: &str) -> &'static str {
    if current == "dark" { "light" } else { "dark" }
}

pub fn theme_value(value: &str) -> &'static str {
    if value == "light" { "light" } else { "dark" }
}

pub fn ease_sweep(progress: f64) -> f64 {
    let x = progress.clamp(0.0, 1.0);
    if x <= 0.0 || x >= 1.0 {
        return x;
    }
    // CSS ease cubic-bezier(0.25, 0.1, 0.25, 1.0) solved via Newton-Raphson.
    let (x1, y1, x2, y2) = (0.25_f64, 0.1_f64, 0.25_f64, 1.0_f64);
    let bez = |t: f64, p1: f64, p2: f64| -> f64 {
        let u = 1.0 - t;
        3.0 * u * u * t * p1 + 3.0 * u * t * t * p2 + t * t * t
    };
    let bez_dx = |t: f64| -> f64 {
        let u = 1.0 - t;
        3.0 * u * u * x1 + 6.0 * u * t * (x2 - x1) + 3.0 * t * t * (1.0 - x2)
    };
    let mut t = x;
    for _ in 0..8 {
        let dx = bez(t, x1, x2) - x;
        if dx.abs() < 1e-6 {
            break;
        }
        let d = bez_dx(t);
        if d.abs() < 1e-6 {
            break;
        }
        t = (t - dx / d).clamp(0.0, 1.0);
    }
    bez(t, y1, y2).clamp(0.0, 1.0)
}

pub fn sweep_clip_inset(progress: f64, rtl: bool) -> String {
    let edge = ((-0.2 + progress.clamp(0.0, 1.0) * 1.4) * 100.0).clamp(0.0, 100.0);
    if rtl {
        format!("inset(0 {edge:.3}% 0 0)")
    } else {
        format!("inset(0 0 0 {edge:.3}%)")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanguageOption {
    pub code: String,
    pub label: String,
}

pub fn language_options(query: &str) -> Vec<LanguageOption> {
    let mut all = Vec::with_capacity(nvsf_core::languages::catalog().len() + 3);
    all.push(LanguageOption {
        code: "en".into(),
        label: "English".into(),
    });
    all.push(LanguageOption {
        code: "fa".into(),
        label: "Persian / فارسی".into(),
    });
    all.push(LanguageOption {
        code: "ja".into(),
        label: "Japanese / 日本語".into(),
    });
    for language in nvsf_core::languages::catalog() {
        all.push(LanguageOption {
            code: language.code.clone(),
            label: format!("{} ({})", language.name, language.code),
        });
    }
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|part| part.to_lowercase())
        .collect();
    if terms.is_empty() {
        return all;
    }
    all.into_iter()
        .filter(|entry| {
            let label = entry.label.to_lowercase();
            let code = entry.code.to_lowercase();
            terms
                .iter()
                .all(|term| label.contains(term) || code.contains(term))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ocr_proposals_reject_reused_ids_after_project_changes_or_later_edits() {
        let mut cue = Cue::new(1, 0, 1200, 2200, "Current project 日本".into()).unwrap();
        cue.crop = Some("current-source-crop".into());
        cue.reviewed = true;
        let before = vec![cue];
        let proposal = OcrProposal {
            cue_id: 1,
            revision: 7,
            recognition: nvsf_core::Recognition {
                text: "Earlier proposal فارسی".into(),
                confidence: Some(0.8),
                crop: Some("proposal-crop".into()),
            },
        };
        for (selected, revision) in [(Some(1), 8), (Some(1), 6), (Some(2), 7), (None, 7)] {
            let mut cues = before.clone();
            let mut history = nvsf_core::EditHistory::default();
            assert!(
                history
                    .apply(&mut cues, |cues| proposal.apply(cues, selected, revision))
                    .is_err()
            );
            assert_eq!(cues, before);
            assert!(!history.can_undo());
        }
        let mut cues = before.clone();
        let mut history = nvsf_core::EditHistory::default();
        history
            .apply(&mut cues, |cues| proposal.apply(cues, Some(1), 7))
            .unwrap();
        assert_eq!(cues[0].text, "Earlier proposal فارسی");
        assert_eq!(cues[0].raw_text, "Current project 日本");
        assert_eq!(cues[0].crop.as_deref(), Some("current-source-crop"));
        assert_eq!((cues[0].start_ms, cues[0].end_ms), (1200, 2200));
        assert_eq!(cues[0].confidence, Some(0.8));
        assert!(cues[0].edited);
        assert!(!cues[0].reviewed);
        history.undo(&mut cues);
        assert_eq!(cues, before);
    }

    #[test]
    fn invalid_ocr_proposals_preserve_the_original_cue() {
        let before = vec![Cue::new(1, 0, 10, 20, "Original".into()).unwrap()];
        for (id, text, confidence) in [
            (1, "", Some(0.8)),
            (1, "Recognized", Some(f32::NAN)),
            (999, "Recognized", Some(0.8)),
        ] {
            let proposal = OcrProposal {
                cue_id: id,
                revision: 7,
                recognition: nvsf_core::Recognition {
                    text: text.into(),
                    confidence,
                    crop: None,
                },
            };
            let mut cues = before.clone();
            assert!(proposal.apply(&mut cues, Some(id), 7).is_err());
            assert_eq!(cues, before);
        }
    }

    #[test]
    fn review_mutations_wait_for_extraction_and_native_operations() {
        for state in ["running", "paused", "cancelling"] {
            assert!(
                !review_mutation_allowed(true, state, false, false),
                "{state}"
            );
        }
        for state in ["idle", "completed", "cancelled", "failed"] {
            assert!(
                review_mutation_allowed(true, state, false, false),
                "{state}"
            );
            assert!(
                !review_mutation_allowed(true, state, true, false),
                "{state}"
            );
            assert!(
                !review_mutation_allowed(true, state, false, true),
                "{state}"
            );
            assert!(
                !review_mutation_allowed(false, state, false, false),
                "{state}"
            );
        }
    }

    #[test]
    fn language_search_filters_by_any_term_and_code() {
        let all = language_options("");
        assert_eq!(all.len(), nvsf_core::languages::catalog().len() + 3);
        assert_eq!(all[0].code, "en");
        assert_eq!(all[1].code, "fa");
        assert_eq!(all[2].code, "ja");

        let persian = language_options("فارسی");
        assert_eq!(persian.len(), 1);
        assert_eq!(persian[0].code, "fa");

        let chinese_vert = language_options("chinese vertical");
        assert!(chinese_vert.iter().any(|l| l.code == "chi_sim_vert"));
        assert!(chinese_vert.iter().any(|l| l.code == "chi_tra_vert"));
        assert!(!chinese_vert.iter().any(|l| l.code == "en"));

        let by_code = language_options("aze_cyrl");
        assert_eq!(by_code.len(), 1);
        assert_eq!(by_code[0].code, "aze_cyrl");

        assert!(language_options("zzzz-no-language").is_empty());
    }
    #[test]
    fn bounded_review_and_ascii_timeline() {
        assert_eq!(window_range(100_000, 72 * 99_999), 99_995..100_000);
        assert!(window_range(100_000, 0).len() <= WINDOW_ROWS);
        assert_eq!(window_range(0, usize::MAX), 0..0);
        for ms in [0, 1, 59_999, 3_600_000, 360_123_456] {
            assert_eq!(parse_time(&time_label(ms)), Ok(ms));
        }
        for text in [
            "00:60:00.000",
            "00:00:00.1",
            "۰۰:۰۰:۰۰.۰۰۰",
            "18446744073709551615:00:00.000",
        ] {
            assert!(parse_time(text).is_err());
        }
        let mut cue = Cue::new(1, 0, 0, 10, "سلام 日本 <b>text</b>".into()).expect("valid cue");
        assert!(matches(&cue, "日本", "unreviewed"));
        cue.reviewed = true;
        assert!(!matches(&cue, "", "unreviewed"));
        assert_eq!(route_value("#review"), Some("Review"));
        assert_eq!(route_value("#main-content"), None);
        assert_eq!(theme_value("light"), "light");
        assert_eq!(theme_value("dark"), "dark");
        assert_eq!(theme_value("invalid"), "dark");
        assert_eq!(resolve_theme(None, true), "dark");
        assert_eq!(resolve_theme(None, false), "light");
        assert_eq!(resolve_theme(Some("system"), false), "light");
        assert_eq!(resolve_theme(Some("light"), true), "light");
        assert_eq!(resolve_theme(Some("dark"), false), "dark");
        assert_eq!(opposite_theme("dark"), "light");
        assert_eq!(opposite_theme("light"), "dark");
        assert_eq!(ease_sweep(0.0), 0.0);
        assert_eq!(ease_sweep(1.0), 1.0);
        assert!(ease_sweep(0.5) > 0.7 && ease_sweep(0.5) < 0.9);
        assert_eq!(sweep_clip_inset(0.0, true), "inset(0 0.000% 0 0)");
        assert_eq!(sweep_clip_inset(1.0, false), "inset(0 0 0 100.000%)");
        assert!(region_style(Rect::default()).contains("top:70%"));
    }
    #[test]
    fn preview_coordinates_follow_scaled_scrolled_frame_not_viewport() {
        assert_eq!(
            frame_point(200.0, 100.0, [-100.0, -50.0, 600.0, 300.0]),
            Some((0.5, 0.5))
        );
        assert_eq!(
            frame_point(-200.0, 400.0, [-100.0, -50.0, 600.0, 300.0]),
            Some((0.0, 1.0))
        );
        for bounds in [
            [0.0, 0.0, 0.0, 10.0],
            [0.0, 0.0, 10.0, -1.0],
            [f64::NAN, 0.0, 10.0, 10.0],
        ] {
            assert!(frame_point(1.0, 1.0, bounds).is_none());
        }
        assert!(frame_point(f64::INFINITY, 0.0, [0.0, 0.0, 10.0, 10.0]).is_none());
    }

    #[test]
    fn explicit_visual_replacement_keeps_original_evidence_and_time() {
        let mut cue = Cue::new(8, 2, 1200, 2200, "Original 日本".into()).expect("valid cue");
        cue.crop = Some("original-crop".into());
        cue.confidence = Some(0.5);
        let before = cue.clone();
        let mut cues = vec![cue];
        replace_proposal_text(&mut cues, 8, "Corrected فارسی".into()).expect("valid replacement");
        assert_eq!(
            (cues[0].start_ms, cues[0].end_ms, cues[0].track),
            (1200, 2200, 2)
        );
        assert_eq!(cues[0].raw_text, before.raw_text);
        assert_eq!(cues[0].crop, before.crop);
        assert_eq!(cues[0].confidence, before.confidence);
        assert!(cues[0].edited);
        assert!(!cues[0].reviewed);
        let accepted = cues.clone();
        assert!(replace_proposal_text(&mut cues, 8, String::new()).is_err());
        assert_eq!(cues, accepted);
        assert!(replace_proposal_text(&mut cues, 999, "text".into()).is_err());
    }
}
