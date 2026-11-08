//! `quome git-credential`: a git credential helper for the org's Git repos.
use clap::Parser;
use std::path::Path;
use std::sync::Arc;

use crate::cloud::api::{CloudApi, GitToken};
use crate::cloud::auth::TokenSource;
use crate::cloud::credentials::{cred_hint, CredentialsClient};
use crate::cloud::git::{egress_host, git_host, GitClient};
use crate::cloud::keys::{platform_store, AgentKey, KeyStore};
use crate::cloud::state::{CloudState, Profile};
use crate::errors::{QuomeError, Result};

#[derive(Parser)]
pub struct Args {
    /// get, store or erase (git passes this)
    action: String,
    /// Profile to use instead of matching the host (default: match by host)
    #[arg(long)]
    profile: Option<String>,
}

/// What git told the helper about the repo it wants credentials for.
#[derive(Debug, Default, PartialEq)]
pub struct Request {
    pub protocol: String,
    pub host: String,
    pub path: Option<String>,
}

/// Read git's `key=value` lines up to the first blank line.
pub fn parse_request(input: &str) -> Request {
    let mut r = Request::default();
    for line in input.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            break;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match k {
            "protocol" => r.protocol = v.to_string(),
            "host" => r.host = v.to_string(),
            "path" => r.path = Some(v.to_string()).filter(|p| !p.is_empty()),
            "url" => {
                if let Some((proto, rest)) = v.split_once("://") {
                    r.protocol = proto.to_string();
                    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
                    r.host = host.to_string();
                    r.path = Some(path.to_string()).filter(|p| !p.is_empty());
                }
            }
            _ => {}
        }
    }
    r
}

/// Which service answers for a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredTarget {
    /// `git.<apps_domain>`: the org's own Git repos.
    OrgGit,
    /// `egress.<apps_domain>`: outside repos through the credentials service.
    Egress,
}

/// The profile that serves `req`, with its name and which service the host
/// belongs to. `None` when no connection serves that host.
pub fn pick_profile<'a>(
    state: &'a CloudState,
    req: &Request,
    forced: Option<&str>,
) -> std::result::Result<Option<(String, &'a Profile, CredTarget)>, QuomeError> {
    let only = match forced {
        Some(name) => Some((name, state.profile(Some(name))?)),
        None => None,
    };
    if req.protocol != "https" {
        return Ok(None);
    }
    let host = req.host.to_lowercase();
    let host = host.strip_suffix(":443").unwrap_or(&host);
    let org = req
        .path
        .as_deref()
        .map(|p| p.trim_start_matches('/').split('/').next().unwrap_or(""));
    let target_of = |p: &Profile| {
        if git_host(p).to_lowercase() == host {
            Some(CredTarget::OrgGit)
        } else if egress_host(p).to_lowercase() == host {
            Some(CredTarget::Egress)
        } else {
            None
        }
    };
    let found: Vec<(&String, &Profile, CredTarget)> = state
        .profiles
        .iter()
        .filter(|(n, _)| only.is_none_or(|(name, _)| n.as_str() == name))
        .filter_map(|(n, p)| target_of(p).map(|t| (n, p, t)))
        .filter(|(_, p, _)| org.is_none_or(|o| p.org_slug == o))
        .collect();
    match found.as_slice() {
        [] => Ok(None),
        [(n, p, t)] => Ok(Some(((*n).clone(), p, *t))),
        many => many
            .iter()
            .find(|(n, _, _)| **n == state.default)
            .map(|(n, p, t)| Some(((*n).clone(), *p, *t)))
            .ok_or_else(|| {
                QuomeError::CloudState(format!(
                    "Several Quome Cloud connections serve {}; run `git config --global \
                     credential.https://{}.useHttpPath true` so git sends the org.",
                    req.host, req.host
                ))
            }),
    }
}

/// git's answer for `token`, valid until `now_unix + expires_in`.
pub fn answer(token: &GitToken, now_unix: u64) -> String {
    format!(
        "username=quome\npassword={}\npassword_expiry_utc={}\n",
        token.access_token,
        now_unix + token.expires_in
    )
}

/// The text for git's stdout, or `None` for nothing. `now_unix` reads the
/// clock (Unix seconds).
pub async fn run(
    home: &Path,
    store: &dyn KeyStore,
    args: &Args,
    input: &str,
    now_unix: &dyn Fn() -> u64,
) -> Result<Option<String>> {
    if args.action != "get" {
        return Ok(None);
    }
    let state = CloudState::load_from(home)?;
    let req = parse_request(input);
    let Some((name, profile, target)) = pick_profile(&state, &req, args.profile.as_deref())? else {
        return Ok(None);
    };
    let p = Arc::new(profile.clone());
    let (key, vrc) = AgentKey::load_from(store, &name)?;
    let api = Arc::new(CloudApi::new(&p, TokenSource::new(&p, key, vrc)));
    let token = match target {
        CredTarget::OrgGit => {
            let git = GitClient::new(Arc::new(move || (api.clone(), p.clone())));
            git.credential_token()
                .await
                .map_err(|e| QuomeError::Usage(e.message()))?
        }
        CredTarget::Egress => {
            let creds = CredentialsClient::new(Arc::new(move || api.clone()));
            let t = creds
                .git_token()
                .await
                .map_err(|e| QuomeError::Usage(cred_hint(e)))?;
            GitToken {
                access_token: t.access_token,
                expires_in: t.expires_in,
            }
        }
    };
    // Read the clock after the mint, which may have waited for the server.
    Ok(Some(answer(&token, now_unix())))
}

pub async fn execute(args: Args) -> Result<()> {
    use std::io::Read;
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let home = dirs::home_dir()
        .ok_or_else(|| QuomeError::CloudState("cannot find home directory".into()))?;
    let store = platform_store()?;
    let now = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    };
    if let Some(out) = run(&home, store.as_ref(), &args, &input, &now).await? {
        print!("{out}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::auth::tests::profile;
    use crate::cloud::keys::FileStore;
    use serde_json::json;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ORG: &str = "11111111-1111-1111-1111-111111111111";

    fn req(protocol: &str, host: &str, path: Option<&str>) -> Request {
        Request {
            protocol: protocol.into(),
            host: host.into(),
            path: path.map(Into::into),
        }
    }

    fn two_profiles(default: &str) -> CloudState {
        let mut st = CloudState {
            default: default.into(),
            ..Default::default()
        };
        let mut work = profile("http://127.0.0.1:9");
        work.org_slug = "acme".into();
        let mut home = profile("http://127.0.0.1:9");
        home.org_slug = "beta".into();
        st.profiles.insert("work".into(), work);
        st.profiles.insert("home".into(), home);
        st
    }

    #[test]
    fn parse_request_reads_until_blank_line() {
        let r = parse_request(
            "protocol=https\nhost=git.apps.example\npath=acme/api.git\ncapability[]=authtype\n\nignored=1\n",
        );
        assert_eq!(r, req("https", "git.apps.example", Some("acme/api.git")));
    }

    #[test]
    fn parse_request_splits_url() {
        let r = parse_request("url=https://git.apps.example/acme/api.git\n\n");
        assert_eq!(r, req("https", "git.apps.example", Some("acme/api.git")));
    }

    #[test]
    fn pick_profile_by_host_and_path() {
        let st = two_profiles("work");
        let (name, _, _) = pick_profile(
            &st,
            &req("https", "git.apps.example", Some("beta/x.git")),
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(name, "home");
        let (name, _, _) = pick_profile(&st, &req("https", "Git.Apps.Example:443", None), None)
            .unwrap()
            .unwrap();
        assert_eq!(name, "work");
        assert!(pick_profile(&st, &req("https", "github.com", None), None)
            .unwrap()
            .is_none());
        assert!(
            pick_profile(&st, &req("http", "git.apps.example", None), None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn pick_profile_forced_still_checks_host_and_org() {
        let st = two_profiles("work");
        let forced = Some("work");
        assert!(pick_profile(&st, &req("https", "github.com", None), forced)
            .unwrap()
            .is_none());
        assert!(pick_profile(
            &st,
            &req("https", "git.apps.example", Some("beta/x.git")),
            forced
        )
        .unwrap()
        .is_none());
        assert!(
            pick_profile(&st, &req("http", "git.apps.example", None), forced)
                .unwrap()
                .is_none()
        );
        let (name, _, _) = pick_profile(
            &st,
            &req("https", "git.apps.example", Some("acme/x.git")),
            forced,
        )
        .unwrap()
        .unwrap();
        assert_eq!(name, "work");
        assert!(pick_profile(&st, &req("https", "git.apps.example", None), Some("nope")).is_err());
    }

    #[test]
    fn pick_profile_ambiguous_host() {
        let st = two_profiles("other");
        let e = pick_profile(&st, &req("https", "git.apps.example", None), None).unwrap_err();
        assert!(e.to_string().contains("useHttpPath"), "{e}");
    }

    #[test]
    fn answer_format() {
        assert_eq!(
            answer(
                &GitToken {
                    access_token: "G1".into(),
                    expires_in: 300
                },
                1_000
            ),
            "username=quome\npassword=G1\npassword_expiry_utc=1300\n"
        );
    }

    struct Env {
        home: tempfile::TempDir,
        store: FileStore,
        server: MockServer,
    }

    async fn env() -> Env {
        let home = tempfile::tempdir().unwrap();
        let store = FileStore {
            dir: home.path().to_path_buf(),
        };
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({"token_endpoint": format!("{}/oauth/token", server.uri())}),
                ),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"access_token":"AT","token_type":"Bearer","expires_in":3600}),
            ))
            .mount(&server)
            .await;
        let mut st = CloudState {
            default: "cloud".into(),
            ..Default::default()
        };
        st.profiles.insert("cloud".into(), profile(&server.uri()));
        st.save_to(home.path()).unwrap();
        crate::cloud::keys::AgentKey::generate()
            .store_in(&store, "cloud", "vrc")
            .unwrap();
        Env {
            home,
            store,
            server,
        }
    }

    fn args(action: &str) -> Args {
        Args {
            action: action.into(),
            profile: None,
        }
    }

    const INPUT: &str = "protocol=https\nhost=git.apps.example\n\n";

    fn broker() -> String {
        format!("/api/v1/orgs/{ORG}/git/token")
    }

    fn token(t: &str) -> ResponseTemplate {
        ResponseTemplate::new(200)
            .set_body_json(json!({"access_token": t, "token_type": "Bearer", "expires_in": 300}))
    }

    async fn get(e: &Env, input: &str) -> Result<Option<String>> {
        run(e.home.path(), &e.store, &args("get"), input, &|| 1_000).await
    }

    #[tokio::test]
    async fn get_prints_token() {
        let e = env().await;
        Mock::given(method("POST"))
            .and(path(broker()))
            .and(body_json(json!({"scope": ["repos_read", "repos_write"]})))
            .respond_with(token("G1"))
            .expect(1)
            .mount(&e.server)
            .await;
        let out = get(&e, INPUT).await.unwrap();
        assert_eq!(
            out.as_deref(),
            Some("username=quome\npassword=G1\npassword_expiry_utc=1300\n")
        );
    }

    /// Answers with a token and moves the shared clock to 5000.
    struct MintAt(Arc<std::sync::atomic::AtomicU64>);

    impl wiremock::Respond for MintAt {
        fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
            self.0.store(5_000, std::sync::atomic::Ordering::SeqCst);
            token("G1")
        }
    }

    #[tokio::test]
    async fn expiry_counts_from_after_the_mint() {
        let e = env().await;
        let clock = Arc::new(std::sync::atomic::AtomicU64::new(1_000));
        Mock::given(method("POST"))
            .and(path(broker()))
            .respond_with(MintAt(clock.clone()))
            .expect(1)
            .mount(&e.server)
            .await;
        let now = || clock.load(std::sync::atomic::Ordering::SeqCst);
        let out = run(e.home.path(), &e.store, &args("get"), INPUT, &now)
            .await
            .unwrap()
            .unwrap();
        assert!(out.ends_with("password_expiry_utc=5300\n"), "{out}");
    }

    #[tokio::test]
    async fn get_falls_back_to_read_on_capability_missing() {
        let e = env().await;
        Mock::given(method("POST"))
            .and(path(broker()))
            .and(body_json(json!({"scope": ["repos_read", "repos_write"]})))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_json(json!({"error": "forbidden", "reason": "capability_missing"})),
            )
            .expect(1)
            .mount(&e.server)
            .await;
        Mock::given(method("POST"))
            .and(path(broker()))
            .and(body_json(json!({"scope": ["repos_read"]})))
            .respond_with(token("R1"))
            .expect(1)
            .mount(&e.server)
            .await;
        let out = get(&e, INPUT).await.unwrap().unwrap();
        assert!(out.contains("password=R1\n"), "{out}");
    }

    #[tokio::test]
    async fn get_waits_through_git_starting() {
        let e = env().await;
        Mock::given(method("POST"))
            .and(path(broker()))
            .respond_with(
                ResponseTemplate::new(503)
                    .insert_header("retry-after", "1")
                    .set_body_json(json!({"error": "unavailable", "reason": "git_starting"})),
            )
            .up_to_n_times(1)
            .expect(1)
            .mount(&e.server)
            .await;
        Mock::given(method("POST"))
            .and(path(broker()))
            .respond_with(token("G2"))
            .expect(1)
            .mount(&e.server)
            .await;
        let out = get(&e, INPUT).await.unwrap().unwrap();
        assert!(out.contains("password=G2\n"), "{out}");
    }

    #[tokio::test]
    async fn error_prints_nothing() {
        let e = env().await;
        Mock::given(method("POST"))
            .and(path(broker()))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_json(json!({"error": "forbidden", "reason": "not_a_member"})),
            )
            .mount(&e.server)
            .await;
        assert!(get(&e, INPUT).await.is_err());
    }

    #[tokio::test]
    async fn store_and_erase_do_nothing() {
        let e = env().await;
        for action in ["store", "erase", "bogus"] {
            let out = run(e.home.path(), &e.store, &args(action), INPUT, &|| 1_000)
                .await
                .unwrap();
            assert!(out.is_none(), "{action}");
        }
    }

    #[tokio::test]
    async fn unknown_host_prints_nothing() {
        let e = env().await;
        Mock::given(method("POST"))
            .and(path(broker()))
            .respond_with(token("G1"))
            .expect(0)
            .mount(&e.server)
            .await;
        let out = get(&e, "protocol=https\nhost=github.com\n\n")
            .await
            .unwrap();
        assert!(out.is_none());
    }

    const EGRESS: &str =
        "protocol=https\nhost=egress.apps.example\npath=acme/git/github.com/o/r.git\n\n";

    fn cred_broker() -> String {
        format!("/api/v1/orgs/{ORG}/credentials/token")
    }

    #[test]
    fn egress_host_picks_profile_by_slug() {
        let st = two_profiles("work");
        let (name, _, target) = pick_profile(
            &st,
            &req(
                "https",
                "egress.apps.example",
                Some("beta/git/github.com/o/r.git"),
            ),
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!((name.as_str(), target), ("home", CredTarget::Egress));
        let (_, _, target) = pick_profile(
            &st,
            &req("https", "git.apps.example", Some("beta/x.git")),
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(target, CredTarget::OrgGit);
        let st = two_profiles("other");
        let e = pick_profile(&st, &req("https", "egress.apps.example", None), None).unwrap_err();
        assert!(e.to_string().contains("useHttpPath"), "{e}");
    }

    #[test]
    fn lookalike_hosts_and_foreign_slugs_are_not_served() {
        let st = two_profiles("work");
        let path = Some("acme/git/github.com/o/r.git");
        for host in [
            "egress.apps.example.attacker.example",
            "xegress.apps.example",
            "egress.apps.example.",
            "egress.apps.examplex",
            "evil.egress.apps.example",
            "apps.example",
            "git.apps.example.attacker.example",
        ] {
            assert!(
                pick_profile(&st, &req("https", host, path), None)
                    .unwrap()
                    .is_none(),
                "{host}"
            );
        }
        assert!(pick_profile(
            &st,
            &req(
                "https",
                "egress.apps.example",
                Some("other/git/github.com/o/r.git")
            ),
            None
        )
        .unwrap()
        .is_none());
        assert!(
            pick_profile(&st, &req("http", "egress.apps.example", path), None)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn egress_get_prints_credentials_token() {
        let e = env().await;
        Mock::given(method("POST"))
            .and(path(cred_broker()))
            .and(body_json(json!({"scope": ["credentials_use"]})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "C1", "token_type": "Bearer", "expires_in": 300,
                "url": "https://svc.example"
            })))
            .expect(1)
            .mount(&e.server)
            .await;
        Mock::given(method("POST"))
            .and(path(broker()))
            .respond_with(token("G1"))
            .expect(0)
            .mount(&e.server)
            .await;
        let out = get(&e, EGRESS).await.unwrap();
        assert_eq!(
            out.as_deref(),
            Some("username=quome\npassword=C1\npassword_expiry_utc=1300\n")
        );
    }

    #[tokio::test]
    async fn egress_unknown_org_prints_nothing_and_calls_nothing() {
        let e = env().await;
        Mock::given(method("POST"))
            .and(path(cred_broker()))
            .respond_with(token("C1"))
            .expect(0)
            .mount(&e.server)
            .await;
        for input in [
            "protocol=https\nhost=egress.apps.example\npath=other/git/github.com/o/r.git\n\n",
            "protocol=https\nhost=egress.apps.example.attacker.example\npath=acme/git/github.com/o/r.git\n\n",
            "protocol=http\nhost=egress.apps.example\npath=acme/git/github.com/o/r.git\n\n",
        ] {
            assert!(get(&e, input).await.unwrap().is_none(), "{input}");
        }
    }

    #[tokio::test]
    async fn egress_capability_missing_prints_nothing_and_hints_on_stderr() {
        let e = env().await;
        Mock::given(method("POST"))
            .and(path(cred_broker()))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_json(json!({"error": "forbidden", "reason": "capability_missing"})),
            )
            .expect(1)
            .mount(&e.server)
            .await;
        let err = get(&e, EGRESS).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("Run `quome connect` again and allow Credentials access."),
            "{err}"
        );
    }
}
