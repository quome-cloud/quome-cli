//! `quome mcp`: the Quome Cloud MCP server on stdio. Nothing but protocol goes to stdout.
use clap::Parser;
use std::path::Path;
use std::time::Duration;

use crate::cloud::api::CloudApi;
use crate::cloud::auth::TokenSource;
use crate::cloud::keys::{platform_store, AgentKey, KeyStore};
use crate::cloud::mcp::{stored_reloader, Offline, QuomeMcp};
use crate::cloud::state::{CloudState, Profile};
use crate::errors::{connection_lost, reconnect_command, QuomeError, Result, CLOCK_HINT};
use std::sync::Arc;

#[derive(Parser)]
pub struct Args {
    /// Profile to use (default: the default profile)
    #[arg(long)]
    profile: Option<String>,
}

/// Longest the start-up check of the connection may delay the MCP handshake.
/// A check that does not finish in time is not a verdict: the server starts.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// What `quome mcp` starts with (made once per process, so its size does not matter).
#[allow(clippy::large_enum_variant)]
pub enum Startup {
    /// A connection that is not known to be bad.
    Ready {
        api: CloudApi,
        profile: Profile,
        name: String,
    },
    /// No working connection: the reason and how to fix it, for the agent and the person.
    Offline(String),
}

/// `quome connect`, naming the profile when it is not the default one.
fn connect_command(profile: Option<&str>) -> String {
    match profile {
        Some(p) if p != crate::commands::connect::DEFAULT_PROFILE => {
            format!("quome connect --profile {p}")
        }
        _ => "quome connect".to_string(),
    }
}

/// True when the saved expiry has passed (an unreadable one is left to the server).
fn expired(expires_at: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(expires_at).is_ok_and(|t| t < chrono::Utc::now())
}

/// Load the stored connection and check it: an expiry that has passed, or a token
/// exchange the identity service refuses within `probe`, means the connection is gone.
/// Network trouble is not a verdict; the server starts and its calls say what fails.
pub async fn startup(
    home: &Path,
    store: &dyn KeyStore,
    profile: Option<&str>,
    probe: Duration,
) -> Startup {
    let restart = crate::errors::RESTART_MCP;
    let state = match CloudState::load_from(home) {
        Ok(s) => s,
        Err(e) => {
            return Startup::Offline(format!(
                "This agent's saved Quome Cloud connection could not be read ({e}). \
                 Run `{}` in a terminal to reconnect, {restart}.",
                connect_command(profile)
            ))
        }
    };
    let p = match state.profile(profile) {
        Ok(p) => p.clone(),
        Err(_) => {
            return Startup::Offline(format!(
                "This agent is not connected to Quome Cloud. \
                 Run `{}` in a terminal to connect, {restart}.",
                connect_command(profile)
            ))
        }
    };
    let name = profile.map_or(state.default.clone(), str::to_string);
    let cmd = reconnect_command(&p.api_url, &name);
    let (key, vrc) = match AgentKey::load_from(store, &name) {
        Ok(k) => k,
        Err(e) => {
            return Startup::Offline(format!(
                "This agent's Quome Cloud key could not be read ({e}). Restart this MCP server \
                 and allow access if this computer asks, or run `{cmd}` in a terminal to \
                 reconnect."
            ))
        }
    };
    if expired(&p.expires_at) {
        return Startup::Offline(connection_lost(&cmd, true));
    }
    let api = CloudApi::new(&p, TokenSource::new(&p, key, vrc));
    // The token is cached, so the first tool call does not pay for this again.
    match tokio::time::timeout(probe, api.access_token()).await {
        Ok(Err(QuomeError::CloudDisconnected)) => {
            return Startup::Offline(connection_lost(&cmd, true))
        }
        Ok(Err(QuomeError::CloudInvalidGrant)) => {
            return Startup::Offline(format!("{} {CLOCK_HINT}", connection_lost(&cmd, true)))
        }
        _ => {}
    }
    Startup::Ready {
        api,
        profile: p,
        name,
    }
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
    let home = dirs::home_dir()
        .ok_or_else(|| QuomeError::CloudState("cannot find home directory".into()))?;
    let (api, profile, name) = match startup(
        &home,
        store.as_ref(),
        args.profile.as_deref(),
        PROBE_TIMEOUT,
    )
    .await
    {
        Startup::Ready { api, profile, name } => (api, profile, name),
        Startup::Offline(message) => {
            // Stay up and speak MCP, so the client shows why rather than a closed
            // connection; say it on stderr too (the client's server log).
            eprintln!("{message}");
            return Offline::new(message)
                .serve_stdio()
                .await
                .map_err(|e| QuomeError::CloudState(format!("MCP server stopped: {e}")));
        }
    };
    crate::errors::set_reconnect_command(&profile.api_url, &name);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::auth::tests::profile;
    use crate::cloud::keys::FileStore;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn save(home: &Path, server: &str, expires_at: &str) -> FileStore {
        let store = FileStore {
            dir: home.join(".quome"),
        };
        let mut p = profile(server);
        p.expires_at = expires_at.into();
        let mut st = CloudState {
            default: "cloud".into(),
            ..Default::default()
        };
        st.profiles.insert("cloud".into(), p);
        st.save_to(home).unwrap();
        AgentKey::generate()
            .store_in(&store, "cloud", "vrc")
            .unwrap();
        store
    }

    async fn issuer(token: ResponseTemplate) -> MockServer {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"token_endpoint": format!("{}/oauth/token", s.uri())}),
            ))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(token)
            .mount(&s)
            .await;
        s
    }

    fn offline(s: Startup) -> String {
        match s {
            Startup::Offline(m) => m,
            Startup::Ready { .. } => panic!("expected Offline"),
        }
    }

    #[tokio::test]
    async fn an_expired_connection_starts_offline_without_asking_the_network() {
        let home = tempfile::tempdir().unwrap();
        let s = issuer(ResponseTemplate::new(500)).await;
        let store = save(home.path(), &s.uri(), "2020-01-01T00:00:00Z");
        let m = offline(startup(home.path(), &store, None, PROBE_TIMEOUT).await);
        assert_eq!(
            m,
            format!(
                "This agent's Quome Cloud connection has expired or was revoked. \
                 Run `quome connect {}` in a terminal to reconnect, then restart this MCP server.",
                s.uri()
            )
        );
        assert!(s.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_refused_grant_at_start_up_starts_offline() {
        let home = tempfile::tempdir().unwrap();
        let s = issuer(
            ResponseTemplate::new(400).set_body_json(serde_json::json!({"error":"invalid_grant"})),
        )
        .await;
        let store = save(home.path(), &s.uri(), "2099-01-01T00:00:00Z");
        let m = offline(startup(home.path(), &store, None, PROBE_TIMEOUT).await);
        assert!(
            m.starts_with("This agent's Quome Cloud connection has expired"),
            "{m}"
        );
        assert!(m.contains(&format!("`quome connect {}`", s.uri())), "{m}");
        assert!(m.ends_with(CLOCK_HINT), "{m}");
    }

    #[tokio::test]
    async fn a_good_or_unreachable_connection_starts_ready() {
        let home = tempfile::tempdir().unwrap();
        let s = issuer(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"access_token":"AT","token_type":"Bearer","expires_in":3600}),
        ))
        .await;
        let store = save(home.path(), &s.uri(), "2099-01-01T00:00:00Z");
        assert!(matches!(
            startup(home.path(), &store, None, PROBE_TIMEOUT).await,
            Startup::Ready { .. }
        ));
        // The identity service is down: not a verdict, the server starts.
        let s = issuer(ResponseTemplate::new(503)).await;
        let store = save(home.path(), &s.uri(), "2099-01-01T00:00:00Z");
        assert!(matches!(
            startup(home.path(), &store, None, PROBE_TIMEOUT).await,
            Startup::Ready { .. }
        ));
    }

    #[tokio::test]
    async fn no_connection_starts_offline_naming_the_profile() {
        let home = tempfile::tempdir().unwrap();
        let store = FileStore {
            dir: home.path().join(".quome"),
        };
        let m = offline(startup(home.path(), &store, None, PROBE_TIMEOUT).await);
        assert_eq!(
            m,
            "This agent is not connected to Quome Cloud. Run `quome connect` in a terminal \
             to connect, then restart this MCP server."
        );
        let m = offline(startup(home.path(), &store, Some("work"), PROBE_TIMEOUT).await);
        assert!(m.contains("`quome connect --profile work`"), "{m}");
        // A profile whose key is gone.
        let s = issuer(ResponseTemplate::new(500)).await;
        save(home.path(), &s.uri(), "2099-01-01T00:00:00Z");
        let empty = FileStore {
            dir: home.path().join("elsewhere"),
        };
        let m = offline(startup(home.path(), &empty, None, PROBE_TIMEOUT).await);
        assert!(
            m.starts_with("This agent's Quome Cloud key could not be read"),
            "{m}"
        );
        assert!(m.contains(&format!("`quome connect {}`", s.uri())), "{m}");
    }
}
