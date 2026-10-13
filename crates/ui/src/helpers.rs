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

pub fn theme_value(value: &str) -> &'static str {
    match value {
        "light" => "light",
        "dark" => "dark",
        _ => "system",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_review_and_ascii_timeline() {
        assert_eq!(window_range(100_000, 72 * 99_999), 99_995..100_000);
        assert!(window_range(100_000, 0).len() <= WINDOW_ROWS);
        assert_eq!(window_range(0, usize::MAX), 0..0);
        for ms in [0, 1, 59_999, 3_600_000, 360_123_456] {
            assert_eq!(parse_time(&time_label(ms)).unwrap(), ms);
        }
        for text in [
            "00:60:00.000",
            "00:00:00.1",
            "۰۰:۰۰:۰۰.۰۰۰",
            "18446744073709551615:00:00.000",
        ] {
            assert!(parse_time(text).is_err());
        }
        let mut cue = Cue::new(1, 0, 0, 10, "سلام 日本 <b>text</b>".into()).unwrap();
        assert!(matches(&cue, "日本", "unreviewed"));
        cue.reviewed = true;
        assert!(!matches(&cue, "", "unreviewed"));
        assert_eq!(route_value("#review"), Some("Review"));
        assert_eq!(route_value("#main-content"), None);
        assert_eq!(theme_value("invalid"), "system");
        assert!(region_style(Rect::default()).contains("top:70%"));
    }
    #[test]
    fn explicit_visual_replacement_keeps_original_evidence_and_time() {
        let mut cue = Cue::new(8, 2, 1200, 2200, "Original 日本".into()).unwrap();
        cue.crop = Some("original-crop".into());
        cue.confidence = Some(0.5);
        let before = cue.clone();
        let mut cues = vec![cue];
        replace_proposal_text(&mut cues, 8, "Corrected فارسی".into()).unwrap();
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
