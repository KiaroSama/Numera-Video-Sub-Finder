use super::gui::{Driver, Result};
use serde_json::json;
use std::path::Path;

/// Verifies v0.5.0 studio ergonomics on the Extract workspace:
/// - zero outer vertical scrollbar at default studio viewport and zero inner preview scrollbar at 100% Fit
/// - live ROI drag preview (`.roi-draft`), 8 resize handles, 4 VideoSubFinder separating guides
/// - Extract Undo/Redo toolbar controls revert and restore ROI changes
/// - playback transport bar (`#preview-play`, `#preview-stop`, `#preview-timeline`, `#set-start`, `#set-end`)
/// - header Save / Save As and F1 Shortcuts modal open and dismiss cleanly
pub fn verify(driver: &Driver, output: &Path) -> Result<()> {
    // 1. Default studio geometry: no outer vertical overflow and no preview inner scrollbars at Fit (100%).
    let geom = driver.script(
        "const doc=document.documentElement,vp=document.querySelector('.preview-viewport');\
         return {outerScrollH:doc.scrollHeight,innerH:window.innerHeight,outerScrollW:doc.scrollWidth,innerW:window.innerWidth,\
         vpScrollH:vp?vp.scrollHeight:-1,vpClientH:vp?vp.clientHeight:-1,vpScrollW:vp?vp.scrollWidth:-1,vpClientW:vp?vp.clientWidth:-1,\
         hasSplitter:!!document.querySelector('#inspector-splitter'),\
         hasSave:!!document.querySelector('#header-save'),hasSaveAs:!!document.querySelector('#header-save-as'),\
         hasPlay:!!document.querySelector('#preview-play'),hasStop:!!document.querySelector('#preview-stop'),\
         hasTimeline:!!document.querySelector('#preview-timeline'),hasSetStart:!!document.querySelector('#set-start'),\
         hasSetEnd:!!document.querySelector('#set-end')};",
    )?;
    let outer_ok = geom["outerScrollH"].as_f64().unwrap_or(9999.0)
        <= geom["innerH"].as_f64().unwrap_or(0.0) + 2.0;
    let vp_ok = geom["vpScrollH"].as_f64().unwrap_or(9999.0)
        <= geom["vpClientH"].as_f64().unwrap_or(0.0) + 2.0
        && geom["vpScrollW"].as_f64().unwrap_or(9999.0)
            <= geom["vpClientW"].as_f64().unwrap_or(0.0) + 2.0;
    if !outer_ok
        || !vp_ok
        || geom["hasSplitter"] != true
        || geom["hasSave"] != true
        || geom["hasSaveAs"] != true
        || geom["hasPlay"] != true
        || geom["hasStop"] != true
        || geom["hasTimeline"] != true
        || geom["hasSetStart"] != true
        || geom["hasSetEnd"] != true
    {
        return Err(format!("Studio shell geometry or controls failed: {geom}").into());
    }

    // 2. ROI live drag preview (.roi-draft), 8 resize handles, 4 separating guides, and Extract Undo/Redo.
    let drag_start = driver.script(
        "const f=document.querySelector('.frame');if(!f)return {err:'missing frame'};\
         const r=f.getBoundingClientRect();\
         const x1=r.left+r.width*0.18,y1=r.top+r.height*0.18,x2=r.left+r.width*0.48,y2=r.top+r.height*0.36;\
         const before=document.querySelectorAll('.roi:not(.excluded)').length;\
         f.dispatchEvent(new MouseEvent('mousedown',{bubbles:true,button:0,clientX:x1,clientY:y1}));\
         window.dispatchEvent(new MouseEvent('mousemove',{bubbles:true,button:0,clientX:x2,clientY:y2}));\
         return {before,x2,y2};",
    )?;
    if drag_start.get("err").is_some() {
        return Err(format!("ROI drag setup failed: {drag_start}").into());
    }
    driver.wait("return !!document.querySelector('.roi-draft');", 5)?;
    let roi = driver.script(
        "const f=document.querySelector('.frame'),r=f.getBoundingClientRect();\
         const x2=r.left+r.width*0.48,y2=r.top+r.height*0.36;\
         const draftShown=!!document.querySelector('.roi-draft');\
         window.dispatchEvent(new MouseEvent('mouseup',{bubbles:true,button:0,clientX:x2,clientY:y2}));\
         return {draftShown};",
    )?;
    if roi["draftShown"] != true {
        return Err(format!("ROI rubber-band live drag preview did not appear: {roi}").into());
    }
    let before_count = drag_start["before"].as_u64().unwrap_or(1);
    let wait_added = format!(
        "return document.querySelectorAll('.roi:not(.excluded)').length === {};",
        before_count + 1
    );
    driver.wait(&wait_added, 5)?;

    let handles_and_guides = driver.script(
        "return {handles:document.querySelectorAll('.roi.selected .roi-handle').length,\
         guides:document.querySelectorAll('.roi-guide').length,\
         undoEnabled:document.querySelector('#header-undo')?.disabled===false};",
    )?;
    if handles_and_guides["handles"].as_u64() != Some(8)
        || handles_and_guides["guides"].as_u64() != Some(4)
        || handles_and_guides["undoEnabled"] != true
    {
        return Err(format!(
            "Selected ROI handles/guides or Undo state failed: {handles_and_guides}"
        )
        .into());
    }

    // Undo removes the drawn ROI; Redo restores it; second Undo returns to baseline cleanly.
    driver.click("#header-undo")?;
    let wait_undone = format!(
        "return document.querySelectorAll('.roi:not(.excluded)').length === {} && document.querySelector('#header-redo')?.disabled===false;",
        before_count
    );
    driver.wait(&wait_undone, 5)?;
    driver.click("#header-redo")?;
    driver.wait(&wait_added, 5)?;
    driver.click("#header-undo")?;
    let wait_restored = format!(
        "return document.querySelectorAll('.roi:not(.excluded)').length === {};",
        before_count
    );
    driver.wait(&wait_restored, 5)?;

    // 3. Actual source PTS, compressed keyframe stepping and elapsed-time playback.
    let transport = transport(driver)?;

    // 4. Shortcuts modal opens via header button and closes cleanly.
    driver.click("#header-shortcuts")?;
    driver.wait(
        "return document.querySelector('#shortcuts-modal')?.open === true;",
        5,
    )?;
    driver.script(
        "const b=document.querySelector('#shortcuts-modal button');if(b)b.click();return true;",
    )?;
    driver.wait(
        "return document.querySelector('#shortcuts-modal')?.open !== true;",
        5,
    )?;

    std::fs::write(
        output.join("studio-controls.json"),
        serde_json::to_vec_pretty(&json!({
            "geometry": geom,
            "roi_drag_preview": roi,
            "handles_and_guides": handles_and_guides,
            "undo_redo_verified": true,
            "transport_verified": true,
            "transport": transport,
            "shortcuts_modal_verified": true
        }))?,
    )?;
    Ok(())
}

fn frame(driver: &Driver, expected: u64) -> Result<()> {
    driver.wait(
        &format!(
            "const p=document.querySelector('.preview-shell');return p?.getAttribute('aria-busy')!=='true' && p?.dataset.displayedTimeMs==='{expected}' && p?.dataset.frameDurationMs==='100' && document.querySelector('.source-frame')?.naturalWidth>0;"
        ),
        30,
    )
}

fn seek(driver: &Driver, at: u64) -> Result<()> {
    driver.script(&format!(
        "const t=document.querySelector('#preview-timeline');t.value='{at}';t.dispatchEvent(new Event('input',{{bubbles:true}}));return true;"
    ))?;
    frame(driver, at)
}

fn transport(driver: &Driver) -> Result<serde_json::Value> {
    // The shared en.mp4 fixture has 10 fps and an explicit 10-frame GOP.
    seek(driver, 0)?;
    driver.click("[data-testid=next-frame]")?;
    frame(driver, 100)?;
    for (key, expected) in [("ArrowRight", 200), ("ArrowLeft", 100)] {
        driver.script(&format!(
            "const v=document.querySelector('.preview-viewport');v.focus();v.dispatchEvent(new KeyboardEvent('keydown',{{key:'{key}',bubbles:true}}));return true;"
        ))?;
        frame(driver, expected)?;
    }
    for _ in 0..2 {
        driver.click("[data-testid=previous-frame]")?;
        frame(driver, 0)?;
    }
    seek(driver, 1000)?;
    driver.click("[data-testid=previous-frame]")?;
    frame(driver, 900)?;
    driver.click("[data-testid=next-frame]")?;
    frame(driver, 1000)?;
    seek(driver, 5900)?;
    driver.click("[data-testid=next-frame]")?;
    frame(driver, 5900)?;

    seek(driver, 5500)?;
    driver.click("#preview-play")?;
    driver.wait(
        "return document.querySelector('#preview-play')?.getAttribute('aria-pressed')==='false' && document.querySelector('.preview-shell')?.dataset.displayedTimeMs==='5900' && document.querySelector('.preview-shell')?.getAttribute('aria-busy')!=='true';",
        30,
    )?;
    let playback = driver.request(
        reqwest::Method::POST,
        "/execute/async",
        json!({"script":"const done=arguments[arguments.length-1],started=performance.now();document.querySelector('#preview-play').click();setTimeout(()=>done({elapsed:performance.now()-started,position:Number(document.querySelector('#preview-timeline').value),displayed:Number(document.querySelector('.preview-shell').dataset.displayedTimeMs),playing:document.querySelector('#preview-play').getAttribute('aria-pressed')==='true'}),1500);","args":[]}),
    )?;
    let elapsed = playback["elapsed"]
        .as_f64()
        .ok_or("Missing elapsed playback time")?;
    let position = playback["position"]
        .as_f64()
        .ok_or("Missing playback position")?;
    if elapsed < 1450.0 || (position - elapsed).abs() > 400.0 || playback["playing"] != true {
        return Err(format!("Playback did not restart and follow elapsed time: {playback}").into());
    }
    driver.click("#preview-play")?;
    driver.wait(
        "return document.querySelector('#preview-play')?.getAttribute('aria-pressed')==='false' && document.querySelector('.preview-shell')?.getAttribute('aria-busy')!=='true';",
        30,
    )?;
    let paused = stationary(driver)?;
    driver.click("#preview-play")?;
    driver.wait(
        "return document.querySelector('#preview-play')?.getAttribute('aria-pressed')==='true';",
        5,
    )?;
    seek(driver, 2500)?;
    let manual_seek = stationary(driver)?;
    if manual_seek["after"].as_u64() != Some(2500) {
        return Err(format!("Manual seek did not replace playback: {manual_seek}").into());
    }
    driver.click("#preview-stop")?;
    frame(driver, 0)?;
    Ok(json!({
        "source": "en.mp4; generated 10fps MPEG-4 with GOP10",
        "button_and_keyboard_pts": [0,100,200,100,0],
        "keyframe_pts": [1000,900,1000],
        "first_and_last_clamped": [0,5900],
        "natural_end_restart_and_clock": playback,
        "paused": paused,
        "manual_seek": manual_seek,
        "stop_returned_to_start": true
    }))
}

fn stationary(driver: &Driver) -> Result<serde_json::Value> {
    let result = driver.request(
        reqwest::Method::POST,
        "/execute/async",
        json!({"script":"const done=arguments[arguments.length-1],before=Number(document.querySelector('#preview-timeline').value);setTimeout(()=>done({before,after:Number(document.querySelector('#preview-timeline').value),playing:document.querySelector('#preview-play').getAttribute('aria-pressed')==='true',displayed:Number(document.querySelector('.preview-shell').dataset.displayedTimeMs)}),350);","args":[]}),
    )?;
    if result["before"] != result["after"]
        || result["playing"] != false
        || result["after"] != result["displayed"]
    {
        return Err(format!("Paused or manually sought frame kept advancing: {result}").into());
    }
    Ok(result)
}
