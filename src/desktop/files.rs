//! The two files `quome desktop serve` shares with the desktop app, in the runtime
//! directory: `session.json` (written here, read by the app before every dial) and
//! `state.json` (written by the app, read here). The session's key is never logged,
//! printed or shown: only the app reads it, from a 0600 file in a 0700 directory.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

pub const SESSION_FILE: &str = "session.json";
pub const STATE_FILE: &str = "state.json";
/// The one format version of both files.
pub const VERSION: u32 = 1;

/// `~/Library/Application Support/Quome Desktop/run`.
pub fn runtime_dir(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Application Support")
        .join("Quome Desktop")
        .join("run")
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Size {
    pub w: u16,
    pub h: u16,
}

/// Where the app dials and the key it dials with.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionFile {
    pub v: u32,
    pub url: String,
    pub desktop: String,
    pub key: String,
    pub screen: Size,
    /// RFC 3339.
    pub written_at: String,
}

/// Never prints the key itself.
impl std::fmt::Debug for SessionFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionFile")
            .field("v", &self.v)
            .field("url", &self.url)
            .field("desktop", &self.desktop)
            .field("key", &"<redacted>")
            .field("screen", &self.screen)
            .field("written_at", &self.written_at)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
pub struct Permissions {
    #[serde(default)]
    pub screen_recording: bool,
    #[serde(default)]
    pub accessibility: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
pub struct MainDisplay {
    pub w: u32,
    pub h: u32,
    #[serde(default)]
    pub scale: f64,
}

/// What the app says about itself. Fields it leaves out read as their defaults;
/// fields this version does not know are ignored.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct AppState {
    pub v: u32,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub screen_state: Option<String>,
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default)]
    pub displays: u32,
    #[serde(default)]
    pub main_display: Option<MainDisplay>,
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub dial_failures: u32,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// Make `dir` (and its parents) and set it to 0700.
fn private_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Write `session.json` atomically: a 0600 temp file in the 0700 directory, then a rename.
pub fn write_session(dir: &Path, s: &SessionFile) -> io::Result<()> {
    use std::io::Write;
    private_dir(dir)?;
    let bytes = serde_json::to_vec_pretty(s).map_err(io::Error::other)?;
    let tmp = dir.join(format!(".{SESSION_FILE}.{}.tmp", std::process::id()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let res = (|| {
        let mut f = opts.open(&tmp)?;
        // A temp file left by an earlier run keeps its old mode: set it again.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(&bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, dir.join(SESSION_FILE))
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
        return res;
    }
    // Make the rename durable; best effort (a directory cannot be opened everywhere).
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Remove `session.json`; an absent file is fine.
pub fn remove_session(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(SESSION_FILE));
}

/// `state.json`, when it is there, reads, and is version 1.
pub fn read_state(dir: &Path) -> Option<AppState> {
    let text = std::fs::read(dir.join(STATE_FILE)).ok()?;
    let s: AppState = serde_json::from_slice(&text).ok()?;
    (s.v == VERSION).then_some(s)
}

/// A `state.json` older than this is a stopped or missing app's: it asks for nothing.
pub const STATE_FRESH_FOR: chrono::Duration = chrono::Duration::seconds(60);

/// True when the app wrote `s` within `STATE_FRESH_FOR` of `now`.
pub fn fresh(s: &AppState, now: chrono::DateTime<chrono::Utc>) -> bool {
    s.updated_at
        .as_deref()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .is_some_and(|t| now - t.with_timezone(&chrono::Utc) <= STATE_FRESH_FOR)
}

/// `read_state`, but a state older than a minute reads as no state: the app is not reporting.
pub fn read_fresh_state(dir: &Path) -> Option<AppState> {
    read_state(dir).filter(|s| fresh(s, chrono::Utc::now()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stale_state_reads_as_not_reporting() {
        let dir = tempfile::tempdir().unwrap();
        let at = |secs: i64| {
            (chrono::Utc::now() - chrono::Duration::seconds(secs))
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        };
        let write = |t: String| {
            std::fs::write(
                dir.path().join(STATE_FILE),
                serde_json::json!({"v": 1, "updated_at": t}).to_string(),
            )
            .unwrap()
        };
        write(at(5));
        assert!(read_fresh_state(dir.path()).is_some());
        write(at(120));
        assert!(read_state(dir.path()).is_some());
        assert!(read_fresh_state(dir.path()).is_none());
        std::fs::write(dir.path().join(STATE_FILE), r#"{"v": 1}"#).unwrap();
        assert!(read_fresh_state(dir.path()).is_none(), "no timestamp");
    }

    fn session(key: &str) -> SessionFile {
        SessionFile {
            v: 1,
            url: "wss://desktops.example.test/acme/v1/daemon".into(),
            desktop: "d-1".into(),
            key: key.into(),
            screen: Size { w: 1920, h: 1200 },
            written_at: "2026-11-24T10:00:00Z".into(),
        }
    }

    #[test]
    fn runtime_dir_is_under_application_support() {
        assert_eq!(
            runtime_dir(Path::new("/Users/a")),
            Path::new("/Users/a/Library/Application Support/Quome Desktop/run")
        );
    }

    #[test]
    fn session_file_format_and_modes() {
        let home = tempfile::tempdir().unwrap();
        let dir = runtime_dir(home.path());
        write_session(&dir, &session("qpk_one")).unwrap();
        let v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("session.json")).unwrap()).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "v": 1, "url": "wss://desktops.example.test/acme/v1/daemon", "desktop": "d-1",
                "key": "qpk_one", "screen": {"w": 1920, "h": 1200},
                "written_at": "2026-11-24T10:00:00Z"
            })
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&dir), 0o700);
            assert_eq!(mode(&dir.join("session.json")), 0o600);
        }
        // A second write replaces it and leaves no temp file behind.
        write_session(&dir, &session("qpk_two")).unwrap();
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, ["session.json"]);
        assert!(std::fs::read_to_string(dir.join("session.json"))
            .unwrap()
            .contains("qpk_two"));
        assert!(!format!("{:?}", session("qpk_two")).contains("qpk_two"));

        remove_session(&dir);
        assert!(!dir.join("session.json").exists());
        remove_session(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_loose_runtime_dir_is_tightened() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let dir = runtime_dir(home.path());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        write_session(&dir, &session("k")).unwrap();
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn state_file_reads_the_contract_and_ignores_unknown_fields() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(read_state(d.path()), None);
        std::fs::write(
            d.path().join("state.json"),
            r#"{"v": 1, "version": "0.1.0", "screen_state": "ok",
                "permissions": {"screen_recording": true, "accessibility": false},
                "displays": 1, "main_display": {"w": 3024, "h": 1890, "scale": 2.0},
                "connected": true, "dial_failures": 0, "updated_at": "2026-11-24T10:00:00Z",
                "something_new": {"x": 1}}"#,
        )
        .unwrap();
        let s = read_state(d.path()).unwrap();
        assert_eq!(s.version.as_deref(), Some("0.1.0"));
        assert_eq!(s.screen_state.as_deref(), Some("ok"));
        assert!(s.permissions.screen_recording && !s.permissions.accessibility);
        assert_eq!(s.displays, 1);
        assert_eq!(
            s.main_display,
            Some(MainDisplay {
                w: 3024,
                h: 1890,
                scale: 2.0
            })
        );
        assert!(s.connected);
        assert_eq!(s.dial_failures, 0);

        std::fs::write(
            d.path().join("state.json"),
            r#"{"v": 1, "main_display": null, "dial_failures": 5}"#,
        )
        .unwrap();
        let s = read_state(d.path()).unwrap();
        assert_eq!((s.main_display, s.dial_failures), (None, 5));
    }

    #[test]
    fn another_state_version_or_garbage_is_none() {
        let d = tempfile::tempdir().unwrap();
        for text in [
            r#"{"v": 2, "dial_failures": 5}"#,
            r#"{"dial_failures": 5}"#,
            "{",
            "",
        ] {
            std::fs::write(d.path().join("state.json"), text).unwrap();
            assert_eq!(read_state(d.path()), None, "{text}");
        }
    }
}
