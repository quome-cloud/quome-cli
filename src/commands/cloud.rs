//! `quome cloud status|disconnect`: inspect or drop the local Quome Cloud connection.
use clap::{Parser, Subcommand};
use std::path::Path;

use crate::cloud::api::CloudApi;
use crate::cloud::auth::TokenSource;
use crate::cloud::keys::{platform_store, AgentKey, KeyStore};
use crate::cloud::mcp::describe;
use crate::cloud::state::{CloudState, Profile};
use crate::errors::{QuomeError, Result};

pub const NOT_CONNECTED: &str = "Not connected to Quome Cloud — run `quome connect`";

#[derive(Subcommand)]
pub enum CloudCommands {
    /// Show this computer's Quome Cloud connection and check it is still valid
    Status(StatusArgs),
    /// Forget the local Quome Cloud connection
    Disconnect(DisconnectArgs),
}

#[derive(Parser)]
pub struct StatusArgs {
    /// Profile to show (default: the default profile)
    #[arg(long)]
    profile: Option<String>,
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct DisconnectArgs {
    /// Profile to remove (default: the default profile)
    #[arg(long)]
    profile: Option<String>,
}

fn expiry_date(expires_at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(expires_at)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|_| expires_at.to_string())
}

/// Text to print and the exit code. `Err` is for state we cannot read at all.
pub async fn status(
    home: &Path,
    profile: Option<&str>,
    json: bool,
    store: &dyn KeyStore,
) -> Result<(String, i32)> {
    let state = CloudState::load_from(home)?;
    if state.profiles.is_empty() {
        let out = if json {
            serde_json::to_string_pretty(&serde_json::json!({"connected": false}))?
        } else {
            NOT_CONNECTED.to_string()
        };
        return Ok((out, 1));
    }
    let name = profile.unwrap_or(&state.default).to_string();
    let p: Profile = state.profile(profile)?.clone();

    // Live check: any failure is reported, never raised.
    let live: std::result::Result<String, String> = async {
        let (key, vrc) = AgentKey::load_from(store, &name).map_err(|e| describe(&e))?;
        let api = CloudApi::new(&p, TokenSource::new(&p, key, vrc));
        let me = api.me().await.map_err(|e| describe(&e))?;
        Ok(me
            .orgs
            .iter()
            .find(|o| o.id == p.org_id)
            .map(|o| o.slug.clone())
            .unwrap_or_else(|| p.org_slug.clone()))
    }
    .await;

    let ok = live.is_ok();
    let out = if json {
        serde_json::to_string_pretty(&serde_json::json!({
            "connected": ok,
            "profile": name,
            "agent_name": p.agent_name,
            "agent_principal": p.agent_principal,
            "org_id": p.org_id,
            "org_slug": p.org_slug,
            "expires_at": p.expires_at,
            "api_url": p.api_url,
            "error": live.as_ref().err(),
        }))?
    } else {
        let check = match &live {
            Ok(_) => "ok".to_string(),
            Err(e) => format!("failed — {e}"),
        };
        format!(
            "Quome Cloud\n  Profile: {name}\n  Agent:   {} ({})\n  Org:     {}\n  Expires: {}\n  Check:   {check}",
            p.agent_name,
            p.agent_principal,
            p.org_slug,
            expiry_date(&p.expires_at)
        )
    };
    Ok((out, if ok { 0 } else { 1 }))
}

/// Delete the profile (and its key from `store`); returns the text to print.
pub fn disconnect(home: &Path, profile: Option<&str>, store: &dyn KeyStore) -> Result<String> {
    let mut state = CloudState::load_from(home)?;
    if state.profiles.is_empty() {
        return Ok(NOT_CONNECTED.to_string());
    }
    let name = profile.unwrap_or(&state.default).to_string();
    let p = state.profile(Some(&name))?.clone();
    store.delete(&name)?;
    state.profiles.remove(&name);
    if state.default == name {
        state.default = state.profiles.keys().next().cloned().unwrap_or_default();
    }
    state.save_to(home)?;
    Ok(format!(
        "Disconnected profile \"{name}\" from this computer.\n\
         To revoke this agent's access in Quome Cloud, open {}/dashboard",
        p.console_url.trim_end_matches('/')
    ))
}

pub async fn execute(command: CloudCommands) -> Result<()> {
    let unsupported = |e: QuomeError| -> QuomeError {
        if matches!(e, QuomeError::CloudUnsupported) {
            eprintln!("{e}");
            std::process::exit(2);
        }
        e
    };
    let home = dirs::home_dir()
        .ok_or_else(|| QuomeError::CloudState("cannot find home directory".into()))?;
    match command {
        CloudCommands::Status(a) => {
            let store = platform_store().map_err(unsupported)?;
            let (out, code) = status(&home, a.profile.as_deref(), a.json, store.as_ref()).await?;
            println!("{out}");
            if code != 0 {
                std::process::exit(code);
            }
        }
        CloudCommands::Disconnect(a) => {
            let store = platform_store().map_err(unsupported)?;
            println!(
                "{}",
                disconnect(&home, a.profile.as_deref(), store.as_ref())?
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::auth::tests::profile;
    use crate::cloud::keys::FileStore;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn save(home: &Path, server: &str, key: &FileStore) {
        let mut st = CloudState {
            default: "cloud".into(),
            ..Default::default()
        };
        st.profiles.insert("cloud".into(), profile(server));
        st.save_to(home).unwrap();
        AgentKey::generate().store_in(key, "cloud", "vrc").unwrap();
    }

    async fn api_server(me: ResponseTemplate) -> MockServer {
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
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token":"AT","token_type":"Bearer","expires_in":3600})))
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/me"))
            .respond_with(me)
            .mount(&s)
            .await;
        s
    }

    #[tokio::test]
    async fn status_without_cloud_json_says_not_connected() {
        let home = tempfile::tempdir().unwrap();
        let store = FileStore {
            dir: home.path().to_path_buf(),
        };
        let (out, code) = status(home.path(), None, false, &store).await.unwrap();
        assert_eq!(out, "Not connected to Quome Cloud — run `quome connect`");
        assert_eq!(code, 1);
    }

    #[tokio::test]
    async fn status_reports_live_check() {
        let home = tempfile::tempdir().unwrap();
        let store = FileStore {
            dir: home.path().to_path_buf(),
        };
        let s = api_server(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id":null,"email":null,"display_name":null,
            "orgs":[{"id":"11111111-1111-1111-1111-111111111111","slug":"acme","name":"Acme","role":"admin"}],
            "principal_kind":"agent","apps_domain":"apps.example","api_origin":"x"})))
        .await;
        save(home.path(), &s.uri(), &store);
        let (out, code) = status(home.path(), None, false, &store).await.unwrap();
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("acme") && out.contains("2030-01-01") && out.contains("Check:   ok"));
        let (j, _) = status(home.path(), Some("cloud"), true, &store)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["connected"], true);
        assert_eq!(v["org_slug"], "acme");
    }

    #[tokio::test]
    async fn status_revoked_says_disconnected() {
        let home = tempfile::tempdir().unwrap();
        let store = FileStore {
            dir: home.path().to_path_buf(),
        };
        let s = api_server(ResponseTemplate::new(401).set_body_json(
            serde_json::json!({"detail":"unauthorized","reason":"delegation_revoked"}),
        ))
        .await;
        save(home.path(), &s.uri(), &store);
        let (out, code) = status(home.path(), None, false, &store).await.unwrap();
        assert_eq!(code, 1);
        assert!(
            out.contains(&QuomeError::CloudDisconnected.to_string()),
            "{out}"
        );
    }

    #[test]
    fn disconnect_removes_profile_and_key_and_prints_console() {
        let home = tempfile::tempdir().unwrap();
        let store = FileStore {
            dir: home.path().to_path_buf(),
        };
        save(home.path(), "http://127.0.0.1:9", &store);
        let key_file = home.path().join("cloud-cloud.key");
        assert!(key_file.exists(), "save() stored a key");
        assert!(store.get("cloud").is_ok());
        let out = disconnect(home.path(), None, &store).unwrap();
        assert!(store.get("cloud").is_err(), "key is gone from the store");
        assert!(out.contains("http://127.0.0.1:9/dashboard"), "{out}");
        assert!(!key_file.exists());
        let st = CloudState::load_from(home.path()).unwrap();
        assert!(st.profiles.is_empty());
        assert!(st.default.is_empty());
    }
}
