//! Controlled location witness, not scene-text classification or OCR quality qualification.
use super::Result;
use nvsf_app::{Paths, native_request};
use nvsf_core::Rect;
use std::{path::Path, process::Command, sync::atomic::AtomicBool, time::Duration};

pub fn generate(ffmpeg: &Path, root: &Path, font: &Path) -> Result<()> {
    let font = font
        .to_string_lossy()
        .replace('\\', "/")
        .replace(':', "\\:")
        .replace('\'', "\\'");
    let filter = format!(
        "drawtext=fontfile='{font}':text='Hello':fontsize=32:fontcolor=white:x=40:y=145,drawtext=fontfile='{font}':text='World':fontsize=32:fontcolor=white:x=400:y=145,drawtext=fontfile='{font}':text='16':fontsize=32:fontcolor=white:x=260:y=285"
    );
    let mut command = Command::new(ffmpeg);
    command
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-filter_threads",
            "2",
            "-f",
            "lavfi",
            "-i",
            "color=c=0x17202a:s=640x360:r=2:d=1",
            "-vf",
            &filter,
            "-c:v",
            "mpeg4",
            "-threads",
            "2",
            "-q:v",
            "2",
            "-n",
        ])
        .arg(root.join("regions.mp4"));
    crate::fixture::bounded(&mut command, Duration::from_secs(10))
}

fn covers(region: Rect, point: (f64, f64)) -> bool {
    point.0 >= region.x
        && point.0 <= region.x + region.width
        && point.1 >= region.y
        && point.1 <= region.y + region.height
}

pub fn verify(paths: &Paths, fixture_root: &Path) -> Result<()> {
    let input = fixture_root.join("regions.mp4").canonicalize()?;
    let (response, _) = native_request::supervise(
        paths,
        &input,
        native_request::Operation::Suggest {
            language: "en".into(),
        },
        &AtomicBool::new(false),
    )?;
    let native_request::Response::Suggest(regions) = response else {
        return Err("Full-frame region witness returned a different response".into());
    };
    if regions.len() < 3 || regions.len() > 8 {
        return Err(
            "Full-frame region witness lost disconnected locations or exceeded eight proposals"
                .into(),
        );
    }
    let left = (78.0 / 640.0, 160.0 / 360.0);
    let right = (445.0 / 640.0, 160.0 / 360.0);
    let bottom = (277.0 / 640.0, 300.0 / 360.0);
    for region in &regions {
        region.validate()?;
        if covers(*region, left) && covers(*region, right) {
            return Err("Separated middle detections were collapsed into one envelope".into());
        }
    }
    for point in [left, right, bottom] {
        if !regions.iter().any(|&region| covers(region, point)) {
            return Err(
                "Full-frame proposals omitted a known rendered middle/bottom location".into(),
            );
        }
    }
    Ok(())
}
