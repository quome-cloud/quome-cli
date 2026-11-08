//! Where each agent client keeps its skills, and how to tell it is installed.
//! One row per client; every row cites the evidence it rests on.
use super::ClientId;
use std::path::{Path, PathBuf};

pub struct PathRow {
    pub client: ClientId,
    /// Skills root, relative to the home directory.
    pub skills_root: &'static str,
    /// Binary names that mark the client as installed when found on PATH.
    pub binaries: &'static [&'static str],
    /// Home-relative directories that mark the client as installed.
    pub config_dirs: &'static [&'static str],
}

pub const TABLE: &[PathRow] = &[
    // Claude Code: a folder under ~/.claude/skills holding .claude-plugin/plugin.json
    // is a skills-directory plugin, loaded every session with no install step.
    // https://code.claude.com/docs/en/plugins/create.md
    PathRow {
        client: ClientId::Claude,
        skills_root: ".claude/skills/quome",
        binaries: &["claude"],
        config_dirs: &[".claude"],
    },
    // Codex: user skills live in $HOME/.agents/skills.
    // https://developers.openai.com/codex/skills
    PathRow {
        client: ClientId::Codex,
        skills_root: ".agents/skills",
        binaries: &["codex"],
        config_dirs: &[".codex"],
    },
    // OpenCode reads ~/.agents/skills as a global skills location, so it shares
    // Codex's root and gets no copy of its own (that would load twice).
    // https://opencode.ai/docs/skills
    PathRow {
        client: ClientId::OpenCode,
        skills_root: ".agents/skills",
        binaries: &["opencode"],
        config_dirs: &[".config/opencode"],
    },
    // Antigravity: global skills for App, CLI and IDE in ~/.gemini/config/skills.
    // https://antigravity.google/docs/skills (docs only; not exercised)
    PathRow {
        client: ClientId::Antigravity,
        skills_root: ".gemini/config/skills",
        binaries: &[],
        config_dirs: &[".gemini"],
    },
];

pub fn row(client: ClientId) -> &'static PathRow {
    TABLE
        .iter()
        .find(|r| r.client == client)
        .expect("a row per client")
}

pub fn skills_root(client: ClientId, home: &Path) -> PathBuf {
    home.join(row(client).skills_root)
}

/// True if `name` is an executable file in a PATH directory.
pub fn on_real_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
        .unwrap_or(false)
}

pub fn installed_with(client: ClientId, home: &Path, on_path: &dyn Fn(&str) -> bool) -> bool {
    let r = row(client);
    r.binaries.iter().any(|b| on_path(b)) || r.config_dirs.iter().any(|d| home.join(d).exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_table_has_a_row_per_client() {
        for c in ClientId::ALL {
            assert_eq!(row(c).client, c);
        }
        assert_eq!(TABLE.len(), ClientId::ALL.len());
    }

    #[test]
    fn detection_uses_injected_path_and_config_dirs() {
        let home = tempfile::tempdir().unwrap();
        let none = |_: &str| false;
        for c in ClientId::ALL {
            assert!(!installed_with(c, home.path(), &none));
        }
        assert!(installed_with(ClientId::Codex, home.path(), &|b| b == "codex"));
        assert!(!installed_with(ClientId::Claude, home.path(), &|b| b == "codex"));
        std::fs::create_dir_all(home.path().join(".gemini")).unwrap();
        assert!(installed_with(ClientId::Antigravity, home.path(), &none));
        std::fs::create_dir_all(home.path().join(".config/opencode")).unwrap();
        assert!(installed_with(ClientId::OpenCode, home.path(), &none));
    }
}
