//! Agent clients the CLI can wire up: skill rendering, paths, MCP registration.
pub mod paths;

#[allow(unused_imports)]
pub use super::playbooks::render_skill;
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

/// One agent client. `register_mcp` is only declared here: the adapters below
/// return an error until the registration task replaces them per client.
pub trait ClientAdapter {
    fn id(&self) -> ClientId;
    fn installed(&self, home: &Path) -> bool;
    fn register_mcp(
        &self,
        home: &Path,
        program: &str,
        profile: &str,
    ) -> Result<RegisterOutcome, String>;
    fn skills_root(&self, home: &Path) -> PathBuf;
}

struct Adapter(ClientId);

impl ClientAdapter for Adapter {
    fn id(&self) -> ClientId {
        self.0
    }
    fn installed(&self, home: &Path) -> bool {
        paths::installed_with(self.0, home, &paths::on_real_path)
    }
    fn register_mcp(&self, _: &Path, _: &str, _: &str) -> Result<RegisterOutcome, String> {
        Err("not implemented".into())
    }
    fn skills_root(&self, home: &Path) -> PathBuf {
        paths::skills_root(self.0, home)
    }
}

pub fn adapters() -> Vec<Box<dyn ClientAdapter>> {
    ClientId::ALL
        .iter()
        .map(|c| Box::new(Adapter(*c)) as Box<dyn ClientAdapter>)
        .collect()
}

/// Skill roots to write for the installed clients, each root once. Codex and
/// OpenCode share a root and render identically, so the first one wins.
pub fn skill_targets(installed: &[ClientId], home: &Path) -> Vec<(PathBuf, ClientId)> {
    let mut out: Vec<(PathBuf, ClientId)> = Vec::new();
    for c in installed {
        let root = paths::skills_root(*c, home);
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
