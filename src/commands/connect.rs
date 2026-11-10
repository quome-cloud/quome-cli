//! `quome connect`: delegate this computer's agent key to Quome Cloud through
//! the browser, then save the profile and register the MCP server.
use clap::Parser;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use rand_core::RngCore;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::cloud::api::CloudApi;
use crate::cloud::auth::TokenSource;
use crate::cloud::clients::runner::{CommandRunner, RealRunner};
use crate::cloud::clients::{adapters, paths, ClientId, Ctx, RegisterOutcome};
use crate::cloud::keys::{platform_store, AgentKey, KeyStore};
use crate::cloud::loopback::{self, Callback, Listener};
use crate::cloud::state::{CloudState, Profile};
use crate::errors::{QuomeError, Result};

pub const DEFAULT_API_URL: &str = "https://api.quome.cloud";
pub const DEFAULT_PROFILE: &str = "cloud";
/// The identity service's limit on an agent's display name.
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
    /// Do not register the MCP server or install skills in any agent client
    #[arg(long)]
    no_mcp: bool,
    /// Set up only these clients: claude, codex, opencode, antigravity
    /// (default: every installed client)
    #[arg(long, value_delimiter = ',', value_parser = ClientId::parse)]
    client: Vec<ClientId>,
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

/// What happened to the MCP registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeResult {
    /// `claude` is not on PATH.
    NotFound,
    /// `claude` ran; its stdout is discarded, its stderr kept for failures.
    Ran { ok: bool, stderr: String },
}

pub trait ClaudeRunner {
    fn run(&self, args: &[String]) -> ClaudeResult;
}

struct RealClaude {
    program: String,
}

impl ClaudeRunner for RealClaude {
    fn run(&self, args: &[String]) -> ClaudeResult {
        use crate::cloud::clients::runner::{CommandRunner, RealRunner, RunResult};
        match RealRunner.run(&self.program, args) {
            RunResult::NotFound => ClaudeResult::NotFound,
            RunResult::Ran { ok, stderr, .. } => ClaudeResult::Ran { ok, stderr },
        }
    }
}

/// Everything `run` touches outside this process, injected for tests.
pub struct Deps<'a> {
    pub home: PathBuf,
    pub store: &'a dyn KeyStore,
    pub open: &'a dyn Fn(&str) -> std::io::Result<()>,
    pub claude: &'a dyn ClaudeRunner,
    /// Runs the other clients' own CLIs (`codex mcp add`, ...).
    pub runner: &'a dyn CommandRunner,
    /// True when a program is on PATH (client detection).
    pub on_path: &'a (dyn Fn(&str) -> bool + Sync),
    pub env: &'a dyn Fn(&str) -> Option<String>,
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
    /// Clients to set up; empty means every installed client.
    pub clients: Vec<ClientId>,
}

#[derive(Debug)]
pub struct Outcome {
    pub profile_name: String,
    pub profile: Profile,
    pub mcp: &'static str,
    /// Per selected client: how its MCP registration and skills went.
    pub clients: Vec<ClientOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientOutcome {
    pub client: ClientId,
    /// added, updated, unchanged, manual or skipped.
    pub mcp: &'static str,
    pub skills_written: usize,
    pub skills_removed: usize,
}

impl ClientOutcome {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "client": self.client.name(),
            "mcp": self.mcp,
            "skills": {"written": self.skills_written, "removed": self.skills_removed},
        })
    }
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

/// Unicode format characters (`\p{Cf}`); control characters are `char::is_control`.
fn is_format(c: char) -> bool {
    matches!(c as u32,
        0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x890..=0x891 | 0x8E2 | 0x180E
        | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF
        | 0xFFF9..=0xFFFB | 0x110BD | 0x110CD | 0x13430..=0x1343F | 0x1BCA0..=0x1BCA3
        | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F)
}

fn clean(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() && !is_format(*c))
        .collect()
}

/// The agent's display name: control and format characters removed, at most
/// 64 UTF-16 code units (the console's `name.length`), cut on a character boundary.
pub fn agent_name(given: Option<&str>, hostname: &str) -> String {
    let given = given.map(clean).filter(|n| !n.trim().is_empty());
    let n = given.unwrap_or_else(|| {
        let host = clean(hostname);
        if host.trim().is_empty() {
            "Claude Code".to_string()
        } else {
            format!("Claude Code on {host}")
        }
    });
    let mut units = 0;
    let mut out = String::new();
    for c in n.chars() {
        units += c.len_utf16();
        if units > MAX_NAME {
            break;
        }
        out.push(c);
    }
    out
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

#[cfg(test)]
pub fn mcp_add_args(profile: &str) -> Vec<String> {
    mcp_add_args_for(&mcp_program(), profile)
}

pub fn mcp_add_args_for(program: &str, profile: &str) -> Vec<String> {
    [
        "mcp",
        "add",
        "--scope",
        "user",
        "quome",
        "--",
        program,
        "mcp",
        "--profile",
        profile,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// The program the MCP client should launch: plain `quome` when that is this
/// binary on PATH (so a Homebrew upgrade keeps working), otherwise this
/// binary's absolute path, so an older `quome` earlier on PATH is never used.
pub fn mcp_program() -> String {
    let exe = std::env::current_exe().ok();
    let on_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join("quome"))
            .find(|p| p.is_file())
    });
    mcp_program_from(exe.as_deref(), on_path.as_deref())
}

pub fn mcp_program_from(
    exe: Option<&std::path::Path>,
    on_path: Option<&std::path::Path>,
) -> String {
    let canon = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    match (exe, on_path) {
        (Some(e), Some(p)) if canon(e) == canon(p) => "quome".to_string(),
        (Some(e), _) => canon(e).to_string_lossy().into_owned(),
        (None, _) => "quome".to_string(),
    }
}

pub fn mcp_get_args() -> Vec<String> {
    ["mcp", "get", "quome"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

pub fn mcp_remove_args() -> Vec<String> {
    ["mcp", "remove", "--scope", "user", "quome"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

pub fn mcp_config_json(profile: &str) -> String {
    mcp_config_json_for(&mcp_program(), profile)
}

pub fn mcp_config_json_for(program: &str, profile: &str) -> String {
    serde_json::json!({"mcpServers": {"quome": {
        "command": program, "args": ["mcp", "--profile", profile]}}})
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

/// A throwaway in-memory store, used to hand the new key to `TokenSource`.
#[derive(Default)]
struct MemStore(std::sync::Mutex<std::collections::HashMap<String, String>>);

impl KeyStore for MemStore {
    fn put(&self, profile: &str, secret: &str) -> Result<()> {
        self.0.lock().unwrap().insert(profile.into(), secret.into());
        Ok(())
    }
    fn get(&self, profile: &str) -> Result<String> {
        self.0
            .lock()
            .unwrap()
            .get(profile)
            .cloned()
            .ok_or_else(|| QuomeError::CloudKey("no key".into()))
    }
    fn delete(&self, profile: &str) -> Result<()> {
        self.0.lock().unwrap().remove(profile);
        Ok(())
    }
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
    let api_url = crate::url_policy::validate_api_url(&opts.api_url, "API URL")?;
    let console = console_url(&api_url, opts.console.as_deref());
    let issuer = issuer_url(&api_url, opts.issuer.as_deref());
    crate::url_policy::validate_api_url(&console, "console URL")?;
    crate::url_policy::validate_api_url(&issuer, "issuer URL")?;
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

    // Authenticate from memory; nothing is persisted until /me succeeds.
    let mem = MemStore::default();
    key.store_in(&mem, &profile_name, &cb.vrc)?;
    let (tmp_key, vrc) = AgentKey::load_from(&mem, &profile_name)?;
    let api = CloudApi::new(&profile, TokenSource::new(&profile, tmp_key, vrc));
    let me = me_with_retry(&api, deps.retry_interval, deps.retry_total).await?;
    let org = me
        .orgs
        .first()
        .ok_or_else(|| QuomeError::Usage("This agent has no Quome Cloud organization.".into()))?;
    profile.org_id = org.id.clone();
    profile.org_slug = org.slug.clone();
    profile.apps_domain = me.apps_domain;

    key.store_in(deps.store, &profile_name, &cb.vrc)?;
    let mut st = CloudState::load_from(&deps.home)?;
    st.profiles.insert(profile_name.clone(), profile.clone());
    if st.default.is_empty() || !opts.profile_given {
        st.default = profile_name.clone();
    }
    st.save_to(&deps.home)?;

    let explicit = !opts.clients.is_empty();
    let selected: Vec<ClientId> = if explicit {
        opts.clients.clone()
    } else {
        ClientId::ALL.to_vec()
    };
    let mcp = if opts.no_mcp || !selected.contains(&ClientId::Claude) {
        "skipped"
    } else {
        register_mcp(deps, &profile_name)
    };
    let mut results: Vec<ClientOutcome> = selected
        .iter()
        .map(|c| ClientOutcome {
            client: *c,
            mcp: if *c == ClientId::Claude {
                mcp
            } else {
                "skipped"
            },
            skills_written: 0,
            skills_removed: 0,
        })
        .collect();
    if !opts.no_mcp {
        for (id, m) in setup_other_clients(deps, &selected, explicit, &profile_name) {
            if let Some(r) = results.iter_mut().find(|r| r.client == id) {
                r.mcp = m;
            }
        }
        // `api` above was built before the org was known; make one that has it.
        let synced = match AgentKey::load_from(deps.store, &profile_name) {
            Ok((k, v)) => {
                let api =
                    std::sync::Arc::new(CloudApi::new(&profile, TokenSource::new(&profile, k, v)));
                crate::commands::skills::sync_skills_with(
                    &api,
                    &deps.home,
                    &selected,
                    deps.on_path,
                    crate::commands::skills::SyncOpts::explicit(&profile_name),
                )
                .await
            }
            Err(e) => Err(e),
        };
        match synced {
            Ok(r) => {
                for c in &r.clients {
                    if let Some(o) = results.iter_mut().find(|o| o.client == c.client) {
                        o.skills_written += c.written;
                        o.skills_removed += c.removed;
                    }
                    (deps.say)(&format!(
                        "Skills for {}: {} written, {} removed, {} unchanged.",
                        c.client.name(),
                        c.written,
                        c.removed,
                        c.unchanged
                    ));
                }
                for w in &r.warnings {
                    (deps.say)(&format!("Skills: {w}"));
                }
                for (c, e) in &r.failures {
                    (deps.say)(&format!(
                        "Could not write {} skills: {e}. Run `quome skills sync` to retry.",
                        c.name()
                    ));
                }
            }
            Err(e) => (deps.say)(&format!(
                "Could not install skills: {e}. Run `quome skills sync` to retry."
            )),
        }
    }

    if previous {
        (deps.say)(&format!(
            "The previous connection is still active until it expires or you revoke it in the console: {console}/dashboard"
        ));
    }
    Ok(Outcome {
        profile_name,
        profile,
        mcp,
        clients: results,
    })
}

/// A config snippet for adding the server by hand to a client.
fn manual_snippet(client: ClientId, program: &str, profile: &str) -> String {
    match client {
        ClientId::Codex => format!(
            "[mcp_servers.quome]\ncommand = {program:?}\nargs = [\"mcp\", \"--profile\", {profile:?}]"
        ),
        ClientId::OpenCode => format!(
            "opencode mcp add quome --global -- {program} mcp --profile {profile}\nor, in opencode.json:\n{{\"mcp\":{{\"servers\":{{\"quome\":{{\"type\":\"local\",\"command\":[{program:?},\"mcp\",\"--profile\",{profile:?}]}}}}}}}}"
        ),
        _ => mcp_config_json_for(program, profile),
    }
}

/// Register the quome MCP server with every selected client except Claude Code
/// (handled by `register_mcp`). One line per failure plus the manual snippet;
/// never an error.
fn setup_other_clients(
    deps: &Deps<'_>,
    selected: &[ClientId],
    explicit: bool,
    profile: &str,
) -> Vec<(ClientId, &'static str)> {
    let mut done = Vec::new();
    let program = mcp_program();
    let ctx = Ctx {
        home: &deps.home,
        runner: deps.runner,
        env: deps.env,
    };
    for a in adapters() {
        let id = a.id();
        if id == ClientId::Claude || !selected.contains(&id) {
            continue;
        }
        if !paths::installed_with(id, &deps.home, deps.on_path) {
            if explicit {
                (deps.say)(&format!("{} is not installed; skipped.", id.name()));
            }
            continue;
        }
        match a.register_mcp(&ctx, &program, profile) {
            Ok(RegisterOutcome::Added) => {
                done.push((id, "added"));
                (deps.say)(&format!(
                    "Registered the quome MCP server with {} (profile {profile}).",
                    id.name()
                ))
            }
            Ok(RegisterOutcome::Updated) => {
                done.push((id, "updated"));
                (deps.say)(&format!(
                    "Updated the quome MCP server in {} (profile {profile}).",
                    id.name()
                ))
            }
            Ok(RegisterOutcome::Unchanged) => {
                done.push((id, "unchanged"));
                (deps.say)(&format!(
                    "The quome MCP server in {} is already set up.",
                    id.name()
                ))
            }
            Err(why) => {
                done.push((id, "manual"));
                (deps.say)(&format!(
                    "Could not register with {}: {why}. Add this to its MCP config:",
                    id.name()
                ));
                (deps.say)(&manual_snippet(id, &program, profile));
            }
        }
    }
    done
}

/// How the Claude Code registration went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeMcp {
    Added,
    Updated,
    /// `claude` is not on PATH.
    NotFound,
    Failed(String),
}

/// Point Claude Code's user-scope `quome` MCP server at `profile` through
/// `claude mcp`, replacing an existing one. Shared with the client adapters.
pub fn claude_register(runner: &dyn ClaudeRunner, program: &str, profile: &str) -> ClaudeMcp {
    let exists = match runner.run(&mcp_get_args()) {
        ClaudeResult::NotFound => return ClaudeMcp::NotFound,
        ClaudeResult::Ran { ok, .. } => ok,
    };
    if exists {
        let _ = runner.run(&mcp_remove_args());
    }
    match runner.run(&mcp_add_args_for(program, profile)) {
        ClaudeResult::Ran { ok: true, .. } if exists => ClaudeMcp::Updated,
        ClaudeResult::Ran { ok: true, .. } => ClaudeMcp::Added,
        ClaudeResult::Ran { stderr, .. } => ClaudeMcp::Failed(stderr.trim().to_string()),
        ClaudeResult::NotFound => ClaudeMcp::NotFound,
    }
}

/// Point Claude Code's user-scope `quome` MCP server at `profile`, replacing an
/// existing one. Returns "added", "updated" or "manual".
fn register_mcp(deps: &Deps<'_>, profile: &str) -> &'static str {
    let manual = |why: Option<&str>| {
        if let Some(w) = why {
            (deps.say)(w);
        }
        (deps.say)("Add this to your MCP client config:");
        (deps.say)(&mcp_config_json(profile));
        "manual"
    };
    match claude_register(deps.claude, &mcp_program(), profile) {
        ClaudeMcp::Updated => {
            (deps.say)(&format!(
                "Updated the quome MCP server in Claude Code (profile {profile})."
            ));
            "updated"
        }
        ClaudeMcp::Added => {
            (deps.say)(&format!(
                "Registered the quome MCP server with Claude Code (profile {profile})."
            ));
            "added"
        }
        ClaudeMcp::Failed(stderr) => {
            let why = format!("`claude mcp add` failed: {stderr}");
            manual(Some(&why))
        }
        ClaudeMcp::NotFound => manual(None),
    }
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
        claude: &RealClaude {
            program: "claude".into(),
        },
        runner: &RealRunner,
        on_path: &paths::on_real_path,
        env: &crate::cloud::clients::real_env,
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
        clients: args.client,
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
                "clients": out.clients.iter().map(ClientOutcome::to_json).collect::<Vec<_>>(),
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
            console_url("https://api.staging.example.com", None),
            "https://console.staging.example.com"
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
    fn mcp_program_is_plain_quome_only_when_path_resolves_to_this_binary() {
        let d = tempfile::tempdir().unwrap();
        let me = d.path().join("quome-built");
        std::fs::write(&me, b"x").unwrap();
        let other = d.path().join("quome");
        std::fs::write(&other, b"y").unwrap();
        let link = d.path().join("quome-link");
        std::os::unix::fs::symlink(&me, &link).unwrap();
        let abs = std::fs::canonicalize(&me)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        // An older quome earlier on PATH: launch this binary by absolute path.
        assert_eq!(mcp_program_from(Some(&me), Some(&other)), abs);
        // No quome on PATH at all.
        assert_eq!(mcp_program_from(Some(&me), None), abs);
        // PATH's quome is a symlink to this binary (Homebrew): keep the name.
        assert_eq!(mcp_program_from(Some(&me), Some(&link)), "quome");
        assert_eq!(mcp_program_from(None, None), "quome");
    }

    #[test]
    fn mcp_argv_and_config_are_exact() {
        assert_eq!(
            mcp_add_args_for("quome", "work").join(" "),
            "mcp add --scope user quome -- quome mcp --profile work"
        );
        assert_eq!(
            mcp_config_json_for("quome", "work"),
            r#"{"mcpServers":{"quome":{"command":"quome","args":["mcp","--profile","work"]}}}"#
        );
    }

    fn ran(ok: bool) -> ClaudeResult {
        ClaudeResult::Ran {
            ok,
            stderr: if ok {
                String::new()
            } else {
                "boom: nope\n".into()
            },
        }
    }

    /// Answers `mcp get` from `exists`; everything else with `result`.
    struct FakeClaude {
        calls: Mutex<Vec<Vec<String>>>,
        result: ClaudeResult,
        exists: Mutex<bool>,
    }
    impl ClaudeRunner for FakeClaude {
        fn run(&self, args: &[String]) -> ClaudeResult {
            self.calls.lock().unwrap().push(args.to_vec());
            if args.get(1).map(String::as_str) == Some("get") {
                return ran(*self.exists.lock().unwrap());
            }
            self.result.clone()
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
            clients: vec![],
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
        runner: crate::cloud::clients::runner::fake::Fake,
        /// Programs reported as on PATH.
        on_path: Vec<&'static str>,
    }

    impl Harness {
        fn new(result: ClaudeResult) -> Harness {
            Harness {
                home: tempfile::tempdir().unwrap(),
                said: RefCell::new(vec![]),
                claude: FakeClaude {
                    calls: Mutex::new(vec![]),
                    result,
                    exists: Mutex::new(false),
                },
                opened: RefCell::new(vec![]),
                log: Arc::new(Mutex::new(vec![])),
                runner: crate::cloud::clients::runner::fake::Fake::new(|p, a| {
                    use crate::cloud::clients::runner::fake::{fail, ok};
                    if p == "codex" && a.get(1).map(String::as_str) == Some("get") {
                        fail("no MCP server named quome")
                    } else {
                        ok("")
                    }
                }),
                on_path: vec![],
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
            let on_path = self.on_path.clone();
            let deps = Deps {
                home: self.home.path().to_path_buf(),
                store: &store,
                open: &open,
                claude: &self.claude,
                runner: &self.runner,
                on_path: &|b: &str| on_path.contains(&b),
                env: &crate::cloud::clients::no_env,
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
        let h = Harness::new(ran(true));
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
            [mcp_get_args(), mcp_add_args("work")]
        );
        assert_eq!(out.mcp, "added");
        let said = h.said.borrow().join("\n");
        assert!(said.contains("Registered the quome MCP server"), "{said}");
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
        let h = Harness::new(ran(true));
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
            runner: &h.runner,
            on_path: &|_: &str| false,
            env: &crate::cloud::clients::no_env,
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
    async fn failed_connect_leaves_old_connection_untouched() {
        let s = MockServer::start().await;
        mount_auth(&s).await;
        mount_me(&s, 0).await;
        let h = Harness::new(ran(true));
        let old = h
            .connect(
                opts(&s, "work"),
                "del-old",
                "mac",
                Duration::from_millis(10),
            )
            .await
            .unwrap();
        let state_path = h.home.path().join(".quome/cloud.json");
        let key_path = h.home.path().join(".quome/cloud-work.key");
        let (state_before, key_before) = (
            std::fs::read(&state_path).unwrap(),
            std::fs::read(&key_path).unwrap(),
        );

        let s2 = MockServer::start().await;
        mount_auth(&s2).await;
        mount_me(&s2, 100_000).await;
        let store = FileStore {
            dir: h.home.path().join(".quome"),
        };
        let say = |_: &str| {};
        let open = |u: &str| {
            page(u, "del-new", h.log.clone());
            Ok(())
        };
        let deps = Deps {
            home: h.home.path().to_path_buf(),
            store: &store,
            open: &open,
            claude: &h.claude,
            runner: &h.runner,
            on_path: &|_: &str| false,
            env: &crate::cloud::clients::no_env,
            say: &say,
            hostname: "mac".into(),
            listen_timeout: Duration::from_secs(20),
            retry_interval: Duration::from_millis(20),
            retry_total: Duration::from_millis(100),
        };
        let err = run(opts(&s2, "work"), &deps).await.unwrap_err();
        assert!(matches!(err, QuomeError::CloudNotYetConnected));
        assert_eq!(std::fs::read(&state_path).unwrap(), state_before);
        assert_eq!(std::fs::read(&key_path).unwrap(), key_before);
        let st = CloudState::load_from(h.home.path()).unwrap();
        assert_eq!(st.profiles["work"], old.profile);
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
        assert_eq!(second.mcp, "manual");
    }

    #[tokio::test]
    async fn no_mcp_skips_claude() {
        let s = MockServer::start().await;
        mount_auth(&s).await;
        mount_me(&s, 0).await;
        let h = Harness::new(ran(true));
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

    async fn mount_playbooks(s: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/api/v1/orgs/org-1/agent-playbooks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"name":"ship","area":"apps","title":"Ship","description":"d",
                 "arguments":[],"requires_tools":[],"allowed":true}
            ])))
            .mount(s)
            .await;
    }

    fn mkdirs(h: &Harness, dirs: &[&str]) {
        for d in dirs {
            std::fs::create_dir_all(h.home.path().join(d)).unwrap();
        }
    }

    #[tokio::test]
    async fn connect_sets_up_every_installed_client() {
        let s = MockServer::start().await;
        mount_auth(&s).await;
        mount_me(&s, 0).await;
        mount_playbooks(&s).await;
        let h = Harness::new(ran(true));
        mkdirs(&h, &[".codex", ".config/opencode", ".claude"]);
        let out = h
            .connect(opts(&s, "work"), "d", "mac", Duration::from_millis(10))
            .await
            .unwrap();
        assert_eq!(out.mcp, "added");
        let by = |c: ClientId| out.clients.iter().find(|o| o.client == c).unwrap();
        assert_eq!(by(ClientId::Claude).mcp, "added");
        assert_eq!(by(ClientId::Claude).skills_written, 1);
        assert_eq!(by(ClientId::Codex).mcp, "added");
        assert_eq!(by(ClientId::Codex).skills_written, 1);
        assert_eq!(by(ClientId::Antigravity).mcp, "skipped");
        assert_eq!(by(ClientId::Antigravity).skills_written, 0);
        let j = by(ClientId::Codex).to_json();
        assert_eq!(j["client"], "codex");
        assert_eq!(j["skills"], serde_json::json!({"written": 1, "removed": 0}));
        let calls = h.runner.calls();
        assert!(
            calls.iter().any(|c| c.starts_with("codex mcp add quome")),
            "{calls:?}"
        );
        assert!(
            calls
                .iter()
                .any(|c| c.starts_with("opencode mcp add quome")),
            "{calls:?}"
        );
        assert!(!calls
            .iter()
            .any(|c| c.starts_with("agy") || c.contains("antigravity")));
        let home = h.home.path();
        assert!(home.join(".agents/skills/quome-ship/SKILL.md").is_file());
        assert!(home
            .join(".claude/skills/quome/skills/ship/SKILL.md")
            .is_file());
        assert!(!home.join(".gemini").exists());
    }

    #[tokio::test]
    async fn connect_client_flag_limits() {
        let s = MockServer::start().await;
        mount_auth(&s).await;
        mount_me(&s, 0).await;
        mount_playbooks(&s).await;
        let h = Harness::new(ran(true));
        mkdirs(&h, &[".codex", ".claude", ".gemini/antigravity"]);
        let mut o = opts(&s, "work");
        o.clients = vec![ClientId::Codex];
        let out = h
            .connect(o, "d", "mac", Duration::from_millis(10))
            .await
            .unwrap();
        assert_eq!(out.mcp, "skipped");
        assert!(h.claude.calls.lock().unwrap().is_empty());
        assert!(h.runner.calls().iter().all(|c| c.starts_with("codex ")));
        let home = h.home.path();
        assert!(home.join(".agents/skills/quome-ship/SKILL.md").is_file());
        assert!(!home.join(".claude/skills").exists());
        assert!(!home.join(".gemini/config").exists());
        // `quome mcp`'s background refresh only touches the root connect made.
        let st = CloudState::load_from(home).unwrap();
        let store = FileStore {
            dir: home.join(".quome"),
        };
        let (k, v) = AgentKey::load_from(&store, "work").unwrap();
        let p = st.profile(Some("work")).unwrap().clone();
        let api = std::sync::Arc::new(CloudApi::new(&p, TokenSource::new(&p, k, v)));
        let none = |_: &str| false;
        // An unchanged catalog is skipped; remove a skill so the refresh acts.
        std::fs::remove_dir_all(home.join(".agents/skills/quome-ship")).unwrap();
        let r = crate::commands::skills::sync_skills_with(
            &api,
            home,
            &ClientId::ALL,
            &none,
            crate::commands::skills::SyncOpts::refresh("work"),
        )
        .await
        .unwrap();
        assert_eq!(r.clients.len(), 1);
        assert_eq!(r.clients[0].client, ClientId::Codex);
        assert!(!home.join(".claude/skills").exists());
        assert!(!home.join(".gemini/config").exists());
        // naming a client that is not installed says so and does nothing
        let h2 = Harness::new(ran(true));
        let mut o = opts(&s, "work");
        o.clients = vec![ClientId::OpenCode];
        h2.connect(o, "d", "mac", Duration::from_millis(10))
            .await
            .unwrap();
        assert!(h2
            .said
            .borrow()
            .join("\n")
            .contains("opencode is not installed"));
        assert!(h2.runner.calls().is_empty());
    }

    #[tokio::test]
    async fn client_failure_prints_snippet_and_connect_succeeds() {
        use crate::cloud::clients::runner::fake::{fail, Fake};
        let s = MockServer::start().await;
        mount_auth(&s).await;
        mount_me(&s, 0).await;
        // no playbooks route: the skills step fails too
        let mut h = Harness::new(ran(true));
        h.runner = Fake::new(|_, _| fail("no MCP server named quome"));
        mkdirs(&h, &[".codex"]);
        let out = h
            .connect(opts(&s, "work"), "d", "mac", Duration::from_millis(10))
            .await
            .unwrap();
        assert_eq!(out.profile_name, "work");
        let said = h.said.borrow().join("\n");
        assert!(said.contains("Could not register with codex"), "{said}");
        assert!(said.contains("[mcp_servers.quome]"), "{said}");
        assert!(said.contains("Could not install skills"), "{said}");
    }

    #[test]
    fn opencode_manual_snippet_is_current() {
        let t = manual_snippet(ClientId::OpenCode, "quome", "work");
        assert!(
            t.contains("opencode mcp add quome --global -- quome mcp --profile work"),
            "{t}"
        );
        let json = t.lines().last().unwrap();
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(
            v["mcp"]["servers"]["quome"],
            serde_json::json!({"type":"local","command":["quome","mcp","--profile","work"]})
        );
        assert!(t.contains("\"servers\""));
    }

    #[test]
    fn expiry_date_formats() {
        assert_eq!(expiry_date("2027-01-02T03:04:05Z"), "2027-01-02");
        assert_eq!(expiry_date("soon"), "soon");
    }

    #[tokio::test]
    async fn existing_quome_server_is_removed_then_added_and_reported_updated() {
        let s = MockServer::start().await;
        mount_auth(&s).await;
        mount_me(&s, 0).await;
        let h = Harness::new(ran(true));
        *h.claude.exists.lock().unwrap() = true;
        let out = h
            .connect(opts(&s, "work2"), "d", "mac", Duration::from_millis(10))
            .await
            .unwrap();
        assert_eq!(
            h.claude.calls.lock().unwrap().as_slice(),
            [mcp_get_args(), mcp_remove_args(), mcp_add_args("work2")]
        );
        assert_eq!(out.mcp, "updated");
        assert!(h
            .said
            .borrow()
            .join("\n")
            .contains("Updated the quome MCP server"));
    }

    #[tokio::test]
    async fn claude_stderr_is_shown_only_on_failure() {
        let s = MockServer::start().await;
        mount_auth(&s).await;
        mount_me(&s, 0).await;
        let h = Harness::new(ran(false));
        let out = h
            .connect(opts(&s, "work"), "d", "mac", Duration::from_millis(10))
            .await
            .unwrap();
        let said = h.said.borrow().join("\n");
        assert!(said.contains("boom: nope"), "{said}");
        assert!(said.contains(&mcp_config_json("work")));
        assert_eq!(out.mcp, "manual");

        let h = Harness::new(ran(true));
        h.connect(opts(&s, "work"), "d", "mac", Duration::from_millis(10))
            .await
            .unwrap();
        assert!(!h.said.borrow().join("\n").contains("boom"));
    }

    #[cfg(unix)]
    #[test]
    fn real_runner_captures_output_instead_of_inheriting() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("fakeclaude");
        std::fs::write(
            &bin,
            "#!/bin/sh\necho OUT-NOISE\necho \"ERR $*\" >&2\nexit 3\n",
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let r = RealClaude {
            program: bin.to_string_lossy().into_owned(),
        };
        assert_eq!(
            r.run(&mcp_add_args("p")),
            ClaudeResult::Ran {
                ok: false,
                stderr: format!("ERR {}\n", mcp_add_args("p").join(" "))
            }
        );
        let missing = RealClaude {
            program: dir.path().join("nope").to_string_lossy().into_owned(),
        };
        assert_eq!(missing.run(&[]), ClaudeResult::NotFound);
    }

    #[test]
    fn name_counts_utf16_units_and_cuts_on_char_boundaries() {
        let n = agent_name(None, &"😀".repeat(40));
        assert!(n.encode_utf16().count() <= 64);
        // 15 prefix units + 24 emoji * 2 = 63; a 25th would make 65
        assert_eq!(n.encode_utf16().count(), 63);
        assert!(n.starts_with("Claude Code on 😀"));
        // one unit of room cannot hold an emoji
        let n = agent_name(Some(&format!("{}😀", "a".repeat(63))), "h");
        assert_eq!(n, "a".repeat(63));
    }

    #[test]
    fn name_strips_control_and_format_characters() {
        assert_eq!(
            agent_name(None, "ev\u{202E}il\u{0007}\u{200B}\u{FEFF}"),
            "Claude Code on evil"
        );
        assert_eq!(agent_name(Some("a\u{0}b\tc\u{7f}"), "h"), "abc");
        assert_eq!(agent_name(Some("\u{202E}"), "h"), "Claude Code on h");
        assert_eq!(agent_name(None, "\u{202E}"), "Claude Code");
    }
}
