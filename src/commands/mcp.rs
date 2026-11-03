//! `quome mcp`: the Quome Cloud MCP server on stdio. Nothing but protocol goes to stdout.
use clap::Parser;

use crate::cloud::api::CloudApi;
use crate::cloud::auth::TokenSource;
use crate::cloud::keys::{platform_store, AgentKey};
use crate::cloud::mcp::QuomeMcp;
use crate::cloud::state::CloudState;
use crate::errors::{QuomeError, Result};

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
                "Not connected to Quome Cloud — run `quome connect`".into(),
            ))
        }
        Err(e) => return Err(e),
    };
    let name = args.profile.unwrap_or(state.default);
    let (key, vrc) = AgentKey::load_from(store.as_ref(), &name)?;
    let api = CloudApi::new(&profile, TokenSource::new(&profile, key, vrc));
    QuomeMcp::new(api, profile)
        .serve_stdio()
        .await
        .map_err(|e| QuomeError::CloudState(format!("MCP server stopped: {e}")))
}
