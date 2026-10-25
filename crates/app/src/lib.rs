pub mod assistant;
pub mod export;
pub mod extraction;
pub mod inference;
pub mod languages;
pub mod logging;
pub mod media;
pub mod model_download;
pub mod models;
pub mod native_request;
pub mod paths;
pub mod providers;
pub mod regions;
mod runtime_health;
pub mod runtimes;
pub mod storage;
pub mod tesseract;
pub mod update_payload;
pub mod updates;
pub mod vision;
pub mod worker;

pub use export::export_project;
pub use extraction::{Control, extract};
pub use paths::Paths;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Core(#[from] nvsf_core::Error),
    #[error("Filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid project or metadata: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Operation failed: {0}")]
    Message(String),
    #[error("Job cancelled; completed checkpoint preserved")]
    Cancelled,
    #[error("Required provider unavailable: {0}")]
    Provider(String),
}

pub type Result<T> = std::result::Result<T, AppError>;

pub fn fail(message: impl Into<String>) -> AppError {
    AppError::Message(message.into())
}
