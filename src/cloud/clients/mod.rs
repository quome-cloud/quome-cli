//! Agent clients the CLI can wire up: skill rendering, paths, MCP registration.
pub mod antigravity;
pub mod claude;
pub mod codex;
pub mod jsonedit;
pub mod opencode;
pub mod paths;
pub mod runner;

#[allow(unused_imports)]
pub use super::playbooks::{render_skill, render_skills};
use runner::CommandRunner;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ClientId {
    Claude,
    Codex,
    OpenCode,
    Antigravity,
}

impl ClientId {
    pub const ALL: [ClientId; 4] = [
        ClientId::Claude,
        ClientId::Codex,
        ClientId::OpenCode,
        ClientId::Antigravity,
    ];
}

pub struct SkillFile {
    pub dir_name: String,
    pub contents: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RegisterOutcome {
    Added,
    Updated,
    Unchanged,
}

/// What an adapter may touch: the home directory and the process runner, both
/// injected so tests never reach the real machine.
pub struct Ctx<'a> {
    pub home: &'a Path,
    pub runner: &'a dyn CommandRunner,
    /// Environment lookup (real: `std::env::var`), injected for hermetic tests.
    pub env: &'a dyn Fn(&str) -> Option<String>,
}

pub fn real_env(k: &str) -> Option<String> {
    std::env::var(k).ok()
}

#[cfg(test)]
pub fn no_env(_: &str) -> Option<String> {
    None
}

/// One agent client.
pub trait ClientAdapter {
    fn id(&self) -> ClientId;
    fn installed(&self, home: &Path) -> bool;
    /// Point the client's `quome` MCP server at `program mcp --profile <profile>`.
    /// `Err` means the caller should print the manual snippet.
    fn register_mcp(
        &self,
        ctx: &Ctx,
        program: &str,
        profile: &str,
    ) -> Result<RegisterOutcome, String>;
    /// Directory that directly holds skill folders (for Claude, `<plugin root>/skills`).
    fn skills_dir(&self, home: &Path) -> PathBuf;
    /// Plugin root that needs a `.claude-plugin/plugin.json`; Claude only.
    fn plugin_root(&self, home: &Path) -> Option<PathBuf>;
}

pub fn adapters() -> Vec<Box<dyn ClientAdapter>> {
    vec![
        Box::new(claude::Claude),
        Box::new(codex::Codex),
        Box::new(opencode::OpenCode),
        Box::new(antigravity::Antigravity),
    ]
}

/// Skill directories to write for the installed clients, each root once. Codex and
/// OpenCode share a root and render identically, so the first one wins.
pub fn skill_targets(installed: &[ClientId], home: &Path) -> Vec<(PathBuf, ClientId)> {
    let mut out: Vec<(PathBuf, ClientId)> = Vec::new();
    for c in installed {
        let root = paths::skills_dir(*c, home);
        if !out.iter().any(|(r, _)| *r == root) {
            out.push((root, *c));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_root_is_written_once() {
        let home = Path::new("/h");
        let t = skill_targets(
            &[ClientId::Codex, ClientId::OpenCode, ClientId::Claude],
            home,
        );
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].1, ClientId::Codex);
    }

    #[test]
    fn adapters_cover_every_client() {
        let ids: Vec<_> = adapters().iter().map(|a| a.id()).collect();
        assert_eq!(ids, ClientId::ALL.to_vec());
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    use std::collections::BTreeMap;

    /// OpenCode's discovery rule for a global skills root (docs: opencode.ai/docs/skills,
    /// "Discovery"): SKILL.md files may be nested in grouping directories; a directory
    /// holding SKILL.md is one skill and is not searched further; the skill ID is that
    /// directory's name. Returns ID -> SKILL.md paths.
    fn opencode_skills(root: &Path, out: &mut BTreeMap<String, Vec<PathBuf>>) {
        let Ok(rd) = std::fs::read_dir(root) else {
            return;
        };
        if root.join("SKILL.md").is_file() {
            let id = root.file_name().unwrap().to_string_lossy().into_owned();
            out.entry(id).or_default().push(root.join("SKILL.md"));
            return;
        }
        for e in rd.flatten() {
            if e.path().is_dir() {
                opencode_skills(&e.path(), out);
            }
        }
    }

    fn write_skill(root: &Path, dir: &str) {
        let d = root.join(dir);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("SKILL.md"), "x").unwrap();
    }

    /// OpenCode scans `~/.claude/skills` and `~/.agents/skills` recursively, so it
    /// sees the Claude plugin's skills (`<name>`) as well as the shared copies
    /// (`quome-<name>`). Different IDs, same playbook: harmless, and the layout
    /// keeps Claude skills out of `~/.claude/skills/<x>/SKILL.md`.
    #[test]
    fn opencode_sees_claude_plugin_skills_too() {
        let home = tempfile::tempdir().unwrap();
        let names = ["deploy", "sandbox"];
        let targets = skill_targets(&[ClientId::Claude, ClientId::OpenCode], home.path());
        assert_eq!(targets.len(), 2);
        for (root, client) in &targets {
            for n in names {
                let dir = match client {
                    ClientId::Claude => n.to_string(),
                    _ => format!("quome-{n}"),
                };
                write_skill(root, &dir);
            }
        }
        let mut seen = BTreeMap::new();
        for root in [
            ".config/opencode/skills",
            ".claude/skills",
            ".agents/skills",
        ] {
            opencode_skills(&home.path().join(root), &mut seen);
        }
        // Every ID once; each playbook reachable as exactly two IDs.
        let ids: Vec<_> = seen.keys().cloned().collect();
        assert_eq!(ids, ["deploy", "quome-deploy", "quome-sandbox", "sandbox"]);
        assert!(seen.values().all(|v| v.len() == 1));
        // Nothing sits at ~/.claude/skills/<x>/SKILL.md (depth 2), which would add a
        // stray root-level `/quome`-style skill.
        for p in seen.values().flatten() {
            if let Ok(rel) = p.strip_prefix(home.path().join(".claude/skills")) {
                assert_eq!(rel.components().count(), 4, "{rel:?}");
            }
        }
        // Codex and OpenCode share one root, written once.
        let t = skill_targets(&[ClientId::Codex, ClientId::OpenCode], home.path());
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].0, home.path().join(".agents/skills"));
    }
}
