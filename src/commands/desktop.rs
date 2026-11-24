//! `quome desktop connect`: link this Mac to an organization as a desktop its
//! agents can drive.
use clap::{Args, Subcommand};
use std::path::Path;
use std::time::Duration;

use crate::cloud::api::CloudApi;
use crate::cloud::auth::TokenSource;
use crate::cloud::clients::runner::{CommandRunner, RunResult};
use crate::cloud::keys::{AgentKey, KeyStore};
use crate::cloud::state::CloudState;
use crate::commands::connect::{self, Deps, Options};
use crate::desktop::app;
use crate::desktop::config::{self, DesktopConfig, PROFILE, SCOPE};
use crate::desktop::service;
use crate::desktop::size::pick_size;
use crate::errors::{QuomeError, Result};
use crate::machine::service as machine_service;

#[derive(Subcommand)]
pub enum DesktopCommands {
    /// Link this Mac to an organization as a desktop its agents can use
    Connect(ConnectArgs),
    /// Run the link between this computer and the platform (started by launchd)
    Serve(ServeArgs),
}

impl DesktopCommands {
    /// True for a command a service manager runs in the background, which never
    /// checks for updates or prints the update line.
    pub fn runs_in_background(&self) -> bool {
        match self {
            DesktopCommands::Connect(_) => false,
            DesktopCommands::Serve(_) => true,
        }
    }
}

#[derive(Args)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct ServeArgs {
    /// Install and start the background agents, then exit
    #[arg(long, conflicts_with = "uninstall")]
    install: bool,
    /// Stop and remove the background agents, then exit
    #[arg(long)]
    uninstall: bool,
}

#[derive(Args)]
pub struct ConnectArgs {
    /// Quome Cloud API URL
    #[arg(default_value = connect::DEFAULT_API_URL)]
    api_url: String,
    /// Organization (slug) this computer serves
    #[arg(long)]
    org: Option<String>,
    /// Name shown for this computer (default: the computer's name)
    #[arg(long)]
    name: Option<String>,
    /// Give agents the smaller screen size of this display's shape
    #[arg(long)]
    small_frames: bool,
    /// Unlink an existing desktop link first
    #[arg(long)]
    replace: bool,
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

pub struct ConnectOpts {
    pub api_url: String,
    pub org: Option<String>,
    pub name: Option<String>,
    pub small_frames: bool,
    pub replace: bool,
    /// Console and issuer URLs (default: derived from the API URL).
    pub console: Option<String>,
    pub issuer: Option<String>,
}

#[derive(Debug)]
pub struct Connected {
    pub org_slug: String,
    pub org_id: String,
    pub name: String,
    pub expires_at: String,
    pub config: DesktopConfig,
}

pub const MEMBER_APPROVAL: &str = "Only an Owner or Admin can approve a computer. \
     Ask one to approve it, or to run this command with you.";
pub const NOT_TURNED_ON: &str = "Linking computers is not turned on for this organization.";
/// When linking computers is off, the approval page refuses and nothing comes back, so
/// a wait that runs out names that as the likely cause.
pub const NO_APPROVAL: &str = "No approval arrived. If the approval page said this permission \
     isn't available, linking computers is not turned on for this organization: \
     ask an Owner or Admin.";

fn short_hostname(hostname: &str) -> String {
    let short = hostname.split('.').next().unwrap_or("").trim();
    if short.is_empty() {
        "This computer".into()
    } else {
        short.into()
    }
}

/// The name the Mac shows in Sharing settings, else its short hostname.
fn computer_name(run: &dyn CommandRunner, hostname: &str) -> String {
    match run.run("scutil", &["--get".into(), "ComputerName".into()]) {
        RunResult::Ran {
            ok: true, stdout, ..
        } if !stdout.trim().is_empty() => stdout.trim().to_string(),
        _ => short_hostname(hostname),
    }
}

fn has_desktop_profile(home: &Path) -> Result<bool> {
    Ok(CloudState::load_from(home)?.profiles.contains_key(PROFILE))
}

/// True for the platform's refusal of a scope the organization does not offer. Defensive:
/// today that refusal is shown on the approval page and never reaches the CLI (see
/// `NO_APPROVAL`), but an API answer carrying it reads the same way.
fn scope_not_allowed(e: &QuomeError) -> bool {
    match e {
        QuomeError::ApiRefused { status, reason, .. } => {
            *status == 422 && reason == "scope_not_allowed"
        }
        QuomeError::ApiError(m) => m.starts_with("422") && m.contains("scope_not_allowed"),
        _ => false,
    }
}

/// True when the computer's own set-up after the identity step did not finish: no
/// settings file, no screen in it, or no app installed.
fn setup_unfinished(home: &Path) -> Result<bool> {
    let done = matches!(
        config::load(home)?,
        Some(DesktopConfig {
            screen: Some(_),
            panes_shown: true,
            ..
        })
    ) && app::app_path(home).join(app::EXECUTABLE).is_file();
    Ok(!done)
}

/// Finish the set-up of a computer that is linked already, without a second approval.
async fn resume(o: &ConnectOpts, deps: &Deps<'_>) -> Result<Connected> {
    let p = CloudState::load_from(&deps.home)?
        .profile(Some(PROFILE))?
        .clone();
    if let Some(org) = o.org.as_deref() {
        if org != p.org_slug {
            return Err(QuomeError::Usage(format!(
                "This computer is linked to org \"{}\", not \"{org}\". \
                 Run `quome desktop connect --replace` to link it again.",
                p.org_slug
            )));
        }
    }
    (deps.say)("This computer is linked; finishing its set-up.");
    let cfg = match config::load(&deps.home)? {
        Some(c) => DesktopConfig {
            small_frames: if c.screen.is_some() {
                c.small_frames
            } else {
                o.small_frames
            },
            ..c
        },
        None => DesktopConfig {
            api: p.api_url.clone(),
            org: p.org_slug.clone(),
            screen: None,
            small_frames: o.small_frames,
            panes_shown: false,
        },
    };
    let cfg = prepare_computer(cfg, deps).await?;
    config::save(&deps.home, &cfg)?;
    let cfg = start_computer(cfg, deps).await?;
    Ok(Connected {
        org_slug: p.org_slug,
        org_id: p.org_id,
        name: p.agent_name,
        expires_at: p.expires_at,
        config: cfg,
    })
}

/// Link this computer: the identity step, then the computer's own set-up. A computer
/// linked already whose set-up did not finish resumes it; `--replace` links afresh.
pub async fn connect(o: ConnectOpts, deps: &Deps<'_>) -> Result<Connected> {
    if has_desktop_profile(&deps.home)? {
        if !o.replace && setup_unfinished(&deps.home)? {
            return resume(&o, deps).await;
        }
        if !o.replace {
            return Err(QuomeError::Usage(
                "This computer is already linked. \
                 Run `quome desktop connect --replace` to link it again."
                    .into(),
            ));
        }
        disconnect(&deps.home, deps.store, deps.say).await?;
    }

    let name = o
        .name
        .clone()
        .unwrap_or_else(|| computer_name(deps.runner, &deps.hostname));
    let opts = Options {
        api_url: o.api_url,
        name: Some(name),
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
        QuomeError::CloudNotYetConnected => QuomeError::Usage(MEMBER_APPROVAL.into()),
        QuomeError::CloudState(m) if m == crate::cloud::loopback::TIMED_OUT => {
            QuomeError::Usage(NO_APPROVAL.into())
        }
        e if scope_not_allowed(&e) => QuomeError::Usage(NOT_TURNED_ON.into()),
        e => e,
    })?;

    let cfg = DesktopConfig {
        api: out.profile.api_url.clone(),
        org: out.profile.org_slug.clone(),
        screen: None,
        small_frames: o.small_frames,
        // The permissions outlive a re-link: do not ask for them again.
        panes_shown: config::load(&deps.home)?.is_some_and(|c| c.panes_shown),
    };
    let cfg = prepare_computer(cfg, deps).await?;
    config::save(&deps.home, &cfg)?;
    let cfg = start_computer(cfg, deps).await?;
    Ok(Connected {
        org_slug: out.profile.org_slug,
        org_id: out.profile.org_id,
        name: out.profile.agent_name,
        expires_at: out.profile.expires_at,
        config: cfg,
    })
}

/// After the identity step and before the config is saved: download, check and install
/// the desktop app, ask it about the displays, and pick the agents' size from the main
/// display's shape into `cfg.screen`. No main display stops here.
async fn prepare_computer(mut cfg: DesktopConfig, deps: &Deps<'_>) -> Result<DesktopConfig> {
    let api = api_for(&deps.home, deps.store)?
        .ok_or_else(|| QuomeError::Usage("This computer is not linked.".into()))?;
    (deps.say)("Downloading Quome Desktop...");
    let installed = app::install_app(&api, &deps.home, deps.runner, deps.env).await?;
    (deps.say)(&format!(
        "Installed Quome Desktop {} in {}.",
        installed.version,
        installed.path.display()
    ));
    let probe = app::probe(&installed.path, deps.runner)?;
    let (w, h, _) = app::main_display(&probe)?;
    // A size picked before (a resumed set-up) stays: the platform fixes it at the
    // first session.
    if cfg.screen.is_none() {
        cfg.screen = Some(pick_size(w, h, cfg.small_frames));
    }
    Ok(cfg)
}

/// After the config is saved: install and start both background agents. After the
/// first install only, open the two permission panes and say what to turn on.
async fn start_computer(cfg: DesktopConfig, deps: &Deps<'_>) -> Result<DesktopConfig> {
    let uid = machine_service::current_uid(deps.runner)?;
    let exe = std::env::current_exe()?;
    service::install_services(
        &deps.home,
        deps.runner,
        &exe,
        uid,
        &machine_service::Waits::REAL,
    )?;
    // Keyed on a mark saved after the agents are in, so a failed first install
    // shows the panes on the next run.
    if cfg.panes_shown {
        return Ok(cfg);
    }
    service::open_panes(deps.runner);
    (deps.say)(service::PANES_SAID);
    let cfg = DesktopConfig {
        panes_shown: true,
        ..cfg
    };
    config::save(&deps.home, &cfg)?;
    Ok(cfg)
}

fn api_for(home: &Path, store: &dyn KeyStore) -> Result<Option<CloudApi>> {
    let st = CloudState::load_from(home)?;
    let Some(p) = st.profiles.get(PROFILE) else {
        return Ok(None);
    };
    let (key, vrc) = AgentKey::load_from(store, PROFILE)?;
    Ok(Some(CloudApi::new(p, TokenSource::new(p, key, vrc))))
}

#[derive(Debug, Default)]
pub struct Disconnected {
    pub was_connected: bool,
    pub revoked: bool,
    pub revoke_error: Option<String>,
}

/// Unlink on the platform (a failure is reported, not fatal), then delete the key
/// and the `desktop` profile. The default profile is left alone.
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
            Ok(Some(api)) => tokio::time::timeout(Duration::from_secs(30), api.desktop_revoke())
                .await
                .unwrap_or_else(|_| Err(QuomeError::Usage("the request timed out".into()))),
            Ok(None) => Ok(()),
            Err(e) => Err(e),
        };
        match revoked {
            Ok(()) => report.revoked = true,
            Err(e) => {
                say(&format!(
                    "Could not unlink on the platform: {e}; remove this computer \
                     from the organization's desktops in the console"
                ));
                report.revoke_error = Some(e.to_string());
            }
        }
    }
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

pub async fn execute(command: DesktopCommands) -> Result<()> {
    if let Some((message, code)) =
        crate::desktop::unsupported(std::env::consts::OS, std::env::consts::ARCH)
    {
        eprintln!("{message}");
        std::process::exit(code);
    }
    run(command).await
}

#[cfg(target_os = "macos")]
async fn run(command: DesktopCommands) -> Result<()> {
    match command {
        DesktopCommands::Connect(a) => execute_connect(a).await,
        DesktopCommands::Serve(a) => execute_serve(a).await,
    }
}

#[cfg(not(target_os = "macos"))]
async fn run(_command: DesktopCommands) -> Result<()> {
    Err(QuomeError::Usage(crate::desktop::UNSUPPORTED.into()))
}

#[cfg(target_os = "macos")]
async fn execute_serve(a: ServeArgs) -> Result<()> {
    let home = dirs::home_dir()
        .ok_or_else(|| QuomeError::CloudState("cannot find home directory".into()))?;
    let run = crate::cloud::clients::runner::RealRunner;
    if a.install {
        let uid = machine_service::current_uid(&run)?;
        let exe = std::env::current_exe()?;
        service::install_services(&home, &run, &exe, uid, &machine_service::Waits::REAL)?;
        println!("Installed the desktop agents.");
        Ok(())
    } else if a.uninstall {
        let uid = machine_service::current_uid(&run)?;
        service::uninstall_services(&home, &run, uid)?;
        println!("Removed the desktop agents.");
        Ok(())
    } else {
        let store = crate::cloud::keys::KeychainStore;
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        tokio::spawn(async move {
            super::models::stop_signal().await;
            let _ = stop_tx.send(true);
        });
        let out = crate::desktop::serve::serve(
            &home,
            &store,
            crate::desktop::serve::Timing::default(),
            super::models::program_path(),
            stop_rx,
        )
        .await?;
        if let Some(m) = out.message() {
            println!("{m}");
        }
        let code = crate::desktop::serve::serve_exit_code(&out);
        if code != 0 {
            std::process::exit(code);
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
async fn execute_connect(a: ConnectArgs) -> Result<()> {
    let home = dirs::home_dir()
        .ok_or_else(|| QuomeError::CloudState("cannot find home directory".into()))?;
    let store = crate::cloud::keys::KeychainStore;
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
            small_frames: a.small_frames,
            replace: a.replace,
            console: None,
            issuer: None,
        },
        &deps,
    )
    .await?;
    let screen = c.config.screen.map(|(w, h)| format!("{w}x{h}"));
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
                "screen": screen,
                "small_frames": c.config.small_frames,
            }))?
        );
    } else {
        println!(
            "Linked {} to org {} as a desktop agents can use.",
            c.name, c.org_slug
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::state::Profile;
    use crate::desktop::app::fixture::{self as app_fixture, FakeMac};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Stands in for the login Keychain: keeps keys in memory, records each `put`.
    #[derive(Default)]
    struct FakeKeychain {
        keys: Mutex<HashMap<String, String>>,
        puts: Mutex<Vec<String>>,
    }

    impl KeyStore for FakeKeychain {
        fn put(&self, profile: &str, secret: &str) -> Result<()> {
            self.puts.lock().unwrap().push(profile.to_string());
            self.keys
                .lock()
                .unwrap()
                .insert(profile.to_string(), secret.to_string());
            Ok(())
        }
        fn get(&self, profile: &str) -> Result<String> {
            self.keys
                .lock()
                .unwrap()
                .get(profile)
                .cloned()
                .ok_or_else(|| QuomeError::CloudKey(format!("no key for \"{profile}\"")))
        }
        fn delete(&self, profile: &str) -> Result<()> {
            self.keys.lock().unwrap().remove(profile);
            Ok(())
        }
    }

    fn me_ok() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": null, "email": null, "display_name": null,
            "orgs": [{"id": "org-1", "slug": "acme", "name": "Acme", "role": "admin"}],
            "principal_kind": "agent", "apps_domain": "apps.example",
            "api_origin": "https://x"
        }))
    }

    async fn mount_platform(s: &MockServer, me: ResponseTemplate) {
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
            .respond_with(me)
            .mount(s)
            .await;
        let t = app_fixture::app_tarball();
        Mock::given(method("GET"))
            .and(path("/api/v1/orgs/org-1/desktops/downloads/quome-desktop"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("X-Checksum-Sha256", app_fixture::sha(&t).as_str())
                    .insert_header("X-Quome-Desktop-Version", "0.1.0")
                    .set_body_bytes(t),
            )
            .mount(s)
            .await;
    }

    /// The "browser": delivers the delegation to the loopback listener.
    fn page(url: &str) {
        let parsed = reqwest::Url::parse(url).unwrap();
        let q: HashMap<_, _> = parsed.query_pairs().into_owned().collect();
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

    struct NoClaude(std::sync::atomic::AtomicUsize);
    impl connect::ClaudeRunner for NoClaude {
        fn run(&self, _: &[String]) -> connect::ClaudeResult {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            connect::ClaudeResult::NotFound
        }
    }

    struct Harness {
        home: tempfile::TempDir,
        store: FakeKeychain,
        opened: RefCell<Vec<String>>,
        said: RefCell<Vec<String>>,
        runner: crate::cloud::clients::runner::fake::Fake,
        claude: NoClaude,
        /// The "browser" delivers the approval; false = the page refused it.
        approves: bool,
        /// `QUOME_DESKTOP_DEV=1` in the environment.
        dev_env: bool,
        listen_timeout: Duration,
    }

    impl Harness {
        fn new() -> Harness {
            Harness::with_computer_name("")
        }

        fn with_computer_name(name: &'static str) -> Harness {
            Harness::with_mac(FakeMac {
                computer_name: name.into(),
                ..FakeMac::default()
            })
        }

        fn with_mac(mac: FakeMac) -> Harness {
            Harness {
                home: tempfile::tempdir().unwrap(),
                store: FakeKeychain::default(),
                opened: RefCell::new(vec![]),
                said: RefCell::new(vec![]),
                runner: mac.runner(),
                claude: NoClaude(Default::default()),
                approves: true,
                dev_env: false,
                listen_timeout: Duration::from_secs(20),
            }
        }

        async fn connect(
            &self,
            s: &MockServer,
            edit: impl Fn(&mut ConnectOpts),
        ) -> Result<Connected> {
            let say = |m: &str| self.said.borrow_mut().push(m.to_string());
            let dev_env = self.dev_env;
            let env = move |k: &str| (dev_env && k == "QUOME_DESKTOP_DEV").then(|| "1".to_string());
            let open = |u: &str| {
                self.opened.borrow_mut().push(u.to_string());
                if self.approves {
                    page(u);
                }
                Ok(())
            };
            let deps = Deps {
                home: self.home.path().to_path_buf(),
                store: &self.store,
                open: &open,
                claude: &self.claude,
                runner: &self.runner,
                on_path: &|_: &str| true,
                env: &env,
                say: &say,
                hostname: "studio.local".into(),
                listen_timeout: self.listen_timeout,
                retry_interval: Duration::from_millis(20),
                retry_total: Duration::from_millis(200),
            };
            let mut o = ConnectOpts {
                api_url: s.uri(),
                org: None,
                name: None,
                small_frames: false,
                replace: false,
                console: Some("https://console.example".into()),
                issuer: Some(s.uri()),
            };
            edit(&mut o);
            connect(o, &deps).await
        }

        fn query(&self, i: usize) -> HashMap<String, String> {
            let url = reqwest::Url::parse(&self.opened.borrow()[i]).unwrap();
            url.query_pairs().into_owned().collect()
        }
    }

    fn seed_default(home: &Path, store: &dyn KeyStore) {
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
    async fn desktop_connect_uses_its_own_profile_and_keychain_store() {
        let s = MockServer::start().await;
        mount_platform(&s, me_ok()).await;
        let h = Harness::new();
        seed_default(h.home.path(), &h.store);
        let before = CloudState::load_from(h.home.path()).unwrap().profiles["cloud"].clone();

        let c = h.connect(&s, |_| {}).await.unwrap();
        assert_eq!((c.org_slug.as_str(), c.name.as_str()), ("acme", "studio"));

        assert_eq!(*h.store.puts.lock().unwrap(), ["cloud", "desktop"]);
        assert!(h.store.get("desktop").is_ok());
        assert_eq!(h.store.get("cloud").unwrap(), "default-key");
        // Nothing went to a key file.
        assert!(!h.home.path().join(".quome/cloud-desktop.key").exists());

        let st = CloudState::load_from(h.home.path()).unwrap();
        assert!(st.profiles.contains_key("desktop"));
        assert!(!st.profiles.contains_key("models"));
        assert_eq!(st.default, "cloud");
        assert_eq!(st.profiles["cloud"], before);
        assert_eq!(h.claude.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_first_desktop_link_does_not_become_the_default_profile() {
        let s = MockServer::start().await;
        mount_platform(&s, me_ok()).await;
        let h = Harness::new();
        h.connect(&s, |_| {}).await.unwrap();
        assert_eq!(CloudState::load_from(h.home.path()).unwrap().default, "");
    }

    #[tokio::test]
    async fn desktop_connect_requests_only_desktops_serve() {
        let s = MockServer::start().await;
        mount_platform(&s, me_ok()).await;
        let h = Harness::new();
        h.connect(&s, |o| o.org = Some("acme".into()))
            .await
            .unwrap();
        assert_eq!(h.opened.borrow().len(), 1);
        let q = h.query(0);
        assert_eq!(q["scope"], "desktops_serve");
        assert_eq!(q["org"], "acme");
        assert_eq!(q["name"], "studio");
        assert!(!q.contains_key("apps"));
    }

    #[tokio::test]
    async fn the_default_name_is_the_computers_name() {
        let s = MockServer::start().await;
        mount_platform(&s, me_ok()).await;
        let h = Harness::with_computer_name("Studio Upstairs\n");
        let c = h.connect(&s, |_| {}).await.unwrap();
        assert_eq!(c.name, "Studio Upstairs");
        assert_eq!(h.query(0)["name"], "Studio Upstairs");
        assert_eq!(h.runner.calls()[0], "scutil --get ComputerName");
        assert_eq!(
            h.runner
                .calls()
                .iter()
                .filter(|c| c.starts_with("scutil"))
                .count(),
            1
        );

        let h = Harness::new();
        let c = h
            .connect(&s, |o| o.name = Some("Front desk".into()))
            .await
            .unwrap();
        assert_eq!(c.name, "Front desk");
    }

    #[tokio::test]
    async fn connect_saves_the_desktop_config() {
        let s = MockServer::start().await;
        mount_platform(&s, me_ok()).await;
        let h = Harness::new();
        let c = h.connect(&s, |o| o.small_frames = true).await.unwrap();
        let want = DesktopConfig {
            panes_shown: true,
            api: s.uri(),
            org: "acme".into(),
            screen: Some((1440, 900)),
            small_frames: true,
        };
        assert_eq!(c.config, want);
        assert_eq!(config::load(h.home.path()).unwrap(), Some(want));
    }

    #[tokio::test]
    async fn connect_installs_the_app_and_picks_the_screen() {
        let s = MockServer::start().await;
        mount_platform(&s, me_ok()).await;
        // 3024x1890 is 16:10.
        let h = Harness::new();
        let c = h.connect(&s, |_| {}).await.unwrap();
        assert_eq!(c.config.screen, Some((1920, 1200)));
        let app = h.home.path().join("Applications/Quome Desktop.app");
        assert!(app.join("Contents/MacOS/quome-desktop").is_file());
        assert!(h.runner.calls().contains(&format!(
            "{}/Contents/MacOS/quome-desktop probe --json",
            app.display()
        )));
        // A 16:9 display, with and without --small-frames.
        let mac = FakeMac {
            probe: r#"{"displays": 1, "main_display": {"w": 2560, "h": 1440, "scale": 1.0}}"#
                .into(),
            ..FakeMac::default()
        };
        let h = Harness::with_mac(mac.clone());
        let c = h.connect(&s, |_| {}).await.unwrap();
        assert_eq!(c.config.screen, Some((1920, 1080)));
        let h = Harness::with_mac(mac);
        let c = h.connect(&s, |o| o.small_frames = true).await.unwrap();
        assert_eq!(c.config.screen, Some((1366, 768)));
        assert_eq!(
            config::load(h.home.path()).unwrap().unwrap().screen,
            Some((1366, 768))
        );
    }

    #[tokio::test]
    async fn no_display_stops_connect() {
        let s = MockServer::start().await;
        mount_platform(&s, me_ok()).await;
        let h = Harness::with_mac(FakeMac {
            probe: r#"{"displays": 0, "main_display": null}"#.into(),
            ..FakeMac::default()
        });
        let e = h.connect(&s, |_| {}).await.unwrap_err().to_string();
        assert_eq!(
            e,
            "No display is attached. Attach a monitor or an HDMI dummy plug, then run this again."
        );
        assert_eq!(config::load(h.home.path()).unwrap(), None);
    }

    #[tokio::test]
    async fn a_refused_app_stops_connect() {
        let s = MockServer::start().await;
        mount_platform(&s, me_ok()).await;
        let h = Harness::with_mac(FakeMac {
            app_team: "ZZZZZ99999".into(),
            ..FakeMac::default()
        });
        let e = h.connect(&s, |_| {}).await.unwrap_err().to_string();
        assert_eq!(e, crate::desktop::app::FOREIGN_TEAM);
        assert!(!h
            .home
            .path()
            .join("Applications/Quome Desktop.app")
            .exists());
        assert_eq!(config::load(h.home.path()).unwrap(), None);
    }

    #[tokio::test]
    async fn the_dev_setting_comes_from_the_connect_environment() {
        let s = MockServer::start().await;
        mount_platform(&s, me_ok()).await;
        let unsigned = FakeMac {
            self_team: None,
            ..FakeMac::default()
        };
        let h = Harness::with_mac(unsigned.clone());
        let e = h.connect(&s, |_| {}).await.unwrap_err().to_string();
        assert_eq!(e, crate::desktop::app::UNSIGNED);
        let mut h = Harness::with_mac(unsigned);
        h.dev_env = true;
        h.connect(&s, |_| {}).await.unwrap();
    }

    #[tokio::test]
    async fn an_unfinished_setup_resumes_without_a_second_approval() {
        let s = MockServer::start().await;
        mount_platform(&s, me_ok()).await;
        let mut h = Harness::with_mac(FakeMac {
            probe: r#"{"displays": 0, "main_display": null}"#.into(),
            ..FakeMac::default()
        });
        let e = h.connect(&s, |_| {}).await.unwrap_err().to_string();
        assert!(e.starts_with("No display is attached."), "{e}");
        assert_eq!(config::load(h.home.path()).unwrap(), None);

        // A display is attached now; running connect again finishes the set-up.
        h.runner = FakeMac::default().runner();
        let c = h.connect(&s, |o| o.small_frames = true).await.unwrap();
        assert_eq!(h.opened.borrow().len(), 1, "no second approval");
        assert_eq!(*h.store.puts.lock().unwrap(), ["desktop"]);
        assert_eq!((c.org_slug.as_str(), c.name.as_str()), ("acme", "studio"));
        assert_eq!(c.config.screen, Some((1440, 900)));
        assert_eq!(config::load(h.home.path()).unwrap(), Some(c.config.clone()));

        // Finished now: a plain re-run refuses as before.
        let e = h.connect(&s, |_| {}).await.unwrap_err().to_string();
        assert!(e.starts_with("This computer is already linked."), "{e}");

        // The app removed: it resumes, reinstalls, and keeps the screen it had.
        std::fs::remove_dir_all(h.home.path().join("Applications/Quome Desktop.app")).unwrap();
        let c = h.connect(&s, |o| o.small_frames = false).await.unwrap();
        assert_eq!(h.opened.borrow().len(), 1);
        assert_eq!(c.config.screen, Some((1440, 900)));
        assert!(h
            .home
            .path()
            .join("Applications/Quome Desktop.app/Contents/MacOS/quome-desktop")
            .is_file());

        // Another org is not a resume.
        std::fs::remove_file(config::config_path(h.home.path())).unwrap();
        let e = h
            .connect(&s, |o| o.org = Some("other".into()))
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("linked to org \"acme\", not \"other\""), "{e}");
        assert_eq!(h.opened.borrow().len(), 1);
    }

    #[tokio::test]
    async fn replace_links_afresh_even_when_setup_is_unfinished() {
        let s = MockServer::start().await;
        mount_platform(&s, me_ok()).await;
        Mock::given(method("DELETE"))
            .and(path("/api/v1/orgs/org-1/desktops/computers/me"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&s)
            .await;
        let mut h = Harness::with_mac(FakeMac {
            probe: r#"{"displays": 0, "main_display": null}"#.into(),
            ..FakeMac::default()
        });
        assert!(h.connect(&s, |_| {}).await.is_err());
        h.runner = FakeMac::default().runner();
        h.connect(&s, |o| o.replace = true).await.unwrap();
        assert_eq!(h.opened.borrow().len(), 2, "a fresh approval");
        assert_eq!(*h.store.puts.lock().unwrap(), ["desktop", "desktop"]);
    }

    #[tokio::test]
    async fn wrong_org_withdraws_computer() {
        let s = MockServer::start().await;
        mount_platform(&s, me_ok()).await;
        Mock::given(method("DELETE"))
            .and(path("/api/v1/orgs/org-1/desktops/computers/me"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&s)
            .await;
        let h = Harness::new();
        let e = h
            .connect(&s, |o| o.org = Some("other".into()))
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("\"acme\"") && e.contains("\"other\""), "{e}");
        assert!(
            e.contains("The approval for this computer was withdrawn."),
            "{e}"
        );
        assert!(!e.contains("AT"), "{e}");
        let seen = s.received_requests().await.unwrap();
        assert!(!seen
            .iter()
            .any(|r| r.method.as_str() == "DELETE" && r.url.path().contains("/models/")));
        assert!(h.store.puts.lock().unwrap().is_empty());
        assert!(!h.home.path().join(".quome/cloud.json").exists());
        assert_eq!(config::load(h.home.path()).unwrap(), None);
    }

    #[tokio::test]
    async fn member_approval_message() {
        let s = MockServer::start().await;
        mount_platform(
            &s,
            ResponseTemplate::new(401).set_body_json(
                serde_json::json!({"detail": "unauthorized", "reason": "agent_not_connected"}),
            ),
        )
        .await;
        let h = Harness::new();
        let e = h.connect(&s, |_| {}).await.unwrap_err().to_string();
        assert_eq!(
            e,
            "Only an Owner or Admin can approve a computer. \
             Ask one to approve it, or to run this command with you."
        );
        assert!(h.store.puts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn not_turned_on_message() {
        let s = MockServer::start().await;
        mount_platform(
            &s,
            ResponseTemplate::new(422).set_body_json(
                serde_json::json!({"detail": "not allowed", "reason": "scope_not_allowed"}),
            ),
        )
        .await;
        let h = Harness::new();
        let e = h.connect(&s, |_| {}).await.unwrap_err().to_string();
        assert_eq!(
            e,
            "Linking computers is not turned on for this organization."
        );
    }

    #[tokio::test]
    async fn no_approval_names_the_likely_cause() {
        let s = MockServer::start().await;
        mount_platform(&s, me_ok()).await;
        let mut h = Harness::new();
        h.approves = false;
        h.listen_timeout = Duration::from_millis(200);
        let e = h.connect(&s, |_| {}).await.unwrap_err().to_string();
        assert_eq!(
            e,
            "No approval arrived. If the approval page said this permission isn't available, \
             linking computers is not turned on for this organization: ask an Owner or Admin."
        );
        assert!(h.store.puts.lock().unwrap().is_empty());
        assert!(!h.home.path().join(".quome/cloud.json").exists());
    }

    #[tokio::test]
    async fn connect_refuses_when_linked_and_replace_unlinks_first() {
        let s = MockServer::start().await;
        mount_platform(&s, me_ok()).await;
        Mock::given(method("DELETE"))
            .and(path("/api/v1/orgs/org-1/desktops/computers/me"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&s)
            .await;
        let h = Harness::new();
        h.connect(&s, |_| {}).await.unwrap();
        let e = h.connect(&s, |_| {}).await.unwrap_err().to_string();
        assert_eq!(
            e,
            "This computer is already linked. Run `quome desktop connect --replace` to link it again."
        );
        assert_eq!(h.opened.borrow().len(), 1, "no second browser trip");
        h.connect(&s, |o| o.replace = true).await.unwrap();
        assert_eq!(h.opened.borrow().len(), 2);
        assert_eq!(*h.store.puts.lock().unwrap(), ["desktop", "desktop"]);
    }

    #[tokio::test]
    async fn disconnect_removes_only_the_desktop() {
        let s = MockServer::start().await;
        mount_platform(&s, me_ok()).await;
        Mock::given(method("DELETE"))
            .and(path("/api/v1/orgs/org-1/desktops/computers/me"))
            .respond_with(
                ResponseTemplate::new(500).set_body_json(serde_json::json!({"detail": "boom"})),
            )
            .mount(&s)
            .await;
        let h = Harness::new();
        seed_default(h.home.path(), &h.store);
        h.connect(&s, |_| {}).await.unwrap();
        let said = RefCell::new(vec![]);
        let r = disconnect(h.home.path(), &h.store, &|m| {
            said.borrow_mut().push(m.to_string())
        })
        .await
        .unwrap();
        assert!(r.was_connected && !r.revoked && r.revoke_error.is_some());
        assert!(said.borrow().join("\n").contains("Could not unlink"));
        let st = CloudState::load_from(h.home.path()).unwrap();
        assert!(st.profiles.contains_key("cloud") && !st.profiles.contains_key("desktop"));
        assert_eq!(st.default, "cloud");
        assert!(h.store.get("desktop").is_err());
        assert_eq!(h.store.get("cloud").unwrap(), "default-key");
        let r = disconnect(h.home.path(), &h.store, &|_| {}).await.unwrap();
        assert!(!r.was_connected);
    }

    #[tokio::test]
    async fn connect_installs_agents_and_opens_panes_once() {
        let s = MockServer::start().await;
        mount_platform(&s, me_ok()).await;
        let h = Harness::new();
        h.connect(&s, |_| {}).await.unwrap();
        assert!(service::plist_path(h.home.path(), service::SERVE_LABEL).exists());
        assert!(service::plist_path(h.home.path(), service::APP_LABEL).exists());
        let calls = h.runner.calls();
        let opens: Vec<_> = calls.iter().filter(|c| c.starts_with("open ")).collect();
        assert_eq!(opens.len(), 2, "{calls:?}");
        assert!(calls
            .iter()
            .any(|c| c.starts_with("launchctl bootstrap gui/501 ")));
        // A second set-up (replace) does not say it again.
        let before = h
            .said
            .borrow()
            .iter()
            .filter(|m| m.contains("Turn on Quome Desktop"))
            .count();
        assert_eq!(before, 1);
        h.connect(&s, |o| o.replace = true).await.unwrap();
        let after = h
            .said
            .borrow()
            .iter()
            .filter(|m| m.contains("Turn on Quome Desktop"))
            .count();
        assert_eq!(after, 1);
    }

    #[tokio::test]
    async fn a_failed_first_install_shows_the_panes_on_the_retry() {
        let s = MockServer::start().await;
        mount_platform(&s, me_ok()).await;
        let mac = FakeMac::default();
        // Bootstrap fails the first time through (both tries), then works.
        mac.bootstrap_failures
            .store(2, std::sync::atomic::Ordering::SeqCst);
        let h = Harness::with_mac(mac);
        assert!(h.connect(&s, |_| {}).await.is_err());
        let shown = |h: &Harness| {
            h.said
                .borrow()
                .iter()
                .filter(|m| m.contains("Turn on Quome Desktop"))
                .count()
        };
        assert_eq!(shown(&h), 0);
        assert!(!h.runner.calls().iter().any(|c| c.starts_with("open ")));
        h.connect(&s, |_| {}).await.unwrap();
        assert_eq!(shown(&h), 1);
        assert_eq!(
            h.runner
                .calls()
                .iter()
                .filter(|c| c.starts_with("open "))
                .count(),
            2
        );
        h.connect(&s, |o| o.replace = true).await.unwrap();
        assert_eq!(shown(&h), 1);
    }

    #[test]
    fn serve_flags() {
        use clap::Parser;
        #[derive(Parser)]
        struct Cli {
            #[command(subcommand)]
            c: DesktopCommands,
        }
        let cli = Cli::try_parse_from(["desktop", "serve", "--install"]).unwrap();
        let DesktopCommands::Serve(a) = cli.c else {
            panic!()
        };
        assert!(a.install && !a.uninstall);
        assert!(Cli::try_parse_from(["desktop", "serve", "--install", "--uninstall"]).is_err());
        assert!(DesktopCommands::Serve(ServeArgs {
            install: false,
            uninstall: false
        })
        .runs_in_background());
    }

    #[test]
    fn connect_flags() {
        use clap::Parser;
        #[derive(Parser)]
        struct Cli {
            #[command(subcommand)]
            c: DesktopCommands,
        }
        let cli = Cli::try_parse_from([
            "desktop",
            "connect",
            "https://api.example.test",
            "--org",
            "acme",
            "--name",
            "Studio",
            "--small-frames",
            "--replace",
            "--json",
        ])
        .unwrap();
        let DesktopCommands::Connect(a) = cli.c else {
            panic!()
        };
        assert_eq!(a.api_url, "https://api.example.test");
        assert_eq!(a.org.as_deref(), Some("acme"));
        assert_eq!(a.name.as_deref(), Some("Studio"));
        assert!(a.small_frames && a.replace && a.json);
        assert!(Cli::try_parse_from(["desktop", "connect", "--console", "x"]).is_err());
    }
}
