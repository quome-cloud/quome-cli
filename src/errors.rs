use std::sync::OnceLock;
use thiserror::Error;

/// The exact `quome connect` command that reconnects the connection in use, set once by
/// a command that runs on a stored connection (the MCP server, `quome cloud status`).
/// Unset, the disconnected messages say plain `quome connect`.
static RECONNECT: OnceLock<String> = OnceLock::new();

/// The command that reconnects `profile` to `api_url`: the API URL and profile are named
/// only when they differ from `quome connect`'s defaults, so a default connection reads
/// `quome connect`.
pub fn reconnect_command(api_url: &str, profile: &str) -> String {
    let mut cmd = String::from("quome connect");
    let url = api_url.trim_end_matches('/');
    if url != crate::commands::connect::DEFAULT_API_URL {
        cmd.push(' ');
        cmd.push_str(url);
    }
    if profile != crate::commands::connect::DEFAULT_PROFILE {
        cmd.push_str(" --profile ");
        cmd.push_str(profile);
    }
    cmd
}

/// Records the reconnect command for this process (the first call wins).
pub fn set_reconnect_command(api_url: &str, profile: &str) {
    let _ = RECONNECT.set(reconnect_command(api_url, profile));
}

fn reconnect() -> &'static str {
    RECONNECT.get().map_or("quome connect", String::as_str)
}

#[derive(Error, Debug)]
pub enum QuomeError {
    #[error("Not logged in. Run `quome login` first.")]
    NotLoggedIn,

    #[error("No linked organization. Run `quome link` to connect.")]
    NoLinkedOrg,

    #[error("No linked application. Run `quome link` to connect.")]
    NoLinkedApp,

    #[error(
        "Unauthorized. The API key was rejected — it is deleted, expired, or flagged \
         \"Legacy (non-functional)\" on the dashboard's API Keys page. Create a new \
         Full-access key there and run `quome login`."
    )]
    Unauthorized,

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("API error: {0}")]
    ApiError(String),

    /// An API error answer that keeps its HTTP status and stable `reason`
    /// beside the message (the text is the same as `ApiError`'s).
    #[error("API error: {message}")]
    ApiRefused {
        message: String,
        status: u16,
        reason: String,
    },

    /// The service answered 503 and may be ready shortly; `retry_after` is the
    /// `Retry-After` header in seconds, when sent as an integer.
    #[error("API error: {message}")]
    RetryLater {
        message: String,
        retry_after: Option<u64>,
    },

    /// A problem the CLI detected itself (bad input, a command the current
    /// credential can never run) — printed verbatim, no "API error" prefix.
    #[error("{0}")]
    Usage(String),

    #[error("Rate limited. Please wait and try again.")]
    RateLimited,

    #[error("Invalid response from server")]
    InvalidResponse,

    /// Quome Cloud agent state (`~/.quome/cloud.json`) problems.
    #[error("Quome Cloud: {0}")]
    CloudState(String),

    /// Quome Cloud agent key storage problems.
    #[error("Quome Cloud key storage: {0}")]
    CloudKey(String),

    /// The delegation was revoked (401 `delegation_revoked`).
    #[error(
        "This agent was disconnected from Quome Cloud — run `{}` again",
        reconnect()
    )]
    CloudDisconnected,

    /// The token exchange was refused with `invalid_grant`: the delegation was revoked or
    /// expired, the key no longer matches, or the local clock is off.
    #[error(
        "This agent was disconnected from Quome Cloud — run `{}` again. \
         If you just reconnected, check this computer's clock is correct.",
        reconnect()
    )]
    CloudInvalidGrant,

    /// The user has not finished approving in the browser (401 `agent_not_connected`).
    #[error("Finish approving in the browser, then try again.")]
    CloudNotYetConnected,

    #[allow(dead_code)] // constructed on Windows builds only
    #[error("Quome Cloud is not supported on this platform yet.")]
    CloudUnsupported,

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, QuomeError>;

#[cfg(test)]
mod tests {
    use super::reconnect_command;

    #[test]
    fn reconnect_command_names_only_what_differs_from_the_defaults() {
        assert_eq!(
            reconnect_command("https://api.quome.cloud", "cloud"),
            "quome connect"
        );
        assert_eq!(
            reconnect_command("https://api.quome.cloud/", "cloud"),
            "quome connect"
        );
        assert_eq!(
            reconnect_command("https://api.example.test", "cloud"),
            "quome connect https://api.example.test"
        );
        assert_eq!(
            reconnect_command("https://api.example.test/", "work"),
            "quome connect https://api.example.test --profile work"
        );
        assert_eq!(
            reconnect_command("https://api.quome.cloud", "work"),
            "quome connect --profile work"
        );
    }
}
