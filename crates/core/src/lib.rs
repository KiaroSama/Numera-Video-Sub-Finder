mod edit;
mod export;
mod geometry;
mod time;
mod tracking;
mod types;

pub use edit::*;
pub use export::*;
pub use geometry::*;
pub use time::*;
pub use tracking::*;
pub use types::*;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid input: {0}")]
    Invalid(String),
    #[error("Timestamp arithmetic overflow")]
    Overflow,
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn normalize_persian(text: &str, enabled: bool) -> String {
    if enabled {
        text.replace('ي', "ی").replace('ك', "ک")
    } else {
        text.into()
    }
}
