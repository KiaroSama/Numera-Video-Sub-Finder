use super::gui::{Driver, Result, decode_base64};
use crate::ci_icons;
use serde_json::{Value, json};
use std::path::Path;

pub fn icons(output: &Path) -> Result<()> {
    let root = Path::new("crates/desktop/icons");
    let mut records = Vec::new();
    for (file, size) in [
        ("32x32.png", 32),
        ("128x128.png", 128),
        ("128x128@2x.png", 256),
        ("icon.png", 512),
    ] {
        let pixels = image::open(root.join(file))?;
        if pixels.color() != image::ColorType::Rgba8 {
            return Err(format!("Native icon is not a 32-bit RGBA PNG: {file}").into());
        }
        ci_icons::check_alpha(&pixels.into_rgba8(), size)?;
        records.push(json!({"file":file,"width":size,"height":size,"transparent_corners":true,"opaque_center":true}));
    }
    let sizes = ci_icons::validate_ico(&std::fs::read(root.join("icon.ico"))?)?;
    records.push(json!({"file":"icon.ico","sizes":sizes,"encoding":"PNG RGBA8","transparent_corners":true,"opaque_center":true}));
    std::fs::write(
        output.join("native-logo.json"),
        serde_json::to_vec_pretty(&records)?,
    )?;
    Ok(())
}

pub fn theme(driver: &Driver, output: &Path, name: &str) -> Result<()> {
    let observation = driver.script(
        "const root=getComputedStyle(document.documentElement);const panel=getComputedStyle(document.querySelector('.panel'));const frame=getComputedStyle(document.querySelector('.source-frame'));const logo=document.querySelector('.brand img');return {theme:document.documentElement.dataset.theme,background:panel.backgroundColor,text:panel.color,muted:root.getPropertyValue('--muted').trim(),control:root.getPropertyValue('--strong-border').trim(),input:root.getPropertyValue('--input').trim(),accent:root.getPropertyValue('--accent').trim(),accentHover:root.getPropertyValue('--accent-hover').trim(),accentPressed:root.getPropertyValue('--accent-pressed').trim(),onAccent:root.getPropertyValue('--on-accent').trim(),frameFilter:frame.filter,frameOpacity:frame.opacity,sourcePixels:(()=>{const im=document.querySelector('.source-frame');const c=document.createElement('canvas');c.width=im.naturalWidth;c.height=im.naturalHeight;const x=c.getContext('2d');x.drawImage(im,0,0);let h=2166136261;for(const v of x.getImageData(0,0,c.width,c.height).data)h=Math.imul(h^v,16777619);return [c.width,c.height,h>>>0];})(),logoLoaded:logo.complete&&logo.naturalWidth>0,logoSource:logo.getAttribute('src'),logoAlpha:(()=>{const c=document.createElement('canvas');c.width=c.height=32;const x=c.getContext('2d');x.drawImage(logo,0,0,32,32);return [x.getImageData(0,0,1,1).data[3],x.getImageData(31,31,1,1).data[3],x.getImageData(16,16,1,1).data[3]];})()};",
    )?;
    let baseline_path = output.join("source-pixels.json");
    if name == "dark" {
        std::fs::write(
            &baseline_path,
            serde_json::to_vec(&observation["sourcePixels"])?,
        )?;
    } else {
        let original: Value = serde_json::from_slice(&std::fs::read(&baseline_path)?)?;
        if original != observation["sourcePixels"] {
            return Err("Source image pixels changed across workspace themes".into());
        }
    }
    let text = contrast(&observation["text"], &observation["background"])?;
    let muted = contrast(&observation["muted"], &observation["background"])?;
    let control = contrast(&observation["control"], &observation["input"])?;
    let accent = contrast(&observation["onAccent"], &observation["accent"])?;
    let hover = contrast(&observation["onAccent"], &observation["accentHover"])?;
    let pressed = contrast(&observation["onAccent"], &observation["accentPressed"])?;
    if text < 4.5
        || muted < 4.5
        || control < 3.0
        || [accent, hover, pressed].iter().any(|c| *c < 4.5)
        || !observation["accentHover"]
            .as_str()
            .is_some_and(|color| color.eq_ignore_ascii_case("#CB4242"))
        || !observation["accentPressed"]
            .as_str()
            .is_some_and(|color| color.eq_ignore_ascii_case("#AF3333"))
    {
        return Err(format!("Rendered {name} contrast or accent token failed: text={text:.2} muted={muted:.2} control={control:.2} accent={accent:.2} hover={hover:.2} ({}) pressed={pressed:.2} ({})", observation["accentHover"], observation["accentPressed"]).into());
    }
    if observation["frameFilter"] != "none"
        || observation["frameOpacity"] != "1"
        || observation["logoLoaded"] != true
        || observation["logoSource"] != "logo.svg"
        || observation["logoAlpha"] != json!([0, 0, 255])
    {
        return Err("Rendered source pixels or official circular logo contract failed".into());
    }
    std::fs::write(
        output.join(format!("visual-{name}.json")),
        serde_json::to_vec_pretty(
            &json!({"observation":observation,"text_contrast":text,"muted_contrast":muted,"control_contrast":control,"accent_contrast":accent,"accent_hover_contrast":hover,"accent_pressed_contrast":pressed,"accent_state_method":"computed theme tokens used by normal/hover/pressed CSS; physical pointer states unverified"}),
        )?,
    )?;
    let screenshot = driver.request(reqwest::Method::GET, "/screenshot", json!({}))?;
    std::fs::write(
        output.join(format!("workspace-{name}.png")),
        decode_base64(screenshot.as_str().ok_or("Missing theme screenshot")?)?,
    )?;
    Ok(())
}

pub fn cue_selection(driver: &Driver, output: &Path) -> Result<()> {
    let mut states = Vec::new();
    for selected in [0, 1, 0] {
        driver.click(&format!(".cue-row:nth-of-type({})", selected + 1))?;
        driver.wait(
            &format!(
                "const rows=Array.from(document.querySelectorAll('.cue-row'));return rows.length>=2&&rows.every((row,index)=>row.getAttribute('aria-pressed')===(index==={selected}?'true':'false')&&row.classList.contains('selected')===(index==={selected}));"
            ),
            5,
        )?;
        states.push(driver.script("return Array.from(document.querySelectorAll('.cue-row'),row=>({cue:row.querySelector('.cue-number')?.textContent,pressed:row.getAttribute('aria-pressed')}));")?);
    }
    std::fs::write(
        output.join("cue-aria-state.json"),
        serde_json::to_vec_pretty(&states)?,
    )?;
    Ok(())
}

pub fn preview_navigation(driver: &Driver, output: &Path) -> Result<()> {
    let fitted_width = driver
        .script("return document.querySelector('.frame')?.getBoundingClientRect().width;")?
        .as_f64()
        .filter(|width| *width > 0.0)
        .ok_or("Missing fitted frame width")?;
    driver.select("select[aria-label='Preview zoom']", "400")?;
    driver.wait(&format!("const f=document.querySelector('.frame');return Math.abs(f?.getBoundingClientRect().width/{fitted_width}-4)<0.05;"), 5)?;
    driver.click("button[aria-label='Pan preview right']")?;
    driver.click("button[aria-label='Pan preview down']")?;
    let observation = driver.script("const v=document.querySelector('.preview-viewport'),f=document.querySelector('.frame'),r=f.getBoundingClientRect();const before=document.querySelectorAll('.roi.excluded').length;const toggle=document.querySelector('.draw-mode input');toggle.click();const x=Math.round(r.left+r.width*.25),y=Math.round(r.top+r.height*.25),x2=Math.round(r.left+r.width*.5),y2=Math.round(r.top+r.height*.5);f.dispatchEvent(new MouseEvent('mousedown',{bubbles:true,button:0,clientX:x,clientY:y}));f.dispatchEvent(new MouseEvent('mouseup',{bubbles:true,button:0,clientX:x2,clientY:y2}));toggle.click();return {left:v.scrollLeft,top:v.scrollTop,before,width:r.width,height:r.height};")?;
    driver.wait(
        "return document.querySelectorAll('.roi.excluded').length===1;",
        5,
    )?;
    let region = driver.script("const s=document.querySelector('.roi.excluded').style;return [parseFloat(s.left),parseFloat(s.top),parseFloat(s.width),parseFloat(s.height)];")?;
    if observation["before"] != 0
        || observation["left"].as_f64().unwrap_or(0.0) <= 0.0
        || observation["top"].as_f64().unwrap_or(0.0) <= 0.0
        || region.as_array().is_none_or(|values| {
            values.len() != 4
                || values
                    .iter()
                    .any(|v| v.as_f64().is_none_or(|n| (n - 25.0).abs() > 1.0))
        })
    {
        return Err(format!(
            "Zoom/pan changed normalized source coordinates: {observation} {region}"
        )
        .into());
    }
    driver.click("button[aria-label='Remove exclusion 1']")?;
    driver.click("button[title='Reset zoom (Ctrl+0)']")?;
    driver.wait("const v=document.querySelector('.preview-viewport'),f=document.querySelector('.frame');if(!v||!f)return false;const r=f.getBoundingClientRect(),fit=Math.min(v.clientWidth,v.clientHeight*r.width/r.height);return Math.abs(r.width-fit)<2&&v.scrollLeft===0&&v.scrollTop===0&&document.querySelectorAll('.roi.excluded').length===0;", 5)?;
    std::fs::write(
        output.join("preview-navigation.json"),
        serde_json::to_vec_pretty(
            &json!({"fit_width":fitted_width,"observation":observation,"region_percent":region,"reset":true}),
        )?,
    )?;
    Ok(())
}

pub fn layout(driver: &Driver, output: &Path) -> Result<()> {
    let original = driver.request(reqwest::Method::GET, "/window/rect", json!({}))?;
    let result = (|| -> Result<()> {
        let mut prior_width = driver
            .script("return innerWidth;")?
            .as_f64()
            .ok_or("Missing viewport width")?;
        for (width, height) in [(960, 640), (1280, 860)] {
            driver.request(
                reqwest::Method::POST,
                "/window/rect",
                json!({"width":width,"height":height}),
            )?;
            driver.wait(&format!("return Math.abs(innerWidth-{prior_width})>20 && !!document.querySelector('.app-shell');"), 5)?;
            let bounds = driver.script("return {width:innerWidth,scroll:document.documentElement.scrollWidth,client:document.documentElement.clientWidth,frame:document.querySelector('.source-frame').getBoundingClientRect().width};")?;
            if bounds["scroll"]
                .as_f64()
                .ok_or("Missing layout scroll width")?
                > bounds["client"]
                    .as_f64()
                    .ok_or("Missing layout client width")?
                    + 1.0
                || bounds["frame"].as_f64().unwrap_or(0.0) <= 0.0
            {
                return Err(
                    format!("Workspace overflow at requested width {width}: {bounds}").into(),
                );
            }
            prior_width = bounds["width"].as_f64().ok_or("Missing resized viewport")?;
            std::fs::write(
                output.join(format!("layout-{width}.json")),
                serde_json::to_vec_pretty(&bounds)?,
            )?;
        }
        // The pinned driver emits synthetic Tab events without native focus traversal.
        // Programmatic focus verifies the visible focus contract, not keyboard navigation.
        let observe = "const e=document.querySelector('#header-theme');e.focus();const s=getComputedStyle(e);return {documentFocused:document.hasFocus(),matchesFocus:e.matches(':focus'),active:document.activeElement===e,outline:s.outlineStyle,width:s.outlineWidth,method:'programmatic; native Tab traversal unverified'};";
        let before = driver.script(observe)?;
        std::fs::write(
            output.join("control-focus-before.json"),
            serde_json::to_vec_pretty(&before)?,
        )?;
        // Session selection in the pinned driver is logical only.
        // Focus the owned window through the native CI surface.
        let native = driver.request(
            reqwest::Method::POST,
            "/execute/async",
            json!({"script":"const done=arguments[arguments.length-1];window.__TAURI__.core.invoke('e2e_focus').then(focused=>done({focused}),()=>done({error:'Native focus command failed'}));","args":[]}),
        )?;
        let settled = driver.wait("const e=document.querySelector('#header-theme');e.focus();return document.hasFocus() && e.matches(':focus') && document.activeElement===e;", 5);
        let focused = driver.script(observe)?;
        std::fs::write(
            output.join("control-focus.json"),
            serde_json::to_vec_pretty(&json!({"before":before,"native":native,"after":focused}))?,
        )?;
        settled?;
        // Native parent focus is diagnostic: an embedded WebView can hold input focus
        // while its parent reports false.
        if !native["focused"].is_boolean()
            || native["error"].is_string()
            || focused["documentFocused"] != true
            || focused["matchesFocus"] != true
            || focused["active"] != true
            || focused["outline"] == "none"
            || focused["width"] == "0px"
        {
            return Err(
                format!("Control focus is not visible: native={native} webview={focused}").into(),
            );
        }
        skip_link(driver, output)
    })();
    let restored = driver.request(reqwest::Method::POST, "/window/rect", original);
    result?;
    restored?;
    Ok(())
}

fn skip_link(driver: &Driver, output: &Path) -> Result<()> {
    driver.wait("const link=document.querySelector('.skip-link');link.focus();return document.activeElement===link&&link.matches(':focus');", 5)?;
    let receipt = driver.script(r#"
        const link=document.querySelector('.skip-link'),r=link.getBoundingClientRect();
        const points=[[r.left+r.width/2,r.top+r.height/2],[r.left+12,r.top+12],[r.right-12,r.top+12]];
        return {method:'programmatic focus, rendered hit testing, and DOM link activation; native Tab traversal unverified',
            previousHash:location.hash,
            bounds:[r.x,r.y,r.width,r.height],viewport:[innerWidth,innerHeight],
            visible:r.left>=0&&r.top>=0&&r.right<=innerWidth&&r.bottom<=innerHeight,
            points:points.map(([x,y])=>{const hit=document.elementFromPoint(x,y);return {x,y,link:link.contains(hit),hit:hit?.id||hit?.tagName};})};
    "#)?;
    let screenshot = driver.request(reqwest::Method::GET, "/screenshot", json!({}))?;
    std::fs::write(
        output.join("skip-link-focus.png"),
        decode_base64(screenshot.as_str().ok_or("Missing skip-link screenshot")?)?,
    )?;
    driver.script("document.querySelector('.skip-link').click();return true;")?;
    driver.wait(
        "return document.activeElement===document.querySelector('#main-content');",
        5,
    )?;
    let activation = driver.script("return {activeId:document.activeElement?.id,hash:location.hash,route:document.querySelector('h1')?.textContent};")?;
    std::fs::write(
        output.join("skip-link-focus.json"),
        serde_json::to_vec_pretty(&json!({"focus":receipt,"activation":activation}))?,
    )?;
    driver.script(&format!(
        "location.hash={};document.querySelector('#header-theme').focus();return true;",
        serde_json::to_string(&receipt["previousHash"])?
    ))?;
    if receipt["visible"] != true
        || receipt["points"].as_array().is_none_or(|points| {
            points.len() != 3 || points.iter().any(|point| point["link"] != true)
        })
    {
        return Err(format!("Focused skip link is obscured by workspace chrome: {receipt}").into());
    }
    Ok(())
}

fn contrast(a: &Value, b: &Value) -> Result<f64> {
    let a = luminance(a.as_str().ok_or("Missing rendered foreground")?)?;
    let b = luminance(b.as_str().ok_or("Missing rendered background")?)?;
    Ok((a.max(b) + 0.05) / (a.min(b) + 0.05))
}

fn luminance(color: &str) -> Result<f64> {
    let color = color.trim();
    let channels = if let Some(hex) = color
        .strip_prefix('#')
        .filter(|s| s.len() == 6 && s.is_ascii())
    {
        [0, 2, 4]
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map(f64::from))
            .into_iter()
            .collect::<std::result::Result<Vec<_>, _>>()?
    } else if let Some(rgb) = color.strip_prefix("rgb(").and_then(|s| s.strip_suffix(')')) {
        rgb.split(',')
            .map(|s| s.trim().parse::<f64>())
            .collect::<std::result::Result<Vec<_>, _>>()?
    } else {
        return Err(format!("Unsupported rendered color: {color}").into());
    };
    if channels.len() != 3
        || channels
            .iter()
            .any(|c| !c.is_finite() || !(0.0..=255.0).contains(c))
    {
        return Err("Invalid rendered color channels".into());
    }
    let linear = |v: f64| {
        let v = v / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    Ok(linear(channels[0]) * 0.2126 + linear(channels[1]) * 0.7152 + linear(channels[2]) * 0.0722)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn contrast_uses_independent_wcag_black_white_reference() {
        assert!((contrast(&json!("#ffffff"), &json!("rgb(0, 0, 0)")).unwrap() - 21.0).abs() < 1e-9);
        assert!(contrast(&json!("transparent"), &json!("#ffffff")).is_err());
        assert!(contrast(&json!("rgb(256, 0, 0)"), &json!("#ffffff")).is_err());
    }
}
