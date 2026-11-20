use crate::format::Format;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unknown or unsupported format: {0}")]
    UnknownFormat(String),
    #[error("no conversion path from {from} to {to}")]
    NoPath { from: Format, to: Format },
    #[error("conversion cancelled")]
    Cancelled,
    #[error("{0}")]
    Failed(String),
}

impl Error {
    pub fn failed(msg: impl Into<String>) -> Self {
        Error::Failed(msg.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
