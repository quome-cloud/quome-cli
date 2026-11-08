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

    /// Files OpenCode would load as skills from its global roots (each root is
    /// scanned recursively for `SKILL.md`).
    fn opencode_skill_files(home: &Path) -> Vec<PathBuf> {
        fn walk(d: &Path, out: &mut Vec<PathBuf>) {
            let Ok(rd) = std::fs::read_dir(d) else { return };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.file_name().is_some_and(|n| n == "SKILL.md") {
                    out.push(p);
                }
            }
        }
        let mut out = Vec::new();
        for root in [
            ".config/opencode/skills",
            ".claude/skills",
            ".agents/skills",
        ] {
            walk(&home.join(root), &mut out);
        }
        out
    }

    #[test]
    fn no_double_load_claude_and_opencode() {
        let home = tempfile::tempdir().unwrap();
        let installed = [ClientId::Claude, ClientId::OpenCode];
        let targets = skill_targets(&installed, home.path());
        assert_eq!(targets.len(), 2);
        // Claude's skills sit one level below the plugin root, never at
        // ~/.claude/skills/<x>/SKILL.md, so they are not a root-level skill.
        let claude = paths::skills_dir(ClientId::Claude, home.path());
        assert_eq!(
            claude,
            home.path().join(".claude/skills/quome/skills"),
            "claude skills live inside the plugin"
        );
        for (root, _) in &targets {
            let dir = root.join("check");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SKILL.md"), "x").unwrap();
        }
        for f in opencode_skill_files(home.path()) {
            let rel = f.strip_prefix(home.path().join(".claude/skills")).ok();
            if let Some(rel) = rel {
                // ~/.claude/skills/<plugin>/skills/<name>/SKILL.md: depth 4, not 2.
                assert_eq!(rel.components().count(), 4, "{rel:?}");
            }
        }
        assert_eq!(opencode_skill_files(home.path()).len(), 2);
        // Codex and OpenCode share one root, written once.
        let t = skill_targets(&[ClientId::Codex, ClientId::OpenCode], home.path());
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].0, home.path().join(".agents/skills"));
    }
}
