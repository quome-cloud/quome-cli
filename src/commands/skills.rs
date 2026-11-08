//! `quome skills sync`: keep the Quome playbook skills in each installed agent
//! client up to date. Only names recorded in a per-root manifest are ever deleted.
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use crate::cloud::api::CloudApi;
use crate::cloud::auth::TokenSource;
use crate::cloud::clients::{adapters, paths, render_skills, skill_targets, ClientId};
use crate::cloud::keys::{platform_store, AgentKey};
use crate::cloud::playbooks::{Playbook, TOOL_NAMES};
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

fn read_manifest(root: &Path) -> Option<Manifest> {
    serde_json::from_slice(&std::fs::read(root.join(MANIFEST)).ok()?).ok()
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
    files: &[crate::cloud::clients::SkillFile],
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
        if write_if_changed(&root.join(&f.dir_name).join("SKILL.md"), &f.contents)? {
            out.written += 1;
        } else {
            out.unchanged += 1;
        }
        names.push(f.dir_name.clone());
    }
    if let (Some(pr), false) = (plugin_root, names.is_empty()) {
        write_if_changed(
            &pr.join(".claude-plugin").join("plugin.json"),
            &plugin_json(),
        )?;
    }
    if let Some(m) = &old {
        for n in &m.names {
            if names.contains(n) || !removable(n, client) {
                continue;
            }
            let dir = root.join(n);
            if dir.is_dir() {
                std::fs::remove_dir_all(&dir)?;
                out.removed += 1;
            }
        }
    }
    let manifest = Manifest {
        catalog_hash: hash.to_string(),
        names,
    };
    write_if_changed(
        &root.join(MANIFEST),
        &(serde_json::to_string_pretty(&manifest).unwrap_or_default() + "\n"),
    )?;
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
pub async fn sync_skills(api: &CloudApi, home: &Path, clients: &[ClientId]) -> Result<SyncReport> {
    sync_skills_with(api, home, clients, &paths::on_real_path).await
}

pub async fn sync_skills_with(
    api: &CloudApi,
    home: &Path,
    clients: &[ClientId],
    on_path: &(dyn Fn(&str) -> bool + Sync),
) -> Result<SyncReport> {
    let installed = installed_of(clients, home, on_path);
    let mut report = SyncReport::default();
    if installed.is_empty() {
        return Ok(report);
    }
    let pbs = api.agent_playbooks(TOOL_NAMES).await?;
    let hash = catalog_hash(&pbs);
    let all = adapters();
    for (root, client) in skill_targets(&installed, home) {
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
        match sync_root(client, &root, plugin.as_deref(), &hash, &files) {
            Ok(s) => report.clients.push(s),
            Err(e) => report
                .failures
                .push((client, format!("{}: {e}", root.display()))),
        }
    }
    Ok(report)
}

/// Fire-and-forget sync for `quome mcp`: never delays the handshake, never prints
/// (stdout is the MCP transport), and drops every failure.
pub fn spawn_background_sync(api: CloudApi, home: PathBuf) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let _ = sync_skills(&api, &home, &ClientId::ALL).await;
    })
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
    let api = CloudApi::new(&p, TokenSource::new(&p, key, vrc));
    let home = dirs::home_dir()
        .ok_or_else(|| QuomeError::CloudState("cannot find home directory".into()))?;
    let clients = if client.is_empty() {
        ClientId::ALL.to_vec()
    } else {
        client
    };
    let report = sync_skills(&api, &home, &clients).await?;
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
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ORG: &str = "11111111-1111-1111-1111-111111111111";

    async fn setup() -> (MockServer, CloudApi) {
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
        (s, api)
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
        let r = sync_skills_with(&api, h.path(), &ClientId::ALL, &none)
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
        sync_skills_with(&api, h.path(), &ClientId::ALL, &none)
            .await
            .unwrap();
        assert!(!h.path().join(".claude").exists());
        assert!(!h.path().join(".gemini").exists());
        assert!(h.path().join(".agents/skills/quome-ship").is_dir());
        // nothing installed: no request at all
        let h2 = home_with(&[]);
        s.reset().await;
        let r = sync_skills_with(&api, h2.path(), &ClientId::ALL, &none)
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
        sync_skills_with(&api, h.path(), &ClientId::ALL, &none)
            .await
            .unwrap();
        serve(&s, &["ship"]).await;
        let r = sync_skills_with(&api, h.path(), &ClientId::ALL, &none)
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
        let r = sync_skills_with(&api, h.path(), &[ClientId::Claude], &none)
            .await
            .unwrap();
        assert_eq!(r.clients[0].written, 0);
        assert!(!h.path().join(".claude/skills").exists());

        serve(&s, &["ship"]).await;
        sync_skills_with(&api, h.path(), &[ClientId::Claude], &none)
            .await
            .unwrap();
        serve(&s, &[]).await;
        let r = sync_skills_with(&api, h.path(), &[ClientId::Claude], &none)
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
        sync_skills_with(&api, h.path(), &ClientId::ALL, &none)
            .await
            .unwrap();
        serve(&s, &[]).await;
        sync_skills_with(&api, h.path(), &ClientId::ALL, &none)
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
        sync_skills_with(&api, h.path(), &ClientId::ALL, &none)
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
        sync_skills_with(&api, h.path(), &ClientId::ALL, &none)
            .await
            .unwrap();
        let before = read_manifest(&h.path().join(".agents/skills")).unwrap();
        let r = sync_skills_with(&api, h.path(), &ClientId::ALL, &none)
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
        let before = std::fs::read_dir(h.path()).unwrap().count();
        // Resolves to () with no panic and no error surface; the function body
        // has no print of any kind (stdout is the MCP transport).
        spawn_background_sync(api, h.path().to_path_buf())
            .await
            .unwrap();
        assert_eq!(std::fs::read_dir(h.path()).unwrap().count(), before);
        assert!(!h.path().join(".agents").exists());
        let src = include_str!("skills.rs");
        let body = src
            .split("pub fn spawn_background_sync")
            .nth(1)
            .unwrap()
            .split("pub fn print_report")
            .next()
            .unwrap();
        assert!(!body.contains("println!") && !body.contains("print!"));
        assert!(!body.contains("eprint"));
    }
}
