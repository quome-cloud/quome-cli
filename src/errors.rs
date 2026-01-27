use thiserror::Error;

#[derive(Error, Debug)]
pub enum QuomeError {
    #[error("Not logged in. Run `quome login` first.")]
    NotLoggedIn,

    #[error("No linked organization. Run `quome link` to connect.")]
    NoLinkedOrg,

    #[error("No linked application. Run `quome link` to connect.")]
    NoLinkedApp,

    #[error("Unauthorized. Your session may have expired. Run `quome login`.")]
    Unauthorized,

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("API error: {0}")]
    ApiError(String),

    #[error("Rate limited. Please wait and try again.")]
    RateLimited,

    #[error("Invalid response from server")]
    InvalidResponse,

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, QuomeError>;
