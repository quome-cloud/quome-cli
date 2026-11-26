use std::sync::OnceLock;
use thiserror::Error;

/// The exact `quome connect` command that reconnects the connection in use, set once by
/// a command that runs on a stored connection (the MCP server, `quome cloud status`).
/// Unset, the disconnected messages say plain `quome connect`.
static RECONNECT: OnceLock<String> = OnceLock::new();

/// The command that reconnects `profile` to `api_url`: the API URL is always named (so
/// the command works as written, wherever the connection points), the profile only when
/// it is not `quome connect`'s default.
pub fn reconnect_command(api_url: &str, profile: &str) -> String {
    let mut cmd = format!("quome connect {}", api_url.trim_end_matches('/'));
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

/// What the MCP server adds to the reconnect advice: it cannot pick up a new
/// connection by itself once its own reload found nothing new.
pub const RESTART_MCP: &str = "then restart this MCP server";

/// Added when the identity service refused the grant (a skewed clock is a common cause).
pub const CLOCK_HINT: &str = "If you just reconnected, check this computer's clock is correct.";

/// Why an agent's stored connection no longer works, with the command `cmd` that
/// reconnects it; `restart` adds the MCP server's restart step. `quome cloud status`
/// and `quome mcp` both say this.
pub fn connection_lost(cmd: &str, restart: bool) -> String {
    let then = if restart {
        format!(", {RESTART_MCP}")
    } else {
        String::new()
    };
    format!(
        "This agent's Quome Cloud connection has expired or was revoked. \
         Run `{cmd}` in a terminal to reconnect{then}."
    )
}

/// `connection_lost` for a session error `e` (`CloudDisconnected`, `CloudInvalidGrant`)
/// with this process's reconnect command; `None` for any other error.
pub fn session_lost_message(e: &QuomeError, restart: bool) -> Option<String> {
    let m = connection_lost(reconnect(), restart);
    match e {
        QuomeError::CloudDisconnected => Some(m),
        QuomeError::CloudInvalidGrant => Some(format!("{m} {CLOCK_HINT}")),
        _ => None,
    }
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

    /// The delegation was revoked (401 `delegation_revoked`), or a fresh token was
    /// refused with a plain 401.
    #[error("{}", connection_lost(reconnect(), false))]
    CloudDisconnected,

    /// The token exchange was refused with `invalid_grant`: the delegation was revoked or
    /// expired, the key no longer matches, or the local clock is off.
    #[error("{} {CLOCK_HINT}", connection_lost(reconnect(), false))]
    CloudInvalidGrant,

    /// The console's connect page refused the link (`refused` on the loopback
    /// callback): the code when it is a plain one (`^[a-z_]{1,32}$`).
    #[error("{}", crate::cloud::loopback::refusal_message(.0.as_deref(), None))]
    ConsoleRefused(Option<String>),

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
    use super::{connection_lost, reconnect_command};

    #[test]
    fn connection_lost_says_why_and_how_to_reconnect() {
        assert_eq!(
            connection_lost("quome connect https://api.example.test", true),
            "This agent's Quome Cloud connection has expired or was revoked. \
             Run `quome connect https://api.example.test` in a terminal to reconnect, \
             then restart this MCP server."
        );
        assert!(connection_lost("quome connect x", false).ends_with("to reconnect."));
    }

    #[test]
    fn reconnect_command_always_names_the_api_url() {
        assert_eq!(
            reconnect_command("https://api.quome.cloud", "cloud"),
            "quome connect https://api.quome.cloud"
        );
        assert_eq!(
            reconnect_command("https://api.quome.cloud/", "cloud"),
            "quome connect https://api.quome.cloud"
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
            "quome connect https://api.quome.cloud --profile work"
        );
    }
}
