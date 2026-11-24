use super::gui::{Driver, Result};
use serde_json::{Value, json};
use std::path::Path;

fn pixels(driver: &Driver) -> Result<Value> {
    driver.script(
        r#"
        const image=document.querySelector('.source-frame');
        if(!image?.complete||!image.naturalWidth) return null;
        const canvas=document.createElement('canvas');
        canvas.width=image.naturalWidth;canvas.height=image.naturalHeight;
        const context=canvas.getContext('2d');context.drawImage(image,0,0);
        const data=context.getImageData(0,0,canvas.width,canvas.height).data;
        let hash=2166136261;
        for(const value of data) hash=Math.imul(hash^value,16777619);
        const samples=[[10,10],[20,20],[30,30]].map(([x,y])=>
            Array.from(context.getImageData(x,y,1,1).data).slice(0,3));
        return {width:canvas.width,height:canvas.height,samples,pixel_hash:hash>>>0};
    "#,
    )
}

/// Adds a distinct queued extraction without replacing the direct-extraction witness.
pub fn start_queued_b(driver: &Driver, output: &Path) -> Result<Value> {
    driver.click("nav[aria-label='Workspace navigation'] .nav-item:nth-of-type(1)")?;
    driver.wait(
        "return document.querySelector('h1')?.textContent==='Extract';",
        5,
    )?;
    driver.click("[data-testid=e2e-load-queue-fixture]")?;
    driver.wait("const image=document.querySelector('.source-frame');return image?.complete&&image.naturalWidth===640&&image.naturalHeight===360&&document.querySelector('.preview-shell')?.getAttribute('aria-busy')==='false'&&document.querySelector('[data-testid=e2e-load-queue-fixture]')?.disabled===false;", 15)?;
    let expected = pixels(driver)?;
    if expected["width"] != 640 || expected["height"] != 360 {
        return Err(format!("Queued B preview must be the English fixture: {expected}").into());
    }
    driver.click("#extract-queue")?;
    driver.wait("return document.querySelector('#extract-queue')?.disabled===false && Array.from(document.querySelectorAll('[role=status]')).some(e=>e.textContent.includes('Saved to the durable queue'));", 10)?;
    driver.click("[data-testid=e2e-load-contrast]")?;
    driver.wait("const image=document.querySelector('.source-frame');return image?.complete&&image.naturalWidth===320&&image.naturalHeight===240&&document.querySelector('.preview-shell')?.getAttribute('aria-busy')==='false';", 15)?;
    let before = pixels(driver)?;
    if before["samples"].as_array().is_none_or(|samples| {
        samples.iter().any(|sample| {
            sample[0].as_u64().unwrap_or(0) < 150 || sample[1].as_u64().unwrap_or(255) > 80
        })
    }) {
        return Err(
            format!("Contrast A must have independently decoded red pixels: {before}").into(),
        );
    }
    driver.click("nav[aria-label='Workspace navigation'] .nav-item:nth-of-type(2)")?;
    driver.wait("return Array.from(document.querySelectorAll('.queue-row')).some(row=>row.querySelector('strong')?.textContent==='en.mp4'&&row.querySelector('[data-testid=queue-start]')?.disabled===false);", 5)?;
    let started = driver.script(
        r#"
        const row=Array.from(document.querySelectorAll('.queue-row'))
            .find(row=>row.querySelector('strong')?.textContent==='en.mp4');
        const button=row?.querySelector('[data-testid=queue-start]');
        if(!button||button.disabled)return false;
        button.click();return true;
    "#,
    )?;
    if started != true {
        return Err("The real queue start control did not start B".into());
    }
    let evidence = json!({"expected_b":expected,"before_a":before});
    std::fs::write(
        output.join("source-switch-start.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    Ok(evidence)
}

pub fn verify_completed_b(driver: &Driver, output: &Path, before: Value) -> Result<()> {
    // Do not click a cue or seek: either action would conceal the original wrong-source defect.
    driver.wait("const image=document.querySelector('.source-frame');return image?.complete&&image.naturalWidth===640&&image.naturalHeight===360&&document.querySelector('.preview-shell')?.getAttribute('aria-busy')==='false';", 15)?;
    let actual = pixels(driver)?;
    let expected = before["expected_b"]["samples"]
        .as_array()
        .ok_or("Missing B samples")?;
    let samples = actual["samples"]
        .as_array()
        .ok_or("Missing completed B samples")?;
    if expected.len() != samples.len()
        || expected.iter().zip(samples).any(|(a, b)| {
            (0..3).any(|i| {
                a[i].as_i64()
                    .zip(b[i].as_i64())
                    .is_none_or(|(a, b)| (a - b).abs() > 3)
            })
        })
    {
        return Err(
            format!("Completed B review shows pixels from a different source: {actual}").into(),
        );
    }
    std::fs::write(
        output.join("source-switch-review.json"),
        serde_json::to_vec_pretty(&json!({
            "source_sha":std::env::var("GITHUB_SHA")?,"target":std::env::var("NVSF_TARGET_ID")?,
            "before":before,"completed_b":actual,"queue_started_through_ui":true,
            "correct_source_before_any_manual_seek":true,"passed":true
        }))?,
    )?;
    Ok(())
}
