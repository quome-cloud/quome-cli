//! `quome fn …` — functions-as-a-service on Quome sandboxes.
//!
//! The work (QRust -> WebAssembly compilation, the warm sandbox pool, the HTTP gateway) lives in
//! `qufunction`, a separate binary. This command is a thin passthrough, like `quome host`: it
//! finds `qufunction`, hands it the credentials of the login this CLI holds, and replaces itself
//! with it, so stdio, signals (Ctrl-C on `quome fn serve`) and the exit code are qufunction's own.
//!
//! What it forwards, unless already set in the environment:
//!
//! - `QUOME_API_KEY`  the stored login (or `QUOME_TOKEN`)
//! - `QUOME_BASE_URL` the API URL this CLI talks to
//! - `QUOME_ORG_ID`   the linked org (or `QUOME_ORG`, or the key's org)

use clap::Args;
use std::path::PathBuf;
use std::process::Command;

use crate::config::Config;
use crate::errors::{QuomeError, Result};
use crate::settings::Settings;

const BINARY_NAME: &str = "qufunction";
const INSTALL_HINT: &str = "cargo install --git https://github.com/quome-cloud/qufunction";

#[derive(Args)]
#[command(disable_help_flag = true)]
pub struct FnArgs {
    /// Arguments for qufunction (deploy, invoke, ls, pool, serve, ...; `quome fn --help` for all)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

pub async fn execute(args: FnArgs) -> Result<()> {
    let bin = find_binary().ok_or_else(|| {
        QuomeError::Usage(format!(
            "`quome fn` needs the qufunction binary, which was not found on PATH.\n  Install it: {INSTALL_HINT}\n  \
             (or set QUFUNCTION_BIN=/path/to/qufunction)"
        ))
    })?;
    let config = Config::load().unwrap_or_default();
    let settings = Settings::load().unwrap_or_default();
    let mut cmd = Command::new(&bin);
    cmd.args(&args.args);
    for (key, value) in forwarded_env(&config, &settings, |k| std::env::var(k).is_ok()) {
        cmd.env(key, value);
    }
    run(cmd, &bin)
}

/// The variables to set for qufunction: only those the user has not set themselves.
fn forwarded_env(
    config: &Config,
    settings: &Settings,
    is_set: impl Fn(&str) -> bool,
) -> Vec<(&'static str, String)> {
    let mut env = Vec::new();
    if !is_set("QUOME_API_KEY") {
        if let Some(token) = config.get_token_string() {
            env.push(("QUOME_API_KEY", token));
        }
    }
    if !is_set("QUOME_BASE_URL") {
        env.push(("QUOME_BASE_URL", settings.get_api_url()));
    }
    if !is_set("QUOME_ORG_ID") {
        if let Ok(Some(org)) = config.get_linked_org_id() {
            env.push(("QUOME_ORG_ID", org.to_string()));
        }
    }
    env
}

fn find_binary() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("QUFUNCTION_BIN") {
        return Some(PathBuf::from(p));
    }
    let on_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(BINARY_NAME))
            .find(|p| p.is_file())
    });
    on_path.or_else(|| {
        let home = dirs::home_dir()?;
        [".cargo/bin", ".quome/bin"]
            .iter()
            .map(|d| home.join(d).join(BINARY_NAME))
            .find(|p| p.is_file())
    })
}

#[cfg(unix)]
fn run(mut cmd: Command, bin: &std::path::Path) -> Result<()> {
    use std::os::unix::process::CommandExt;
    // Only returns on failure to exec.
    let err = cmd.exec();
    Err(QuomeError::ApiError(format!(
        "could not run {}: {err}",
        bin.display()
    )))
}

#[cfg(not(unix))]
fn run(mut cmd: Command, bin: &std::path::Path) -> Result<()> {
    let status = cmd
        .status()
        .map_err(|e| QuomeError::ApiError(format!("could not run {}: {e}", bin.display())))?;
    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwards_only_what_the_user_has_not_set() {
        let config = Config::default();
        let settings = Settings::default();
        let env = forwarded_env(&config, &settings, |_| false);
        let keys: Vec<&str> = env.iter().map(|(k, _)| *k).collect();
        // No stored login and no linked org in a default config: only the URL is forwarded.
        assert!(keys.contains(&"QUOME_BASE_URL"));
        assert!(env
            .iter()
            .any(|(k, v)| *k == "QUOME_BASE_URL" && *v == settings.get_api_url()));

        let env = forwarded_env(&config, &settings, |_| true);
        assert!(env.is_empty(), "explicit environment always wins");
    }
}
