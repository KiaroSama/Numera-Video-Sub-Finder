//! Native Rust/WASM WebGL1 sweep transition ported from glimm (MIT, Noman Ijaz)
//! with the brushed-silver cosine palette from godverify.com.

use crate::{
    glimm_resource::OwnedResource,
    helpers::{ease_sweep, sweep_clip_inset},
};
use gloo_timers::callback::Timeout;
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{
    Document, Element, HtmlCanvasElement, HtmlElement, WebGlBuffer, WebGlContextAttributes,
    WebGlProgram, WebGlRenderingContext as Gl, WebGlShader, WebGlUniformLocation, Window,
};

const DURATION_MS: f64 = 680.0;
const DEADLINE_MS: u32 = 1_000;
const PAL_A: [f32; 3] = [0.74, 0.75, 0.78];
const PAL_B: [f32; 3] = [0.24, 0.24, 0.24];
const PAL_C: [f32; 3] = [1.0, 1.0, 1.0];
const PAL_D: [f32; 3] = [0.0, 0.02, 0.05];

const VS: &str = "attribute vec2 a;\nvoid main(){ gl_Position = vec4(a, 0.0, 1.0); }";

const FS: &str = r#"precision mediump float;
uniform vec2 uRes;
uniform float uTime;
uniform float uProgress;
uniform float uAlpha;
uniform float uBandTight;
uniform float uPosStart;
uniform float uPosEnd;
uniform float uHueShift;
uniform float uDirection;
uniform float uWaveAmount;
uniform float uRippleAmount;
uniform float uWaveSpeed;
uniform float uBrightness;
uniform float uSwellAmount;
uniform vec3 uPalA;
uniform vec3 uPalB;
uniform vec3 uPalC;
uniform vec3 uPalD;

#define PI 3.14159265359

vec3 pal(in float t, in vec3 a, in vec3 b, in vec3 c, in vec3 d) {
  return a + b * cos(2.0 * PI * (c * t + d));
}

void main() {
  vec2 uv = gl_FragCoord.xy / uRes;
  float axis = mix(uv.x, uv.y, uDirection);
  float cross = mix(uv.y, uv.x, uDirection);
  float pos = uPosStart + uProgress * (uPosEnd - uPosStart);
  float tw = uTime * uWaveSpeed;
  float waveX = sin(cross * 4.2 + tw * 0.08 + 0.3) * 0.004 * uWaveAmount;
  float d = (axis - pos) - waveX;
  float band = exp(-d * d * uBandTight);
  float dhDaxis = -2.0 * d * uBandTight * band;
  vec2 slope = vec2(mix(dhDaxis, 0.0, uDirection), mix(0.0, dhDaxis, uDirection));
  vec3 N = normalize(vec3(-slope.x * 0.18, slope.y * 0.18, 1.0));
  float behind = max(-d * sign(uPosEnd - uPosStart), 0.0);
  float trail = smoothstep(0.0, 0.08, behind) * (1.0 - smoothstep(0.12, 0.65, behind)) * 0.10;
  float midpointFocus = 4.0 * uProgress * (1.0 - uProgress);
  float halo = exp(-d * d * 2.5) * 0.05 * midpointFocus;
  float intensity = clamp(max(band, trail) + halo * (1.0 - band), 0.0, 1.0);
  float ripple = sin(cross * 12.0 + axis * 3.0 + tw * 0.40) * 0.015 * uRippleAmount;
  float t = N.x * 0.12 + N.y * 0.08 + axis * 0.90 + cross * 0.16 + ripple + uHueShift + uTime * 0.04;
  vec3 col = (pal(t, uPalA, uPalB, uPalC, uPalD) * 0.50
            + pal(t - 0.18, uPalA, uPalB, uPalC, uPalD) * 0.25
            + pal(t + 0.18, uPalA, uPalB, uPalC, uPalD) * 0.25) * uBrightness;
  vec3 V = vec3(0.0, 0.0, 1.0);
  vec3 L = normalize(vec3(0.35, 0.55, 0.9));
  vec3 H = normalize(L + V);
  float fresnel = pow(1.0 - clamp(dot(N, V), 0.0, 1.0), 3.0);
  float spec = pow(clamp(dot(N, H), 0.0, 1.0), 80.0);
  float entryFade = mix(0.2, 1.0, midpointFocus);
  float bodyA = intensity * uAlpha * entryFade;
  vec3 bodyPM = col * bodyA;
  float highMask = band * uAlpha * entryFade * uSwellAmount;
  vec3 highEmit = (col * fresnel * 0.55 + vec3(spec) * 1.1) * highMask;
  float highA = (fresnel * 0.4 + spec * 0.9) * highMask;
  gl_FragColor = vec4(bodyPM + highEmit, min(bodyA + highA, 1.0));
}"#;

struct Uniforms {
    res: Option<WebGlUniformLocation>,
    time: Option<WebGlUniformLocation>,
    progress: Option<WebGlUniformLocation>,
    alpha: Option<WebGlUniformLocation>,
    band_tight: Option<WebGlUniformLocation>,
    pos_start: Option<WebGlUniformLocation>,
    pos_end: Option<WebGlUniformLocation>,
    hue_shift: Option<WebGlUniformLocation>,
    direction: Option<WebGlUniformLocation>,
    wave_amount: Option<WebGlUniformLocation>,
    ripple_amount: Option<WebGlUniformLocation>,
    wave_speed: Option<WebGlUniformLocation>,
    brightness: Option<WebGlUniformLocation>,
    swell_amount: Option<WebGlUniformLocation>,
    pal_a: Option<WebGlUniformLocation>,
    pal_b: Option<WebGlUniformLocation>,
    pal_c: Option<WebGlUniformLocation>,
    pal_d: Option<WebGlUniformLocation>,
}

struct GlEngine {
    canvas: OwnedResource<HtmlCanvasElement>,
    gl: Gl,
    program: OwnedResource<WebGlProgram>,
    _buffer: OwnedResource<WebGlBuffer>,
    u: Uniforms,
    origin_ms: f64,
}

#[derive(Default)]
struct SweepState {
    engine: Option<Rc<GlEngine>>,
    generation: u64,
    frame: Option<(i32, Closure<dyn FnMut(f64)>)>,
    transition: Option<JsValue>,
    deadline: Option<Timeout>,
}

thread_local! {
    static STATE: RefCell<SweepState> = RefCell::new(SweepState::default());
}

fn compile_shader(gl: &Gl, kind: u32, source: &str) -> Option<OwnedResource<WebGlShader>> {
    let cleanup = gl.clone();
    let shader = OwnedResource::new(gl.create_shader(kind)?, move |shader| {
        cleanup.delete_shader(Some(shader));
    });
    gl.shader_source(&shader, source);
    gl.compile_shader(&shader);
    if gl
        .get_shader_parameter(&shader, Gl::COMPILE_STATUS)
        .as_bool()
        .unwrap_or(false)
    {
        Some(shader)
    } else {
        None
    }
}

fn init_engine(window: &Window, document: &Document) -> Option<Rc<GlEngine>> {
    let body = document.body()?;
    let canvas: HtmlCanvasElement = document.create_element("canvas").ok()?.dyn_into().ok()?;
    let canvas = OwnedResource::new(canvas, |canvas| canvas.remove());
    canvas.set_attribute("id", "nvsf-glimm-canvas").ok()?;
    canvas.set_attribute("data-glimm-layer", "").ok()?;
    canvas.set_attribute("aria-hidden", "true").ok()?;
    let style = canvas.style();
    style.set_property("position", "fixed").ok()?;
    style.set_property("inset", "0").ok()?;
    style.set_property("width", "100vw").ok()?;
    style.set_property("height", "100vh").ok()?;
    style.set_property("pointer-events", "none").ok()?;
    style.set_property("z-index", "2147483646").ok()?;
    style.set_property("opacity", "0").ok()?;

    let attrs = WebGlContextAttributes::new();
    attrs.set_alpha(true);
    attrs.set_antialias(false);
    attrs.set_premultiplied_alpha(true);
    let ctx = canvas
        .get_context_with_context_options("webgl", &attrs)
        .ok()??;
    let gl: Gl = ctx.dyn_into().ok()?;

    let vs = compile_shader(&gl, Gl::VERTEX_SHADER, VS)?;
    let fs = compile_shader(&gl, Gl::FRAGMENT_SHADER, FS)?;
    let cleanup = gl.clone();
    let program = OwnedResource::new(gl.create_program()?, move |program| {
        cleanup.use_program(None);
        cleanup.delete_program(Some(program));
    });
    gl.attach_shader(&program, &vs);
    gl.attach_shader(&program, &fs);
    gl.link_program(&program);
    gl.detach_shader(&program, &vs);
    gl.detach_shader(&program, &fs);
    drop(vs);
    drop(fs);
    if !gl
        .get_program_parameter(&program, Gl::LINK_STATUS)
        .as_bool()
        .unwrap_or(false)
    {
        return None;
    }

    let cleanup = gl.clone();
    let buffer = OwnedResource::new(gl.create_buffer()?, move |buffer| {
        cleanup.delete_buffer(Some(buffer));
    });
    gl.bind_buffer(Gl::ARRAY_BUFFER, Some(&buffer));
    let verts = js_sys::Float32Array::from(&[-1.0_f32, -1.0, 3.0, -1.0, -1.0, 3.0][..]);
    gl.buffer_data_with_array_buffer_view(Gl::ARRAY_BUFFER, &verts, Gl::STATIC_DRAW);
    gl.use_program(Some(&program));
    let loc = gl.get_attrib_location(&program, "a");
    if loc >= 0 {
        let idx = loc as u32;
        gl.enable_vertex_attrib_array(idx);
        gl.vertex_attrib_pointer_with_i32(idx, 2, Gl::FLOAT, false, 0, 0);
    }
    gl.enable(Gl::BLEND);
    gl.blend_func(Gl::ONE, Gl::ONE_MINUS_SRC_ALPHA);

    let u = Uniforms {
        res: gl.get_uniform_location(&program, "uRes"),
        time: gl.get_uniform_location(&program, "uTime"),
        progress: gl.get_uniform_location(&program, "uProgress"),
        alpha: gl.get_uniform_location(&program, "uAlpha"),
        band_tight: gl.get_uniform_location(&program, "uBandTight"),
        pos_start: gl.get_uniform_location(&program, "uPosStart"),
        pos_end: gl.get_uniform_location(&program, "uPosEnd"),
        hue_shift: gl.get_uniform_location(&program, "uHueShift"),
        direction: gl.get_uniform_location(&program, "uDirection"),
        wave_amount: gl.get_uniform_location(&program, "uWaveAmount"),
        ripple_amount: gl.get_uniform_location(&program, "uRippleAmount"),
        wave_speed: gl.get_uniform_location(&program, "uWaveSpeed"),
        brightness: gl.get_uniform_location(&program, "uBrightness"),
        swell_amount: gl.get_uniform_location(&program, "uSwellAmount"),
        pal_a: gl.get_uniform_location(&program, "uPalA"),
        pal_b: gl.get_uniform_location(&program, "uPalB"),
        pal_c: gl.get_uniform_location(&program, "uPalC"),
        pal_d: gl.get_uniform_location(&program, "uPalD"),
    };
    if gl.is_context_lost() || gl.get_error() != Gl::NO_ERROR {
        return None;
    }
    body.append_child(&canvas).ok()?;
    let origin_ms = now_ms(window);
    Some(Rc::new(GlEngine {
        canvas,
        gl,
        program,
        _buffer: buffer,
        u,
        origin_ms,
    }))
}

impl GlEngine {
    fn resize(&self, window: &Window) {
        let dpr = window.device_pixel_ratio().clamp(1.0, 2.0);
        let rect = self.canvas.get_bounding_client_rect();
        // Limit software rasterization without shrinking the on-screen canvas.
        let scale = dpr.min(640.0 / rect.width().max(rect.height()));
        let w = ((rect.width() * scale).round() as u32).max(1);
        let h = ((rect.height() * scale).round() as u32).max(1);
        if self.canvas.width() != w || self.canvas.height() != h {
            self.canvas.set_width(w);
            self.canvas.set_height(h);
            self.gl.viewport(
                0,
                0,
                self.canvas.width() as i32,
                self.canvas.height() as i32,
            );
        }
    }

    fn draw(&self, now: f64, progress: f32, rtl: bool, brightness: f32) {
        let gl = &self.gl;
        let u = &self.u;
        let (pos_start, pos_end) = if rtl {
            (1.2_f32, -0.2_f32)
        } else {
            (-0.2_f32, 1.2_f32)
        };
        let elapsed_s = ((now - self.origin_ms).max(0.0) / 1000.0) as f32;
        gl.use_program(Some(&self.program));
        gl.clear_color(0.0, 0.0, 0.0, 0.0);
        gl.clear(Gl::COLOR_BUFFER_BIT);
        gl.uniform2f(
            u.res.as_ref(),
            self.canvas.width() as f32,
            self.canvas.height() as f32,
        );
        gl.uniform1f(u.time.as_ref(), elapsed_s);
        gl.uniform1f(u.progress.as_ref(), progress);
        gl.uniform1f(u.alpha.as_ref(), 1.0);
        gl.uniform1f(u.band_tight.as_ref(), 14.0);
        gl.uniform1f(u.pos_start.as_ref(), pos_start);
        gl.uniform1f(u.pos_end.as_ref(), pos_end);
        gl.uniform1f(u.hue_shift.as_ref(), 0.08);
        gl.uniform1f(u.direction.as_ref(), 0.0);
        gl.uniform1f(u.wave_amount.as_ref(), 0.4);
        gl.uniform1f(u.ripple_amount.as_ref(), 1.0);
        gl.uniform1f(u.wave_speed.as_ref(), 1.0);
        gl.uniform1f(u.brightness.as_ref(), brightness);
        gl.uniform1f(u.swell_amount.as_ref(), 0.55);
        gl.uniform3f(u.pal_a.as_ref(), PAL_A[0], PAL_A[1], PAL_A[2]);
        gl.uniform3f(u.pal_b.as_ref(), PAL_B[0], PAL_B[1], PAL_B[2]);
        gl.uniform3f(u.pal_c.as_ref(), PAL_C[0], PAL_C[1], PAL_C[2]);
        gl.uniform3f(u.pal_d.as_ref(), PAL_D[0], PAL_D[1], PAL_D[2]);
        gl.draw_arrays(Gl::TRIANGLES, 0, 3);
    }

    fn hide(&self) {
        let _ = self.canvas.style().set_property("opacity", "0");
        self.gl.clear_color(0.0, 0.0, 0.0, 0.0);
        self.gl.clear(Gl::COLOR_BUFFER_BIT);
    }
}

fn now_ms(window: &Window) -> f64 {
    window
        .performance()
        .map_or_else(js_sys::Date::now, |p| p.now())
}

fn reduced_motion(window: &Window) -> bool {
    window
        .match_media("(prefers-reduced-motion: reduce)")
        .ok()
        .flatten()
        .is_some_and(|m| m.matches())
}

fn transition_promise(transition: &JsValue, property: &str) -> Option<JsFuture> {
    let promise = js_sys::Reflect::get(transition, &property.into())
        .ok()?
        .dyn_into::<js_sys::Promise>()
        .ok()?;
    Some(JsFuture::from(promise))
}

fn skip_transition(transition: &JsValue) {
    if let Ok(skip) = js_sys::Reflect::get(transition, &JsValue::from_str("skipTransition"))
        && let Ok(func) = skip.dyn_into::<js_sys::Function>()
    {
        let _ = func.call0(transition);
    }
}

fn cleanup_dom(root: &Element) {
    let _ = root.remove_attribute("data-glimm-transition");
    if let Some(html) = root.dyn_ref::<HtmlElement>() {
        let _ = html.style().remove_property("--glimm-old-clip");
    }
}

fn cancel_active(window: &Window, root: &Element) -> u64 {
    let (frame, transition, deadline, engine, next_gen) = STATE.with(|cell| {
        let mut st = cell.borrow_mut();
        st.generation = st.generation.wrapping_add(1);
        let engine = if st
            .engine
            .as_ref()
            .is_some_and(|engine| engine.gl.is_context_lost())
        {
            st.engine.take()
        } else {
            st.engine.clone()
        };
        (
            st.frame.take(),
            st.transition.take(),
            st.deadline.take(),
            engine,
            st.generation,
        )
    });
    cleanup_dom(root);
    drop(deadline);
    if let Some((id, callback)) = frame {
        let _ = window.cancel_animation_frame(id);
        // A frame can finish the sweep from inside this callback.
        spawn_local(async move { drop(callback) });
    }
    if let Some(vt) = transition {
        skip_transition(&vt);
    }
    if let Some(eng) = engine {
        if eng.gl.is_context_lost() {
            // A cancelled frame can still own the engine until its callback is released.
            eng.canvas.remove();
        } else {
            eng.hide();
        }
    }
    next_gen
}

fn is_generation(my_gen: u64) -> bool {
    STATE.with(|cell| cell.borrow().generation == my_gen)
}

fn finish_generation(window: &Window, root: &Element, my_gen: u64, target: &str) {
    if is_generation(my_gen) {
        let _ = root.set_attribute("data-theme", target);
        cancel_active(window, root);
    }
}

pub fn sweep_theme(from: &str, to: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let Some(root) = document.document_element() else {
        return;
    };
    let my_gen = cancel_active(&window, &root);
    if from == to || reduced_motion(&window) {
        let _ = root.set_attribute("data-theme", to);
        return;
    }

    let engine = STATE.with(|cell| {
        let mut st = cell.borrow_mut();
        if st.engine.is_none() {
            st.engine = init_engine(&window, &document);
        }
        st.engine.clone()
    });
    let Some(engine) = engine else {
        let _ = root.set_attribute("data-theme", to);
        return;
    };

    engine.resize(&window);
    let rtl = from == "dark";
    let brightness = if rtl { 0.85_f32 } else { 1.0_f32 };

    let mut has_vt = false;
    if let Ok(vt_prop) = js_sys::Reflect::get(&document, &JsValue::from_str("startViewTransition"))
        && let Ok(vt_fn) = vt_prop.dyn_into::<js_sys::Function>()
    {
        let _ = root.set_attribute("data-glimm-transition", "");
        if let Some(html) = root.dyn_ref::<HtmlElement>() {
            let _ = html.style().set_property("--glimm-old-clip", "inset(0)");
        }
        let root_cb = root.clone();
        let target = to.to_owned();
        let cb = Closure::once_into_js(move || {
            // skipTransition can still run a pending update callback.
            if is_generation(my_gen) {
                let _ = root_cb.set_attribute("data-theme", &target);
            }
        });
        match vt_fn.call1(&document, &cb) {
            Ok(vt) => {
                for property in ["updateCallbackDone", "finished"] {
                    if let Some(promise) = transition_promise(&vt, property) {
                        spawn_local(async move {
                            let _ = promise.await;
                        });
                    }
                }
                let Some(ready) = transition_promise(&vt, "ready") else {
                    let _ = root.set_attribute("data-theme", to);
                    cleanup_dom(&root);
                    skip_transition(&vt);
                    return;
                };
                let win_ready = window.clone();
                let root_ready = root.clone();
                let target_ready = to.to_owned();
                spawn_local(async move {
                    if ready.await.is_err() {
                        finish_generation(&win_ready, &root_ready, my_gen, &target_ready);
                    }
                });
                STATE.with(|cell| cell.borrow_mut().transition = Some(vt));
                has_vt = true;
            }
            Err(_) => {
                cleanup_dom(&root);
                let _ = root.set_attribute("data-theme", to);
            }
        }
    } else {
        let _ = root.set_attribute("data-theme", to);
    }

    let _ = engine.canvas.style().set_property("opacity", "1");
    let start_ms = now_ms(&window);
    let win_loop = window.clone();
    let root_loop = root.clone();
    let target_loop = to.to_owned();
    let step = Closure::new(move |_raf_ts: f64| {
        if !is_generation(my_gen) {
            return;
        }
        if engine.gl.is_context_lost() {
            finish_generation(&win_loop, &root_loop, my_gen, &target_loop);
            return;
        }
        let now = now_ms(&win_loop);
        let raw = ((now - start_ms) / DURATION_MS).clamp(0.0, 1.0);
        if raw < 1.0 {
            let eased = ease_sweep(raw);
            if has_vt && let Some(html) = root_loop.dyn_ref::<HtmlElement>() {
                let _ = html
                    .style()
                    .set_property("--glimm-old-clip", &sweep_clip_inset(eased, rtl));
            }
            engine.draw(now, eased as f32, rtl, brightness);
            let scheduled = STATE.with(|cell| {
                let mut st = cell.borrow_mut();
                let Some((id, callback)) = &mut st.frame else {
                    return false;
                };
                match win_loop.request_animation_frame(callback.as_ref().unchecked_ref()) {
                    Ok(next_id) => {
                        *id = next_id;
                        true
                    }
                    Err(_) => false,
                }
            });
            if scheduled {
                return;
            }
        }
        finish_generation(&win_loop, &root_loop, my_gen, &target_loop);
    });

    if let Ok(id) = window.request_animation_frame(step.as_ref().unchecked_ref()) {
        let win_timeout = window.clone();
        let root_timeout = root.clone();
        let target_timeout = to.to_owned();
        let deadline = Timeout::new(DEADLINE_MS, move || {
            finish_generation(&win_timeout, &root_timeout, my_gen, &target_timeout);
        });
        STATE.with(|cell| {
            let mut st = cell.borrow_mut();
            st.frame = Some((id, step));
            st.deadline = Some(deadline);
        });
    } else {
        finish_generation(&window, &root, my_gen, to);
    }
}
