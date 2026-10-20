use super::gui::{Driver, Result, decode_base64};
use serde_json::{Value, json};
use std::path::Path;

pub fn icons(output: &Path) -> Result<()> {
    let root = Path::new("crates/desktop/icons");
    let mut records = Vec::new();
    for (file, size) in [
        ("32x32.png", 32),
        ("128x128.png", 128),
        ("128x128@2x.png", 256),
    ] {
        let path = root.join(file);
        let pixels = image::open(&path)?.to_rgba8();
        if pixels.dimensions() != (size, size)
            || pixels.get_pixel(0, 0)[3] != 0
            || pixels.get_pixel(size - 1, size - 1)[3] != 0
            || pixels.get_pixel(size / 2, size / 2)[3] != 255
        {
            return Err(format!("Native circular icon geometry/alpha failed: {file}").into());
        }
        records.push(json!({"file":file,"width":size,"height":size,"transparent_corners":true,"opaque_center":true}));
    }
    for (file, magic) in [
        ("icon.ico", &b"\0\0\x01\0"[..]),
        ("icon.icns", &b"icns"[..]),
    ] {
        let bytes = std::fs::read(root.join(file))?;
        if !bytes.starts_with(magic) || bytes.len() < 32 {
            return Err(format!("Native icon container missing or malformed: {file}").into());
        }
    }
    std::fs::write(
        output.join("native-logo.json"),
        serde_json::to_vec_pretty(&records)?,
    )?;
    Ok(())
}

pub fn theme(driver: &Driver, output: &Path, name: &str) -> Result<()> {
    let observation = driver.script(
        "const root=getComputedStyle(document.documentElement);const panel=getComputedStyle(document.querySelector('.panel'));const frame=getComputedStyle(document.querySelector('.source-frame'));const logo=document.querySelector('.brand img');return {theme:document.documentElement.dataset.theme,background:panel.backgroundColor,text:panel.color,muted:root.getPropertyValue('--muted').trim(),control:root.getPropertyValue('--strong-border').trim(),input:root.getPropertyValue('--input').trim(),accent:root.getPropertyValue('--accent').trim(),onAccent:root.getPropertyValue('--on-accent').trim(),frameFilter:frame.filter,frameOpacity:frame.opacity,sourcePixels:(()=>{const im=document.querySelector('.source-frame');const c=document.createElement('canvas');c.width=im.naturalWidth;c.height=im.naturalHeight;const x=c.getContext('2d');x.drawImage(im,0,0);let h=2166136261;for(const v of x.getImageData(0,0,c.width,c.height).data)h=Math.imul(h^v,16777619);return [c.width,c.height,h>>>0];})(),logoLoaded:logo.complete&&logo.naturalWidth>0,logoSource:logo.getAttribute('src'),logoAlpha:(()=>{const c=document.createElement('canvas');c.width=c.height=32;const x=c.getContext('2d');x.drawImage(logo,0,0,32,32);return [x.getImageData(0,0,1,1).data[3],x.getImageData(31,31,1,1).data[3],x.getImageData(16,16,1,1).data[3]];})()};",
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
    if text < 4.5 || muted < 4.5 || control < 3.0 || accent < 4.5 {
        return Err(format!("Rendered {name} contrast failed: text={text:.2} muted={muted:.2} control={control:.2} accent={accent:.2}").into());
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
            &json!({"observation":observation,"text_contrast":text,"muted_contrast":muted,"control_contrast":control,"accent_contrast":accent}),
        )?,
    )?;
    let screenshot = driver.request(reqwest::Method::GET, "/screenshot", json!({}))?;
    std::fs::write(
        output.join(format!("workspace-{name}.png")),
        decode_base64(screenshot.as_str().ok_or("Missing theme screenshot")?)?,
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
        for width in [720, 1280] {
            driver.request(
                reqwest::Method::POST,
                "/window/rect",
                json!({"width":width,"height":860}),
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
        // Showing a macOS window does not activate its application; session selection
        // in the pinned driver is logical only. Activate through the native CI surface.
        let native = driver.request(
            reqwest::Method::POST,
            "/execute/async",
            json!({"script":"const done=arguments[arguments.length-1];window.__TAURI__.core.invoke('e2e_focus').then(focused=>done({focused}),()=>done({error:'Native focus command failed'}));","args":[]}),
        )?;
        let focused = driver.script(observe)?;
        std::fs::write(
            output.join("control-focus.json"),
            serde_json::to_vec_pretty(&json!({"before":before,"native":native,"after":focused}))?,
        )?;
        if native["focused"] != true
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
        Ok(())
    })();
    let restored = driver.request(reqwest::Method::POST, "/window/rect", original);
    result?;
    restored?;
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
