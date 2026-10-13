use nvsf_core::{MediaInfo, Progress};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use wasm_bindgen::prelude::*;

fn arguments(value: &serde_json::Value) -> Result<JsValue, String> {
    value
        .serialize(&serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true))
        .map_err(|e| e.to_string())
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], catch)]
    async fn invoke(command: &str, args: JsValue) -> Result<JsValue, JsValue>;
}

pub async fn call<T: DeserializeOwned>(
    command: &str,
    args: serde_json::Value,
) -> Result<T, String> {
    let args = arguments(&args)?;
    let value = invoke(command, args).await.map_err(error_text)?;
    serde_wasm_bindgen::from_value(value).map_err(|e| format!("Invalid native response: {e}"))
}

pub async fn preview(token: &str, time_ms: u64) -> Result<String, String> {
    let args = arguments(&serde_json::json!({"token": token, "timeMs": time_ms}))?;
    let bytes = invoke("preview_frame", args).await.map_err(error_text)?;
    let bytes = js_sys::Uint8Array::new(&bytes);
    if bytes.length() > 8 * 1024 * 1024 {
        return Err("Preview exceeds binary transport limit".into());
    }
    let parts = js_sys::Array::new();
    parts.push(&bytes.buffer());
    let options = web_sys::BlobPropertyBag::new();
    options.set_type("image/png");
    let blob = web_sys::Blob::new_with_buffer_source_sequence_and_options(&parts, &options)
        .map_err(error_text)?;
    web_sys::Url::create_object_url_with_blob(&blob).map_err(error_text)
}

fn error_text(value: JsValue) -> String {
    value
        .as_string()
        .or_else(|| {
            js_sys::JSON::stringify(&value)
                .ok()
                .and_then(|v| v.as_string())
        })
        .unwrap_or_else(|| {
            "Native bridge unavailable. Launch the packaged desktop application".into()
        })
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Source {
    pub token: String,
    pub name: String,
    pub fingerprint: String,
    pub media: MediaInfo,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Destination {
    pub token: String,
    pub name: String,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub struct JobStatus {
    pub id: String,
    pub state: String,
    pub progress: Option<Progress>,
    pub error: Option<String>,
    pub project_ready: bool,
}
#[derive(Clone, Debug, Deserialize)]
pub struct QueueEntry {
    pub id: String,
    pub name: String,
    pub output_name: String,
    pub language: String,
    pub device: String,
    pub quality: String,
    pub state: String,
    pub needs_regrant: bool,
    pub resumable: bool,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Started {
    pub id: String,
    pub source: Source,
    pub output: Destination,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Model {
    pub id: String,
    pub languages: Vec<String>,
    pub installed: bool,
    pub verified: bool,
    pub message: String,
}
#[derive(Clone, Debug, Deserialize)]
pub struct HistoryEntry {
    pub token: String,
    pub name: String,
    pub modified: Option<u64>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Doctor {
    pub data_root: String,
    pub models_root: String,
    pub runtimes_present: bool,
    pub log_available: bool,
    pub platform: String,
    pub models: Vec<Model>,
    pub message: String,
}
