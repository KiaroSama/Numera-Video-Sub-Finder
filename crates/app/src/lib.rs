pub mod assistant;
pub mod export;
pub mod extraction;
pub mod inference;
pub mod logging;
pub mod media;
pub mod model_download;
pub mod models;
pub mod paths;
pub mod providers;
pub mod regions;
pub mod storage;
pub mod tesseract;
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
    #[error("Media decoding failed: {0}")]
    Media(#[from] ffmpeg_next::Error),
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
