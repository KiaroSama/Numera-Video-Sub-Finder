use super::gui::{Driver, Result};
use serde_json::{Value, json};
use std::path::Path;

pub fn verify(driver: &Driver, output: &Path) -> Result<()> {
    let initial = driver.script(
        r#"
        if (document.querySelector('[data-glimm-layer]')) throw new Error('Theme failure witness must precede the first animation');
        const canvasProto = HTMLCanvasElement.prototype, glProto = window.WebGLRenderingContext?.prototype;
        const native = {
            getContext: canvasProto.getContext, appendChild: Node.prototype.appendChild,
            createShader: glProto?.createShader, deleteShader: glProto?.deleteShader,
            createProgram: glProto?.createProgram, deleteProgram: glProto?.deleteProgram,
            createBuffer: glProto?.createBuffer, deleteBuffer: glProto?.deleteBuffer,
            getProgramParameter: glProto?.getProgramParameter,
            getShaderParameter: glProto?.getShaderParameter, getError: glProto?.getError,
            matchMedia: window.matchMedia
        };
        const audit = {
            native, mode: 'unavailable', reduced: false, current: {}, peak: 0, lossEvents: 0,
            originalTheme: document.documentElement.dataset.theme,
            saved: localStorage.getItem('nvsf-theme')
        };
        window.__nvsfThemeAudit = audit;
        const owns = gl => gl.canvas?.id === 'nvsf-glimm-canvas';
        const count = name => audit.current[name] = (audit.current[name] ?? 0) + 1;
        canvasProto.getContext = function(kind) {
            if (this.id === 'nvsf-glimm-canvas' && kind === 'webgl') {
                count('contextRequests');
                if (this.isConnected) count('attachedDuringInitialization');
                if (audit.mode === 'unavailable') { count('injected'); return null; }
                if (audit.mode === 'context-error') {
                    count('injected'); throw new DOMException('Injected unavailable context', 'NotSupportedError');
                }
            }
            return native.getContext.apply(this, arguments);
        };
        Node.prototype.appendChild = function(child) {
            if (child.id === 'nvsf-glimm-canvas' && audit.mode === 'append') {
                count('injected');
                throw new DOMException('Injected animation append failure', 'HierarchyRequestError');
            }
            const result = native.appendChild.apply(this, arguments);
            if (child.id === 'nvsf-glimm-canvas') {
                audit.peak = Math.max(audit.peak, document.querySelectorAll('[data-glimm-layer]').length);
            }
            return result;
        };
        if (glProto) {
        glProto.createShader = function(kind) {
            if (owns(this) && ((audit.mode === 'vertex' && kind === this.VERTEX_SHADER) ||
                (audit.mode === 'fragment' && kind === this.FRAGMENT_SHADER))) {
                count('injected'); return null;
            }
            const shader = native.createShader.apply(this, arguments);
            if (owns(this) && shader) count('shadersCreated');
            return shader;
        };
        glProto.deleteShader = function(shader) {
            if (owns(this) && shader) count('shadersDeleted');
            return native.deleteShader.apply(this, arguments);
        };
        for (const [kind, suffix] of [['program', 'Program'], ['buffer', 'Buffer']]) {
            glProto['create' + suffix] = function() {
                if (owns(this) && audit.mode === kind) { count('injected'); return null; }
                const resource = native['create' + suffix].apply(this, arguments);
                if (owns(this) && resource) count(kind + 'sCreated');
                return resource;
            };
            glProto['delete' + suffix] = function(resource) {
                if (owns(this) && resource) count(kind + 'sDeleted');
                return native['delete' + suffix].apply(this, arguments);
            };
        }
        glProto.getProgramParameter = function(program, parameter) {
            if (owns(this) && audit.mode === 'link' && parameter === this.LINK_STATUS) {
                count('injected'); return false;
            }
            return native.getProgramParameter.apply(this, arguments);
        };
        glProto.getShaderParameter = function(shader, parameter) {
            if (owns(this) && audit.mode === 'compile' && parameter === this.COMPILE_STATUS) {
                count('injected'); return false;
            }
            return native.getShaderParameter.apply(this, arguments);
        };
        glProto.getError = function() {
            if (owns(this) && audit.mode === 'gl-error') { count('injected'); return this.OUT_OF_MEMORY; }
            return native.getError.apply(this, arguments);
        };
        }
        window.matchMedia = function(query) {
            const result = native.matchMedia.apply(this, arguments);
            if (query === '(prefers-reduced-motion: reduce)') {
                Object.defineProperty(result, 'matches', {value: audit.reduced});
            }
            return result;
        };
        const probe = document.createElement('canvas');
        let gl = null, probeError = null;
        try { gl = native.getContext.call(probe, 'webgl'); }
        catch (error) { probeError = error.name; }
        const loss = gl?.getExtension('WEBGL_lose_context');
        if (loss) loss.loseContext();
        return {webglAvailable: !!gl, contextLossAvailable: !!loss, probeError, theme: audit.originalTheme};
        "#,
    )?;
    let result = (|| -> Result<Value> {
        let mut failures = Vec::new();
        for _ in 0..3 {
            failures.push(failed_init(driver, "unavailable")?);
        }
        failures.push(failed_init(driver, "context-error")?);
        if initial["webglAvailable"] == true {
            for stage in [
                "vertex", "fragment", "compile", "program", "link", "buffer", "gl-error", "append",
            ] {
                failures.push(failed_init(driver, stage)?);
            }
        }
        driver.script("const a=window.__nvsfThemeAudit;a.mode='unavailable';a.current={};a.reduced=true;return true;")?;
        toggle(driver)?;
        toggle(driver)?;
        let reduced = snapshot(driver)?;
        if reduced["canvases"] != 0
            || reduced["counts"]["contextRequests"].as_u64().unwrap_or(0) != 0
        {
            return Err(
                format!("Reduced motion attempted animation initialization: {reduced}").into(),
            );
        }

        let mut losses = Vec::new();
        if initial["webglAvailable"] == true {
            driver.script("const a=window.__nvsfThemeAudit;a.mode='healthy';a.current={};a.reduced=false;return true;")?;
            toggle(driver)?;
            let healthy = snapshot(driver)?;
            if healthy["canvases"] != 1
                || healthy["counts"]["attachedDuringInitialization"]
                    .as_u64()
                    .unwrap_or(0)
                    != 0
            {
                return Err(format!(
                    "Successful animation did not own one initialized canvas: {healthy}"
                )
                .into());
            }
            if initial["contextLossAvailable"] == true {
                for _ in 0..2 {
                    lose_context(driver, false)?;
                    toggle(driver)?;
                    let loss = snapshot(driver)?;
                    if loss["canvases"] != 1 || loss["contextLost"] != false || loss["peak"] != 1 {
                        return Err(format!(
                            "Lost animation context was reused or duplicated: {loss}"
                        )
                        .into());
                    }
                    losses.push(loss);
                }
                lose_context(driver, true)?;
                driver.wait("const a=window.__nvsfThemeAudit;return document.documentElement.dataset.theme===a.activeTarget && !document.documentElement.hasAttribute('data-glimm-transition') && document.querySelectorAll('[data-glimm-layer]').length===0;", 5)?;
                losses.push(snapshot(driver)?);
                toggle(driver)?;
                lose_context(driver, false)?;
                driver.script(
                    "const a=window.__nvsfThemeAudit;a.reduced=true;a.current={};return true;",
                )?;
                toggle(driver)?;
                let reduced_loss = snapshot(driver)?;
                if reduced_loss["canvases"] != 0
                    || reduced_loss["counts"]["contextRequests"]
                        .as_u64()
                        .unwrap_or(0)
                        != 0
                {
                    return Err(format!(
                        "Reduced-motion switch retained a lost canvas: {reduced_loss}"
                    )
                    .into());
                }
                losses.push(reduced_loss);
            }
        }
        driver.script("window.__nvsfThemeAudit.reduced=true;return true;")?;
        if driver.script("return document.documentElement.dataset.theme;")? != initial["theme"] {
            toggle(driver)?;
        }
        Ok(json!({
            "environment": initial, "initialization_failures": failures,
            "reduced_motion": reduced, "context_losses": losses,
            "context_loss_verified": initial["contextLossAvailable"] == true,
            "resource_failure_paths_verified": initial["webglAvailable"] == true
        }))
    })();
    let restored = driver.script(
        r#"
        const a = window.__nvsfThemeAudit, native = a.native, gl = window.WebGLRenderingContext?.prototype;
        HTMLCanvasElement.prototype.getContext = native.getContext;
        Node.prototype.appendChild = native.appendChild;
        if (gl) for (const name of ['createShader','deleteShader','createProgram','deleteProgram','createBuffer','deleteBuffer','getProgramParameter','getShaderParameter','getError']) gl[name] = native[name];
        window.matchMedia = native.matchMedia;
        if (a.saved === null) localStorage.removeItem('nvsf-theme');
        else localStorage.setItem('nvsf-theme', a.saved);
        delete window.__nvsfThemeAudit;
        return true;
        "#,
    );
    let receipt = result?;
    restored?;
    std::fs::write(
        output.join("theme-failures.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    Ok(())
}

fn failed_init(driver: &Driver, stage: &str) -> Result<Value> {
    driver.script(&format!(
        "const a=window.__nvsfThemeAudit;a.mode={};a.current={{}};return true;",
        serde_json::to_string(stage)?
    ))?;
    toggle(driver)?;
    let record = snapshot(driver)?;
    if record["canvases"] != 0
        || record["counts"]["injected"].as_u64().unwrap_or(0) == 0
        || record["counts"]["attachedDuringInitialization"]
            .as_u64()
            .unwrap_or(0)
            != 0
        || ["shaders", "programs", "buffers"].iter().any(|kind| {
            record["counts"][format!("{kind}Created")]
                .as_u64()
                .unwrap_or(0)
                != record["counts"][format!("{kind}Deleted")]
                    .as_u64()
                    .unwrap_or(0)
        })
    {
        return Err(
            format!("Theme initialization failure leaked resources at {stage}: {record}").into(),
        );
    }
    Ok(record)
}

fn toggle(driver: &Driver) -> Result<()> {
    let target = if driver.script("return document.documentElement.dataset.theme;")? == "dark" {
        "light"
    } else {
        "dark"
    };
    driver.click("#header-theme")?;
    driver.wait(&format!("return document.documentElement.dataset.theme==='{target}' && localStorage.getItem('nvsf-theme')==='{target}' && !document.documentElement.hasAttribute('data-glimm-transition') && document.documentElement.style.getPropertyValue('--glimm-old-clip')==='' && Array.from(document.querySelectorAll('[data-glimm-layer]')).every(e=>e.style.opacity==='0');"), 15)
}

fn lose_context(driver: &Driver, while_active: bool) -> Result<()> {
    let started = driver.script(&format!(
        r#"
        const a=window.__nvsfThemeAudit, before=a.lossEvents;
        if ({while_active}) {{
            a.activeTarget=document.documentElement.dataset.theme==='dark'?'light':'dark';
            document.querySelector('#header-theme').click();
        }}
        const canvas=document.querySelector('[data-glimm-layer]');
        const gl=canvas && a.native.getContext.call(canvas,'webgl');
        const extension=gl?.getExtension('WEBGL_lose_context');
        if (!extension) return {{error:'Missing context-loss extension on live animation'}};
        const active=canvas.style.opacity==='1';
        canvas.addEventListener('webglcontextlost',()=>a.lossEvents++,{{once:true}});
        extension.loseContext();
        return {{before,active}};
        "#
    ))?;
    if started["error"].is_string() || (while_active && started["active"] != true) {
        return Err(format!("Could not exercise actual context loss: {started}").into());
    }
    driver.wait(
        &format!(
            "return window.__nvsfThemeAudit.lossEvents > {};",
            started["before"]
                .as_u64()
                .ok_or("Missing context-loss event counter")?
        ),
        5,
    )
}

fn snapshot(driver: &Driver) -> Result<Value> {
    driver.script(
        r#"
        const a=window.__nvsfThemeAudit, canvas=document.querySelector('[data-glimm-layer]');
        const gl=canvas && a.native.getContext.call(canvas,'webgl');
        return {stage:a.mode, counts:a.current, canvases:document.querySelectorAll('[data-glimm-layer]').length,
            peak:a.peak, contextLost:gl ? gl.isContextLost() : null, theme:document.documentElement.dataset.theme};
        "#,
    )
}
