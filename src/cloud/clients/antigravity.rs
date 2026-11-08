//! Antigravity has no MCP CLI: edit `~/.gemini/config/mcp_config.json`, setting only
//! `mcpServers.quome`. A file that does not parse is left alone.
use super::{jsonedit, paths, ClientAdapter, ClientId, Ctx, RegisterOutcome};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub struct Antigravity;

fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    // Temp file in the same directory, then rename, so a crash never leaves a
    // half-written config.
    let tmp = path.with_extension("json.quome-tmp");
    let res = std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, path));
    res.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot write {}: {e}", path.display())
    })
}

pub fn config_path(home: &Path) -> PathBuf {
    home.join(".gemini/config/mcp_config.json")
}

impl ClientAdapter for Antigravity {
    fn id(&self) -> ClientId {
        ClientId::Antigravity
    }
    fn installed(&self, home: &Path) -> bool {
        paths::installed_with(ClientId::Antigravity, home, &paths::on_real_path)
    }
    fn register_mcp(
        &self,
        ctx: &Ctx,
        program: &str,
        profile: &str,
    ) -> Result<RegisterOutcome, String> {
        let path = config_path(ctx.home);
        let want = json!({"command": program, "args": ["mcp", "--profile", profile]});
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
        };
        if text.trim().is_empty() {
            let doc = json!({"mcpServers": {"quome": want}});
            write(&path, &jsonedit::render_like(&text, &doc))?;
            return Ok(RegisterOutcome::Added);
        }
        // Strict parse: a file with comments is not ours to rewrite.
        let doc = serde_json::from_str::<Value>(&text)
            .ok()
            .filter(Value::is_object)
            .ok_or_else(|| format!("{} is not valid JSON; left untouched", path.display()))?;
        let outcome = match &doc["mcpServers"] {
            Value::Null => RegisterOutcome::Added,
            Value::Object(m) => match m.get("quome") {
                Some(cur) if cur == &want => return Ok(RegisterOutcome::Unchanged),
                Some(_) => RegisterOutcome::Updated,
                None => RegisterOutcome::Added,
            },
            _ => {
                return Err(format!(
                    "`mcpServers` in {} is not an object",
                    path.display()
                ))
            }
        };
        let out = jsonedit::splice_set(&text, &["mcpServers", "quome"], &want)
            .ok_or_else(|| format!("cannot edit {}; left untouched", path.display()))?;
        write(&path, &out)?;
        Ok(outcome)
    }
    fn skills_dir(&self, home: &Path) -> PathBuf {
        paths::skills_dir(ClientId::Antigravity, home)
    }
    fn plugin_root(&self, home: &Path) -> Option<PathBuf> {
        paths::plugin_root(ClientId::Antigravity, home)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::clients::runner::fake::{ok, Fake};

    fn reg(home: &Path, p: &str) -> Result<RegisterOutcome, String> {
        let r = Fake::new(|_, _| ok(""));
        Antigravity.register_mcp(
            &Ctx {
                home,
                runner: &r,
                env: &crate::cloud::clients::no_env,
            },
            "quome",
            p,
        )
    }

    const FIXTURE: &str = "{\n    \"theme\": \"dark\",\n    \"mcpServers\": {\n        \"other\": {\n            \"command\": \"x\",\n            \"args\": [\"a\", \"b\"]\n        }\n    },\n    \"zeta\": [1, 2.5, true]\n}\n";

    #[test]
    fn config_merge_preserves_other_bytes() {
        let h = tempfile::tempdir().unwrap();
        let p = config_path(h.path());
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, FIXTURE).unwrap();
        assert_eq!(reg(h.path(), "dev"), Ok(RegisterOutcome::Added));
        let after = std::fs::read_to_string(&p).unwrap();
        let quome = "        \"quome\": {\n            \"command\": \"quome\",\n            \"args\": [\n                \"mcp\",\n                \"--profile\",\n                \"dev\"\n            ]\n        }\n";
        // Removing the added entry gives back the original bytes exactly.
        let expect = FIXTURE.replace(
            "        }\n    },\n    \"zeta\"",
            &format!("        }},\n{}    }},\n    \"zeta\"", quome),
        );
        assert_eq!(after, expect);
    }

    #[test]
    fn register_twice_is_unchanged() {
        let h = tempfile::tempdir().unwrap();
        assert_eq!(reg(h.path(), "dev"), Ok(RegisterOutcome::Added));
        let first = std::fs::read(config_path(h.path())).unwrap();
        assert_eq!(reg(h.path(), "dev"), Ok(RegisterOutcome::Unchanged));
        assert_eq!(std::fs::read(config_path(h.path())).unwrap(), first);
        assert_eq!(reg(h.path(), "work"), Ok(RegisterOutcome::Updated));
    }

    #[test]
    fn creates_missing_file_and_dirs() {
        let h = tempfile::tempdir().unwrap();
        reg(h.path(), "dev").unwrap();
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(config_path(h.path())).unwrap()).unwrap();
        assert_eq!(
            v["mcpServers"]["quome"],
            json!({"command":"quome","args":["mcp","--profile","dev"]})
        );
    }

    #[test]
    fn unparseable_config_is_left_alone() {
        let h = tempfile::tempdir().unwrap();
        let p = config_path(h.path());
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        for bad in ["{ not json", "// c\n{\"mcpServers\":{}}", "[1]"] {
            std::fs::write(&p, bad).unwrap();
            assert!(reg(h.path(), "dev").is_err());
            assert_eq!(std::fs::read_to_string(&p).unwrap(), bad);
        }
    }

    #[test]
    fn splice_variants_keep_surrounding_bytes() {
        let h = tempfile::tempdir().unwrap();
        let p = config_path(h.path());
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let q = r#"{"command":"quome","args":["mcp","--profile","dev"]}"#;
        let cases = [
            // compact, no mcpServers
            ("{\"a\":[1, 2]}", format!("{{\"a\":[1, 2],\"mcpServers\":{{\"quome\":{q}}}}}")),
            // empty mcpServers, tab indent
            (
                "{\n\t\"mcpServers\": {}\n}",
                "{\n\t\"mcpServers\": {\n\t\t\"quome\": {\n\t\t\t\"command\": \"quome\",\n\t\t\t\"args\": [\n\t\t\t\t\"mcp\",\n\t\t\t\t\"--profile\",\n\t\t\t\t\"dev\"\n\t\t\t]\n\t\t}\n\t}\n}".to_string(),
            ),
            // update in place, odd spacing kept
            (
                "{ \"x\" : 1 , \"mcpServers\":{\"quome\":{\"command\":\"old\"},\"o\":2} }",
                format!("{{ \"x\" : 1 , \"mcpServers\":{{\"quome\":{q},\"o\":2}} }}"),
            ),
        ];
        for (before, after) in cases {
            std::fs::write(&p, before).unwrap();
            reg(h.path(), "dev").unwrap();
            assert_eq!(std::fs::read_to_string(&p).unwrap(), after);
        }
    }

    #[test]
    fn crlf_files_get_crlf_insertions() {
        let h = tempfile::tempdir().unwrap();
        let p = config_path(h.path());
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            &p,
            "{\r\n  \"mcpServers\": {\r\n    \"o\": 1\r\n  }\r\n}\r\n",
        )
        .unwrap();
        reg(h.path(), "dev").unwrap();
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(
            after.starts_with("{\r\n  \"mcpServers\": {\r\n    \"o\": 1,\r\n    \"quome\": {\r\n")
        );
        assert!(
            !after.replace("\r\n", "").contains('\n'),
            "bare LF in {after:?}"
        );
    }

    #[test]
    fn write_leaves_no_temp_file() {
        let h = tempfile::tempdir().unwrap();
        reg(h.path(), "dev").unwrap();
        let names: Vec<_> = std::fs::read_dir(config_path(h.path()).parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["mcp_config.json"]);
    }

    #[test]
    fn not_installed_does_nothing() {
        let h = tempfile::tempdir().unwrap();
        assert!(!paths::installed_with(
            ClientId::Antigravity,
            h.path(),
            &|_| false
        ));
        assert!(!h.path().join(".gemini").exists());
    }
}
