use crate::{Paths, Result, fail, models, providers};
use image::RgbImage;
use ndarray::{Array4, ArrayViewD};
use nvsf_core::Recognition;
use ort::{environment::Environment, session::Session, value::TensorRef};
use serde::Deserialize;

pub type InferenceObserver<'a> = dyn FnMut(&str, &[usize], &[f32]) -> Result<()> + 'a;

pub struct Ocr {
    _environment: Environment,
    detector: Session,
    recognizer: Option<Session>,
    tesseract: Option<crate::tesseract::Tesseract>,
    dictionary: Vec<String>,
    language: String,
    pub provider: String,
    _runtime: libloading::Library,
}

#[derive(Deserialize)]
struct Meta {
    #[serde(rename = "PostProcess")]
    post: Post,
}
#[derive(Deserialize)]
struct Post {
    character_dict: Vec<String>,
}

impl Ocr {
    pub fn new(
        paths: &Paths,
        language: &str,
        device: &str,
        threads: usize,
        required: bool,
    ) -> Result<Self> {
        let runtime = paths.runtime(if cfg!(windows) {
            "onnxruntime.dll"
        } else if cfg!(target_os = "macos") {
            "libonnxruntime.dylib"
        } else {
            "libonnxruntime.so"
        })?;
        let library = paths.library(crate::runtimes::ONNX_LIBRARY)?;
        static ENV: std::sync::OnceLock<Environment> = std::sync::OnceLock::new();
        static INIT: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let guard = INIT
            .lock()
            .map_err(|_| fail("Inference initialization lock failed"))?;
        let env = if let Some(env) = ENV.get() {
            env.clone()
        } else {
            let env = ort::init_from(runtime)
                .map_err(|e| fail(e.to_string()))?
                .with_telemetry(false)
                .with_name("nvsf")
                .build()
                .map_err(|e| fail(e.to_string()))?;
            ENV.set(env.clone())
                .map_err(|_| fail("Inference environment already initialized"))?;
            env
        };
        drop(guard);
        let id = if language == "ja-specialist" {
            Some("ja-specialist")
        } else {
            nvsf_core::languages::find(language)
                .ok_or_else(|| fail("Choose a language from the official catalog"))?;
            nvsf_core::languages::paddle_model(language)
        };
        if id.is_none() && required {
            return Err(crate::AppError::Provider(
                "This language uses Tesseract CPU recognition; required acceleration cannot be met"
                    .into(),
            ));
        }
        let det = models::verify(paths, "det")?;
        let (detector, provider) = providers::session(&env, &det, device, threads, required)?;
        let mut dictionary = Vec::new();
        let (recognizer, tesseract) = if let Some(id) = id {
            let rec = models::verify(paths, id)?;
            let (recognizer, actual) =
                providers::session(&env, &rec, &provider, threads, required)?;
            if actual != provider {
                return Err(fail("Detection and recognition provider mismatch"));
            }
            let meta: Meta = serde_yaml_ng::from_str(models::metadata(id)?)
                .map_err(|e| fail(format!("Invalid model dictionary: {e}")))?;
            dictionary.push(String::new());
            dictionary.extend(meta.post.character_dict);
            dictionary.push(" ".into());
            (Some(recognizer), None)
        } else {
            (
                None,
                Some(crate::tesseract::Tesseract::new(paths, language)?),
            )
        };
        let provider = if tesseract.is_some() {
            format!(
                "detector={provider};recognizer=tesseract-cpu (no admitted accelerator recognizer for this language)"
            )
        } else {
            provider
        };
        Ok(Self {
            _environment: env,
            _runtime: library,
            detector,
            recognizer,
            tesseract,
            dictionary,
            language: language.into(),
            provider,
        })
    }

    pub fn detect(&mut self, image: &RgbImage) -> Result<Vec<(u32, u32, u32, u32)>> {
        self.detect_inner(image, None)
    }

    /// Observes actual borrowed tensors synchronously; ordinary extraction records no pixels.
    pub fn detect_observed(
        &mut self,
        image: &RgbImage,
        observer: &mut InferenceObserver<'_>,
    ) -> Result<Vec<(u32, u32, u32, u32)>> {
        self.detect_inner(image, Some(observer))
    }

    fn detect_inner(
        &mut self,
        image: &RgbImage,
        mut observer: Option<&mut InferenceObserver<'_>>,
    ) -> Result<Vec<(u32, u32, u32, u32)>> {
        let longest = image.width().max(image.height()).max(1) as f32;
        let ratio = 960.0 / longest;
        let width = ((image.width() as f32 * ratio / 32.0).round().max(1.0) as u32) * 32;
        let height = ((image.height() as f32 * ratio / 32.0).round().max(1.0) as u32) * 32;
        let resized =
            image::imageops::resize(image, width, height, image::imageops::FilterType::Triangle);
        let mut input = Array4::<f32>::zeros((1, 3, height as usize, width as usize));
        let mean = [0.485, 0.456, 0.406];
        let std = [0.229, 0.224, 0.225];
        for (x, y, p) in resized.enumerate_pixels() {
            for c in 0..3 {
                input[[0, c, y as usize, x as usize]] =
                    (f32::from(p[2 - c]) / 255.0 - mean[c]) / std[c];
            }
        }
        if let Some(observe) = observer.as_deref_mut() {
            observe(
                "detector-input",
                input.shape(),
                input
                    .as_slice()
                    .ok_or_else(|| fail("Noncontiguous detector input"))?,
            )?;
        }
        let outputs = self
            .detector
            .run(ort::inputs![
                TensorRef::from_array_view(&input).map_err(|e| fail(e.to_string()))?
            ])
            .map_err(|e| fail(e.to_string()))?;
        let output = outputs[0]
            .try_extract_array::<f32>()
            .map_err(|e| fail(e.to_string()))?;
        let shape = output.shape();
        if shape.len() != 4 || shape[0] != 1 || shape[1] != 1 {
            return Err(fail("Unexpected detector tensor shape"));
        }
        if let Some(observe) = observer {
            observe(
                "detector-output",
                shape,
                output
                    .as_slice()
                    .ok_or_else(|| fail("Noncontiguous detector output"))?,
            )?;
        }
        let (width, height) = (shape[3], shape[2]);
        crate::vision::db_regions(output, width, height, image.width(), image.height())
    }

    pub fn recognize(&mut self, crop: &RgbImage) -> Result<Recognition> {
        self.recognize_inner(crop, None)
    }

    pub fn recognize_observed(
        &mut self,
        crop: &RgbImage,
        observer: &mut InferenceObserver<'_>,
    ) -> Result<Recognition> {
        self.recognize_inner(crop, Some(observer))
    }

    fn recognize_inner(
        &mut self,
        crop: &RgbImage,
        mut observer: Option<&mut InferenceObserver<'_>>,
    ) -> Result<Recognition> {
        if crop.width() == 0 || crop.height() == 0 {
            return Err(fail("Empty recognition crop"));
        }
        if let Some(tesseract) = &mut self.tesseract {
            let vertical = nvsf_core::languages::find(&self.language).is_some_and(|l| l.vertical);
            return tesseract.recognize(crop, vertical);
        }
        let width =
            ((48.0 * crop.width() as f64 / crop.height() as f64).ceil() as u32).clamp(8, 3200);
        let padded = width.max(320);
        let resized =
            image::imageops::resize(crop, width, 48, image::imageops::FilterType::CatmullRom);
        let mut input = Array4::<f32>::zeros((1, 3, 48, padded as usize));
        for (x, y, p) in resized.enumerate_pixels() {
            for c in 0..3 {
                input[[0, c, y as usize, x as usize]] = f32::from(p[2 - c]) / 127.5 - 1.0;
            }
        }
        if let Some(observe) = observer.as_deref_mut() {
            observe(
                "recognizer-input",
                input.shape(),
                input
                    .as_slice()
                    .ok_or_else(|| fail("Noncontiguous recognizer input"))?,
            )?;
        }
        let outputs = self
            .recognizer
            .as_mut()
            .ok_or_else(|| fail("Recognition backend is not initialized"))?
            .run(ort::inputs![
                TensorRef::from_array_view(&input).map_err(|e| fail(e.to_string()))?
            ])
            .map_err(|e| fail(e.to_string()))?;
        let output = outputs[0]
            .try_extract_array::<f32>()
            .map_err(|e| fail(e.to_string()))?;
        if let Some(observe) = observer {
            observe(
                "recognizer-output",
                output.shape(),
                output
                    .as_slice()
                    .ok_or_else(|| fail("Noncontiguous recognizer output"))?,
            )?;
        }
        let (text, confidence) = decode_ctc(output, &self.dictionary)?;
        let text = if nvsf_core::languages::paddle_model(&self.language) == Some("fa") {
            arabic_model_order(&text)
        } else {
            text
        };
        Ok(Recognition {
            text,
            confidence: Some(confidence),
            crop: None,
        })
    }
}

pub fn decode_ctc(output: ArrayViewD<'_, f32>, dictionary: &[String]) -> Result<(String, f32)> {
    let shape = output.shape();
    if shape.len() != 3 || shape[0] != 1 || shape[2] != dictionary.len() {
        return Err(fail(format!(
            "Recognition vocabulary mismatch: tensor={shape:?} dictionary={}",
            dictionary.len()
        )));
    }
    let mut text = String::new();
    let mut sum = 0.0;
    let mut count = 0;
    let mut previous = usize::MAX;
    for t in 0..shape[1] {
        let mut selected = 0;
        let mut score = f32::NEG_INFINITY;
        for c in 0..shape[2] {
            let v = output[[0, t, c]];
            if !v.is_finite() {
                return Err(fail("Nonfinite recognition score"));
            }
            if v > score {
                score = v;
                selected = c;
            }
        }
        if selected != 0 && selected != previous {
            text.push_str(&dictionary[selected]);
            sum += score;
            count += 1;
        }
        previous = selected;
    }
    Ok((text, if count > 0 { sum / count as f32 } else { 0.0 }))
}

fn arabic_model_order(text: &str) -> String {
    // This model decodes visual token order; reverse token groups, not stored logical Persian.
    let mut groups: Vec<String> = Vec::new();
    let mut latin = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || " :*./%+-".contains(c) {
            latin.push(c);
        } else {
            if !latin.is_empty() {
                groups.push(std::mem::take(&mut latin));
            }
            groups.push(c.to_string());
        }
    }
    if !latin.is_empty() {
        groups.push(latin);
    }
    groups.into_iter().rev().collect()
}
