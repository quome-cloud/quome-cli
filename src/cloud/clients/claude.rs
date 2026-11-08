//! Claude Code: wraps the `claude mcp` flow `quome connect` already uses.
use super::runner::{CommandRunner, RunResult};
use super::{paths, ClientAdapter, ClientId, Ctx, RegisterOutcome};
use crate::commands::connect::{claude_register, ClaudeMcp, ClaudeResult, ClaudeRunner};
use std::path::{Path, PathBuf};

pub struct Claude;

/// Presents the shared runner as the `claude` program.
struct ViaRunner<'a>(&'a dyn CommandRunner);

impl ClaudeRunner for ViaRunner<'_> {
    fn run(&self, args: &[String]) -> ClaudeResult {
        match self.0.run("claude", args) {
            RunResult::NotFound => ClaudeResult::NotFound,
            RunResult::Ran { ok, stderr, .. } => ClaudeResult::Ran { ok, stderr },
        }
    }
}

impl ClientAdapter for Claude {
    fn id(&self) -> ClientId {
        ClientId::Claude
    }
    fn installed(&self, home: &Path) -> bool {
        paths::installed_with(ClientId::Claude, home, &paths::on_real_path)
    }
    fn register_mcp(
        &self,
        ctx: &Ctx,
        program: &str,
        profile: &str,
    ) -> Result<RegisterOutcome, String> {
        match claude_register(&ViaRunner(ctx.runner), program, profile) {
            ClaudeMcp::Added => Ok(RegisterOutcome::Added),
            ClaudeMcp::Updated => Ok(RegisterOutcome::Updated),
            ClaudeMcp::NotFound => Err("`claude` is not on PATH".into()),
            ClaudeMcp::Failed(e) => Err(format!("`claude mcp add` failed: {e}")),
        }
    }
    fn skills_dir(&self, home: &Path) -> PathBuf {
        paths::skills_dir(ClientId::Claude, home)
    }
    fn plugin_root(&self, home: &Path) -> Option<PathBuf> {
        paths::plugin_root(ClientId::Claude, home)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::clients::runner::fake::{fail, ok, Fake};

    fn reg(r: &Fake) -> Result<RegisterOutcome, String> {
        let h = tempfile::tempdir().unwrap();
        Claude.register_mcp(
            &Ctx {
                home: h.path(),
                runner: r,
                env: &crate::cloud::clients::no_env,
            },
            "quome",
            "dev",
        )
    }

    #[test]
    fn adds_with_the_connect_argv() {
        let r = Fake::new(|_, a| if a[1] == "get" { fail("") } else { ok("") });
        assert_eq!(reg(&r), Ok(RegisterOutcome::Added));
        assert_eq!(
            r.calls(),
            vec![
                "claude mcp get quome",
                "claude mcp add --scope user quome -- quome mcp --profile dev"
            ]
        );
    }

    #[test]
    fn existing_entry_is_replaced() {
        let r = Fake::new(|_, _| ok(""));
        assert_eq!(reg(&r), Ok(RegisterOutcome::Updated));
        assert_eq!(r.calls()[1], "claude mcp remove --scope user quome");
    }

    #[test]
    fn failure_and_missing_cli_are_errors() {
        let r = Fake::new(|_, a| {
            if a[1] == "add" {
                fail("bad\n")
            } else {
                fail("")
            }
        });
        assert!(reg(&r).unwrap_err().contains("bad"));
        let r = Fake::new(|_, _| RunResult::NotFound);
        assert!(reg(&r).is_err());
    }
}
