//! `quome skills sync`: keep the Quome playbook skills in each installed agent
//! client up to date. Only names recorded in a per-root manifest are ever deleted.
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::cloud::api::CloudApi;
use crate::cloud::auth::TokenSource;
use crate::cloud::clients::{adapters, paths, render_skills, skill_targets, ClientId};
use crate::cloud::keys::{platform_store, AgentKey};
use crate::cloud::playbooks::{callable_tools, Playbook};
use crate::cloud::state::CloudState;
use crate::errors::{QuomeError, Result};

pub const MANIFEST: &str = ".quome-skills.json";

#[derive(Subcommand)]
pub enum SkillsCommands {
    /// Write the current playbooks as skills into every installed agent client
    Sync {
        /// Limit to these clients (claude, codex, opencode, antigravity)
        #[arg(long, value_delimiter = ',', value_parser = ClientId::parse)]
        client: Vec<ClientId>,
        /// Profile to use (default: the default profile)
        #[arg(long)]
        profile: Option<String>,
    },
}

#[derive(Serialize, Deserialize, Default, PartialEq, Debug)]
struct Manifest {
    catalog_hash: String,
    names: Vec<String>,
    /// The profile that last wrote this root. Absent in older manifests, which
    /// any profile may take over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile: Option<String>,
}

/// Who is syncing and what it may create.
#[derive(Debug, Clone, Copy)]
pub struct SyncOpts<'a> {
    /// The profile writing the skills; recorded in each root's manifest.
    pub profile: &'a str,
    /// Background mode: only update roots that already hold a manifest from this
    /// profile (or from before manifests named one). Never creates a root.
    pub refresh_only: bool,
}

impl<'a> SyncOpts<'a> {
    /// Explicit sync or connect: creates roots and takes over another profile's.
    pub fn explicit(profile: &'a str) -> Self {
        SyncOpts {
            profile,
            refresh_only: false,
        }
    }
    pub fn refresh(profile: &'a str) -> Self {
        SyncOpts {
            profile,
            refresh_only: true,
        }
    }
}

/// What happened to one skills root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientSync {
    pub client: ClientId,
    pub root: PathBuf,
    pub written: usize,
    pub removed: usize,
    pub unchanged: usize,
}

#[derive(Debug, Default)]
pub struct SyncReport {
    pub clients: Vec<ClientSync>,
    pub warnings: Vec<String>,
    /// Roots that could not be written; the others are unaffected.
    pub failures: Vec<(ClientId, String)>,
}

/// Hash of the playbook list plus this CLI's version, so an upgrade that changes
/// the skill format re-syncs.
pub fn catalog_hash(pbs: &[Playbook]) -> String {
    let canon = serde_json::to_string(pbs).unwrap_or_default();
    let mut h = Sha256::new();
    h.update(env!("CARGO_PKG_VERSION").as_bytes());
    h.update([0]);
    h.update(canon.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// A single plain path component: safe to join onto a skills root.
fn safe_component(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0'])
        && !name.contains("..")
        && name != MANIFEST
}

/// May we delete `<root>/<name>`? Only manifest names that are safe components,
/// and for the shared roots only our `quome-` folders.
fn removable(name: &str, client: ClientId) -> bool {
    safe_component(name) && (client == ClientId::Claude || name.starts_with("quome-"))
}

/// True if the root's manifest records this catalog hash and every skill it
/// names is still on disk, so a refresh has nothing to write.
fn up_to_date(root: &Path, hash: &str) -> bool {
    read_manifest(root).is_some_and(|m| {
        m.catalog_hash == hash
            && m.names
                .iter()
                .all(|n| safe_component(n) && root.join(n).join("SKILL.md").is_file())
    })
}

fn read_manifest(root: &Path) -> Option<Manifest> {
    serde_json::from_slice(&std::fs::read(root.join(MANIFEST)).ok()?).ok()
}

/// True if `p` is a symlink (not followed). Skills are never written through one.
fn is_link(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

/// Write `contents` to `path` unless it already holds them. True if written.
fn write_if_changed(path: &Path, contents: &str) -> std::io::Result<bool> {
    if std::fs::read_to_string(path).is_ok_and(|c| c == contents) {
        return Ok(false);
    }
    let dir = path.parent().expect("file has a parent");
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    std::io::Write::write_all(&mut tmp, contents.as_bytes())?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(true)
}

fn plugin_json() -> String {
    format!(
        "{}\n",
        serde_json::json!({
            "name": "quome",
            "description": "Quome Cloud playbooks: build, deploy, sign-in, email and more through the quome MCP server.",
            "version": env!("CARGO_PKG_VERSION"),
        })
    )
}

fn sync_root(
    client: ClientId,
    root: &Path,
    plugin_root: Option<&Path>,
    hash: &str,
    profile: &str,
    files: &[crate::cloud::clients::SkillFile],
    warnings: &mut Vec<String>,
) -> std::io::Result<ClientSync> {
    let mut out = ClientSync {
        client,
        root: root.to_path_buf(),
        written: 0,
        removed: 0,
        unchanged: 0,
    };
    let old = read_manifest(root);
    if files.is_empty() && old.is_none() {
        return Ok(out);
    }
    let mut names = Vec::new();
    for f in files {
        if !safe_component(&f.dir_name) {
            continue;
        }
        let dir = root.join(&f.dir_name);
        let file = dir.join("SKILL.md");
        if is_link(&dir) || is_link(&file) {
            warnings.push(format!(
                "skipped {}: it is a symbolic link; skills are never written through one",
                dir.display()
            ));
            continue;
        }
        if write_if_changed(&file, &f.contents)? {
            out.written += 1;
        } else {
            out.unchanged += 1;
        }
        names.push(f.dir_name.clone());
    }
    if let (Some(pr), false) = (plugin_root, names.is_empty()) {
        let pdir = pr.join(".claude-plugin");
        let pfile = pdir.join("plugin.json");
        if is_link(&pdir) || is_link(&pfile) {
            warnings.push(format!(
                "skipped {}: it is a symbolic link; skills are never written through one",
                pfile.display()
            ));
        } else {
            write_if_changed(&pfile, &plugin_json())?;
        }
    }
    if let Some(m) = &old {
        for n in &m.names {
            if names.contains(n) || !removable(n, client) {
                continue;
            }
            let dir = root.join(n);
            if is_link(&dir) {
                continue;
            }
            if dir.is_dir() {
                match std::fs::remove_dir_all(&dir) {
                    Ok(()) => out.removed += 1,
                    // Gone since we looked: already removed.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
            }
        }
    }
    let manifest = Manifest {
        catalog_hash: hash.to_string(),
        names,
        profile: Some(profile.to_string()),
    };
    let mpath = root.join(MANIFEST);
    if is_link(&mpath) {
        warnings.push(format!(
            "skipped {}: it is a symbolic link; skills are never written through one",
            mpath.display()
        ));
    } else {
        write_if_changed(
            &mpath,
            &(serde_json::to_string_pretty(&manifest).unwrap_or_default() + "\n"),
        )?;
    }
    Ok(out)
}

/// The requested clients that are actually installed, in order, each once.
pub fn installed_of(
    clients: &[ClientId],
    home: &Path,
    on_path: &dyn Fn(&str) -> bool,
) -> Vec<ClientId> {
    let mut v: Vec<ClientId> = Vec::new();
    for c in clients {
        if !v.contains(c) && paths::installed_with(*c, home, on_path) {
            v.push(*c);
        }
    }
    v
}

/// Sync skills into the installed ones of `clients`.
pub async fn sync_skills(
    api: &Arc<CloudApi>,
    home: &Path,
    clients: &[ClientId],
    opts: SyncOpts<'_>,
) -> Result<SyncReport> {
    sync_skills_with(api, home, clients, &paths::on_real_path, opts).await
}

pub async fn sync_skills_with(
    api: &Arc<CloudApi>,
    home: &Path,
    clients: &[ClientId],
    on_path: &(dyn Fn(&str) -> bool + Sync),
    opts: SyncOpts<'_>,
) -> Result<SyncReport> {
    let installed = installed_of(clients, home, on_path);
    let mut report = SyncReport::default();
    let mut targets = skill_targets(&installed, home);
    if opts.refresh_only {
        targets.retain(|(root, _)| {
            read_manifest(root)
                .is_some_and(|m| m.profile.as_deref().is_none_or(|p| p == opts.profile))
        });
    }
    if targets.is_empty() {
        return Ok(report);
    }
    let tools = callable_tools(api).await;
    let tools: Vec<&str> = tools.iter().map(String::as_str).collect();
    let pbs = api.agent_playbooks(&tools).await?;
    let hash = catalog_hash(&pbs);
    if opts.refresh_only {
        // Same catalog as last time and every skill still in place: nothing to do.
        targets.retain(|(root, _)| !up_to_date(root, &hash));
    }
    let all = adapters();
    for (root, client) in targets {
        let (files, warnings) = render_skills(&pbs, client);
        for w in warnings {
            if !report.warnings.contains(&w) {
                report.warnings.push(w);
            }
        }
        let plugin = all
            .iter()
            .find(|a| a.id() == client)
            .and_then(|a| a.plugin_root(home));
        let mut skipped = Vec::new();
        let res = sync_root(
            client,
            &root,
            plugin.as_deref(),
            &hash,
            opts.profile,
            &files,
            &mut skipped,
        );
        for w in skipped {
            if !report.warnings.contains(&w) {
                report.warnings.push(w);
            }
        }
        match res {
            Ok(s) => report.clients.push(s),
            Err(e) => report
                .failures
                .push((client, format!("{}: {e}", root.display()))),
        }
    }
    Ok(report)
}

/// The background refresh for `quome mcp`: never prints (stdout is the MCP
/// transport) and drops every failure. It only updates roots that `quome
/// connect` or `quome skills sync` already set up for this profile; it never
/// creates one.
pub async fn background_sync(api: &Arc<CloudApi>, home: &Path, profile: &str) {
    let _ = sync_skills(api, home, &ClientId::ALL, SyncOpts::refresh(profile)).await;
}

/// Fire-and-forget `background_sync` over a connection the caller shares, so it
/// never delays the handshake or opens a second token source.
pub fn spawn_background_sync(
    api: Arc<CloudApi>,
    home: PathBuf,
    profile: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move { background_sync(&api, &home, &profile).await })
}

pub fn print_report(r: &SyncReport) {
    for c in &r.clients {
        println!(
            "{}: {} written, {} removed, {} unchanged ({})",
            c.client.name(),
            c.written,
            c.removed,
            c.unchanged,
            c.root.display()
        );
    }
    for w in &r.warnings {
        eprintln!("warning: {w}");
    }
    for (c, e) in &r.failures {
        eprintln!("{}: could not write skills: {e}", c.name());
    }
}

pub async fn execute(cmd: SkillsCommands) -> Result<()> {
    let SkillsCommands::Sync { client, profile } = cmd;
    let store = platform_store()?;
    let state = CloudState::load()?;
    let p = match state.profile(profile.as_deref()) {
        Ok(p) => p.clone(),
        Err(_) if state.profiles.is_empty() => {
            return Err(QuomeError::CloudState(
                crate::commands::cloud::NOT_CONNECTED.into(),
            ))
        }
        Err(e) => return Err(e),
    };
    let name = profile.unwrap_or(state.default);
    let (key, vrc) = AgentKey::load_from(store.as_ref(), &name)?;
    let api = Arc::new(CloudApi::new(&p, TokenSource::new(&p, key, vrc)));
    let home = dirs::home_dir()
        .ok_or_else(|| QuomeError::CloudState("cannot find home directory".into()))?;
    let clients = if client.is_empty() {
        ClientId::ALL.to_vec()
    } else {
        client
    };
    let report = sync_skills(&api, &home, &clients, SyncOpts::explicit(&name)).await?;
    if report.clients.is_empty() && report.failures.is_empty() {
        println!("No installed agent clients found; nothing to do.");
    }
    print_report(&report);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::auth::tests::profile;
    use crate::cloud::playbooks::TOOL_NAMES;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ORG: &str = "11111111-1111-1111-1111-111111111111";

    async fn setup() -> (MockServer, Arc<CloudApi>) {
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
                "access_token": "AT", "token_type": "Bearer", "expires_in": 3600
            })))
            .mount(&s)
            .await;
        let p = profile(&s.uri());
        let api = CloudApi::new(&p, TokenSource::new(&p, AgentKey::generate(), "vrc".into()));
        (s, Arc::new(api))
    }

    fn pb(name: &str) -> serde_json::Value {
        serde_json::json!({"name":name,"area":"apps","title":format!("Do {name}"),
            "description":"d","arguments":[],"requires_tools":[],"allowed":true})
    }

    async fn serve(s: &MockServer, names: &[&str]) {
        s.reset().await;
        // reset drops auth mocks too; remount them
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
        let body: Vec<_> = names.iter().map(|n| pb(n)).collect();
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/orgs/{ORG}/agent-playbooks")))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(s)
            .await;
    }

    fn none(_: &str) -> bool {
        false
    }

    fn home_with(dirs: &[&str]) -> tempfile::TempDir {
        let h = tempfile::tempdir().unwrap();
        for d in dirs {
            std::fs::create_dir_all(h.path().join(d)).unwrap();
        }
        h
    }

    fn claude_skills(h: &Path) -> PathBuf {
        h.join(".claude/skills/quome/skills")
    }

    #[tokio::test]
    async fn sync_writes_one_skill_per_playbook() {
        let (s, api) = setup().await;
        serve(&s, &["ship", "mail"]).await;
        let h = home_with(&[".claude", ".codex"]);
        let r = sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::explicit("p"),
        )
        .await
        .unwrap();
        assert_eq!(r.clients.len(), 2);
        assert!(claude_skills(h.path()).join("ship/SKILL.md").is_file());
        assert!(claude_skills(h.path()).join("mail/SKILL.md").is_file());
        let a = h.path().join(".agents/skills");
        assert!(a.join("quome-ship/SKILL.md").is_file());
        assert!(a.join("quome-mail/SKILL.md").is_file());
        let pj = std::fs::read_to_string(
            h.path()
                .join(".claude/skills/quome/.claude-plugin/plugin.json"),
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&pj).unwrap();
        assert_eq!(v["name"], "quome");
        assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
        assert!(!h.path().join(".claude/skills/quome/SKILL.md").exists());
        let m = read_manifest(&a).unwrap();
        assert_eq!(m.names.len(), 2);
        assert_eq!(m.catalog_hash.len(), 64);
    }

    #[tokio::test]
    async fn not_installed_clients_are_untouched() {
        let (s, api) = setup().await;
        serve(&s, &["ship"]).await;
        let h = home_with(&[".codex"]);
        sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::explicit("p"),
        )
        .await
        .unwrap();
        assert!(!h.path().join(".claude").exists());
        assert!(!h.path().join(".gemini").exists());
        assert!(h.path().join(".agents/skills/quome-ship").is_dir());
        // nothing installed: no request at all
        let h2 = home_with(&[]);
        s.reset().await;
        let r = sync_skills_with(
            &api,
            h2.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::explicit("p"),
        )
        .await
        .unwrap();
        assert!(r.clients.is_empty());
        assert!(s.received_requests().await.unwrap().is_empty());
        assert_eq!(std::fs::read_dir(h2.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn sync_removes_dropped_playbooks() {
        let (s, api) = setup().await;
        serve(&s, &["ship", "mail"]).await;
        let h = home_with(&[".claude", ".codex"]);
        sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::explicit("p"),
        )
        .await
        .unwrap();
        serve(&s, &["ship"]).await;
        let r = sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::explicit("p"),
        )
        .await
        .unwrap();
        assert!(r.clients.iter().all(|c| c.removed == 1));
        assert!(!claude_skills(h.path()).join("mail").exists());
        assert!(!h.path().join(".agents/skills/quome-mail").exists());
        assert!(h.path().join(".agents/skills/quome-ship").exists());
    }

    #[tokio::test]
    async fn empty_catalog_removes_managed_and_creates_no_plugin() {
        let (s, api) = setup().await;
        serve(&s, &[]).await;
        let h = home_with(&[".claude"]);
        let r = sync_skills_with(
            &api,
            h.path(),
            &[ClientId::Claude],
            &none,
            SyncOpts::explicit("p"),
        )
        .await
        .unwrap();
        assert_eq!(r.clients[0].written, 0);
        assert!(!h.path().join(".claude/skills").exists());

        serve(&s, &["ship"]).await;
        sync_skills_with(
            &api,
            h.path(),
            &[ClientId::Claude],
            &none,
            SyncOpts::explicit("p"),
        )
        .await
        .unwrap();
        serve(&s, &[]).await;
        let r = sync_skills_with(
            &api,
            h.path(),
            &[ClientId::Claude],
            &none,
            SyncOpts::explicit("p"),
        )
        .await
        .unwrap();
        assert_eq!(r.clients[0].removed, 1);
        assert!(!claude_skills(h.path()).join("ship").exists());
        assert!(h
            .path()
            .join(".claude/skills/quome/.claude-plugin/plugin.json")
            .is_file());
    }

    #[tokio::test]
    async fn sync_never_deletes_unmanaged() {
        let (s, api) = setup().await;
        serve(&s, &["ship"]).await;
        let h = home_with(&[".codex"]);
        let a = h.path().join(".agents/skills");
        std::fs::create_dir_all(a.join("quome-notes")).unwrap();
        std::fs::write(a.join("quome-notes/SKILL.md"), "mine").unwrap();
        sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::explicit("p"),
        )
        .await
        .unwrap();
        serve(&s, &[]).await;
        sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::explicit("p"),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(a.join("quome-notes/SKILL.md")).unwrap(),
            "mine"
        );
        assert!(!a.join("quome-ship").exists());
    }

    #[tokio::test]
    async fn tampered_manifest_cannot_delete_outside_or_unprefixed() {
        let (s, api) = setup().await;
        serve(&s, &["ship"]).await;
        let h = home_with(&[".codex"]);
        let a = h.path().join(".agents/skills");
        std::fs::create_dir_all(a.join("other-skill")).unwrap();
        std::fs::create_dir_all(h.path().join("victim")).unwrap();
        let m = serde_json::json!({"catalog_hash":"x","names":
            ["../../victim","other-skill","a/b","..","","quome-gone"]});
        std::fs::write(a.join(MANIFEST), m.to_string()).unwrap();
        std::fs::create_dir_all(a.join("quome-gone")).unwrap();
        sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::explicit("p"),
        )
        .await
        .unwrap();
        assert!(h.path().join("victim").is_dir());
        assert!(a.join("other-skill").is_dir());
        assert!(!a.join("quome-gone").exists());
    }

    #[test]
    fn removal_guards() {
        for bad in ["", ".", "..", "a/b", "a\\b", "../x", "x/..", MANIFEST] {
            assert!(!removable(bad, ClientId::Claude), "{bad:?}");
        }
        assert!(removable("ship", ClientId::Claude));
        assert!(!removable("ship", ClientId::Codex));
        assert!(removable("quome-ship", ClientId::Codex));
    }

    #[tokio::test]
    async fn sync_noop_when_hash_unchanged() {
        let (s, api) = setup().await;
        serve(&s, &["ship"]).await;
        let h = home_with(&[".claude", ".codex"]);
        sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::explicit("p"),
        )
        .await
        .unwrap();
        let before = read_manifest(&h.path().join(".agents/skills")).unwrap();
        let r = sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::explicit("p"),
        )
        .await
        .unwrap();
        assert!(r
            .clients
            .iter()
            .all(|c| c.written == 0 && c.removed == 0 && c.unchanged == 1));
        assert_eq!(
            before,
            read_manifest(&h.path().join(".agents/skills")).unwrap()
        );
    }

    #[tokio::test]
    async fn mcp_start_sync_failure_is_silent() {
        let (s, api) = setup().await;
        s.reset().await; // every request now 404s
        let h = home_with(&[".claude", ".codex"]);
        // A root that would be refreshed, so the failing fetch is reached.
        let a = h.path().join(".agents/skills");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(a.join(MANIFEST), r#"{"catalog_hash":"x","names":[]}"#).unwrap();
        let before = std::fs::read(a.join(MANIFEST)).unwrap();
        // Resolves to () with no panic and no error surface; the function body
        // has no print of any kind (stdout is the MCP transport).
        spawn_background_sync(api, h.path().to_path_buf(), "p".into())
            .await
            .unwrap();
        assert_eq!(std::fs::read(a.join(MANIFEST)).unwrap(), before);
        assert!(!h.path().join(".claude/skills").exists());
        let src = include_str!("skills.rs");
        let body = src
            .split("pub async fn background_sync")
            .nth(1)
            .unwrap()
            .split("pub fn print_report")
            .next()
            .unwrap();
        assert!(!body.contains("println!") && !body.contains("print!"));
        assert!(!body.contains("eprint"));
    }

    fn mtime(p: &Path) -> std::time::SystemTime {
        std::fs::metadata(p).unwrap().modified().unwrap()
    }

    #[tokio::test]
    async fn refresh_skips_roots_whose_catalog_is_unchanged() {
        let (s, api) = setup().await;
        serve(&s, &["ship"]).await;
        let h = home_with(&[".codex"]);
        sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::explicit("p"),
        )
        .await
        .unwrap();
        let a = h.path().join(".agents/skills");
        let skill = a.join("quome-ship/SKILL.md");
        let (m0, k0) = (mtime(&a.join(MANIFEST)), mtime(&skill));
        std::thread::sleep(std::time::Duration::from_millis(30));
        // A hand edit is left alone: the root is not even compared.
        let r = sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::refresh("p"),
        )
        .await
        .unwrap();
        assert!(r.clients.is_empty());
        assert_eq!((m0, k0), (mtime(&a.join(MANIFEST)), mtime(&skill)));
        // A missing skill defeats the shortcut, so it is restored.
        std::fs::remove_dir_all(a.join("quome-ship")).unwrap();
        let r = sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::refresh("p"),
        )
        .await
        .unwrap();
        assert_eq!(r.clients[0].written, 1);
        assert!(skill.is_file());
    }

    #[tokio::test]
    async fn refresh_rewrites_when_the_catalog_changed() {
        let (s, api) = setup().await;
        serve(&s, &["ship"]).await;
        let h = home_with(&[".codex"]);
        sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::explicit("p"),
        )
        .await
        .unwrap();
        serve(&s, &["ship", "mail"]).await;
        let r = sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::refresh("p"),
        )
        .await
        .unwrap();
        assert_eq!(r.clients[0].written, 1);
        assert!(h
            .path()
            .join(".agents/skills/quome-mail/SKILL.md")
            .is_file());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_skill_dirs_are_skipped_with_a_warning() {
        let (s, api) = setup().await;
        serve(&s, &["x", "ship"]).await;
        let h = home_with(&[".codex"]);
        let outside = tempfile::tempdir().unwrap();
        let a = h.path().join(".agents/skills");
        std::fs::create_dir_all(&a).unwrap();
        std::os::unix::fs::symlink(outside.path(), a.join("quome-x")).unwrap();
        let r = sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::explicit("p"),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
        assert!(
            r.warnings.iter().any(|w| w.contains("quome-x")),
            "{:?}",
            r.warnings
        );
        assert!(a.join("quome-ship/SKILL.md").is_file());
        assert!(!read_manifest(&a)
            .unwrap()
            .names
            .contains(&"quome-x".to_string()));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_skill_file_and_plugin_json_are_not_followed() {
        let (s, api) = setup().await;
        serve(&s, &["ship", "mail"]).await;
        let h = home_with(&[".claude"]);
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("t");
        std::fs::write(&target, "keep").unwrap();
        let root = claude_skills(h.path());
        std::fs::create_dir_all(root.join("ship")).unwrap();
        std::os::unix::fs::symlink(&target, root.join("ship/SKILL.md")).unwrap();
        let pdir = h.path().join(".claude/skills/quome/.claude-plugin");
        std::fs::create_dir_all(&pdir).unwrap();
        std::os::unix::fs::symlink(&target, pdir.join("plugin.json")).unwrap();
        let r = sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::explicit("p"),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
        assert!(r.warnings.len() >= 2, "{:?}", r.warnings);
    }

    #[tokio::test]
    async fn background_sync_survives_server_errors_and_unreachable_hosts() {
        let (s, api) = setup().await;
        let h = home_with(&[".codex"]);
        let a = h.path().join(".agents/skills");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(a.join(MANIFEST), r#"{"catalog_hash":"x","names":[]}"#).unwrap();
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/orgs/{ORG}/agent-playbooks")))
            .respond_with(ResponseTemplate::new(500))
            .mount(&s)
            .await;
        // Returns () without panicking; nothing is written.
        background_sync(&api, h.path(), "p").await;
        assert_eq!(
            std::fs::read_to_string(a.join(MANIFEST)).unwrap(),
            r#"{"catalog_hash":"x","names":[]}"#
        );
        drop(s);
        background_sync(&api, h.path(), "p").await;
        assert_eq!(std::fs::read_dir(&a).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn background_sync_never_creates_a_root() {
        let (s, api) = setup().await;
        serve(&s, &["ship"]).await;
        let h = home_with(&[".claude", ".codex", ".gemini/antigravity"]);
        // Nothing set up yet: no roots written, no request made.
        let r = sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::refresh("p"),
        )
        .await
        .unwrap();
        assert!(r.clients.is_empty());
        assert!(!h.path().join(".agents").exists());
        assert!(!h.path().join(".claude/skills").exists());
        assert!(!h.path().join(".gemini/config").exists());
        assert!(s.received_requests().await.unwrap().is_empty());
        // `connect --client codex` set up only the shared root.
        sync_skills_with(
            &api,
            h.path(),
            &[ClientId::Codex],
            &none,
            SyncOpts::explicit("p"),
        )
        .await
        .unwrap();
        serve(&s, &["ship", "mail"]).await;
        let r = sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::refresh("p"),
        )
        .await
        .unwrap();
        assert_eq!(r.clients.len(), 1);
        assert_eq!(r.clients[0].client, ClientId::Codex);
        assert!(h
            .path()
            .join(".agents/skills/quome-mail/SKILL.md")
            .is_file());
        assert!(!h.path().join(".claude/skills").exists());
        assert!(!h.path().join(".gemini/config").exists());
    }

    #[tokio::test]
    async fn background_sync_skips_another_profiles_root_and_explicit_takes_over() {
        let (s, api) = setup().await;
        serve(&s, &["ship"]).await;
        let h = home_with(&[".codex"]);
        let a = h.path().join(".agents/skills");
        sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::explicit("work"),
        )
        .await
        .unwrap();
        assert_eq!(read_manifest(&a).unwrap().profile.as_deref(), Some("work"));
        serve(&s, &["ship", "mail"]).await;
        let r = sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::refresh("dev"),
        )
        .await
        .unwrap();
        assert!(r.clients.is_empty());
        assert!(!a.join("quome-mail").exists());
        assert_eq!(read_manifest(&a).unwrap().profile.as_deref(), Some("work"));
        // Same profile refreshes.
        let r = sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::refresh("work"),
        )
        .await
        .unwrap();
        assert_eq!(r.clients.len(), 1);
        assert!(a.join("quome-mail").is_dir());
        // An explicit sync under another profile takes the root over.
        sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::explicit("dev"),
        )
        .await
        .unwrap();
        assert_eq!(read_manifest(&a).unwrap().profile.as_deref(), Some("dev"));
    }

    #[tokio::test]
    async fn manifest_without_profile_matches_any_and_is_stamped() {
        let (s, api) = setup().await;
        serve(&s, &["ship"]).await;
        let h = home_with(&[".codex"]);
        let a = h.path().join(".agents/skills");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(a.join(MANIFEST), r#"{"catalog_hash":"x","names":[]}"#).unwrap();
        let r = sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::refresh("dev"),
        )
        .await
        .unwrap();
        assert_eq!(r.clients.len(), 1);
        assert!(a.join("quome-ship/SKILL.md").is_file());
        assert_eq!(read_manifest(&a).unwrap().profile.as_deref(), Some("dev"));
    }

    #[tokio::test]
    async fn already_deleted_skill_dir_is_not_a_failure() {
        let (s, api) = setup().await;
        serve(&s, &["ship"]).await;
        let h = home_with(&[".codex"]);
        let a = h.path().join(".agents/skills");
        std::fs::create_dir_all(&a).unwrap();
        let m = serde_json::json!({"catalog_hash":"x","names":["quome-gone"]});
        std::fs::write(a.join(MANIFEST), m.to_string()).unwrap();
        let r = sync_skills_with(
            &api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::explicit("p"),
        )
        .await
        .unwrap();
        assert!(r.failures.is_empty());
        assert_eq!(r.clients[0].removed, 0);
        assert_eq!(read_manifest(&a).unwrap().names, ["quome-ship"]);
    }

    // --- the tools a playbook may need, when the org's MCP gateway serves them ---

    const MCP: &str = "/api/v1/orgs/11111111-1111-1111-1111-111111111111/mcp";

    async fn mount_playbooks(s: &MockServer) {
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/orgs/{ORG}/agent-playbooks")))
            .respond_with(ResponseTemplate::new(200).set_body_json(vec![pb("ship")]))
            .mount(s)
            .await;
    }

    async fn mount_front_door(s: &MockServer, door: &str) {
        Mock::given(method("GET"))
            .and(path(format!("{MCP}/config")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"front_door": door})),
            )
            .mount(s)
            .await;
    }

    async fn mount_gateway(s: &MockServer) {
        Mock::given(method("POST"))
            .and(path(format!("{MCP}/token")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "GWTOK", "expires_in": 3600, "url": format!("{}/gw", s.uri())
            })))
            .mount(s)
            .await;
        Mock::given(method("GET"))
            .and(path("/gw"))
            .respond_with(ResponseTemplate::new(405))
            .mount(s)
            .await;
        let rpc = |req: &wiremock::Request| {
            let b: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            let id = b["id"].clone();
            match b["method"].as_str().unwrap() {
                "initialize" => ResponseTemplate::new(200)
                    .insert_header("mcp-session-id", "sess")
                    .set_body_json(serde_json::json!({"jsonrpc":"2.0","id":id,"result":{
                        "protocolVersion":"2025-06-18","capabilities":{},
                        "serverInfo":{"name":"gw","version":"1"}}})),
                "tools/list" => ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"tools":[
                        {"name":"find_tools","inputSchema":{"type":"object"}},
                        {"name":"list_sandboxes","inputSchema":{"type":"object"}},
                        {"name":"deploy_app","inputSchema":{"type":"object"}}]}}),
                ),
                _ => ResponseTemplate::new(202),
            }
        };
        Mock::given(method("POST"))
            .and(path("/gw"))
            .respond_with(rpc)
            .mount(s)
            .await;
    }

    /// The `tools` query of the playbook list request.
    async fn asked_tools(s: &MockServer) -> String {
        let reqs = s.received_requests().await.unwrap();
        let r = reqs
            .iter()
            .find(|r| r.url.path().ends_with("/agent-playbooks"))
            .expect("playbooks were not asked for");
        r.url
            .query_pairs()
            .find(|(k, _)| k == "tools")
            .map(|(_, v)| v.into_owned())
            .unwrap()
    }

    async fn run_sync(api: &Arc<CloudApi>) {
        let h = home_with(&[".codex"]);
        sync_skills_with(
            api,
            h.path(),
            &ClientId::ALL,
            &none,
            SyncOpts::explicit("p"),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn skills_sync_tools_param_is_gateway_callable_set() {
        let (s, api) = setup().await;
        mount_front_door(&s, "gateway").await;
        mount_gateway(&s).await;
        mount_playbooks(&s).await;
        run_sync(&api).await;
        // what the gateway lists, then the CLI's own, then the old app-tool names
        assert_eq!(
            asked_tools(&s).await,
            "find_tools,list_sandboxes,deploy_app,sync_folder,git_clone_url,find_app_tools,describe_app_tool,call_app_tool"
        );
    }

    #[tokio::test]
    async fn skills_sync_uses_the_fixed_list_when_direct() {
        let (s, api) = setup().await;
        mount_front_door(&s, "direct").await;
        mount_playbooks(&s).await;
        run_sync(&api).await;
        assert_eq!(asked_tools(&s).await, TOOL_NAMES.join(","));
    }

    #[tokio::test]
    async fn skills_sync_uses_the_fixed_list_without_mcp_call() {
        let (s, api) = setup().await;
        mount_front_door(&s, "gateway").await;
        Mock::given(method("POST"))
            .and(path(format!("{MCP}/token")))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_json(serde_json::json!({"reason": "capability_missing"})),
            )
            .mount(&s)
            .await;
        mount_playbooks(&s).await;
        run_sync(&api).await;
        assert_eq!(asked_tools(&s).await, TOOL_NAMES.join(","));
    }
}
