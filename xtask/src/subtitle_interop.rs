use crate::fixture::bounded;
use nvsf_core::{Cue, export_ass, export_vtt, parse_srt};
use std::{error::Error, path::Path, process::Command, time::Duration};

pub fn verify(ffmpeg: &Path, root: &Path) -> Result<(), Box<dyn Error>> {
    let directory = root.join("subtitle formats 日本");
    std::fs::create_dir(&directory)?;
    for (format, cue, expected_start, expected_end) in [
        (
            "webvtt",
            Cue::new(
                1,
                0,
                1234,
                5678,
                "<b>سلام &amp; 日本</b>\nnext > literal".into(),
            )?,
            1234,
            5678,
        ),
        ("ass", Cue::new(1, 0, 1, 9, "سلام\n日本".into())?, 0, 10),
    ] {
        let input = directory.join(format!("input.{format}"));
        let output = directory.join(format!("decoded-{format}.srt"));
        let text = if format == "webvtt" {
            export_vtt(std::slice::from_ref(&cue))?
        } else {
            export_ass(std::slice::from_ref(&cue))?
        };
        std::fs::write(&input, text)?;
        // Decode to plain text so ASS font/style metadata is not confused with cue text.
        let mut command = Command::new(ffmpeg);
        command
            .args([
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-xerror",
                "-copyts",
                "-f",
                format,
                "-i",
            ])
            .arg(&input)
            .args(["-map", "0:s:0", "-c:s", "text", "-f", "srt", "-n"])
            .arg(&output);
        bounded(&mut command, Duration::from_secs(10))?;
        let decoded = parse_srt(&std::fs::read_to_string(&output)?)?;
        if decoded.len() != 1
            || decoded[0].text != cue.text
            || decoded[0].start_ms != expected_start
            || decoded[0].end_ms != expected_end
        {
            return Err(format!(
                "Independent FFmpeg {format} decoding changed cue count, literal text or timing"
            )
            .into());
        }
    }
    println!(
        "Independent native FFmpeg VTT/ASS decoding preserved literal Unicode/text and positive timing"
    );
    Ok(())
}
