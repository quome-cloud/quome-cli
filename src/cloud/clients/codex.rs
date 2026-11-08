//! Codex registers through `codex mcp add`; `get --json` tells us whether the entry
//! already matches.
use super::runner::{CommandRunner, RunResult};
use super::{paths, ClientAdapter, ClientId, Ctx, RegisterOutcome};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct Codex;

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

fn matches(stdout: &str, program: &str, profile: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(stdout) else {
        return false;
    };
    let t = &v["transport"];
    t["command"] == program && t["args"] == serde_json::json!(["mcp", "--profile", profile])
}

fn run(r: &dyn CommandRunner, args: &[String]) -> Result<(bool, String, String), String> {
    match r.run("codex", args) {
        RunResult::NotFound => Err("`codex` is not on PATH".into()),
        RunResult::Ran { ok, stdout, stderr } => Ok((ok, stdout, stderr)),
    }
}

impl ClientAdapter for Codex {
    fn id(&self) -> ClientId {
        ClientId::Codex
    }
    fn installed(&self, home: &Path) -> bool {
        paths::installed_with(ClientId::Codex, home, &paths::on_real_path)
    }
    fn register_mcp(
        &self,
        ctx: &Ctx,
        program: &str,
        profile: &str,
    ) -> Result<RegisterOutcome, String> {
        let (exists, out, _) = run(ctx.runner, &argv(&["mcp", "get", "quome", "--json"]))?;
        if exists && matches(&out, program, profile) {
            return Ok(RegisterOutcome::Unchanged);
        }
        if exists {
            let _ = run(ctx.runner, &argv(&["mcp", "remove", "quome"]))?;
        }
        let mut add = argv(&["mcp", "add", "quome", "--", program, "mcp", "--profile"]);
        add.push(profile.into());
        let (ok, _, stderr) = run(ctx.runner, &add)?;
        if !ok {
            return Err(format!("`codex mcp add` failed: {}", stderr.trim()));
        }
        Ok(if exists {
            RegisterOutcome::Updated
        } else {
            RegisterOutcome::Added
        })
    }
    fn skills_dir(&self, home: &Path) -> PathBuf {
        paths::skills_dir(ClientId::Codex, home)
    }
    fn plugin_root(&self, home: &Path) -> Option<PathBuf> {
        paths::plugin_root(ClientId::Codex, home)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::clients::runner::fake::{fail, ok, Fake};

    const SAME: &str = r#"{"name":"quome","transport":{"type":"stdio","command":"quome","args":["mcp","--profile","dev"]}}"#;

    fn reg(r: &Fake, h: &Path) -> Result<RegisterOutcome, String> {
        Codex.register_mcp(&Ctx { home: h, runner: r }, "quome", "dev")
    }

    #[test]
    fn adds_when_absent_with_exact_argv() {
        let h = tempfile::tempdir().unwrap();
        let r = Fake::new(|_, a| if a[1] == "get" { fail("none") } else { ok("") });
        assert_eq!(reg(&r, h.path()), Ok(RegisterOutcome::Added));
        assert_eq!(
            r.calls(),
            vec![
                "codex mcp get quome --json",
                "codex mcp add quome -- quome mcp --profile dev"
            ]
        );
    }

    #[test]
    fn identical_entry_is_unchanged() {
        let h = tempfile::tempdir().unwrap();
        let r = Fake::new(|_, a| if a[1] == "get" { ok(SAME) } else { fail("no") });
        assert_eq!(reg(&r, h.path()), Ok(RegisterOutcome::Unchanged));
        assert_eq!(r.calls().len(), 1);
    }

    #[test]
    fn different_entry_is_removed_then_added() {
        let h = tempfile::tempdir().unwrap();
        let other = SAME.replace("dev", "work");
        let r = Fake::new(move |_, a| if a[1] == "get" { ok(&other) } else { ok("") });
        assert_eq!(reg(&r, h.path()), Ok(RegisterOutcome::Updated));
        assert_eq!(
            r.calls(),
            vec![
                "codex mcp get quome --json",
                "codex mcp remove quome",
                "codex mcp add quome -- quome mcp --profile dev"
            ]
        );
    }

    #[test]
    fn cli_failure_is_an_error() {
        let h = tempfile::tempdir().unwrap();
        let r = Fake::new(|_, a| {
            if a[1] == "add" {
                fail("boom\n")
            } else {
                fail("")
            }
        });
        let e = reg(&r, h.path()).unwrap_err();
        assert!(e.contains("boom"), "{e}");
    }

    #[test]
    fn not_installed_does_nothing() {
        let h = tempfile::tempdir().unwrap();
        let r = Fake::new(|_, _| RunResult::NotFound);
        assert!(reg(&r, h.path()).is_err());
        assert_eq!(r.calls().len(), 1);
        assert_eq!(std::fs::read_dir(h.path()).unwrap().count(), 0);
    }
}
