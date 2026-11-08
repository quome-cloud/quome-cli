//! OpenCode registers through `opencode mcp add --global`, which overwrites an
//! existing entry and keeps comments and other keys. There is no `get`, so we read
//! `opencode.json` (read-only, comment tolerant) to tell Unchanged from Updated.
use super::runner::RunResult;
use super::{jsonedit, paths, ClientAdapter, ClientId, Ctx, RegisterOutcome};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub struct OpenCode;

/// The config file the CLI reads and writes: `$XDG_CONFIG_HOME/opencode` when set and
/// non-empty, else `~/.config/opencode`; `opencode.json`, or `opencode.jsonc` when
/// only that exists (the CLI edits `.json` first if both are present).
pub fn config_path(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    let dir = match env("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(x) => PathBuf::from(x).join("opencode"),
        None => home.join(".config/opencode"),
    };
    let json = dir.join("opencode.json");
    let jsonc = dir.join("opencode.jsonc");
    if !json.exists() && jsonc.exists() {
        jsonc
    } else {
        json
    }
}

enum Current {
    Missing,
    Same,
    Different,
}

fn current(ctx: &Ctx, program: &str, profile: &str) -> Result<Current, String> {
    let path = config_path(ctx.home, ctx.env);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Current::Missing),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(Current::Missing);
    }
    // The CLI would mangle a file it cannot parse, so refuse before calling it.
    let doc = jsonedit::parse_lenient(&text)
        .filter(Value::is_object)
        .ok_or_else(|| format!("{} is not valid JSON; left untouched", path.display()))?;
    let entry = &doc["mcp"]["servers"]["quome"];
    if entry.is_null() {
        return Ok(Current::Missing);
    }
    let want = json!([program, "mcp", "--profile", profile]);
    Ok(if entry["type"] == "local" && entry["command"] == want {
        Current::Same
    } else {
        Current::Different
    })
}

impl ClientAdapter for OpenCode {
    fn id(&self) -> ClientId {
        ClientId::OpenCode
    }
    fn register_mcp(
        &self,
        ctx: &Ctx,
        program: &str,
        profile: &str,
    ) -> Result<RegisterOutcome, String> {
        let cur = current(ctx, program, profile)?;
        if matches!(cur, Current::Same) {
            return Ok(RegisterOutcome::Unchanged);
        }
        let args: Vec<String> = [
            "mcp",
            "add",
            "quome",
            "--global",
            "--",
            program,
            "mcp",
            "--profile",
            profile,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        match ctx.runner.run("opencode", &args) {
            RunResult::NotFound => Err("`opencode` is not on PATH".into()),
            RunResult::Ran {
                ok: false, stderr, ..
            } => Err(format!("`opencode mcp add` failed: {}", stderr.trim())),
            RunResult::Ran { .. } => Ok(match cur {
                Current::Different => RegisterOutcome::Updated,
                _ => RegisterOutcome::Added,
            }),
        }
    }
    fn plugin_root(&self, home: &Path) -> Option<PathBuf> {
        paths::plugin_root(ClientId::OpenCode, home)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::clients::runner::fake::{fail, ok, Fake};

    fn reg(r: &Fake, h: &Path) -> Result<RegisterOutcome, String> {
        OpenCode.register_mcp(
            &Ctx {
                home: h,
                runner: r,
                env: &crate::cloud::clients::no_env,
            },
            "quome",
            "dev",
        )
    }

    fn write(h: &Path, text: &str) -> PathBuf {
        let p = config_path(h, &crate::cloud::clients::no_env);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        p
    }

    const SAME: &str = "{\n  // keep me\n  \"mcp\": {\"servers\": {\"quome\": {\"type\": \"local\", \"command\": [\"quome\", \"mcp\", \"--profile\", \"dev\"],},},},\n}\n";

    #[test]
    fn adds_with_exact_argv_and_never_edits_the_file() {
        let h = tempfile::tempdir().unwrap();
        let r = Fake::new(|_, _| ok(""));
        assert_eq!(reg(&r, h.path()), Ok(RegisterOutcome::Added));
        assert_eq!(
            r.calls(),
            vec!["opencode mcp add quome --global -- quome mcp --profile dev"]
        );
        assert!(!config_path(h.path(), &crate::cloud::clients::no_env).exists());
    }

    #[test]
    fn identical_entry_in_commented_config_is_unchanged() {
        let h = tempfile::tempdir().unwrap();
        let p = write(h.path(), SAME);
        let r = Fake::new(|_, _| fail("should not run"));
        assert_eq!(reg(&r, h.path()), Ok(RegisterOutcome::Unchanged));
        assert!(r.calls().is_empty());
        assert_eq!(std::fs::read_to_string(p).unwrap(), SAME);
    }

    #[test]
    fn different_entry_is_updated_by_the_cli() {
        let h = tempfile::tempdir().unwrap();
        write(h.path(), &SAME.replace("dev", "work"));
        let r = Fake::new(|_, _| ok(""));
        assert_eq!(reg(&r, h.path()), Ok(RegisterOutcome::Updated));
        assert_eq!(r.calls().len(), 1);
    }

    #[test]
    fn config_without_our_entry_is_added() {
        let h = tempfile::tempdir().unwrap();
        write(h.path(), "{\"model\":\"a/b\"}");
        let r = Fake::new(|_, _| ok(""));
        assert_eq!(reg(&r, h.path()), Ok(RegisterOutcome::Added));
    }

    #[test]
    fn unparseable_config_is_left_alone() {
        let h = tempfile::tempdir().unwrap();
        let p = write(h.path(), "{ not json");
        let r = Fake::new(|_, _| ok(""));
        assert!(reg(&r, h.path()).is_err());
        assert!(r.calls().is_empty());
        assert_eq!(std::fs::read_to_string(p).unwrap(), "{ not json");
    }

    #[test]
    fn cli_failure_is_an_error_and_missing_cli_does_nothing() {
        let h = tempfile::tempdir().unwrap();
        let r = Fake::new(|_, _| fail("denied\n"));
        assert!(reg(&r, h.path()).unwrap_err().contains("denied"));
        let r = Fake::new(|_, _| RunResult::NotFound);
        assert!(reg(&r, h.path()).is_err());
        assert_eq!(std::fs::read_dir(h.path()).unwrap().count(), 0);
    }

    fn reg_env(
        r: &Fake,
        h: &Path,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<RegisterOutcome, String> {
        OpenCode.register_mcp(
            &Ctx {
                home: h,
                runner: r,
                env,
            },
            "quome",
            "dev",
        )
    }

    #[test]
    fn xdg_config_home_wins_over_stale_home_config() {
        let h = tempfile::tempdir().unwrap();
        let x = tempfile::tempdir().unwrap();
        // A stale entry under ~/.config would give a false Unchanged.
        write(h.path(), SAME);
        let xp = x.path().to_string_lossy().into_owned();
        let env = move |k: &str| (k == "XDG_CONFIG_HOME").then(|| xp.clone());
        let r = Fake::new(|_, _| ok(""));
        assert_eq!(reg_env(&r, h.path(), &env), Ok(RegisterOutcome::Added));
        assert_eq!(r.calls().len(), 1);
        // The guard reads the XDG file too.
        let p = x.path().join("opencode/opencode.json");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "{ not json").unwrap();
        assert!(reg_env(&r, h.path(), &env).is_err());
        assert_eq!(r.calls().len(), 1);
        std::fs::write(&p, SAME).unwrap();
        assert_eq!(reg_env(&r, h.path(), &env), Ok(RegisterOutcome::Unchanged));
    }

    #[test]
    fn empty_xdg_config_home_falls_back_to_home() {
        let h = tempfile::tempdir().unwrap();
        write(h.path(), SAME);
        let env = |k: &str| (k == "XDG_CONFIG_HOME").then(String::new);
        let r = Fake::new(|_, _| ok(""));
        assert_eq!(reg_env(&r, h.path(), &env), Ok(RegisterOutcome::Unchanged));
    }

    #[test]
    fn jsonc_is_read_when_it_is_the_only_config() {
        let h = tempfile::tempdir().unwrap();
        let d = h.path().join(".config/opencode");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("opencode.jsonc"), SAME).unwrap();
        let r = Fake::new(|_, _| ok(""));
        assert_eq!(
            reg_env(&r, h.path(), &crate::cloud::clients::no_env),
            Ok(RegisterOutcome::Unchanged)
        );
        // With both present the CLI edits .json, so that is the one we read.
        std::fs::write(d.join("opencode.json"), "{}").unwrap();
        assert_eq!(
            reg_env(&r, h.path(), &crate::cloud::clients::no_env),
            Ok(RegisterOutcome::Added)
        );
    }

    #[test]
    fn lenient_parse_ignores_comments_but_not_strings() {
        let v =
            jsonedit::parse_lenient("{\"u\": \"http://x/*y*/\", /* c */ \"a\": [1,],}").unwrap();
        assert_eq!(v["u"], "http://x/*y*/");
        assert_eq!(v["a"], json!([1]));
    }
}
