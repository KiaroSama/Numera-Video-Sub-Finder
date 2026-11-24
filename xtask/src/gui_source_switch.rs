use super::gui::{Driver, Result};
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{Duration, Instant},
};

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

fn queue_entries(driver: &Driver) -> Result<Vec<Value>> {
    let result = driver.request(
        reqwest::Method::POST,
        "/execute/async",
        json!({
            "script": "const done=arguments[arguments.length-1];window.__TAURI__.core.invoke('queue_list').then(entries=>done({entries}),error=>done({error:String(error)}));",
            "args": []
        }),
    )?;
    let entries = result["entries"]
        .as_array()
        .cloned()
        .ok_or_else(|| format!("Could not inspect native source-switch queue: {result}"))?;
    let mut ids = std::collections::HashSet::new();
    for entry in &entries {
        let id = entry["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("Native source-switch queue contains a missing or invalid ID")?;
        if !ids.insert(id.to_owned()) {
            return Err(format!("Native source-switch queue contains duplicate ID {id}").into());
        }
    }
    Ok(entries)
}

fn wait_queue_entry(
    driver: &Driver,
    phase: &str,
    select: impl Fn(&[Value]) -> Result<Option<Value>>,
) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let entries = queue_entries(driver)?;
        if let Some(entry) = select(&entries)? {
            return Ok(entry);
        }
        let error =
            driver.script("return document.querySelector('[role=alert]')?.textContent || '';")?;
        if error.as_str().is_some_and(|error| !error.is_empty()) {
            return Err(format!("Source-switch queue failed during {phase}: {error}").into());
        }
        if Instant::now() >= deadline {
            return Err(
                format!("Source-switch queue deadline during {phase}; entries={entries:?}").into(),
            );
        }
        std::thread::park_timeout(Duration::from_millis(50));
    }
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
    let queue_before = queue_entries(driver)?;
    let completed_distractor = queue_before
        .iter()
        .find(|entry| {
            entry["name"] == "en.mp4"
                && entry["output_name"] == "ui-en.srt"
                && entry["state"] == "completed"
        })
        .cloned()
        .ok_or("Source-switch witness requires the retained completed direct en.mp4 record")?;
    driver.click("#extract-queue")?;
    let queued_b = wait_queue_entry(driver, "B admission", |entries| {
        if queue_before
            .iter()
            .any(|old| !entries.iter().any(|entry| entry["id"] == old["id"]))
        {
            return Err(format!(
                "B admission removed an existing queue record; before={queue_before:?}; after={entries:?}"
            )
            .into());
        }
        let added: Vec<_> = entries
            .iter()
            .filter(|entry| !queue_before.iter().any(|old| old["id"] == entry["id"]))
            .collect();
        if added.is_empty() {
            return Ok(None);
        }
        if added.len() != 1 {
            return Err(format!(
                "B admission must add exactly one native queue record; before={queue_before:?}; after={entries:?}"
            )
            .into());
        }
        let entry = added[0];
        if entry["name"] != "en.mp4"
            || entry["output_name"] != "ui-queue-b.srt"
            || entry["state"] != "queued"
            || entry["needs_regrant"] != false
            || entry["resumable"] != false
            || !entry["error"].is_null()
        {
            return Err(format!("New B queue identity/state mismatch: {entry}").into());
        }
        let ready = driver.script("return document.querySelector('#extract-queue')?.disabled===false && Array.from(document.querySelectorAll('[role=status]')).some(e=>e.textContent.includes('Saved to the durable queue'));")?;
        Ok((ready == true).then(|| entry.clone()))
    })?;
    let queue_id = queued_b["id"]
        .as_str()
        .ok_or("New B queue record is missing its stable ID")?
        .to_owned();
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
    driver.wait(
        &format!(
            "const rows=Array.from(document.querySelectorAll('.queue-row')).filter(row=>row.getAttribute('data-queue-id')==={});return document.querySelector('h1')?.textContent==='Queue'&&rows.length===1&&rows[0].querySelector('[data-testid=queue-start]')?.disabled===false;",
            serde_json::to_string(&queue_id)?
        ),
        5,
    )?;
    let clicked = driver.request(
        reqwest::Method::POST,
        "/execute/sync",
        json!({
            "script": r#"
                const id=arguments[0];
                const rows=Array.from(document.querySelectorAll('.queue-row'));
                const matches=rows.filter(row=>row.getAttribute('data-queue-id')===id);
                const row=matches.length===1 ? matches[0] : null;
                const button=row?.querySelector('[data-testid=queue-start]');
                const receipt={
                    queue_id:id,
                    matching_rows:matches.length,
                    clicked:false,
                    rows:rows.map(row=>({
                        id:row.getAttribute('data-queue-id'),
                        name:row.querySelector('strong')?.textContent ?? null,
                        disabled:row.querySelector('[data-testid=queue-start]')?.disabled ?? null
                    }))
                };
                if(!button||button.disabled)return receipt;
                button.click();
                receipt.clicked=true;
                return receipt;
            "#,
            "args": [queue_id]
        }),
    )?;
    if clicked["clicked"] != true
        || clicked["matching_rows"] != 1
        || clicked["queue_id"] != queue_id
    {
        return Err(format!("Could not click the exact B queue control: {clicked}").into());
    }
    let evidence = json!({
        "expected_b": expected,
        "before_a": before,
        "queue_before": queue_before,
        "completed_distractor": completed_distractor,
        "queued_b": queued_b,
        "queue_id": queue_id,
        "queue_click": clicked
    });
    std::fs::write(
        output.join("source-switch-start.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    Ok(evidence)
}

pub fn verify_completed_b(driver: &Driver, output: &Path, before: Value) -> Result<()> {
    let queue_id = before["queue_id"]
        .as_str()
        .ok_or("Source-switch start receipt is missing the B queue ID")?;
    let distractor_id = before["completed_distractor"]["id"]
        .as_str()
        .ok_or("Source-switch start receipt is missing the completed distractor ID")?;
    let completed_queue_entry = wait_queue_entry(driver, "B completion", |entries| {
        let distractor = entries
            .iter()
            .find(|entry| entry["id"] == distractor_id)
            .ok_or("The original completed en.mp4 queue record was removed")?;
        if distractor["state"] != "completed" {
            return Err(format!(
                "The original completed en.mp4 record changed state: {distractor}"
            )
            .into());
        }
        let entry = entries
            .iter()
            .find(|entry| entry["id"] == queue_id)
            .ok_or("The exact B queue record disappeared before completion")?;
        if entry["name"] != "en.mp4"
            || entry["output_name"] != "ui-queue-b.srt"
            || entry["needs_regrant"] != false
            || !entry["error"].is_null()
        {
            return Err(format!("B queue identity or grant changed: {entry}").into());
        }
        match entry["state"].as_str() {
            Some("completed") => Ok(Some(entry.clone())),
            Some("queued" | "running") => Ok(None),
            _ => Err(format!("The exact B queue record did not complete: {entry}").into()),
        }
    })?;
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
            "queue_id":queue_id,"completed_queue_entry":completed_queue_entry,
            "correct_source_before_any_manual_seek":true,"passed":true
        }))?,
    )?;
    Ok(())
}
