//! `quome models connect | status | disconnect | serve`: pair this computer with an
//! organization so it can serve its local models to the org.
use clap::{Args, Subcommand};
use std::path::Path;
use std::time::Duration;

use crate::cloud::api::CloudApi;
use crate::cloud::auth::TokenSource;
use crate::cloud::keys::{AgentKey, KeyStore};
use crate::cloud::state::CloudState;
use crate::commands::connect::{self, Deps, Options};
use crate::errors::{QuomeError, Result};
use crate::machine::config::{self, MachineConfig, ServerKind, MAX_CONCURRENCY, PROFILE};
use crate::machine::service;

/// Warn this many days before the delegation ends.
const EXPIRY_WARN_DAYS: i64 = 7;
const SCOPE: &str = "models_serve";

#[derive(Subcommand)]
pub enum ModelsCommands {
    /// Pair this computer with an organization to serve its local models
    Connect(ConnectArgs),
    /// Show this machine's connection, settings and what the platform sees
    Status(JsonArgs),
    /// Revoke this machine and forget its connection
    Disconnect(JsonArgs),
    /// Serve this machine's models to the organization
    Serve {
        /// Install it as a background service that starts at login
        #[arg(long)]
        install: bool,
        /// Remove the background service
        #[arg(long)]
        uninstall: bool,
        /// Log as JSON lines
        #[arg(long)]
        json_logs: bool,
    },
}

#[derive(Args)]
pub struct ConnectArgs {
    /// Quome Cloud API URL
    #[arg(default_value = connect::DEFAULT_API_URL)]
    api_url: String,
    /// Organization (slug) this machine serves
    #[arg(long)]
    org: Option<String>,
    /// Name shown for this machine (default: this computer's short hostname)
    #[arg(long)]
    name: Option<String>,
    /// Local server: ollama, vllm, llama.cpp, mlx or quexo
    #[arg(long, value_parser = ServerKind::parse)]
    server: Option<ServerKind>,
    /// URL of the local server (plain http only to this computer)
    #[arg(long)]
    url: Option<String>,
    /// Most requests served at once (1 to 64)
    #[arg(long)]
    max_concurrency: Option<u32>,
    /// Console URL (default: the API URL with `api.` replaced by `console.`)
    #[arg(long)]
    console: Option<String>,
    /// Issuer URL (default: the API URL with `api.` replaced by `auth.`)
    #[arg(long)]
    issuer: Option<String>,
    /// Disconnect an existing machine connection first
    #[arg(long)]
    replace: bool,
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct JsonArgs {
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

pub struct ConnectOpts {
    pub api_url: String,
    pub org: Option<String>,
    pub name: Option<String>,
    pub server: Option<ServerKind>,
    pub url: Option<String>,
    pub max_concurrency: Option<u32>,
    pub console: Option<String>,
    pub issuer: Option<String>,
    pub replace: bool,
}

#[derive(Debug)]
pub struct Connected {
    pub org_slug: String,
    pub org_id: String,
    pub name: String,
    pub expires_at: String,
    pub config: MachineConfig,
}

fn short_hostname(hostname: &str) -> String {
    let short = hostname.split('.').next().unwrap_or("").trim();
    if short.is_empty() {
        "This machine".into()
    } else {
        short.into()
    }
}

/// Flags over the existing file over the defaults.
fn merged_config(o: &ConnectOpts, existing: MachineConfig) -> Result<MachineConfig> {
    let mut c = existing;
    if let Some(kind) = o.server {
        if kind != c.server && o.url.is_none() {
            c.url = kind.default_url().into();
        }
        c.server = kind;
    }
    if let Some(u) = &o.url {
        c.url = u.clone();
    }
    if let Some(n) = o.max_concurrency {
        c.max_concurrency = n;
    }
    if !(1..=MAX_CONCURRENCY).contains(&c.max_concurrency) {
        return Err(QuomeError::Usage(format!(
            "--max-concurrency must be between 1 and {MAX_CONCURRENCY}"
        )));
    }
    config::validate_local_url(&c.url)?;
    Ok(c)
}

fn has_machine_profile(home: &Path) -> Result<bool> {
    Ok(CloudState::load_from(home)?.profiles.contains_key(PROFILE))
}

pub async fn connect(o: ConnectOpts, deps: &Deps<'_>) -> Result<Connected> {
    let cfg_path = config::config_path(&deps.home, deps.env);
    let cfg = merged_config(&o, config::load(&cfg_path)?)?;

    if has_machine_profile(&deps.home)? {
        if !o.replace {
            return Err(QuomeError::Usage(
                "This machine is already connected. Run `quome models disconnect` first.".into(),
            ));
        }
        disconnect(&deps.home, deps.store, deps.say).await?;
    }

    let opts = Options {
        api_url: o.api_url,
        name: Some(o.name.unwrap_or_else(|| short_hostname(&deps.hostname))),
        profile: PROFILE.into(),
        profile_given: true,
        console: o.console,
        issuer: o.issuer,
        no_mcp: true,
        clients: vec![],
        scopes: vec![SCOPE.into()],
        org: o.org,
    };
    let out = connect::run(opts, deps).await.map_err(|e| match e {
        QuomeError::CloudNotYetConnected => QuomeError::Usage(
            "The approval was not recorded. Only an Owner or Admin can approve a machine, \
             and the organization's machine limit applies; the browser page shows why."
                .into(),
        ),
        e => e,
    })?;
    config::save(&cfg_path, &cfg)?;
    Ok(Connected {
        org_slug: out.profile.org_slug,
        org_id: out.profile.org_id,
        name: out.profile.agent_name,
        expires_at: out.profile.expires_at,
        config: cfg,
    })
}

fn api_for(home: &Path, store: &dyn KeyStore) -> Result<Option<CloudApi>> {
    let st = CloudState::load_from(home)?;
    let Some(p) = st.profiles.get(PROFILE) else {
        return Ok(None);
    };
    let (key, vrc) = AgentKey::load_from(store, PROFILE)?;
    Ok(Some(CloudApi::new(p, TokenSource::new(p, key, vrc))))
}

fn key_file_mode(home: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let p = crate::cloud::state::quome_dir(home).join(format!("cloud-{PROFILE}.key"));
        let m = std::fs::metadata(p).ok()?.permissions().mode();
        Some(format!("{:04o}", m & 0o7777))
    }
    #[cfg(not(unix))]
    {
        let _ = home;
        None
    }
}

/// Whole days until `expires_at`, rounded up; `None` when it cannot be read.
fn days_left(expires_at: &str) -> Option<i64> {
    let end = chrono::DateTime::parse_from_rfc3339(expires_at).ok()?;
    let secs = (end.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_seconds();
    Some(if secs <= 0 {
        0
    } else {
        (secs + 86_399) / 86_400
    })
}

fn expiry_warning(expires_at: &str) -> Option<String> {
    let d = days_left(expires_at)?;
    if d > EXPIRY_WARN_DAYS {
        return None;
    }
    if d == 0 {
        return Some(
            "This machine's approval has expired — run `quome models connect --replace`.".into(),
        );
    }
    Some(format!(
        "This machine's approval expires in {d} day{} — run `quome models connect --replace` before then",
        if d == 1 { "" } else { "s" }
    ))
}

/// Text (or JSON) to print and the exit code.
pub async fn status(
    home: &Path,
    env: &dyn Fn(&str) -> Option<String>,
    store: &dyn KeyStore,
    json: bool,
) -> Result<(String, i32)> {
    let st = match CloudState::load_from(home) {
        Ok(st) => st,
        Err(e) if json => {
            return Ok((
                serde_json::to_string_pretty(&serde_json::json!({
                    "connected": false, "api": null, "org": null, "server": null,
                    "error": e.to_string(),
                }))?,
                0,
            ))
        }
        Err(e) => return Err(e),
    };
    let Some(p) = st.profiles.get(PROFILE).cloned() else {
        return Ok(if json {
            (
                serde_json::to_string_pretty(&serde_json::json!({
                    "connected": false, "api": null, "org": null, "server": null,
                }))?,
                0,
            )
        } else {
            (
                "This machine is not connected. Run `quome models connect`.".into(),
                1,
            )
        });
    };
    let cfg = config::load(&config::config_path(home, env));
    let installed = service::installed(home);
    let mode = key_file_mode(home);
    let days = days_left(&p.expires_at);
    let warning = expiry_warning(&p.expires_at);

    // The platform's view; any failure is reported, never raised.
    let remote: serde_json::Value = match api_for(home, store) {
        Ok(Some(api)) => match api.models_machine_self().await {
            Ok(m) => serde_json::json!({
                "id": m.id, "name": m.name, "state": m.state, "connected": m.connected,
                "since": m.since, "delegation_expires_at": m.delegation_expires_at,
                "approved_by": m.approved_by,
            }),
            Err(QuomeError::NotFound(_)) => "unknown".into(),
            Err(e) => serde_json::json!({ "error": e.to_string() }),
        },
        Ok(None) => "unknown".into(),
        Err(e) => serde_json::json!({ "error": e.to_string() }),
    };

    if json {
        let (server, conf) = match &cfg {
            Ok(c) => (
                serde_json::json!(c.server.name()),
                serde_json::json!({
                    "url": c.url, "max_concurrency": c.max_concurrency,
                    "api_key_file": c.api_key_file,
                }),
            ),
            Err(e) => (
                serde_json::Value::Null,
                serde_json::json!({ "error": e.to_string() }),
            ),
        };
        let v = serde_json::json!({
            "connected": true,
            "api": p.api_url,
            "org": p.org_slug,
            "server": server,
            "org_id": p.org_id,
            "name": p.agent_name,
            "expires_at": p.expires_at,
            "expires_in_days": days,
            "config": conf,
            "key_file_mode": mode,
            "service_installed": installed,
            "remote": remote,
        });
        return Ok((serde_json::to_string_pretty(&v)?, 0));
    }

    let mut out = format!(
        "Model machine\n  Name:    {}\n  Org:     {}\n  API:     {}\n  Expires: {}",
        p.agent_name,
        p.org_slug,
        p.api_url,
        p.expires_at.split('T').next().unwrap_or(&p.expires_at)
    );
    match &cfg {
        Ok(c) => out.push_str(&format!(
            "\n  Server:  {} at {} (up to {} at once)",
            c.server.name(),
            c.url,
            c.max_concurrency
        )),
        Err(e) => out.push_str(&format!("\n  Config:  {e}")),
    }
    out.push_str(&format!(
        "\n  Key:     {}\n  Service: {}",
        mode.map_or("missing".to_string(), |m| format!("file, mode {m}")),
        if installed {
            "installed"
        } else {
            "not installed"
        }
    ));
    match &remote {
        serde_json::Value::String(_) => out.push_str("\n  Platform: unknown"),
        v if v.get("error").is_some() => out.push_str(&format!(
            "\n  Platform: {}",
            v["error"].as_str().unwrap_or("")
        )),
        v => out.push_str(&format!(
            "\n  Platform: {} ({})",
            v["state"].as_str().unwrap_or(""),
            if v["connected"].as_bool() == Some(true) {
                "connected"
            } else {
                "not connected"
            }
        )),
    }
    if let Some(w) = warning {
        out.push_str(&format!("\n\n{w}"));
    }
    Ok((out, 0))
}

#[derive(Debug, Default)]
pub struct Disconnected {
    pub was_connected: bool,
    pub revoked: bool,
    pub revoke_error: Option<String>,
}

/// Revoke on the platform (a failure is reported, not fatal), then delete the key
/// and the `models` profile. The default profile is left alone.
pub async fn disconnect(
    home: &Path,
    store: &dyn KeyStore,
    say: &dyn Fn(&str),
) -> Result<Disconnected> {
    let mut report = Disconnected::default();
    let mut st = CloudState::load_from(home)?;
    report.was_connected = st.profiles.contains_key(PROFILE);
    if report.was_connected {
        let revoked = match api_for(home, store) {
            Ok(Some(api)) => {
                tokio::time::timeout(Duration::from_secs(30), api.models_machine_revoke())
                    .await
                    .unwrap_or_else(|_| Err(QuomeError::Usage("the request timed out".into())))
            }
            Ok(None) => Ok(()),
            Err(e) => Err(e),
        };
        match revoked {
            Ok(()) => report.revoked = true,
            Err(e) => {
                say(&format!(
                    "Could not revoke on the platform: {e}; revoke it in Models › Machines"
                ));
                report.revoke_error = Some(e.to_string());
            }
        }
    }
    // The background service is uninstalled here once `serve --install` exists.
    store.delete(PROFILE)?;
    if report.was_connected {
        st.profiles.remove(PROFILE);
        if st.default == PROFILE {
            st.default = st.first_user_profile().unwrap_or_default();
        }
        st.save_to(home)?;
    }
    Ok(report)
}

fn home() -> Result<std::path::PathBuf> {
    dirs::home_dir().ok_or_else(|| QuomeError::CloudState("cannot find home directory".into()))
}

pub async fn execute(command: ModelsCommands) -> Result<()> {
    match command {
        ModelsCommands::Connect(a) => execute_connect(a).await,
        ModelsCommands::Status(a) => {
            let home = home()?;
            let store = config::key_store(&home);
            let (out, code) =
                status(&home, &crate::cloud::clients::real_env, &store, a.json).await?;
            println!("{out}");
            if code != 0 {
                std::process::exit(code);
            }
            Ok(())
        }
        ModelsCommands::Disconnect(a) => {
            let home = home()?;
            let store = config::key_store(&home);
            let json = a.json;
            let say = move |s: &str| {
                if json {
                    eprintln!("{s}");
                } else {
                    println!("{s}");
                }
            };
            let r = disconnect(&home, &store, &say).await?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "disconnected": r.was_connected,
                        "revoked": r.revoked,
                        "revoke_error": r.revoke_error,
                    }))?
                );
            } else if r.was_connected {
                println!("Disconnected this machine.");
            } else {
                println!("This machine was not connected.");
            }
            Ok(())
        }
        ModelsCommands::Serve { .. } => Err(QuomeError::Usage(
            "quome models serve is not available in this build".into(),
        )),
    }
}

async fn execute_connect(a: ConnectArgs) -> Result<()> {
    let home = home()?;
    let store = config::key_store(&home);
    let json = a.json;
    let say = move |s: &str| {
        if json {
            eprintln!("{s}");
        } else {
            println!("{s}");
        }
    };
    let deps = Deps {
        home,
        store: &store,
        open: &connect::open_browser,
        claude: &connect::RealClaude {
            program: "claude".into(),
        },
        runner: &crate::cloud::clients::runner::RealRunner,
        on_path: &crate::cloud::clients::paths::on_real_path,
        env: &crate::cloud::clients::real_env,
        say: &say,
        hostname: gethostname::gethostname().to_string_lossy().into_owned(),
        listen_timeout: crate::cloud::loopback::TIMEOUT,
        retry_interval: connect::RETRY_INTERVAL,
        retry_total: connect::RETRY_TOTAL,
    };
    let c = connect(
        ConnectOpts {
            api_url: a.api_url,
            org: a.org,
            name: a.name,
            server: a.server,
            url: a.url,
            max_concurrency: a.max_concurrency,
            console: a.console,
            issuer: a.issuer,
            replace: a.replace,
        },
        &deps,
    )
    .await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "connected": true,
                "profile": PROFILE,
                "name": c.name,
                "org": c.org_slug,
                "org_id": c.org_id,
                "expires_at": c.expires_at,
                "server": c.config.server.name(),
                "url": c.config.url,
                "max_concurrency": c.config.max_concurrency,
                "next": "quome models serve --install",
            }))?
        );
    } else {
        println!(
            "Connected {} to org {} (serving {} at {}).",
            c.name,
            c.org_slug,
            c.config.server.name(),
            c.config.url
        );
        println!("Next: `quome models serve --install`");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::keys::FileStore;
    use crate::cloud::state::Profile;
    use std::cell::RefCell;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn mount_platform(s: &MockServer) {
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

    /// The "browser": delivers the delegation to the loopback listener.
    fn page(url: &str) {
        let parsed = reqwest::Url::parse(url).unwrap();
        let q: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        let cb = format!("http://127.0.0.1:{}/callback", q["port"]);
        let ok = serde_json::json!({"state": q["state"], "vrc": "VRC-1", "delegation_id": "del-1",
            "agent_principal": "agent-principal", "expires_at": "2027-01-02T03:04:05Z"});
        tokio::spawn(async move {
            reqwest::Client::new()
                .post(&cb)
                .json(&ok)
                .send()
                .await
                .unwrap();
        });
    }

    struct Harness {
        home: tempfile::TempDir,
        opened: RefCell<Vec<String>>,
        said: RefCell<Vec<String>>,
        runner: crate::cloud::clients::runner::fake::Fake,
        claude: NoClaude,
    }

    struct NoClaude(std::sync::atomic::AtomicUsize);
    impl connect::ClaudeRunner for NoClaude {
        fn run(&self, _: &[String]) -> connect::ClaudeResult {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            connect::ClaudeResult::NotFound
        }
    }

    impl Harness {
        fn new() -> Harness {
            Harness {
                home: tempfile::tempdir().unwrap(),
                opened: RefCell::new(vec![]),
                said: RefCell::new(vec![]),
                runner: crate::cloud::clients::runner::fake::Fake::new(|_, _| {
                    crate::cloud::clients::runner::fake::ok("")
                }),
                claude: NoClaude(Default::default()),
            }
        }

        fn store(&self) -> FileStore {
            config::key_store(self.home.path())
        }

        async fn connect(
            &self,
            s: &MockServer,
            edit: impl Fn(&mut ConnectOpts),
        ) -> Result<Connected> {
            let store = self.store();
            let say = |m: &str| self.said.borrow_mut().push(m.to_string());
            let open = |u: &str| {
                self.opened.borrow_mut().push(u.to_string());
                page(u);
                Ok(())
            };
            let deps = Deps {
                home: self.home.path().to_path_buf(),
                store: &store,
                open: &open,
                claude: &self.claude,
                runner: &self.runner,
                on_path: &|_: &str| true,
                env: &crate::cloud::clients::no_env,
                say: &say,
                hostname: "studio.local".into(),
                listen_timeout: Duration::from_secs(20),
                retry_interval: Duration::from_millis(20),
                retry_total: Duration::from_secs(5),
            };
            let mut o = ConnectOpts {
                api_url: s.uri(),
                org: None,
                name: None,
                server: None,
                url: None,
                max_concurrency: None,
                console: Some("https://console.example".into()),
                issuer: Some(s.uri()),
                replace: false,
            };
            edit(&mut o);
            connect(o, &deps).await
        }
    }

    fn seed_default(home: &Path, store: &FileStore) {
        let p = Profile {
            edition: "cloud".into(),
            api_url: "https://api.quome.cloud".into(),
            console_url: "https://console.quome.cloud".into(),
            issuer: "https://auth.quome.cloud".into(),
            agent_name: "a".into(),
            agent_principal: "p".into(),
            kid: "k".into(),
            jkt: "j".into(),
            delegation_id: "d".into(),
            expires_at: "2027-01-01T00:00:00Z".into(),
            org_id: "o".into(),
            org_slug: "s".into(),
            apps_domain: "x".into(),
        };
        let mut st = CloudState {
            default: "cloud".into(),
            ..Default::default()
        };
        st.profiles.insert("cloud".into(), p);
        st.save_to(home).unwrap();
        store.put("cloud", "default-key").unwrap();
    }

    #[tokio::test]
    async fn models_connect_uses_its_own_profile_and_file_store() {
        let s = MockServer::start().await;
        mount_platform(&s).await;
        let h = Harness::new();
        seed_default(h.home.path(), &h.store());
        let c = h.connect(&s, |_| {}).await.unwrap();
        assert_eq!((c.org_slug.as_str(), c.name.as_str()), ("acme", "studio"));

        let url = reqwest::Url::parse(&h.opened.borrow()[0]).unwrap();
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["scope"], "models_serve");
        assert_eq!(q["name"], "studio");

        let key = h.home.path().join(".quome/cloud-models.key");
        assert!(key.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&key).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let st = CloudState::load_from(h.home.path()).unwrap();
        assert!(st.profiles.contains_key("models"));
        assert_eq!(st.default, "cloud");
        assert_eq!(h.store().get("cloud").unwrap(), "default-key");
        assert!(h.runner.calls().is_empty());
        assert_eq!(h.claude.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        let cfg = config::load(&config::config_path(h.home.path(), &|_| None)).unwrap();
        assert_eq!(cfg, MachineConfig::default());
    }

    #[tokio::test]
    async fn a_first_machine_connection_does_not_become_the_default_profile() {
        let s = MockServer::start().await;
        mount_platform(&s).await;
        let h = Harness::new();
        h.connect(&s, |o| o.server = Some(ServerKind::Quexo))
            .await
            .unwrap();
        assert_eq!(CloudState::load_from(h.home.path()).unwrap().default, "");
        let cfg = config::load(&config::config_path(h.home.path(), &|_| None)).unwrap();
        assert_eq!(cfg.server, ServerKind::Quexo);
        assert_eq!(cfg.url, "http://127.0.0.1:52415");
    }

    #[tokio::test]
    async fn models_connect_refuses_when_connected_without_replace() {
        let s = MockServer::start().await;
        mount_platform(&s).await;
        Mock::given(method("DELETE"))
            .and(path("/api/v1/orgs/org-1/models/machines/me"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&s)
            .await;
        let h = Harness::new();
        h.connect(&s, |_| {}).await.unwrap();
        let e = h.connect(&s, |_| {}).await.unwrap_err().to_string();
        assert_eq!(
            e,
            "This machine is already connected. Run `quome models disconnect` first."
        );
        assert_eq!(h.opened.borrow().len(), 1, "no second browser trip");
        h.connect(&s, |o| o.replace = true).await.unwrap();
        assert_eq!(h.opened.borrow().len(), 2);
    }

    #[tokio::test]
    async fn bad_flags_are_refused_before_the_browser_opens() {
        let s = MockServer::start().await;
        let h = Harness::new();
        for edit in [
            (|o: &mut ConnectOpts| o.max_concurrency = Some(65)) as fn(&mut ConnectOpts),
            |o| o.url = Some("http://192.168.1.5:8000".into()),
        ] {
            assert!(h.connect(&s, edit).await.is_err());
        }
        assert!(h.opened.borrow().is_empty());
    }

    fn write_profile(home: &Path, expires_at: &str) {
        let mut st = CloudState::default();
        st.profiles.insert(
            "models".into(),
            Profile {
                edition: "cloud".into(),
                api_url: "https://api.example.test".into(),
                console_url: "https://console.example.test".into(),
                issuer: "https://auth.example.test".into(),
                agent_name: "studio".into(),
                agent_principal: "p".into(),
                kid: "k".into(),
                jkt: "j".into(),
                delegation_id: "d".into(),
                expires_at: expires_at.into(),
                org_id: "org-1".into(),
                org_slug: "acme".into(),
                apps_domain: "x".into(),
            },
        );
        st.save_to(home).unwrap();
    }

    #[tokio::test]
    async fn status_warns_near_expiry() {
        let h = tempfile::tempdir().unwrap();
        let soon = (chrono::Utc::now() + chrono::Duration::days(3)).to_rfc3339();
        write_profile(h.path(), &soon);
        let store = config::key_store(h.path());
        let (out, code) = status(h.path(), &|_| None, &store, false).await.unwrap();
        assert_eq!(code, 0);
        assert!(out.contains("expires in 3 days"), "{out}");
        assert!(out.contains("quome models connect --replace"), "{out}");

        let far = (chrono::Utc::now() + chrono::Duration::days(60)).to_rfc3339();
        write_profile(h.path(), &far);
        let (out, _) = status(h.path(), &|_| None, &store, false).await.unwrap();
        assert!(!out.contains("expires in"), "{out}");
    }

    #[tokio::test]
    async fn status_json_shapes() {
        let h = Harness::new();
        let (out, code) = status(h.home.path(), &|_| None, &h.store(), true)
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&out).unwrap(),
            serde_json::json!({"connected": false, "api": null, "org": null, "server": null})
        );
        let (text, code) = status(h.home.path(), &|_| None, &h.store(), false)
            .await
            .unwrap();
        assert_eq!(code, 1);
        assert!(text.contains("quome models connect"));

        let s = MockServer::start().await;
        mount_platform(&s).await;
        Mock::given(method("GET"))
            .and(path("/api/v1/orgs/org-1/models/machines/me"))
            .respond_with(
                ResponseTemplate::new(404)
                    .set_body_json(serde_json::json!({"reason": "no_machine"})),
            )
            .mount(&s)
            .await;
        h.connect(&s, |o| o.server = Some(ServerKind::Quexo))
            .await
            .unwrap();
        let (out, code) = status(h.home.path(), &|_| None, &h.store(), true)
            .await
            .unwrap();
        assert_eq!(code, 0);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["connected"], true);
        assert_eq!(v["server"], "quexo");
        assert_eq!(v["org"], "acme");
        assert_eq!(v["api"], s.uri());
        assert_eq!(v["remote"], "unknown");
        assert_eq!(v["service_installed"], false);

        // A broken config still gives the required keys.
        let cfg = config::config_path(h.home.path(), &|_| None);
        std::fs::write(&cfg, "not = [valid").unwrap();
        let (out, code) = status(h.home.path(), &|_| None, &h.store(), true)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(code, 0);
        assert!(v["server"].is_null() && v["connected"] == true);
    }

    #[tokio::test]
    async fn disconnect_removes_only_the_machine() {
        let s = MockServer::start().await;
        mount_platform(&s).await;
        Mock::given(method("DELETE"))
            .and(path("/api/v1/orgs/org-1/models/machines/me"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&s)
            .await;
        let h = Harness::new();
        seed_default(h.home.path(), &h.store());
        h.connect(&s, |_| {}).await.unwrap();
        let r = disconnect(h.home.path(), &h.store(), &|_| {})
            .await
            .unwrap();
        assert!(r.was_connected && r.revoked);
        let st = CloudState::load_from(h.home.path()).unwrap();
        assert!(st.profiles.contains_key("cloud") && !st.profiles.contains_key("models"));
        assert_eq!(st.default, "cloud");
        assert_eq!(h.store().get("cloud").unwrap(), "default-key");
        assert!(!h.home.path().join(".quome/cloud-models.key").exists());
    }

    #[tokio::test]
    async fn disconnect_survives_a_failed_revoke() {
        let s = MockServer::start().await;
        mount_platform(&s).await;
        Mock::given(method("DELETE"))
            .and(path("/api/v1/orgs/org-1/models/machines/me"))
            .respond_with(
                ResponseTemplate::new(500).set_body_json(serde_json::json!({"detail": "boom"})),
            )
            .mount(&s)
            .await;
        let h = Harness::new();
        h.connect(&s, |_| {}).await.unwrap();
        let said = RefCell::new(vec![]);
        let r = disconnect(h.home.path(), &h.store(), &|m| {
            said.borrow_mut().push(m.to_string())
        })
        .await
        .unwrap();
        assert!(r.was_connected && !r.revoked);
        let said = said.borrow().join("\n");
        assert!(
            said.contains("Could not revoke on the platform:")
                && said.contains("Models › Machines"),
            "{said}"
        );
        assert!(!CloudState::load_from(h.home.path())
            .unwrap()
            .profiles
            .contains_key("models"));
        assert!(!h.home.path().join(".quome/cloud-models.key").exists());
        // Nothing connected: a no-op, not an error.
        let r = disconnect(h.home.path(), &h.store(), &|_| {})
            .await
            .unwrap();
        assert!(!r.was_connected);
    }

    #[tokio::test]
    async fn status_json_survives_an_unreadable_state_file() {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(h.path().join(".quome")).unwrap();
        std::fs::write(h.path().join(".quome/cloud.json"), "{not json").unwrap();
        let store = config::key_store(h.path());
        let (out, code) = status(h.path(), &|_| None, &store, true).await.unwrap();
        assert_eq!(code, 0);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["connected"], false);
        assert!(v["error"].as_str().is_some_and(|e| !e.is_empty()));
        assert!(status(h.path(), &|_| None, &store, false).await.is_err());
    }

    #[test]
    fn an_expired_approval_reads_plainly() {
        let past = (chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339();
        let w = expiry_warning(&past).unwrap();
        assert_eq!(
            w,
            "This machine's approval has expired — run `quome models connect --replace`."
        );
        assert!(!w.contains("before then"));
    }
}
