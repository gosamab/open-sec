use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("rate limited (retry after {retry_after:?})")]
    RateLimited { retry_after: Option<Duration> },

    #[error("authentication failed")]
    AuthFailed,

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("server error ({status}): {body}")]
    Server { status: u16, body: String },

    #[error("decode error: {0}")]
    Decode(#[from] serde_json::Error),

    #[error("cancelled")]
    Cancelled,
}

pub type ProviderResult<T> = Result<T, ProviderError>;
