use std::io::{IsTerminal, Read};
use std::path::PathBuf;

use clap::Parser;

use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::{QuomeError, Result};
use crate::ui;

const KEY_PREFIX: &str = "qk_";
const DASHBOARD_KEYS_PATH: &str = "/settings/service-accounts";

#[derive(Parser)]
pub struct Args {
    /// API key (qk_...). Lands in shell history — prefer the prompt, --token-file, or stdin.
    #[arg(short, long, conflicts_with_all = ["token_file", "stdin"])]
    token: Option<String>,

    /// Read the API key from a file (first line; the file is not deleted).
    #[arg(long, value_name = "PATH", conflicts_with = "stdin")]
    token_file: Option<PathBuf>,

    /// Read the API key from stdin (automatic when stdin is not a terminal, e.g. a pipe).
    #[arg(long)]
    stdin: bool,
}

/// Where the key came from — decided before anything is read, so the
/// precedence is testable without a terminal.
#[derive(Debug, PartialEq, Eq)]
pub enum TokenSource {
    Flag,
    File(PathBuf),
    Stdin,
    Prompt,
}

pub fn token_source(
    token: &Option<String>,
    token_file: &Option<PathBuf>,
    stdin_flag: bool,
    stdin_is_terminal: bool,
) -> TokenSource {
    if token.is_some() {
        TokenSource::Flag
    } else if let Some(path) = token_file {
        TokenSource::File(path.clone())
    } else if stdin_flag || !stdin_is_terminal {
        TokenSource::Stdin
    } else {
        TokenSource::Prompt
    }
}

/// Trim whatever a paste, an editor, or `echo` wrapped around the key, then
/// refuse anything that is not a Quome key so the user gets a clear message
/// instead of a bare 401.
pub fn normalize_key(raw: &str) -> Result<String> {
    let key = raw
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_string();
    if key.is_empty() {
        return Err(QuomeError::Usage("No API key was provided".into()));
    }
    if !key.starts_with(KEY_PREFIX) {
        return Err(QuomeError::Usage(format!(
            "That doesn't look like a Quome API key (expected it to start with `{}`, got `{}…`)",
            KEY_PREFIX,
            key.chars().take(4).collect::<String>()
        )));
    }
    if key.chars().any(char::is_whitespace) {
        return Err(QuomeError::Usage(
            "The API key contains whitespace — it was probably pasted across a line break".into(),
        ));
    }
    Ok(key)
}

fn read_stdin_key() -> Result<String> {
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    Ok(buf.lines().next().unwrap_or("").to_string())
}

fn prompt_key() -> Result<String> {
    let dashboard = format!(
        "{}{}",
        crate::settings::Settings::load()
            .unwrap_or_default()
            .get_api_url(),
        DASHBOARD_KEYS_PATH
    );
    inquire::Password::new("API key:")
        .without_confirmation()
        // Masked (not hidden) so a paste visibly lands — the #1 support
        // question was "I pasted and nothing happened".
        .with_display_mode(inquire::PasswordDisplayMode::Masked)
        .with_help_message(&format!(
            "Create one at {} → Create key → Full access",
            dashboard
        ))
        .prompt()
        .map_err(|e| QuomeError::Io(std::io::Error::other(e.to_string())))
}

pub async fn execute(args: Args) -> Result<()> {
    let stdin_tty = std::io::stdin().is_terminal();
    let source = token_source(&args.token, &args.token_file, args.stdin, stdin_tty);

    // Only ask "replace the current login?" when we are going to prompt
    // anyway — a scripted login (flag / file / pipe) means "yes".
    let config = Config::load()?;
    if let (Some(user), TokenSource::Prompt) = (&config.user, &source) {
        let mut details: Vec<(&str, String)> = vec![("Key", format!("{}…", user.key_prefix()))];
        if let Some(org) = user.org_label() {
            details.push(("Organization", org));
        }
        if let Some(email) = &user.email {
            details.push(("Email (legacy login)", email.clone()));
        }
        let details_ref: Vec<(&str, &str)> =
            details.iter().map(|(k, v)| (*k, v.as_str())).collect();
        ui::print_detail("Already logged in", &details_ref);

        let confirm = inquire::Confirm::new("Log in with a different key?")
            .with_default(false)
            .prompt()
            .map_err(|e| QuomeError::Io(std::io::Error::other(e.to_string())))?;
        if !confirm {
            return Ok(());
        }
    }

    let raw = match source {
        TokenSource::Flag => args.token.clone().unwrap_or_default(),
        TokenSource::File(path) => std::fs::read_to_string(&path)
            .map_err(|e| {
                QuomeError::Io(std::io::Error::new(
                    e.kind(),
                    format!("could not read {}: {}", path.display(), e),
                ))
            })?
            .lines()
            .next()
            .unwrap_or("")
            .to_string(),
        TokenSource::Stdin => read_stdin_key()?,
        TokenSource::Prompt => prompt_key()?,
    };
    let token = normalize_key(&raw)?;

    let sp = ui::spinner("Validating key...");
    let api_url = crate::settings::Settings::load()
        .unwrap_or_default()
        .get_api_url();
    let client = QuomeClient::new(Some(&token), None)?;
    let identity = client.get_api_key_self().await.map_err(|e| match e {
        // Name the API we actually asked: the most common cause in practice
        // is a key minted on a DIFFERENT Quome environment than the CLI's
        // target (default or QUOME_API_URL), which is indistinguishable from
        // a dead key by status code alone.
        QuomeError::Unauthorized => QuomeError::Usage(format!(
            "{} rejected this key. Either it belongs to a different Quome \
             environment (set QUOME_API_URL to the API this key was created \
             on), or it is deleted, expired, or flagged \"Legacy \
             (non-functional)\" on that environment's API Keys page — create \
             a new Full-access key there and try again.",
            api_url
        )),
        other => other,
    })?;
    sp.finish_and_clear();

    let mut config = Config::load()?;
    config.set_key_login(token, &identity);
    config.save()?;

    let user = config.user.as_ref().expect("just set");
    let org = user.org_label().unwrap_or_default();
    let scopes = if identity.scopes.is_empty() {
        "(none)".to_string()
    } else {
        identity.scopes.join(" ")
    };
    let sa = identity.service_account_id.to_string();
    let prefix = format!("{}…", user.key_prefix());
    ui::print_success(
        "Logged in",
        &[
            ("Key", prefix.as_str()),
            ("Organization", org.as_str()),
            ("Service account", sa.as_str()),
            ("Scopes", scopes.as_str()),
        ],
    );
    if !identity.scopes.iter().any(|s| s == "*") {
        println!(
            "  Scoped key: commands outside these scopes will be refused. \
             See `quome --help` and docs/authentication.md."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_wins_then_file_then_pipe_then_prompt() {
        let file = Some(PathBuf::from("/tmp/k"));
        assert_eq!(
            token_source(&Some("qk_x".into()), &file, true, true),
            TokenSource::Flag
        );
        assert_eq!(
            token_source(&None, &file, true, true),
            TokenSource::File(PathBuf::from("/tmp/k"))
        );
        assert_eq!(token_source(&None, &None, true, true), TokenSource::Stdin);
        // A pipe (stdin not a terminal) reads stdin without being asked to.
        assert_eq!(token_source(&None, &None, false, false), TokenSource::Stdin);
        assert_eq!(token_source(&None, &None, false, true), TokenSource::Prompt);
    }

    #[test]
    fn normalize_strips_paste_noise() {
        assert_eq!(normalize_key("  qk_abc\n").unwrap(), "qk_abc");
        assert_eq!(normalize_key("\"qk_abc\"").unwrap(), "qk_abc");
    }

    #[test]
    fn normalize_rejects_non_keys_with_a_reason() {
        let err = normalize_key("ghp_notquome").unwrap_err().to_string();
        assert!(err.contains("qk_"), "{err}");
        assert!(normalize_key("   ").is_err());
        assert!(normalize_key("qk_ab c")
            .unwrap_err()
            .to_string()
            .contains("whitespace"));
    }
}
