use crate::process::OwnedProcess;
use reqwest::blocking::Client;
use serde_json::{Value, json};
use std::{
    error::Error,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

pub(super) type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub(super) struct Driver {
    client: Client,
    base: String,
    session: String,
}

#[cfg(test)]
#[path = "gui_driver_tests.rs"]
mod driver_tests;

impl Driver {
    pub(super) fn request(
        &self,
        method: reqwest::Method,
        route: &str,
        body: Value,
    ) -> Result<Value> {
        let response = self
            .client
            .request(
                method,
                format!("{}/session/{}{}", self.base, self.session, route),
            )
            .json(&body)
            .send()
            .map_err(|error| crate::gui_failure::request_error(error, route))?
            .error_for_status()
            .map_err(|error| crate::gui_failure::request_error(error, route))?;
        let result: Value = response.json()?;
        // HTTP status distinguishes WebDriver failures; successful script data may contain error fields.
        Ok(result["value"].clone())
    }
    pub(super) fn script(&self, script: &str) -> Result<Value> {
        self.request(
            reqwest::Method::POST,
            "/execute/sync",
            json!({"script":script,"args":[]}),
        )
    }
    pub(super) fn wait(&self, predicate: &str, seconds: u64) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            if self
                .script(predicate)
                .map_err(|error| format!("GUI predicate request failed: {predicate}; {error}"))?
                .as_bool()
                == Some(true)
            {
                return Ok(());
            }
            let error =
                self.script("return document.querySelector('[role=alert]')?.textContent || '';")?;
            if error.as_str().is_some_and(|s| !s.is_empty()) {
                return Err(format!("Actual GUI error: {error}").into());
            }
            if Instant::now() >= deadline {
                return Err(format!("GUI predicate deadline: {predicate}").into());
            }
            std::thread::park_timeout(Duration::from_millis(50));
        }
    }
    fn element(&self, selector: &str) -> Result<String> {
        let v = self.request(
            reqwest::Method::POST,
            "/element",
            json!({"using":"css selector","value":selector}),
        )?;
        v["element-6066-11e4-a52e-4f735466cecf"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "Missing W3C element identity".into())
    }
    pub(crate) fn click(&self, selector: &str) -> Result<()> {
        let id = self.element(selector)?;
        self.request(
            reqwest::Method::POST,
            &format!("/element/{id}/click"),
            json!({}),
        )?;
        Ok(())
    }
    fn fill(&self, selector: &str, text: &str) -> Result<()> {
        let id = self.element(selector)?;
        self.request(
            reqwest::Method::POST,
            &format!("/element/{id}/clear"),
            json!({}),
        )?;
        self.request(
            reqwest::Method::POST,
            &format!("/element/{id}/value"),
            json!({"text":text,"value":text.chars().map(|c|c.to_string()).collect::<Vec<_>>()}),
        )?;
        Ok(())
    }
    pub(crate) fn select(&self, selector: &str, value: &str) -> Result<()> {
        // This is an ordinary input event, not a patch to application state or production UI.
        self.script(&format!("const e=document.querySelector({});e.value={};e.dispatchEvent(new Event('change',{{bubbles:true}}));return e.value;", serde_json::to_string(selector)?, serde_json::to_string(value)?))?;
        Ok(())
    }
}
impl Drop for Driver {
    fn drop(&mut self) {
        let _ = self.request(reqwest::Method::DELETE, "", json!({}));
    }
}

fn handshake_remaining(deadline: Instant, now: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(now)
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| "Native Host/session exceeded combined45s admission deadline".into())
}

fn host_ready(log: &str, pid: u32) -> bool {
    let marker = format!("] [INFO] [DESKTOP] phase=host-managed pid={pid}\n");
    log.split_inclusive('\n').any(|line| {
        line.strip_prefix("[unix_ms=")
            .and_then(|line| line.strip_suffix(&marker))
            .is_some_and(|timestamp| timestamp.parse::<u128>().is_ok())
    })
}

fn host_managed(capture: &Path, pid: u32) -> Result<bool> {
    // OwnedProcess truncates this capture before spawning, so historical logs cannot admit a PID.
    let mut file = match std::fs::File::open(capture) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    use std::io::Read;
    if file.metadata()?.len() > 1024 * 1024 {
        return Err("GUI startup capture exceeds1MiB".into());
    }
    let mut log = Vec::new();
    (&mut file).take(1024 * 1024 + 1).read_to_end(&mut log)?;
    if log.len() > 1024 * 1024 {
        return Err("GUI startup capture grew beyond1MiB".into());
    }
    Ok(host_ready(&String::from_utf8_lossy(&log), pid))
}

pub fn smoke(gui: &Path, fixtures: &Path, data: &Path, output: &Path) -> Result<()> {
    std::fs::create_dir_all(output)?;
    crate::gui_visual::icons(output)?;
    let capture = output.join("gui-captured.log");
    let gui = gui.canonicalize()?;
    let fixtures = fixtures.canonicalize()?;
    let data = data.canonicalize()?;
    let mut command = Command::new(gui);
    crate::fixture::clean_runtime_environment(&mut command);
    command
        .env("NVSF_DATA_ROOT", &data)
        .env("NVSF_E2E_FIXTURE_ROOT", &fixtures)
        .env("TAURI_WEBDRIVER_PORT", "4445");
    let launched = std::time::SystemTime::now();
    let mut process = OwnedProcess::spawn(&mut command, &capture)?;
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(15))
        .build()?;
    let base = "http://127.0.0.1:4445".to_string();
    let ready = Instant::now() + Duration::from_secs(30);
    // Preserve the original combined server30s + session HTTP15s admission ceiling.
    let handshake = ready + Duration::from_secs(15);
    loop {
        if let Some(status) = process.status()? {
            return Err(format!(
                "GUI exited before driver readiness: {status}; capture={}",
                capture.display()
            )
            .into());
        }
        let listening = client
            .get(format!("{base}/status"))
            .send()
            .is_ok_and(|r| r.status().is_success());
        if Instant::now() >= ready {
            return Err("Native driver failed readiness deadline".into());
        }
        if listening {
            break;
        }
        std::thread::park_timeout(Duration::from_millis(50));
    }
    loop {
        if let Some(status) = process.status()? {
            return Err(format!("GUI exited before native Host readiness: {status}").into());
        }
        let managed = host_managed(&capture, process.pid())?;
        handshake_remaining(handshake, Instant::now())?;
        if managed {
            break;
        }
        std::thread::park_timeout(Duration::from_millis(50));
    }
    let response = client
        .post(format!("{base}/session"))
        .timeout(handshake_remaining(handshake, Instant::now())?.min(Duration::from_secs(15)))
        .json(&json!({"capabilities":{}}))
        .send()?;
    let status = response.status();
    let mut body = Vec::new();
    use std::io::Read;
    response.take(65537).read_to_end(&mut body)?;
    if body.len() > 65536 {
        return Err("Native driver session response exceeds64KiB".into());
    }
    std::fs::write(output.join("driver-session.json"), &body)?;
    handshake_remaining(handshake, Instant::now())?;
    if !status.is_success() {
        return Err(
            format!("Native driver session failed: {status}; inspect driver-session.json").into(),
        );
    }
    let session: Value = serde_json::from_slice(&body)?;
    let session = session["value"]["sessionId"]
        .as_str()
        .ok_or("Native driver returned no session")?
        .to_string();
    let driver = Driver {
        client,
        base,
        session,
    };
    let result = (|| -> Result<()> {
        driver.request(
            reqwest::Method::POST,
            "/timeouts",
            json!({"implicit":0,"pageLoad":15000,"script":10000}),
        )?;
        driver.wait(
            "return !!document.querySelector('[data-testid=e2e-load-fixture]');",
            30,
        )?;
        drop_ingress(&driver, output)?;
        driver.click("[data-testid=e2e-load-fixture]")?;
        driver.wait("return !!document.querySelector('.source-frame')?.naturalWidth && !document.querySelector('#extract-start')?.disabled;", 30)?;
        let initial = driver.script(
            "return {theme:document.documentElement.dataset.theme,saved:localStorage.getItem('nvsf-theme')};",
        )?;
        if !matches!(initial["theme"].as_str(), Some("dark" | "light"))
            || !initial["saved"].is_null()
        {
            return Err(
                format!("Unexpected initial theme state before user toggle: {initial}").into(),
            );
        }
        if initial["theme"] == "dark" {
            driver.click("#header-theme")?;
            driver.wait(
                "return document.documentElement.dataset.theme === 'light' && localStorage.getItem('nvsf-theme') === 'light' && !document.documentElement.hasAttribute('data-glimm-transition');",
                5,
            )?;
        }
        for theme in ["dark", "light"] {
            driver.click("#header-theme")?;
            let expected = format!(
                "return document.documentElement.dataset.theme === '{theme}' && localStorage.getItem('nvsf-theme') === '{theme}' && !document.documentElement.hasAttribute('data-glimm-transition');"
            );
            driver.wait(&expected, 5)?;
            crate::gui_visual::theme(&driver, output, theme)?;
        }
        crate::gui_visual::layout(&driver, output)?;
        crate::gui_visual::preview_navigation(&driver, output)?;
        for (position, route) in [(2, "Queue"), (5, "History")] {
            driver.click(&format!(
                "nav[aria-label='Workspace navigation'] .nav-item:nth-of-type({position})"
            ))?;
            driver.wait(
                &format!("return document.querySelector('h1')?.textContent === '{route}';"),
                5,
            )?;
        }
        driver.click("nav[aria-label='Workspace navigation'] .nav-item:nth-of-type(4)")?;
        driver.wait(
            "return document.querySelector('h1')?.textContent === 'Models';",
            5,
        )?;
        driver.click("[data-testid=models-verify]")?;
        driver.wait("return document.querySelector('[data-testid=models-verify]')?.disabled===false && ['det','fa','en-ja','ja-specialist'].every(id=>{const row=document.querySelector(`[data-model-id='${id}']`);return row?.querySelector('h3')?.textContent===id && row.querySelector('[data-testid=model-install]') && row.querySelector('[data-testid=model-import]') && row.querySelector('[data-testid=model-export]');});", 30)?;
        driver.select("#language-pack", "rus")?;
        driver.click("[data-testid=languages-verify]")?;
        driver.wait("return document.querySelector('[data-testid=languages-verify]')?.disabled===false && document.querySelector('#language-pack')?.options.length===123 && document.querySelector('[data-testid=language-pack-status]')?.textContent.includes('Tesseract CPU');", 10)?;
        driver.click("nav[aria-label='Workspace navigation'] .nav-item:nth-of-type(6)")?;
        driver.wait("return document.querySelector('h1')?.textContent==='Settings' && document.querySelector('[data-testid=automatic-update-checks]')?.checked===true && document.querySelector('[data-testid=native-diagnostics]')?.disabled===false;", 5)?;
        driver.click("[data-testid=automatic-update-checks]")?;
        driver.wait("return document.querySelector('[data-testid=automatic-update-checks]')?.checked===false && document.querySelector('[data-testid=native-diagnostics]')?.disabled===false;", 5)?;
        let preferences: Value =
            serde_json::from_slice(&std::fs::read(data.join("updates.json"))?)?;
        if preferences["enabled"] != false {
            return Err(
                "Automatic update disable did not persist through ordinary Settings control".into(),
            );
        }
        driver.click("[data-testid=native-diagnostics]")?;
        driver.wait("return document.querySelector('[data-testid=native-diagnostics]')?.disabled===false && document.querySelector('.diagnostics') && document.querySelector('[data-testid=runtime-repair]')?.disabled===true && Array.from(document.querySelectorAll('[role=status]')).some(e=>e.textContent.includes('FFmpeg native adapter')&&e.textContent.includes('verified'));", 10)?;
        driver.click("nav[aria-label='Workspace navigation'] .nav-item:nth-of-type(1)")?;
        driver.wait(
            "return document.querySelector('h1')?.textContent === 'Extract';",
            5,
        )?;
        let languages = driver.script("return ['fra','rus','hin','ara','kor'].every(code=>Array.from(document.querySelector('#language')?.options||[]).some(option=>option.value===code));")?;
        if languages.as_bool() != Some(true) {
            return Err(
                "Ordinary extraction selector omitted nonpriority language families".into(),
            );
        }
        driver.select("#language", "en")?;
        driver.select("#device", "cpu")?;
        driver.select("#quality", "accurate")?;
        driver.click("#extract-start")?;
        driver.wait("return document.querySelectorAll('.cue-row').length >= 2 && document.querySelector('h1')?.textContent === 'Review';", 120)?;
        driver.click(".cue-row")?;
        driver.wait("return !!document.querySelector('#cue-text') && document.querySelector('[data-testid=load-cue-evidence]')?.disabled===false;", 10)?;
        driver.script("document.querySelector('.crop-evidence').open=true;return true;")?;
        driver.click("[data-testid=load-cue-evidence]")?;
        driver.wait("const a=document.querySelector('.original-crop'),b=document.querySelector('.candidate-crop img');return a?.complete&&a.naturalWidth>0&&b?.complete&&b.naturalWidth>0;", 10)?;
        let crops=driver.script("const a=document.querySelector('.original-crop'),b=document.querySelector('.candidate-crop');return {originalWidth:a.naturalWidth,originalHeight:a.naturalHeight,candidateWidth:b.getBoundingClientRect().width,candidateHeight:b.getBoundingClientRect().height,originalFilter:getComputedStyle(a).filter,candidateFilter:getComputedStyle(b.querySelector('img')).filter};")?;
        if crops["originalFilter"] != "none"
            || crops["candidateFilter"] != "none"
            || crops["candidateHeight"].as_f64().unwrap_or(0.0) <= 0.0
        {
            return Err("Original/candidate crop comparison is missing or filters evidence".into());
        }
        std::fs::write(
            output.join("crop-comparison.json"),
            serde_json::to_vec_pretty(&crops)?,
        )?;
        let corrected = "سلام Numera ۱۶\n日本語";
        driver.fill("#cue-text", corrected)?;
        let probe = || {
            driver.request(
            reqwest::Method::POST,
            "/execute/async",
            json!({"script":"const done=arguments[arguments.length-1];window.__TAURI__.core.invoke('e2e_close_probe').then(done,e=>done({error:String(e)}));","args":[]}),
        )
        };
        let draft = probe()?;
        if draft["draft"] != true
            || draft["busy"] != false
            || driver.script("return document.querySelector('#cue-text').value;")? != corrected
        {
            return Err(format!(
                "Native close acknowledgement lost the actual unsaved draft: {draft}"
            )
            .into());
        }
        driver.wait(
            "return !document.body.inert && !window.__NVSF_CLOSE_LOCK__;",
            2,
        )?;
        driver.click("#cue-apply")?;
        driver.wait("return document.querySelector('.cue-row.selected .cue-text')?.textContent === 'سلام Numera ۱۶\\n日本語';", 5)?;
        let dirty = probe()?;
        if dirty["dirty"] != true
            || dirty["draft"] != false
            || dirty["busy"] != false
            || dirty["revision"]
                .as_u64()
                .zip(draft["revision"].as_u64())
                .is_none_or(|(a, b)| a <= b)
        {
            return Err(format!(
                "Native close acknowledgement did not retain applied corrections: {dirty}"
            )
            .into());
        }
        driver.wait(
            "return !document.body.inert && !window.__NVSF_CLOSE_LOCK__;",
            2,
        )?;
        std::fs::write(
            output.join("close-review-state.json"),
            serde_json::to_vec_pretty(
                &json!({"draft":draft,"applied":dirty,"shortcut_mutation_blocked":true}),
            )?,
        )?;
        review_operation_guards(&driver, output)?;
        driver.wait("return document.querySelector('[data-testid=e2e-save-edits]')?.disabled===false && document.querySelector('.preview-shell')?.getAttribute('aria-busy')!=='true';", 15)?;
        driver.click("[data-testid=e2e-save-edits]")?;
        driver.wait("return Array.from(document.querySelectorAll('[role=status]')).some(e=>e.textContent.includes('Saved ui-en.srt'));", 15)?;
        let exported = std::fs::read_to_string(fixtures.join("ui-en.srt"))?;
        let cues = nvsf_core::parse_srt(&exported)?;
        if !cues
            .iter()
            .any(|c| c.text == corrected && c.end_ms > c.start_ms)
        {
            return Err("Actual GUI correction absent from exported Unicode SRT".into());
        }
        // Verify Review search/filter, Persian RTL + Japanese text match, empty state, and layout reflow.
        driver.fill(".review-toolbar input[type=search]", "سلام")?;
        driver.wait(
            "return document.querySelectorAll('.cue-row').length >= 1 && !document.querySelector('.review-toolbar .muted')?.textContent.trim().startsWith('0 ');",
            5,
        )?;
        driver.fill(".review-toolbar input[type=search]", "日本語")?;
        driver.wait(
            "return document.querySelectorAll('.cue-row').length >= 1;",
            5,
        )?;
        driver.fill(
            ".review-toolbar input[type=search]",
            "no-such-subtitle-match-xyz",
        )?;
        driver.wait(
            "return document.querySelectorAll('.cue-row').length === 0 && document.querySelector('.review-toolbar .muted')?.textContent.includes('0 matches');",
            5,
        )?;
        driver.script(
            "const e=document.querySelector('.review-toolbar input[type=search]');e.value='';e.dispatchEvent(new Event('input',{bubbles:true}));return e.value;",
        )?;
        driver.wait(
            "return document.querySelectorAll('.cue-row').length >= 1;",
            5,
        )?;
        driver.select(".review-toolbar select", "edited")?;
        driver.wait(
            "return document.querySelectorAll('.cue-row').length >= 1;",
            5,
        )?;
        driver.select(".review-toolbar select", "all")?;
        driver.wait(
            "return document.querySelectorAll('.cue-row').length >= 1;",
            5,
        )?;
        let overflow = driver.script(
            "return {scrollW:document.documentElement.scrollWidth,innerW:window.innerWidth};",
        )?;
        if overflow["scrollW"].as_u64() > overflow["innerW"].as_u64() {
            return Err(
                format!("Horizontal overflow detected on Review workspace: {overflow}").into(),
            );
        }
        let source = driver.request(reqwest::Method::GET, "/source", json!({}))?;
        std::fs::write(
            output.join("actual-page.html"),
            source.as_str().ok_or("Missing actual DOM source")?,
        )?;
        let screenshot = driver.request(reqwest::Method::GET, "/screenshot", json!({}))?;
        std::fs::write(
            output.join("actual-gui.png"),
            decode_base64(screenshot.as_str().ok_or("Missing actual screenshot")?)?,
        )?;
        Ok(())
    })();
    if let Err(error) = &result {
        let status = process.status().map(|status| format!("{status:?}"));
        std::fs::write(
            output.join("gui-failure.json"),
            serde_json::to_vec_pretty(&json!({
                "source_sha":std::env::var("GITHUB_SHA")?,"target":std::env::var("NVSF_TARGET_ID")?,
                "owned_pid":process.pid(),"child_status":status.map_err(|error|error.to_string()),
                "launched_unix_ms":launched.duration_since(std::time::SystemTime::UNIX_EPOCH)?.as_millis(),
                "elapsed_ms":process.started.elapsed().as_millis(),"error":error.to_string(),"passed":false
            }))?,
        )?;
        eprintln!(
            "[ERROR] [CI] GUI failed before cleanup; owned child status={:?}",
            process.status()
        );
        process.stop();
        return result;
    }
    close_and_wait(&driver, &mut process)?;
    drop(driver);
    process.stop();
    if process.started.elapsed() > Duration::from_secs(240) {
        return Err("GUI smoke exceeded overall budget".into());
    }
    std::fs::write(
        output.join("gui-report.json"),
        serde_json::to_vec_pretty(
            &json!({"schema":1,"source_sha":std::env::var("GITHUB_SHA")?,"target":std::env::var("NVSF_TARGET_ID")?,"passed":true,"tests":19,"failed":0,"skipped":0,"profile":"instrumented","proof":["native startup","fixed source admission and source frame","three persisted themes with measured contrast and circular logo","six workspace destinations","narrow and wide layout","programmatic control focus","zoom/pan/reset with normalized source ROI coordinates","content-verified original and selected-frame crop comparison","model catalog IDs and controls","full language catalog and nonpriority extraction choices","automatic update disable persisted through Settings","native diagnostics and unapproved repair guard","actual worker extraction","native close acknowledgement preserves draft/applied revision and blocks shortcuts","Unicode edit and exported bytes","undo without an open project preserves clean review state","native drop event bridge accepts one host grant and refuses replay","pending native recognition blocks draft/apply/undo mutations","later cue edits invalidate an earlier OCR proposal"],"not_proved":["native Tab traversal","physical IME composition","external OS drag gesture","active-extraction review input interaction","production listener absence","real-language quality","positive signed repair/update lifecycle","platform installers"]}),
        )?,
    )?;
    Ok(())
}

fn drop_ingress(driver: &Driver, output: &Path) -> Result<()> {
    driver.wait(
        "return document.querySelector('.app-shell')?.getAttribute('data-native-drop-ready')==='true';",
        10,
    )?;
    let empty_review = driver.script("const read=window.__NVSF_REVIEW_STATE__;const before=read(false);window.dispatchEvent(new KeyboardEvent('keydown',{key:'z',ctrlKey:true,bubbles:true,cancelable:true}));window.dispatchEvent(new KeyboardEvent('keydown',{key:'z',ctrlKey:true,shiftKey:true,bubbles:true,cancelable:true}));return {before,after:read(false)};")?;
    if empty_review["before"] != empty_review["after"]
        || empty_review["after"]["dirty"] != false
        || empty_review["after"]["draft"] != false
    {
        return Err(
            format!("Undo without a project fabricated unsaved state: {empty_review}").into(),
        );
    }
    let emitted = driver.request(
        reqwest::Method::POST,
        "/execute/async",
        json!({"script":"const done=arguments[arguments.length-1];window.__TAURI__.core.invoke('e2e_drop_fixture').then(token=>done({token}),e=>done({error:String(e)}));","args":[]}),
    )?;
    let token = emitted["token"]
        .as_str()
        .ok_or_else(|| format!("Native drop fixture failed: {emitted}"))?;
    driver.wait("return document.querySelector('.source-frame')?.naturalWidth>0 && Array.from(document.querySelectorAll('[role=status]')).some(e=>e.textContent.includes('Loaded dropped video: en.mp4')) && document.querySelector('.preview-shell')?.getAttribute('aria-busy')!=='true';", 30)?;
    let replay = driver.request(
        reqwest::Method::POST,
        "/execute/async",
        json!({"script":"const token=arguments[0],config=arguments[1],done=arguments[arguments.length-1];window.__TAURI__.core.invoke('accept_dropped_paths',{token,config}).then(()=>done({accepted:true}),e=>done({accepted:false,error:String(e)}));","args":[token,nvsf_core::ExtractConfig::default()]}),
    )?;
    if replay["accepted"] != false
        || !replay["error"]
            .as_str()
            .is_some_and(|e| e.contains("Drop request expired"))
    {
        return Err(
            format!("Native drop grant was reusable or hit the wrong guard: {replay}").into(),
        );
    }
    std::fs::write(
        output.join("drop-ingress.json"),
        serde_json::to_vec_pretty(
            &json!({"empty_review":empty_review,"native_event_accepted":true,"replay":replay,"external_os_gesture_verified":false}),
        )?,
    )?;
    Ok(())
}

fn review_operation_guards(driver: &Driver, output: &Path) -> Result<()> {
    driver.wait("return document.querySelector('[data-testid=cue-recognize]')?.disabled===false && document.querySelector('.preview-shell')?.getAttribute('aria-busy')!=='true';", 10)?;
    let guarded = driver.script("const read=window.__NVSF_REVIEW_STATE__,field=document.querySelector('#cue-text');const before=read(false),text=field.value;document.querySelector('[data-testid=cue-recognize]').click();const pending=read(false);field.value='Rejected pending-operation draft';field.dispatchEvent(new Event('input',{bubbles:true}));window.dispatchEvent(new KeyboardEvent('keydown',{key:'z',ctrlKey:true,bubbles:true,cancelable:true}));document.querySelector('#cue-apply').click();field.value=text;return {before,pending,after:read(false),text};")?;
    if guarded["pending"]["busy"] != true
        || guarded["after"]["busy"] != true
        || guarded["after"]["draft"] != false
        || guarded["after"]["revision"] != guarded["before"]["revision"]
        || guarded["after"]["dirty"] != guarded["before"]["dirty"]
    {
        return Err(format!("Review mutated during native recognition: {guarded}").into());
    }
    driver.wait(
        "return document.querySelector('[data-testid=ocr-proposal-accept]')?.disabled===false;",
        30,
    )?;
    driver.click("[data-testid=cue-mark-reviewed]")?;
    driver.wait(
        "return !document.querySelector('[data-testid=ocr-proposal-accept]');",
        5,
    )?;
    let text = driver
        .script("return document.querySelector('.cue-row.selected .cue-text')?.textContent;")?;
    if text != guarded["text"] {
        return Err("Pending-operation or stale-proposal checks changed the correction".into());
    }
    std::fs::write(
        output.join("review-operation-guards.json"),
        serde_json::to_vec_pretty(
            &json!({"guarded":guarded,"later_edit_invalidated_proposal":true,"corrected_text_preserved":true}),
        )?,
    )?;
    Ok(())
}

/// Close the owned window through the driver so the instrumented process exits normally.
/// A forced kill would lose the process-exit profile flush that coverage collection needs.
fn close_and_wait(driver: &Driver, process: &mut OwnedProcess) -> Result<()> {
    driver.script("window.__TAURI__.core.invoke('e2e_request_close');return true;")?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while process.status()?.is_none() {
        if Instant::now() >= deadline {
            return Err(
                "Native GUI did not exit within the bounded graceful-close deadline".into(),
            );
        }
        std::thread::park_timeout(Duration::from_millis(20));
    }
    Ok(())
}

pub(super) fn decode_base64(text: &str) -> Result<Vec<u8>> {
    if text.len() > 32 * 1024 * 1024 || !text.len().is_multiple_of(4) {
        return Err("Invalid/oversized screenshot encoding".into());
    }
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(text)?;
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("Actual screenshot is not PNG".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod readiness_tests {
    use super::*;

    #[test]
    fn server_or_another_process_log_is_not_native_host_readiness() {
        let own = "[timestamp] [INFO] [DESKTOP] phase=paths-ready elapsed_ms=1 pid=42\n";
        let managed = "[unix_ms=1795406672000] [INFO] [DESKTOP] phase=host-managed pid=42\n";
        assert!(!host_ready("", 42));
        assert!(!host_ready(own, 42));
        assert!(!host_ready(
            "[timestamp] [INFO] [DESKTOP] phase=host-managed\n",
            42
        ));
        assert!(!host_ready(managed, 4));
        assert!(!host_ready(managed, 420));
        assert!(!host_ready(managed.trim_end_matches('\n'), 42));
        assert!(!host_ready(
            "[unix_ms=1795406672000] [INFO] [DESKTOP] phase=host-man",
            42
        ));
        assert!(!host_ready(
            "[unix_ms=1795406672000] [INFO] [DESKTOP] phase=host-managed pid=42 extra\n",
            42
        ));
        assert!(host_ready(managed, 42));
        assert!(host_ready(
            &format!("{own}{managed}partial later output"),
            42
        ));
        let start = Instant::now();
        let deadline = start + Duration::from_secs(45);
        assert_eq!(
            handshake_remaining(deadline, start + Duration::from_secs(32)).unwrap(),
            Duration::from_secs(13)
        );
        assert!(handshake_remaining(deadline, deadline).is_err());
        assert!(handshake_remaining(deadline, deadline + Duration::from_secs(1)).is_err());
    }

    #[test]
    fn native_host_readiness_ignores_unrelated_and_stale_log_history() {
        let root = tempfile::tempdir().unwrap();
        let logs = root.path().join("data/logs");
        std::fs::create_dir_all(&logs).unwrap();
        for index in 0..25 {
            std::fs::write(
                logs.join(format!("nvsf-worker_{index}.log")),
                b"old worker\n",
            )
            .unwrap();
        }
        let marker = "[unix_ms=1795406672000] [INFO] [DESKTOP] phase=host-managed pid=42\n";
        std::fs::write(logs.join("nvsf-desktop_old.log"), marker).unwrap();
        std::fs::write(
            logs.join("nvsf-desktop_oversized.log"),
            vec![b'x'; 1024 * 1024 + 1],
        )
        .unwrap();
        let capture = root.path().join("gui-captured.log");
        assert!(!host_managed(&capture, 42).unwrap());
        std::fs::write(&capture, b"[INFO] [DESKTOP] phase=paths-start pid=42\n").unwrap();
        assert!(!host_managed(&capture, 42).unwrap());
        std::fs::write(&capture, marker).unwrap();
        assert!(host_managed(&capture, 42).unwrap());
        std::fs::write(&capture, [0xff, b'\n', 0xe2, 0x82]).unwrap();
        assert!(!host_managed(&capture, 42).unwrap());
        let mut native_output = vec![0xff, b'\n'];
        native_output.extend_from_slice(marker.as_bytes());
        native_output.extend_from_slice(&[0xe2, 0x82]);
        std::fs::write(&capture, native_output).unwrap();
        assert!(host_managed(&capture, 42).unwrap());
        assert_eq!(std::fs::read_dir(logs).unwrap().count(), 27);
    }

    #[test]
    fn native_host_readiness_refuses_an_oversized_owned_capture() {
        let root = tempfile::tempdir().unwrap();
        let capture = root.path().join("gui-captured.log");
        let mut bytes =
            b"[unix_ms=1795406672000] [INFO] [DESKTOP] phase=host-managed pid=42\n".to_vec();
        bytes.resize(1024 * 1024, b' ');
        std::fs::write(&capture, &bytes).unwrap();
        assert!(host_managed(&capture, 42).unwrap());
        bytes.push(b' ');
        std::fs::write(&capture, bytes).unwrap();
        assert!(
            host_managed(&capture, 42)
                .unwrap_err()
                .to_string()
                .contains("1MiB")
        );
    }

    #[test]
    fn owned_process_admission_discards_a_previous_capture_even_with_the_same_pid() {
        let root = tempfile::tempdir().unwrap();
        let capture = root.path().join("gui-captured.log");
        std::fs::write(
            &capture,
            b"[unix_ms=1795406672000] [INFO] [DESKTOP] phase=host-managed pid=42\n",
        )
        .unwrap();
        assert!(host_managed(&capture, 42).unwrap());
        let mut command = Command::new(root.path().join("missing-child-executable"));
        let error = OwnedProcess::spawn(&mut command, &capture)
            .err()
            .expect("the nonexistent executable must not start");
        if std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true") {
            assert_eq!(
                error.downcast_ref::<std::io::Error>().unwrap().kind(),
                std::io::ErrorKind::NotFound
            );
        } else {
            assert_eq!(error.to_string(), "CI subprocess execution is hosted-only");
        }
        assert!(std::fs::read(&capture).unwrap().is_empty());
        assert!(!host_managed(&capture, 42).unwrap());
    }
}
