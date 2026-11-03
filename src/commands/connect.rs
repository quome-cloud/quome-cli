//! `quome connect`: delegate this computer's agent key to Quome Cloud through
//! the browser, then save the profile and register the MCP server.
use clap::Parser;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use rand_core::RngCore;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::cloud::api::CloudApi;
use crate::cloud::auth::TokenSource;
use crate::cloud::keys::{platform_store, AgentKey, KeyStore};
use crate::cloud::loopback::{self, Callback, Listener};
use crate::cloud::state::{CloudState, Profile};
use crate::errors::{QuomeError, Result};

pub const DEFAULT_API_URL: &str = "https://api.quome.cloud";
pub const DEFAULT_PROFILE: &str = "cloud";
/// quauth's limit on an agent's display name.
pub const MAX_NAME: usize = 64;
const RETRY_INTERVAL: Duration = Duration::from_secs(2);
const RETRY_TOTAL: Duration = Duration::from_secs(60);

#[derive(Parser)]
pub struct Args {
    /// Quome Cloud API URL
    #[arg(default_value = DEFAULT_API_URL)]
    api_url: String,
    /// Name shown for this agent in the console (default: "Claude Code on <hostname>")
    #[arg(long)]
    name: Option<String>,
    /// Profile to save the connection under
    #[arg(long)]
    profile: Option<String>,
    /// Console URL (default: the API URL with `api.` replaced by `console.`)
    #[arg(long)]
    console: Option<String>,
    /// Issuer URL (default: the API URL with `api.` replaced by `auth.`)
    #[arg(long)]
    issuer: Option<String>,
    /// Do not register the MCP server with Claude Code
    #[arg(long)]
    no_mcp: bool,
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

/// What happened to the MCP registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeResult {
    /// `claude` is not on PATH.
    NotFound,
    /// `claude` ran; true when it exited successfully.
    Ran(bool),
}

pub trait ClaudeRunner {
    fn run(&self, args: &[String]) -> ClaudeResult;
}

struct RealClaude;

impl ClaudeRunner for RealClaude {
    fn run(&self, args: &[String]) -> ClaudeResult {
        match std::process::Command::new("claude").args(args).status() {
            Ok(s) => ClaudeResult::Ran(s.success()),
            Err(_) => ClaudeResult::NotFound,
        }
    }
}

/// Everything `run` touches outside this process, injected for tests.
pub struct Deps<'a> {
    pub home: PathBuf,
    pub store: &'a dyn KeyStore,
    pub open: &'a dyn Fn(&str) -> std::io::Result<()>,
    pub claude: &'a dyn ClaudeRunner,
    /// Progress lines (stdout, or stderr under `--json`).
    pub say: &'a dyn Fn(&str),
    pub hostname: String,
    pub listen_timeout: Duration,
    pub retry_interval: Duration,
    pub retry_total: Duration,
}

pub struct Options {
    pub api_url: String,
    pub name: Option<String>,
    pub profile: String,
    pub profile_given: bool,
    pub console: Option<String>,
    pub issuer: Option<String>,
    pub no_mcp: bool,
}

#[derive(Debug)]
pub struct Outcome {
    pub profile_name: String,
    pub profile: Profile,
    pub mcp: &'static str,
}

/// Replace a leading `api.` in the URL's host with `to.`; other hosts are kept.
fn swap_host(api_url: &str, to: &str) -> String {
    let trimmed = api_url.trim_end_matches('/');
    if let Ok(mut u) = reqwest::Url::parse(trimmed) {
        if let Some(rest) = u.host_str().and_then(|h| h.strip_prefix("api.")) {
            let host = format!("{to}.{rest}");
            if u.set_host(Some(&host)).is_ok() {
                return u.to_string().trim_end_matches('/').to_string();
            }
        }
    }
    trimmed.to_string()
}

pub fn console_url(api_url: &str, over: Option<&str>) -> String {
    over.map(|c| c.trim_end_matches('/').to_string())
        .unwrap_or_else(|| swap_host(api_url, "console"))
}

pub fn issuer_url(api_url: &str, over: Option<&str>) -> String {
    over.map(|c| c.trim_end_matches('/').to_string())
        .unwrap_or_else(|| swap_host(api_url, "auth"))
}

pub fn agent_name(given: Option<&str>, hostname: &str) -> String {
    let n = given
        .map(str::to_string)
        .unwrap_or_else(|| format!("Claude Code on {hostname}"));
    n.chars().take(MAX_NAME).collect()
}

/// `SHA256:<first 4>…<last 4>` of the key thumbprint.
pub fn fingerprint(jkt: &str) -> String {
    let c: Vec<char> = jkt.chars().collect();
    let head: String = c.iter().take(4).collect();
    let tail: String = c[c.len().saturating_sub(4)..].iter().collect();
    format!("SHA256:{head}…{tail}")
}

fn new_state() -> String {
    let mut b = [0u8; 16];
    rand_core::OsRng.fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn connect_url(console: &str, name: &str, key: &AgentKey, port: u16, state: &str) -> String {
    let e = |s: &str| utf8_percent_encode(s, NON_ALPHANUMERIC).to_string();
    let jkt = key.jkt();
    format!(
        "{console}/connect?name={}&x={}&kid={}&jkt={}&port={}&state={}",
        e(name),
        e(&key.x_b64()),
        e(&jkt),
        e(&jkt),
        port,
        e(state)
    )
}

pub fn mcp_add_args(profile: &str) -> Vec<String> {
    [
        "mcp",
        "add",
        "--scope",
        "user",
        "quome",
        "--",
        "quome",
        "mcp",
        "--profile",
        profile,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

pub fn mcp_config_json(profile: &str) -> String {
    serde_json::json!({"mcpServers": {"quome": {
        "command": "quome", "args": ["mcp", "--profile", profile]}}})
    .to_string()
}

fn open_browser(url: &str) -> std::io::Result<()> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|_| ())
}

/// `/me`, retried while the console has not yet recorded the delegation.
async fn me_with_retry(
    api: &CloudApi,
    interval: Duration,
    total: Duration,
) -> Result<crate::cloud::api::Me> {
    let deadline = Instant::now() + total;
    loop {
        match api.me().await {
            Err(QuomeError::CloudNotYetConnected) if Instant::now() + interval <= deadline => {
                tokio::time::sleep(interval).await;
            }
            other => return other,
        }
    }
}

pub async fn run(opts: Options, deps: &Deps<'_>) -> Result<Outcome> {
    let api_url = opts.api_url.trim_end_matches('/').to_string();
    let console = console_url(&api_url, opts.console.as_deref());
    let issuer = issuer_url(&api_url, opts.issuer.as_deref());
    let name = agent_name(opts.name.as_deref(), &deps.hostname);
    let profile_name = opts.profile.clone();

    let previous = CloudState::load_from(&deps.home)?
        .profiles
        .contains_key(&profile_name);

    let key = AgentKey::generate();
    let jkt = key.jkt();
    let state = new_state();
    let listener = Listener::bind()?;
    let url = connect_url(&console, &name, &key, listener.port(), &state);

    (deps.say)(&format!("Key fingerprint: {}", fingerprint(&jkt)));
    (deps.say)(&format!("Opening {url}"));
    if (deps.open)(&url).is_err() {
        (deps.say)("Could not open a browser; open the URL above to continue.");
    }
    (deps.say)("Waiting for you to approve in the browser...");

    let origin = loopback::origin_of(&issuer);
    let timeout = deps.listen_timeout;
    let cb: Callback = tokio::task::spawn_blocking(move || listener.wait(&state, &origin, timeout))
        .await
        .map_err(|e| QuomeError::CloudState(format!("listener failed: {e}")))??;

    let mut profile = Profile {
        edition: "cloud".into(),
        api_url,
        console_url: console.clone(),
        issuer,
        agent_name: name.clone(),
        agent_principal: cb.agent_principal,
        kid: jkt.clone(),
        jkt,
        delegation_id: cb.delegation_id,
        expires_at: cb.expires_at,
        org_id: String::new(),
        org_slug: String::new(),
        apps_domain: String::new(),
    };

    key.store_in(deps.store, &profile_name, &cb.vrc)?;
    let (key, vrc) = AgentKey::load_from(deps.store, &profile_name)?;
    let api = CloudApi::new(&profile, TokenSource::new(&profile, key, vrc));
    let me = me_with_retry(&api, deps.retry_interval, deps.retry_total).await?;
    let org = me
        .orgs
        .first()
        .ok_or_else(|| QuomeError::Usage("This agent has no Quome Cloud organization.".into()))?;
    profile.org_id = org.id.clone();
    profile.org_slug = org.slug.clone();
    profile.apps_domain = me.apps_domain;

    let mut st = CloudState::load_from(&deps.home)?;
    st.profiles.insert(profile_name.clone(), profile.clone());
    if st.default.is_empty() || !opts.profile_given {
        st.default = profile_name.clone();
    }
    st.save_to(&deps.home)?;

    let mcp = if opts.no_mcp {
        "skipped"
    } else {
        match deps.claude.run(&mcp_add_args(&profile_name)) {
            ClaudeResult::Ran(true) => "added",
            ClaudeResult::Ran(false) => {
                (deps.say)("`claude mcp add` failed; add this to your MCP config instead:");
                (deps.say)(&mcp_config_json(&profile_name));
                "printed"
            }
            ClaudeResult::NotFound => {
                (deps.say)("Add this to your MCP client config:");
                (deps.say)(&mcp_config_json(&profile_name));
                "printed"
            }
        }
    };

    if previous {
        (deps.say)(&format!(
            "The previous connection is still active until it expires or you revoke it in the console: {console}/dashboard"
        ));
    }
    Ok(Outcome {
        profile_name,
        profile,
        mcp,
    })
}

fn expiry_date(expires_at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(expires_at)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|_| expires_at.to_string())
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
    let json = args.json;
    let say = move |s: &str| {
        if json {
            eprintln!("{s}");
        } else {
            println!("{s}");
        }
    };
    let deps = Deps {
        home,
        store: store.as_ref(),
        open: &open_browser,
        claude: &RealClaude,
        say: &say,
        hostname: gethostname::gethostname().to_string_lossy().into_owned(),
        listen_timeout: loopback::TIMEOUT,
        retry_interval: RETRY_INTERVAL,
        retry_total: RETRY_TOTAL,
    };
    let opts = Options {
        api_url: args.api_url,
        name: args.name,
        profile_given: args.profile.is_some(),
        profile: args.profile.unwrap_or_else(|| DEFAULT_PROFILE.into()),
        console: args.console,
        issuer: args.issuer,
        no_mcp: args.no_mcp,
    };
    let out = run(opts, &deps).await?;
    let p = &out.profile;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "connected": true,
                "profile": out.profile_name,
                "agent_name": p.agent_name,
                "org_id": p.org_id,
                "org_slug": p.org_slug,
                "expires_at": p.expires_at,
                "mcp": out.mcp,
            }))?
        );
    } else {
        println!(
            "Connected to Quome Cloud as {} (org {}), expires {}",
            p.agent_name,
            p.org_slug,
            expiry_date(&p.expires_at)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::keys::FileStore;
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn urls_derive_from_api_host() {
        assert_eq!(
            console_url("https://api.quome.cloud", None),
            "https://console.quome.cloud"
        );
        assert_eq!(
            issuer_url("https://api.quome.cloud/", None),
            "https://auth.quome.cloud"
        );
        assert_eq!(
            console_url("https://api.dev.quome.cloud", None),
            "https://console.dev.quome.cloud"
        );
        assert_eq!(
            console_url("http://127.0.0.1:9", None),
            "http://127.0.0.1:9"
        );
        assert_eq!(
            console_url("https://api.x.y", Some("https://c.z/")),
            "https://c.z"
        );
        assert_eq!(
            issuer_url("https://api.x.y", Some("https://i.z")),
            "https://i.z"
        );
    }

    #[test]
    fn long_hostname_gives_64_char_name() {
        let n = agent_name(None, &"h".repeat(70));
        assert_eq!(n.chars().count(), 64);
        assert!(n.starts_with("Claude Code on h"));
        assert_eq!(agent_name(None, "mac"), "Claude Code on mac");
        assert_eq!(agent_name(Some("mine"), "mac"), "mine");
    }

    #[test]
    fn fingerprint_shape() {
        assert_eq!(
            fingerprint("kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k"),
            "SHA256:kPrK…rS4k"
        );
    }

    #[test]
    fn state_is_32_lower_hex() {
        let s = new_state();
        assert_eq!(s.len(), 32);
        assert!(s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
        assert_ne!(s, new_state());
    }

    #[test]
    fn connect_url_encodes_every_value() {
        let k = AgentKey::generate();
        let u = connect_url("https://console.x", "A&B c=d/é", &k, 4242, "st");
        assert!(u.starts_with("https://console.x/connect?name=A%26B%20c%3Dd%2F%C3%A9&x="));
        let parsed = reqwest::Url::parse(&u).unwrap();
        let q: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(q["name"], "A&B c=d/é");
        assert_eq!(q["x"], k.x_b64());
        assert_eq!(q["kid"], k.jkt());
        assert_eq!(q["jkt"], k.jkt());
        assert_eq!(q["port"], "4242");
        assert_eq!(q["state"], "st");
    }

    #[test]
    fn mcp_argv_and_config_are_exact() {
        assert_eq!(
            mcp_add_args("work").join(" "),
            "mcp add --scope user quome -- quome mcp --profile work"
        );
        assert_eq!(
            mcp_config_json("work"),
            r#"{"mcpServers":{"quome":{"args":["mcp","--profile","work"],"command":"quome"}}}"#
        );
    }

    struct FakeClaude {
        calls: Mutex<Vec<Vec<String>>>,
        result: ClaudeResult,
    }
    impl ClaudeRunner for FakeClaude {
        fn run(&self, args: &[String]) -> ClaudeResult {
            self.calls.lock().unwrap().push(args.to_vec());
            self.result
        }
    }

    async fn mount_auth(s: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"token_endpoint": format!("{}/oauth/token", s.uri())}),
            ))
            .mount(s)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "AT", "token_type": "Bearer", "expires_in": 3600
            })))
            .mount(s)
            .await;
    }

    async fn mount_me(s: &MockServer, not_connected: u64) {
        if not_connected > 0 {
            Mock::given(method("GET"))
                .and(path("/api/v1/me"))
                .respond_with(ResponseTemplate::new(401).set_body_json(
                    serde_json::json!({"detail":"unauthorized","reason":"agent_not_connected"}),
                ))
                .up_to_n_times(not_connected)
                .mount(s)
                .await;
        }
        Mock::given(method("GET"))
            .and(path("/api/v1/me"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": null, "email": null, "display_name": null,
                "orgs": [{"id": "org-1", "slug": "acme", "name": "Acme", "role": "admin"}],
                "principal_kind": "agent", "apps_domain": "apps.example",
                "api_origin": "https://x"
            })))
            .mount(s)
            .await;
    }

    fn opts(server: &MockServer, profile: &str) -> Options {
        Options {
            api_url: server.uri(),
            name: None,
            profile: profile.into(),
            profile_given: true,
            console: Some("https://console.example".into()),
            issuer: Some(server.uri()),
            no_mcp: false,
        }
    }

    type PageLog = Arc<Mutex<Vec<String>>>;

    /// The "browser": checks the loopback's refusals, then delivers the delegation.
    fn page(url: &str, delegation: &str, log: PageLog) {
        let parsed = reqwest::Url::parse(url).unwrap();
        let q: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        let cb = format!("http://127.0.0.1:{}/callback", q["port"]);
        let state = q["state"].clone();
        let delegation = delegation.to_string();
        tokio::spawn(async move {
            let c = reqwest::Client::new();
            let pre = c
                .request(reqwest::Method::OPTIONS, &cb)
                .send()
                .await
                .unwrap();
            log.lock()
                .unwrap()
                .push(format!("options {}", pre.status().as_u16()));
            let wrong = serde_json::json!({"state":"bad","vrc":"V","delegation_id":"x",
                "agent_principal":"x","expires_at":"x"});
            let r = c.post(&cb).json(&wrong).send().await.unwrap();
            log.lock()
                .unwrap()
                .push(format!("wrong {}", r.status().as_u16()));
            let ok = serde_json::json!({"state":state,"vrc":"VRC-1","delegation_id":delegation,
                "agent_principal":"agent-principal","expires_at":"2027-01-02T03:04:05Z"});
            let r = c.post(&cb).json(&ok).send().await.unwrap();
            let status = r.status().as_u16();
            let echo: serde_json::Value = r.json().await.unwrap();
            assert_eq!(echo["state"], state);
            log.lock().unwrap().push(format!("good {status}"));
        });
    }

    struct Harness {
        home: tempfile::TempDir,
        said: RefCell<Vec<String>>,
        claude: FakeClaude,
        opened: RefCell<Vec<String>>,
        log: PageLog,
    }

    impl Harness {
        fn new(result: ClaudeResult) -> Harness {
            Harness {
                home: tempfile::tempdir().unwrap(),
                said: RefCell::new(vec![]),
                claude: FakeClaude {
                    calls: Mutex::new(vec![]),
                    result,
                },
                opened: RefCell::new(vec![]),
                log: Arc::new(Mutex::new(vec![])),
            }
        }

        async fn connect(
            &self,
            o: Options,
            delegation: &str,
            hostname: &str,
            interval: Duration,
        ) -> Result<Outcome> {
            let store = FileStore {
                dir: self.home.path().join(".quome"),
            };
            let say = |s: &str| self.said.borrow_mut().push(s.to_string());
            let open = |u: &str| {
                self.opened.borrow_mut().push(u.to_string());
                page(u, delegation, self.log.clone());
                Ok(())
            };
            let deps = Deps {
                home: self.home.path().to_path_buf(),
                store: &store,
                open: &open,
                claude: &self.claude,
                say: &say,
                hostname: hostname.into(),
                listen_timeout: Duration::from_secs(20),
                retry_interval: interval,
                retry_total: Duration::from_secs(10),
            };
            run(o, &deps).await
        }
    }

    #[tokio::test]
    async fn connect_end_to_end_with_not_yet_connected_retries() {
        let s = MockServer::start().await;
        mount_auth(&s).await;
        mount_me(&s, 2).await;
        let h = Harness::new(ClaudeResult::Ran(true));
        let host = "h".repeat(70);
        let out = h
            .connect(opts(&s, "work"), "del-1", &host, Duration::from_millis(20))
            .await
            .unwrap();

        assert_eq!(
            *h.log.lock().unwrap(),
            vec!["options 204", "wrong 403", "good 200"]
        );
        // me() was called 3 times: two 401s then success
        let me_calls = s
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == "/api/v1/me")
            .count();
        assert_eq!(me_calls, 3);

        let p = &out.profile;
        assert_eq!(
            (
                p.org_id.as_str(),
                p.org_slug.as_str(),
                p.apps_domain.as_str()
            ),
            ("org-1", "acme", "apps.example")
        );
        assert_eq!(p.agent_principal, "agent-principal");
        assert_eq!(p.delegation_id, "del-1");
        assert_eq!(p.kid, p.jkt);
        assert_eq!(p.agent_name.chars().count(), 64);
        assert_eq!(p.issuer, s.uri());

        let st = CloudState::load_from(h.home.path()).unwrap();
        assert_eq!(st.profiles["work"], *p);
        let store = FileStore {
            dir: h.home.path().join(".quome"),
        };
        let (k, vrc) = AgentKey::load_from(&store, "work").unwrap();
        assert_eq!((k.jkt(), vrc.as_str()), (p.jkt.clone(), "VRC-1"));

        assert_eq!(
            h.claude.calls.lock().unwrap().as_slice(),
            [mcp_add_args("work")]
        );
        let said = h.said.borrow().join("\n");
        assert!(said.contains("SHA256:"));
        assert!(
            said.contains(&h.opened.borrow()[0]),
            "URL printed as well as opened"
        );
        assert!(!said.contains("previous connection"));
    }

    #[tokio::test]
    async fn me_never_connected_fails_with_finish_approving() {
        let s = MockServer::start().await;
        mount_auth(&s).await;
        mount_me(&s, 1000).await;
        let h = Harness::new(ClaudeResult::Ran(true));
        let mut store_dir = FileStore {
            dir: h.home.path().join(".quome"),
        };
        let _ = &mut store_dir;
        let store = FileStore {
            dir: h.home.path().join(".quome"),
        };
        let say = |_: &str| {};
        let open = |u: &str| {
            page(u, "d", h.log.clone());
            Ok(())
        };
        let deps = Deps {
            home: h.home.path().to_path_buf(),
            store: &store,
            open: &open,
            claude: &h.claude,
            say: &say,
            hostname: "mac".into(),
            listen_timeout: Duration::from_secs(20),
            retry_interval: Duration::from_millis(20),
            retry_total: Duration::from_millis(100),
        };
        let err = run(opts(&s, "work"), &deps).await.unwrap_err();
        assert!(matches!(err, QuomeError::CloudNotYetConnected));
        assert!(err
            .to_string()
            .contains("Finish approving in the browser, then try again"));
        assert!(CloudState::load_from(h.home.path())
            .unwrap()
            .profiles
            .is_empty());
        assert!(h.claude.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn second_connect_replaces_key_and_profile_and_prints_hint() {
        let s = MockServer::start().await;
        mount_auth(&s).await;
        mount_me(&s, 0).await;
        let h = Harness::new(ClaudeResult::NotFound);
        let first = h
            .connect(opts(&s, "work"), "del-1", "mac", Duration::from_millis(10))
            .await
            .unwrap();
        assert!(!h.said.borrow().join("\n").contains("previous connection"));
        let second = h
            .connect(opts(&s, "work"), "del-2", "mac", Duration::from_millis(10))
            .await
            .unwrap();

        assert_ne!(first.profile.jkt, second.profile.jkt);
        let st = CloudState::load_from(h.home.path()).unwrap();
        assert_eq!(st.profiles.len(), 1);
        assert_eq!(st.profiles["work"].delegation_id, "del-2");
        let store = FileStore {
            dir: h.home.path().join(".quome"),
        };
        assert_eq!(
            AgentKey::load_from(&store, "work").unwrap().0.jkt(),
            second.profile.jkt
        );
        let said = h.said.borrow().join("\n");
        assert!(said.contains(
            "The previous connection is still active until it expires or you revoke it in the console: https://console.example/dashboard"
        ));
        // no claude on PATH: the JSON config is printed instead
        assert!(said.contains(&mcp_config_json("work")));
        assert_eq!(second.mcp, "printed");
    }

    #[tokio::test]
    async fn no_mcp_skips_claude() {
        let s = MockServer::start().await;
        mount_auth(&s).await;
        mount_me(&s, 0).await;
        let h = Harness::new(ClaudeResult::Ran(true));
        let mut o = opts(&s, "work");
        o.no_mcp = true;
        let out = h
            .connect(o, "d", "mac", Duration::from_millis(10))
            .await
            .unwrap();
        assert_eq!(out.mcp, "skipped");
        assert!(h.claude.calls.lock().unwrap().is_empty());
        assert!(!h.said.borrow().join("\n").contains("mcpServers"));
    }

    #[test]
    fn expiry_date_formats() {
        assert_eq!(expiry_date("2027-01-02T03:04:05Z"), "2027-01-02");
        assert_eq!(expiry_date("soon"), "soon");
    }
}
