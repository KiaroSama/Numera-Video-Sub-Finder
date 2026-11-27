use super::gui::{Driver, Result};
use serde_json::json;
use std::path::Path;

/// Verifies studio ergonomics on the Extract workspace:
/// - zero outer vertical scrollbar at default studio viewport and zero inner preview scrollbar at 100% Fit
/// - live ROI drag preview (`.roi-draft`), 8 resize handles, 4 VideoSubFinder separating guides
/// - Extract Undo/Redo toolbar controls revert and restore ROI changes
/// - playback transport bar (`#preview-play`, `#preview-stop`, `#preview-timeline`, `#set-start`, `#set-end`)
/// - pre-extraction Save / Save As, notifications, Focus Mode, marquee zoom and Shortcuts
pub fn verify(driver: &Driver, output: &Path) -> Result<()> {
    // 1. Default studio geometry and Save controls before any extraction or edits.
    let geom = driver.script(
        "const doc=document.documentElement,vp=document.querySelector('.preview-viewport');\
         return {outerScrollH:doc.scrollHeight,innerH:window.innerHeight,outerScrollW:doc.scrollWidth,innerW:window.innerWidth,\
         vpScrollH:vp?vp.scrollHeight:-1,vpClientH:vp?vp.clientHeight:-1,vpScrollW:vp?vp.scrollWidth:-1,vpClientW:vp?vp.clientWidth:-1,\
         hasSplitter:!!document.querySelector('#inspector-splitter'),\
         hasSave:!!document.querySelector('#header-save'),hasSaveAs:!!document.querySelector('#header-save-as'),\
         saveEnabled:document.querySelector('#header-save')?.disabled===false,\
         saveAsEnabled:document.querySelector('#header-save-as')?.disabled===false,\
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
        || geom["saveEnabled"] != true
        || geom["saveAsEnabled"] != true
        || geom["hasPlay"] != true
        || geom["hasStop"] != true
        || geom["hasTimeline"] != true
        || geom["hasSetStart"] != true
        || geom["hasSetEnd"] != true
    {
        return Err(format!("Studio shell geometry or controls failed: {geom}").into());
    }

    verify_menubar(driver, output)?;
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
    let drawn_pressed = roi_pressed(driver, before_count + 1, before_count)?;
    driver.click("button[aria-label='Select region 1']")?;
    let canvas_pressed = roi_pressed(driver, before_count + 1, 0)?;
    driver.click(&format!(
        ".region-list .region-row:nth-child({}) .region-select",
        before_count + 1
    ))?;
    let inspector_pressed = roi_pressed(driver, before_count + 1, before_count)?;

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
    let studio_v2 = verify_studio_v2(driver)?;

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
            "roi_pressed_states": [drawn_pressed, canvas_pressed, inspector_pressed],
            "undo_redo_verified": true,
            "transport_verified": true,
            "transport": transport,
            "studio_v2": studio_v2,
            "shortcuts_modal_verified": true
        }))?,
    )?;
    Ok(())
}

fn verify_studio_v2(driver: &Driver) -> Result<serde_json::Value> {
    driver.click("#header-notifications")?;
    driver.wait("const p=document.querySelector('#notifications-panel'),b=document.querySelector('#header-notifications');return !!p?.getClientRects().length&&!p.hidden&&p.contains(document.activeElement)&&b?.getAttribute('aria-expanded')==='true';", 5)?;
    let bell_escape = menu_key(driver, "Escape", false)?;
    driver.wait("const p=document.querySelector('#notifications-panel'),b=document.querySelector('#header-notifications');return (!p||p.hidden||p.getClientRects().length===0)&&document.activeElement===b&&b?.getAttribute('aria-expanded')==='false';", 5)?;

    let focused = "return document.documentElement.dataset.focusMode==='true'&&!!document.querySelector('.shell.focus-mode .focus-hud')?.getClientRects().length&&document.querySelector('#header-notifications')?.getClientRects().length===0;";
    let normal = "return document.documentElement.dataset.focusMode!=='true'&&!document.querySelector('.shell.focus-mode')&&!!document.querySelector('#header-notifications')?.getClientRects().length;";
    let focus_enter = menu_key(driver, "`", false)?;
    driver.wait(focused, 5)?;
    let focus_exit = menu_key(driver, "`", false)?;
    driver.wait(normal, 5)?;
    driver.click("#focus-mode-toggle")?;
    driver.wait(focused, 5)?;
    let focus_escape = menu_key(driver, "Escape", false)?;
    driver.wait(normal, 5)?;

    driver.script("document.querySelector('#zoom-tool-toggle').focus();return true;")?;
    let zoom_key = menu_key(driver, "z", false)?;
    driver.wait("return !!document.querySelector('.frame.zoom-tool-active')&&document.querySelector('#zoom-tool-toggle')?.getAttribute('aria-pressed')==='true';", 5)?;
    let drag = driver.script(r#"
        const frame=document.querySelector('.frame'), r=frame.getBoundingClientRect();
        const point=(x,y)=>({bubbles:true,cancelable:true,button:0,buttons:1,clientX:r.left+r.width*x,clientY:r.top+r.height*y});
        const before=frame.getBoundingClientRect().width, count=document.querySelectorAll('.roi').length;
        frame.dispatchEvent(new MouseEvent('mousedown',point(.2,.2)));
        frame.dispatchEvent(new MouseEvent('mousemove',point(.55,.55)));
        return {before,count,end:point(.55,.55)};
    "#)?;
    driver.wait("return !!document.querySelector('.zoom-marquee');", 5)?;
    driver.script(&format!(
        "const frame=document.querySelector('.frame');frame.dispatchEvent(new MouseEvent('mouseup',{{...{},buttons:0}}));return true;",
        drag["end"]
    ))?;
    let zoom_state = "const v=document.querySelector('.preview-viewport'),f=document.querySelector('.frame');return {percent:parseFloat(f.style.getPropertyValue('--zoom-width')),width:f.getBoundingClientRect().width,scroll:[v.scrollLeft,v.scrollTop],count:document.querySelectorAll('.roi').length};";
    driver.wait("const f=document.querySelector('.frame');return parseFloat(f?.style.getPropertyValue('--zoom-width'))>100&&!document.querySelector('.zoom-marquee');", 5)?;
    let zoomed = driver.script(zoom_state)?;
    let enlarged = zoomed["width"]
        .as_f64()
        .zip(drag["before"].as_f64())
        .is_some_and(|(after, before)| after > before && before > 0.0);
    if zoomed["count"] != drag["count"] || !enlarged {
        return Err(format!(
            "Marquee zoom did not enlarge the frame without altering ROIs: {drag} {zoomed}"
        )
        .into());
    }
    driver.click("button[title='Reset zoom (Ctrl+0)']")?;
    driver.wait("const v=document.querySelector('.preview-viewport'),f=document.querySelector('.frame');return parseFloat(f?.style.getPropertyValue('--zoom-width'))===100&&v?.scrollLeft===0&&v?.scrollTop===0;", 5)?;
    let reset = driver.script(zoom_state)?;
    if driver.script("return !!document.querySelector('.frame.zoom-tool-active');")? == true {
        driver.click("#zoom-tool-toggle")?;
    }
    driver.wait("return !document.querySelector('.frame.zoom-tool-active')&&document.querySelector('#zoom-tool-toggle')?.getAttribute('aria-pressed')==='false';", 5)?;

    let timeline_state = "const t=document.querySelector('#preview-timeline');return Object.fromEntries(['--playhead','--range-in','--range-out'].map(k=>[k,t.style.getPropertyValue(k).trim()]));";
    let start = driver.script(timeline_state)?;
    seek(driver, 1000)?;
    let later = driver.script(timeline_state)?;
    let percent = |value: &serde_json::Value| {
        value
            .as_str()?
            .strip_suffix('%')?
            .parse::<f64>()
            .ok()
            .filter(|n| (0.0..=100.0).contains(n))
    };
    if percent(&start["--playhead"]) != Some(0.0)
        || !percent(&later["--playhead"]).is_some_and(|n| n > 0.0)
        || percent(&start["--range-in"]).is_none()
        || percent(&start["--range-out"]).is_none()
        || start["--range-in"] != later["--range-in"]
        || start["--range-out"] != later["--range-out"]
    {
        return Err(format!(
            "Moving the timeline playhead changed or omitted its range: {start} {later}"
        )
        .into());
    }
    seek(driver, 0)?;
    Ok(json!({
        "method":"WebDriver clicks with synthetic keydown and mouse drag events; physical input unverified",
        "bell_escape":bell_escape,"bell_focus_restored":true,
        "focus_entry_and_exit":[focus_enter,focus_exit,focus_escape],
        "zoom_key":zoom_key,"marquee_visible":true,"zoomed":zoomed,"reset":reset,"timeline":[start,later]
    }))
}

pub(super) fn verify_language_packs(driver: &Driver, output: &Path) -> Result<()> {
    driver.wait(r#"
        const list=document.querySelector('.language-packs-panel [data-testid=installed-languages-list]');
        return !!list?.getClientRects().length && list.querySelector('h3')?.textContent.trim()==='Installed language packs'
            && /^Installed: \d+ of \d+ packs$/.test(list.querySelector('[role=status]')?.textContent.trim());
    "#, 10)?;
    let packs = driver.script("const list=document.querySelector('[data-testid=installed-languages-list]');return {heading:list.closest('.language-packs-panel').querySelector('h2').textContent.trim(),summary:list.querySelector('[role=status]').textContent.trim(),autoLoaded:true};")?;
    if packs["heading"] != "Additional language packs" {
        return Err(
            format!("Installed packs list rendered outside the expected section: {packs}").into(),
        );
    }
    std::fs::write(
        output.join("installed-languages-list.json"),
        serde_json::to_vec_pretty(&packs)?,
    )?;
    Ok(())
}

fn roi_pressed(driver: &Driver, count: u64, selected: u64) -> Result<serde_json::Value> {
    driver.wait(
        &format!(
            "const groups=['.roi:not(.excluded) .roi-pick','.region-list .region-select'].map(selector=>Array.from(document.querySelectorAll(selector)));return groups.every(buttons=>buttons.length==={count}&&buttons.every((button,index)=>button.getAttribute('aria-pressed')===(index==={selected}?'true':'false')&&button.parentElement.classList.contains('selected')===(index==={selected})));"
        ),
        5,
    )?;
    driver.script("return {canvas:Array.from(document.querySelectorAll('.roi:not(.excluded) .roi-pick'),button=>button.getAttribute('aria-pressed')),inspector:Array.from(document.querySelectorAll('.region-list .region-select'),button=>button.getAttribute('aria-pressed'))};")
}

fn frame(driver: &Driver, expected: u64) -> Result<()> {
    driver.wait(
        &format!(
            "const p=document.querySelector('.preview-shell'),image=document.querySelector('.source-frame');return p?.getAttribute('aria-busy')==='false' && p?.dataset.displayedTimeMs==='{expected}' && p?.dataset.frameDurationMs==='100' && image?.complete && image.naturalWidth>0 && document.querySelector('#preview-timeline')?.value==='{expected}' && document.querySelector('#preview-play')?.getAttribute('aria-pressed')==='false';"
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
    frame(driver, 5900)?;
    let playback = driver.request(
        reqwest::Method::POST,
        "/execute/async",
        json!({"script":r#"
            const done=arguments[arguments.length-1],started=performance.now(),loadedFrames=[];
            const oldUrl=document.querySelector('.source-frame')?.currentSrc;
            const onLoad=event=>{
                const image=event.target;
                if(!image?.matches?.('.source-frame')||!image.complete||!image.naturalWidth)return;
                const pts=Number(document.querySelector('.preview-shell')?.dataset.displayedTimeMs);
                const playing=document.querySelector('#preview-play')?.getAttribute('aria-pressed')==='true';
                if(!playing||!Number.isFinite(pts)||image.currentSrc!==image.src||image.currentSrc===oldUrl)return;
                if(loadedFrames.some(frame=>frame.url===image.currentSrc))return;
                loadedFrames.push({url:image.currentSrc,pts,playing,width:image.naturalWidth,height:image.naturalHeight});
            };
            document.addEventListener('load',onLoad,true);
            document.querySelector('#preview-play').click();
            setTimeout(()=>{
                document.removeEventListener('load',onLoad,true);
                done({
                    elapsed:performance.now()-started,
                    position:Number(document.querySelector('#preview-timeline').value),
                    displayed:Number(document.querySelector('.preview-shell').dataset.displayedTimeMs),
                    playing:document.querySelector('#preview-play').getAttribute('aria-pressed')==='true',
                    old_url:oldUrl,loaded_frames:loadedFrames
                });
            },1500);
        "#,"args":[]}),
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
    let loaded_frames = playback["loaded_frames"]
        .as_array()
        .ok_or("Missing decoded playback image loads")?;
    if !loaded_frames.iter().any(|frame| {
        frame["playing"] == true
            && frame["url"].as_str().is_some_and(|url| !url.is_empty())
            && frame["url"] != playback["old_url"]
            && frame["pts"]
                .as_u64()
                .is_some_and(|pts| pts > 0 && pts < 5900)
            && frame["width"] == 640
            && frame["height"] == 360
    }) {
        return Err(
            format!("No advancing decoded image loaded during playback: {playback}").into(),
        );
    }
    driver.click("#preview-play")?;
    driver.wait(
        "const p=document.querySelector('.preview-shell'),image=document.querySelector('.source-frame'),timeline=document.querySelector('#preview-timeline');return document.querySelector('#preview-play')?.getAttribute('aria-pressed')==='false' && p?.getAttribute('aria-busy')==='false' && image?.complete && image.naturalWidth>0 && p?.dataset.displayedTimeMs!==undefined && timeline?.value===p.dataset.displayedTimeMs;",
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
    if manual_seek["after"]["time"].as_u64() != Some(2500)
        || manual_seek["after"]["displayed"].as_u64() != Some(2500)
    {
        return Err(format!("Manual seek did not replace playback: {manual_seek}").into());
    }
    driver.script(
        "const timeline=document.querySelector('#preview-timeline');timeline.value='5999';timeline.dispatchEvent(new Event('input',{bubbles:true}));return true;",
    )?;
    frame(driver, 5900)?;
    let manual_end_restart = driver.request(
        reqwest::Method::POST,
        "/execute/async",
        json!({"script":r#"
            const done=arguments[arguments.length-1];
            const sample=()=>({
                position:Number(document.querySelector('#preview-timeline')?.value),
                displayed:Number(document.querySelector('.preview-shell')?.dataset.displayedTimeMs),
                playing:document.querySelector('#preview-play')?.getAttribute('aria-pressed')==='true'
            });
            const before=sample();
            document.querySelector('#preview-play').click();
            setTimeout(()=>done({requested_seek_ms:5999,play_presses:1,before,after:sample()}),0);
        "#,"args":[]}),
    )?;
    if manual_end_restart["before"]["position"] != 5900
        || manual_end_restart["before"]["displayed"] != 5900
        || manual_end_restart["before"]["playing"] != false
        || manual_end_restart["after"]["playing"] != true
        || manual_end_restart["after"]["position"]
            .as_u64()
            .is_none_or(|position| position >= 5900)
    {
        return Err(format!(
            "First Play did not restart the manual endpoint: {manual_end_restart}"
        )
        .into());
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
        "manual_end_restart": manual_end_restart,
        "stop_returned_to_start": true
    }))
}

fn stationary(driver: &Driver) -> Result<serde_json::Value> {
    let result = driver.request(
        reqwest::Method::POST,
        "/execute/async",
        json!({"script":r#"
            const done=arguments[arguments.length-1];
            const sample=()=>{
                const preview=document.querySelector('.preview-shell'),image=document.querySelector('.source-frame');
                return {time:Number(document.querySelector('#preview-timeline')?.value),
                    displayed:Number(preview?.dataset.displayedTimeMs),
                    pressed:document.querySelector('#preview-play')?.getAttribute('aria-pressed'),
                    busy:preview?.getAttribute('aria-busy'),
                    complete:image?.complete===true&&image.naturalWidth>0,
                    source:image?.currentSrc};
            };
            const before=sample();
            if(before.pressed!=='false'||before.busy!=='false'||!before.complete||before.time!==before.displayed){
                done({before,unsettled:true});return;
            }
            setTimeout(()=>done({before,after:sample()}),350);
        "#,"args":[]}),
    )?;
    let before = &result["before"];
    let after = &result["after"];
    if before["time"].as_u64().is_none()
        || before["displayed"].as_u64().is_none()
        || before["time"] != after["time"]
        || before["displayed"] != after["displayed"]
        || before["source"] != after["source"]
        || [before, after].iter().any(|sample| {
            sample["pressed"] != "false"
                || sample["busy"] != "false"
                || sample["complete"] != true
                || sample["time"] != sample["displayed"]
        })
    {
        return Err(format!("Paused or manually sought frame kept advancing: {result}").into());
    }
    Ok(result)
}

fn verify_menubar(driver: &Driver, output: &Path) -> Result<()> {
    driver.wait(r#"const bar = document.querySelector('#app-menubar[role=menubar][aria-label="Application menu"]');
        const items = [...(bar?.querySelectorAll(':scope > button[role=menuitem]') ?? [])];
        return !!bar?.getBoundingClientRect().height && items.map(item => item.textContent.trim()).join(',') === 'File,Edit,View,Tools,Help'
          && items.every((item, i) => item.getAttribute('tabindex') === (i ? '-1' : '0'))
          && document.documentElement.scrollHeight <= document.documentElement.clientHeight
          && getComputedStyle(document.querySelector('.preview-shell')).colorScheme.includes('dark');"#, 5)?;
    let mut keyboard = Vec::new();
    // These cancelable events exercise the app handlers, not native keyboard delivery.
    for (menu, key) in [("file", "Enter"), ("view", " ")] {
        driver.script(&format!(
            "document.querySelector('#menu-trigger-{menu}').focus();return true;"
        ))?;
        let opened = menu_key(driver, key, false)?;
        if opened["prevented"] != true {
            return Err(format!(
                "Menu activation did not cancel the native button toggle: {opened}"
            )
            .into());
        }
        driver.wait(&format!("const menu=document.querySelector('[aria-labelledby=menu-trigger-{menu}]:not([hidden])');return !!menu&&document.activeElement===menu.querySelector('button:enabled');"), 5)?;
        keyboard.push(opened);
        for (arrow, index) in [("ArrowUp", "items.length-1"), ("ArrowDown", "0")] {
            keyboard.push(menu_key(driver, arrow, false)?);
            driver.wait(&format!("const items=[...document.querySelectorAll('[aria-labelledby=menu-trigger-{menu}] button:enabled')];return items.length>0&&document.activeElement===items[{index}];"), 5)?;
        }
        keyboard.push(menu_key(driver, "Escape", false)?);
        driver.wait(&format!("return document.activeElement===document.querySelector('#menu-trigger-{menu}[aria-expanded=false]');"), 5)?;
    }
    driver.click("#menu-trigger-file")?;
    driver.wait(r#"return !!document.querySelector('#menu-trigger-file[aria-expanded=true]') && !!document.querySelector('[aria-labelledby=menu-trigger-file]:not([hidden]) [role=menuitem]');"#, 5)?;
    keyboard.push(menu_key(driver, "ArrowRight", false)?);
    driver.wait(r#"return !!document.querySelector('#menu-trigger-file[aria-expanded=false]') && document.activeElement === document.querySelector('#menu-trigger-edit[aria-expanded=true]');"#, 5)?;
    keyboard.push(menu_key(driver, "Escape", false)?);
    driver.wait(r#"return document.activeElement === document.querySelector('#menu-trigger-edit[aria-expanded=false]');"#, 5)?;
    std::fs::write(
        output.join("menubar-keyboard.json"),
        serde_json::to_vec_pretty(
            &json!({"method":"synthetic cancelable keydown on focused controls; native keyboard delivery unverified","events":keyboard}),
        )?,
    )?;
    verify_about(driver, output)?;
    Ok(())
}

fn menu_key(driver: &Driver, key: &str, control: bool) -> Result<serde_json::Value> {
    driver.script(&format!(r#"
        const target=document.activeElement;
        if(!target)throw new Error('No focused control for keyboard witness');
        const event=new KeyboardEvent('keydown',{{key:{},ctrlKey:{control},bubbles:true,cancelable:true}});
        target.dispatchEvent(event);
        return {{key:event.key,control:event.ctrlKey,target:target.id||target.tagName,prevented:event.defaultPrevented}};
    "#, serde_json::to_string(key)?))
}

fn modal_workspace(driver: &Driver) -> Result<serde_json::Value> {
    driver.script(r#"
        const required=selector=>{const element=document.querySelector(selector);if(!element)throw new Error(`Missing modal witness control: ${selector}`);return element;};
        const regions=[...document.querySelectorAll('.region-row input')].map(input=>input.value);
        const selected=[...document.querySelectorAll('.region-select')].map(button=>button.getAttribute('aria-pressed'));
        const image=required('.source-frame'),preview=required('.preview-shell');
        if(!regions.length||selected.length!==regions.length||selected.some(value=>!['true','false'].includes(value))||
            regions.some(value=>{const numbers=value.split(',').map(Number);return numbers.length!==4||numbers.some(number=>!Number.isFinite(number));})||
            !image.complete||image.naturalWidth!==640||image.naturalHeight!==360||!preview.dataset.displayedTimeMs)
            throw new Error('Modal witness requires loaded fixture pixels, source PTS, and nonempty region controls');
        return {route:required('h1').textContent,source:required('.source-line strong').textContent,
            regions,selected,frameSize:[image.naturalWidth,image.naturalHeight],
            timeline:required('#preview-timeline').value,displayed:preview.dataset.displayedTimeMs,
            playing:required('#preview-play').getAttribute('aria-pressed'),
            zoom:required('select[aria-label="Preview zoom"]').value,
            start:required('#range-start').value,end:required('#range-end').value,
            saveDisabled:required('#header-save').disabled,
            dialogs:[...document.querySelectorAll('dialog[open]')].map(dialog=>dialog.id)};
    "#)
}

fn verify_about(driver: &Driver, output: &Path) -> Result<()> {
    seek(driver, 1000)?;
    let original = modal_workspace(driver)?;
    if original["route"] != "Extract"
        || original["source"] != "en.mp4"
        || original["timeline"] != "1000"
        || original["displayed"] != "1000"
        || original["playing"] != "false"
    {
        return Err(format!(
            "About witness did not start from the paused source fixture: {original}"
        )
        .into());
    }
    let exclusions = driver
        .script("return document.querySelectorAll('.roi.excluded').length;")?
        .as_u64()
        .ok_or("Missing exclusion count")?;
    driver.click("#menu-trigger-edit")?;
    driver.click("#menu-item-edit-3")?;
    driver.wait(&format!("return document.querySelectorAll('.roi.excluded').length==={}&&!!document.querySelector('.roi.excluded.selected');", exclusions + 1), 5)?;
    driver.click("#menu-trigger-help")?;
    click_about(driver)?;
    driver.wait(&format!(r#"const dialog=document.querySelector('#about-modal[open][role=dialog][aria-modal=true]');return !!dialog&&dialog.contains(document.activeElement)&&[...dialog.querySelectorAll('p')].some(p=>p.textContent.trim()==='Version {}');"#, env!("CARGO_PKG_VERSION")), 5)?;
    let before = modal_workspace(driver)?;
    let mut events = Vec::new();
    for (key, control) in [
        ("Delete", false),
        ("z", true),
        ("ArrowRight", false),
        ("i", false),
        ("o", false),
        ("=", true),
        ("F1", false),
    ] {
        let event = menu_key(driver, key, control)?;
        driver.wait(
            "return document.querySelector('.preview-shell')?.getAttribute('aria-busy')==='false';",
            30,
        )?;
        let after = modal_workspace(driver)?;
        if event["prevented"] != true || after != before {
            return Err(format!(
                "About allowed a background command: event={event} before={before} after={after}"
            )
            .into());
        }
        events.push(json!({"event":event,"after":after}));
    }
    driver.click("#about-modal button")?;
    driver.wait("return !document.querySelector('#about-modal[open]')&&document.activeElement===document.querySelector('#menu-trigger-help');", 5)?;
    driver.script("document.querySelector('.preview-viewport').focus();return true;")?;
    let resumed = menu_key(driver, "Delete", false)?;
    driver.wait(
        &format!("return document.querySelectorAll('.roi.excluded').length==={exclusions};"),
        5,
    )?;
    let restored = modal_workspace(driver)?;
    if resumed["prevented"] != true || restored["regions"] != original["regions"] {
        return Err(format!(
            "Background hotkey did not resume after About closed: {resumed} {restored}"
        )
        .into());
    }
    driver.click("#menu-trigger-help")?;
    click_about(driver)?;
    driver.wait("return !!document.querySelector('#about-modal[open]')&&document.querySelector('#about-modal').contains(document.activeElement);", 5)?;
    let escape = menu_key(driver, "Escape", false)?;
    if escape["prevented"] != false {
        return Err(format!("About swallowed the native Escape cancellation: {escape}").into());
    }
    // Synthetic keys have no native default action; invoke the browser's cancel event separately.
    let cancel = driver.script("const event=new Event('cancel',{cancelable:true});document.querySelector('#about-modal').dispatchEvent(event);return {prevented:event.defaultPrevented};")?;
    driver.wait("return !document.querySelector('#about-modal[open]');", 5)?;
    let shortcuts_open = menu_key(driver, "F1", false)?;
    driver.wait("return !!document.querySelector('#shortcuts-modal[open]')&&document.querySelector('#shortcuts-modal').contains(document.activeElement);", 5)?;
    let shortcuts_escape = menu_key(driver, "Escape", false)?;
    driver.wait(
        "return !document.querySelector('#shortcuts-modal[open]');",
        5,
    )?;
    menu_key(driver, "F1", false)?;
    driver.wait("return !!document.querySelector('#shortcuts-modal[open]')&&document.querySelector('#shortcuts-modal').contains(document.activeElement);", 5)?;
    let shortcuts_close = menu_key(driver, "F1", false)?;
    driver.wait(
        "return !document.querySelector('#shortcuts-modal[open]');",
        5,
    )?;
    if cancel["prevented"] != true
        || shortcuts_open["prevented"] != true
        || shortcuts_escape["prevented"] != true
        || shortcuts_close["prevented"] != true
    {
        return Err(format!("Modal cancel or shortcuts handling failed: {cancel} {shortcuts_open} {shortcuts_escape} {shortcuts_close}").into());
    }
    seek(driver, 0)?;
    std::fs::write(
        output.join("about-shortcut-isolation.json"),
        serde_json::to_vec_pretty(
            &json!({"method":"synthetic cancelable keydown on focused controls and separate dialog cancel event; native keyboard delivery unverified","about_activation":"DOM focus before native button click method; physical pointer delivery unverified","before":before,"blocked_commands":events,"after_close":restored,"resumed_hotkey":resumed,"escape":escape,"cancel":cancel,"shortcuts_open":shortcuts_open,"shortcuts_escape":shortcuts_escape,"shortcuts_close":shortcuts_close}),
        )?,
    )?;
    Ok(())
}

fn click_about(driver: &Driver) -> Result<()> {
    // The pinned click endpoint focuses after dispatch, overriding the menu's focus restoration.
    // Match the browser's focus-before-click order while retaining the actual button handler.
    driver.script("const item=document.querySelector('#menu-item-help-1');if(!item||item.disabled||!item.getClientRects().length)throw new Error('About menu item is not available');item.focus();item.click();return true;")?;
    Ok(())
}
