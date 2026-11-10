//! `quome mcp`: the Quome Cloud MCP server on stdio. Nothing but protocol goes to stdout.
use clap::Parser;

use crate::cloud::api::CloudApi;
use crate::cloud::auth::TokenSource;
use crate::cloud::keys::{platform_store, AgentKey};
use crate::cloud::mcp::{stored_reloader, QuomeMcp};
use crate::cloud::state::CloudState;
use crate::errors::{QuomeError, Result};
use std::sync::Arc;

#[derive(Parser)]
pub struct Args {
    /// Profile to use (default: the default profile)
    #[arg(long)]
    profile: Option<String>,
}

pub async fn execute(args: Args) -> Result<()> {
    let store = match platform_store() {
        Ok(s) => s,
        Err(QuomeError::CloudUnsupported) => {
            eprintln!("{}", QuomeError::CloudUnsupported);
            std::process::exit(2);
        }
        Err(e) => return Err(e),
    };
    let state = CloudState::load()?;
    let profile = match state.profile(args.profile.as_deref()) {
        Ok(p) => p.clone(),
        Err(_) if state.profiles.is_empty() => {
            return Err(QuomeError::CloudState(
                crate::commands::cloud::NOT_CONNECTED.into(),
            ))
        }
        Err(e) => return Err(e),
    };
    let name = args.profile.unwrap_or(state.default);
    let (key, vrc) = AgentKey::load_from(store.as_ref(), &name)?;
    let api = CloudApi::new(&profile, TokenSource::new(&profile, key, vrc));
    let home = dirs::home_dir()
        .ok_or_else(|| QuomeError::CloudState("cannot find home directory".into()))?;
    // Refresh the agent skills in the background over the server's own
    // connection (one token source): it must not delay the MCP handshake and
    // prints nothing (stdout is the protocol). Failures are dropped.
    let api = Arc::new(api);
    drop(crate::commands::skills::spawn_background_sync(
        api.clone(),
        home.clone(),
        name.clone(),
    ));
    let reload = stored_reloader(home, Arc::from(store), name);
    QuomeMcp::new_shared(api, profile)
        .with_reloader(reload)
        .with_latest_release(crate::auto_update::latest_release)
        .serve_stdio()
        .await
        .map_err(|e| QuomeError::CloudState(format!("MCP server stopped: {e}")))
}
