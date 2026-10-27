use crate::{Cue, Error, Result, format_time, parse_time};
use std::fmt::Write;

pub fn export_srt(cues: &[Cue], bom: bool, crlf: bool) -> Result<String> {
    let mut out = if bom {
        "\u{feff}".into()
    } else {
        String::new()
    };
    for (i, c) in cues.iter().enumerate() {
        c.validate()?;
        write!(
            out,
            "{}\n{} --> {}\n{}\n\n",
            i + 1,
            format_time(c.start_ms),
            format_time(c.end_ms),
            c.text.replace('\r', "")
        )
        .map_err(|_| Error::Invalid("subtitle formatting failed".into()))?;
    }
    Ok(if crlf { out.replace('\n', "\r\n") } else { out })
}

pub fn export_vtt(cues: &[Cue]) -> Result<String> {
    let mut out = "WEBVTT\n\n".to_string();
    for c in cues {
        c.validate()?;
        write!(
            out,
            "{} --> {}\n{}\n\n",
            format_time(c.start_ms).replace(',', "."),
            format_time(c.end_ms).replace(',', "."),
            c.text
        )
        .map_err(|_| Error::Invalid("subtitle formatting failed".into()))?;
    }
    Ok(out)
}

pub fn export_ass(cues: &[Cue]) -> Result<String> {
    let mut out = "[Script Info]\nScriptType: v4.00+\nPlayResX: 1920\nPlayResY: 1080\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Arial,48,&H00FFFFFF,&H00FFFFFF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,0,2,20,20,40,1\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n".to_string();
    for c in cues {
        c.validate()?;
        let stamp = |ms| {
            format_time(ms)
                .replace(',', ".")
                .chars()
                .take(format_time(ms).len() - 1)
                .collect::<String>()
        };
        let text = c
            .text
            .replace('\\', "\\\\")
            .replace('{', "\\{")
            .replace('}', "\\}")
            .replace('\n', "\\N");
        writeln!(
            out,
            "Dialogue: {},{},{},Default,,0,0,0,,{}",
            c.track,
            stamp(c.start_ms),
            stamp(c.end_ms),
            text
        )
        .map_err(|_| Error::Invalid("subtitle formatting failed".into()))?;
    }
    Ok(out)
}

pub fn parse_srt(text: &str) -> Result<Vec<Cue>> {
    if text.len() > 64 * 1024 * 1024 {
        return Err(Error::Invalid("subtitle input exceeds limit".into()));
    }
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let mut cues = Vec::new();
    for block in text
        .trim_start()
        .trim_end_matches('\n')
        .split("\n\n")
        .filter(|b| !b.trim().is_empty())
    {
        let mut lines = block.lines();
        let index = lines
            .next()
            .ok_or_else(|| Error::Invalid("missing index".into()))?;
        if index.parse::<usize>().ok() != Some(cues.len() + 1) {
            return Err(Error::Invalid("nonsequential cue index".into()));
        }
        let times = lines
            .next()
            .ok_or_else(|| Error::Invalid("missing timecodes".into()))?;
        let (start, end) = times
            .split_once(" --> ")
            .ok_or_else(|| Error::Invalid("invalid interval separator".into()))?;
        let body = lines.collect::<Vec<_>>().join("\n");
        cues.push(Cue::new(
            cues.len() as u64 + 1,
            0,
            parse_time(start)?,
            parse_time(end)?,
            body,
        )?);
        if cues.len() > 100_000 {
            return Err(Error::Invalid("too many cues".into()));
        }
    }
    Ok(cues)
}
