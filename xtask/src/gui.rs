use crate::process::OwnedProcess;
use reqwest::blocking::Client;
use serde_json::{Value, json};
use std::{
    error::Error,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct Driver {
    client: Client,
    base: String,
    session: String,
}

impl Driver {
    fn request(&self, method: reqwest::Method, route: &str, body: Value) -> Result<Value> {
        let response = self
            .client
            .request(
                method,
                format!("{}/session/{}{}", self.base, self.session, route),
            )
            .json(&body)
            .send()?
            .error_for_status()?;
        let result: Value = response.json()?;
        if result["value"]["error"].is_string() {
            return Err(format!("WebDriver error: {}", result["value"]).into());
        }
        Ok(result["value"].clone())
    }
    fn script(&self, script: &str) -> Result<Value> {
        self.request(
            reqwest::Method::POST,
            "/execute/sync",
            json!({"script":script,"args":[]}),
        )
    }
    fn wait(&self, predicate: &str, seconds: u64) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            if self.script(predicate)?.as_bool() == Some(true) {
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
    fn click(&self, selector: &str) -> Result<()> {
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
    fn select(&self, selector: &str, value: &str) -> Result<()> {
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

pub fn smoke(gui: &Path, fixtures: &Path, data: &Path, output: &Path) -> Result<()> {
    std::fs::create_dir_all(output)?;
    let capture = output.join("gui-captured.log");
    let gui = gui.canonicalize()?;
    let fixtures = fixtures.canonicalize()?;
    let data = data.canonicalize()?;
    let mut command = Command::new(gui);
    command
        .env("NVSF_DATA_ROOT", &data)
        .env("NVSF_E2E_FIXTURE_ROOT", &fixtures)
        .env("TAURI_WEBDRIVER_PORT", "4445");
    let mut process = OwnedProcess::spawn(&mut command, &capture)?;
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(15))
        .build()?;
    let base = "http://127.0.0.1:4445".to_string();
    let ready = Instant::now() + Duration::from_secs(30);
    loop {
        if process.status()?.is_some() {
            return Err("GUI exited before driver readiness; inspect capture".into());
        }
        if client
            .get(format!("{base}/status"))
            .send()
            .is_ok_and(|r| r.status().is_success())
        {
            break;
        }
        if Instant::now() >= ready {
            return Err("Native driver failed readiness deadline".into());
        }
        std::thread::park_timeout(Duration::from_millis(50));
    }
    let session: Value = client
        .post(format!("{base}/session"))
        .json(&json!({"capabilities":{}}))
        .send()?
        .error_for_status()?
        .json()?;
    let session = session["value"]["sessionId"]
        .as_str()
        .ok_or("Native driver returned no session")?
        .to_string();
    let driver = Driver {
        client,
        base,
        session,
    };
    driver.request(
        reqwest::Method::POST,
        "/timeouts",
        json!({"implicit":0,"pageLoad":15000,"script":10000}),
    )?;
    driver.wait(
        "return !!document.querySelector('[data-testid=e2e-load-fixture]');",
        30,
    )?;
    driver.click("[data-testid=e2e-load-fixture]")?;
    driver.wait("return !!document.querySelector('.source-frame')?.naturalWidth && !document.querySelector('#extract-start')?.disabled;", 30)?;
    for theme in ["dark", "light", "system"] {
        driver.select("#header-theme", theme)?;
        let expected = format!(
            "return document.documentElement.dataset.theme === '{theme}' && localStorage.getItem('nvsf-theme') === '{theme}';"
        );
        driver.wait(&expected, 5)?;
        let styles = driver.script("const e=document.querySelector('.panel');const s=getComputedStyle(e);return {background:s.backgroundColor,color:s.color,font:s.fontSize,theme:document.documentElement.dataset.theme};")?;
        std::fs::write(
            output.join(format!("theme-{theme}.json")),
            serde_json::to_vec_pretty(&styles)?,
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
    let preferences: Value = serde_json::from_slice(&std::fs::read(data.join("updates.json"))?)?;
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
        return Err("Ordinary extraction selector omitted nonpriority language families".into());
    }
    driver.select("#language", "en")?;
    driver.select("#device", "cpu")?;
    driver.select("#quality", "accurate")?;
    driver.click("#extract-start")?;
    driver.wait("return document.querySelectorAll('.cue-row').length >= 2 && document.querySelector('h1')?.textContent === 'Review';", 120)?;
    driver.click(".cue-row")?;
    driver.wait("return !!document.querySelector('#cue-text');", 5)?;
    let corrected = "سلام Numera ۱۶\n日本語";
    driver.fill("#cue-text", corrected)?;
    driver.click("#cue-apply")?;
    driver.wait("return document.querySelector('.cue-row.selected .cue-text')?.textContent === 'سلام Numera ۱۶\\n日本語';", 5)?;
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
    drop(driver);
    process.stop();
    if process.started.elapsed() > Duration::from_secs(240) {
        return Err("GUI smoke exceeded overall budget".into());
    }
    std::fs::write(
        output.join("gui-report.json"),
        serde_json::to_vec_pretty(
            &json!({"schema":1,"source_sha":std::env::var("GITHUB_SHA")?,"target":std::env::var("NVSF_TARGET_ID")?,"passed":true,"tests":9,"failed":0,"skipped":0,"profile":"instrumented","proof":["native startup","fixed source admission and source frame","three persisted themes","model catalog IDs and controls","full language catalog and nonpriority extraction choices","automatic update disable persisted through Settings","native diagnostics and unapproved repair guard","actual worker extraction","Unicode edit and exported bytes"],"not_proved":["physical IME composition","production listener absence","real-language quality","positive signed repair/update lifecycle","platform installers"]}),
        )?,
    )?;
    Ok(())
}

fn decode_base64(text: &str) -> Result<Vec<u8>> {
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
