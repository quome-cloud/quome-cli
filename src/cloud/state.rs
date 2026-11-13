use crate::errors::QuomeError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub edition: String,
    pub api_url: String,
    pub console_url: String,
    pub issuer: String,
    pub agent_name: String,
    pub agent_principal: String,
    pub kid: String,
    pub jkt: String,
    pub delegation_id: String,
    pub expires_at: String,
    pub org_id: String,
    pub org_slug: String,
    pub apps_domain: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudState {
    #[serde(default)]
    pub default: String,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

/// `~/.quome` (or the injected home's `.quome`).
pub fn quome_dir(home: &Path) -> PathBuf {
    home.join(".quome")
}

/// Atomically write `bytes` to `path` with mode 0600 (temp file + rename).
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<(), QuomeError> {
    use std::io::Write;
    let dir = path
        .parent()
        .ok_or_else(|| QuomeError::CloudState("invalid path".into()))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let res = (|| {
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(res?)
}

fn home() -> Result<PathBuf, QuomeError> {
    dirs::home_dir().ok_or_else(|| QuomeError::CloudState("cannot find home directory".into()))
}

impl Profile {
    /// Refuse a profile whose URLs would send credentials over plain http.
    fn check_urls(&self, name: &str) -> Result<(), QuomeError> {
        for (field, v) in [
            ("api_url", &self.api_url),
            ("console_url", &self.console_url),
            ("issuer", &self.issuer),
        ] {
            crate::url_policy::validate_api_url(v, &format!("{field} of profile \"{name}\""))?;
        }
        Ok(())
    }
}

impl CloudState {
    pub fn load() -> Result<CloudState, QuomeError> {
        Self::load_from(&home()?)
    }

    pub fn save(&self) -> Result<(), QuomeError> {
        self.save_to(&home()?)
    }

    /// A missing file is an empty state (nothing connected yet).
    pub fn load_from(home: &Path) -> Result<CloudState, QuomeError> {
        match std::fs::read_to_string(quome_dir(home).join("cloud.json")) {
            Ok(s) => Ok(serde_json::from_str(&s)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(CloudState::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save_to(&self, home: &Path) -> Result<(), QuomeError> {
        let json = serde_json::to_vec_pretty(self)?;
        write_private(&quome_dir(home).join("cloud.json"), &json)
    }

    /// True when a profile other than the model machine's is saved.
    pub fn has_user_profiles(&self) -> bool {
        self.first_user_profile().is_some()
    }

    /// The first profile that is not the model machine's, which never serves as a
    /// default or fallback for the commands that act as the user's agent.
    pub fn first_user_profile(&self) -> Option<String> {
        self.profiles
            .keys()
            .find(|k| k.as_str() != crate::machine::config::PROFILE)
            .cloned()
    }

    /// The named profile, or the default one when `name` is `None`.
    pub fn profile(&self, name: Option<&str>) -> Result<&Profile, QuomeError> {
        let name = match name {
            Some(n) => n,
            None if self.default.is_empty() => {
                return Err(QuomeError::CloudState(
                    crate::commands::cloud::NOT_CONNECTED.into(),
                ))
            }
            None => &self.default,
        };
        let p = self
            .profiles
            .get(name)
            .ok_or_else(|| QuomeError::CloudState(format!("no profile named \"{name}\"")))?;
        p.check_urls(name)?;
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(n: &str) -> Profile {
        Profile {
            edition: "cloud".into(),
            api_url: "https://api.quome.cloud".into(),
            console_url: "https://console.quome.cloud".into(),
            issuer: "https://auth.quome.cloud".into(),
            agent_name: format!("agent-{n}"),
            agent_principal: format!("principal-{n}"),
            kid: format!("kid-{n}"),
            jkt: format!("jkt-{n}"),
            delegation_id: format!("del-{n}"),
            expires_at: "2027-01-01T00:00:00Z".into(),
            org_id: format!("org-{n}"),
            org_slug: format!("slug-{n}"),
            apps_domain: "apps.quome.cloud".into(),
        }
    }

    #[test]
    fn profile_with_plain_http_remote_url_is_refused() {
        let mut s = state();
        let name = s.default.clone();
        s.profiles.get_mut(&name).unwrap().api_url = "http://api.example.com".into();
        let e = s.profile(None).unwrap_err().to_string();
        assert!(e.contains("api_url") && e.contains("https"), "{e}");
        let mut s = state();
        s.profiles.get_mut(&name).unwrap().issuer = "http://auth.example.com".into();
        assert!(s.profile(None).is_err());
        let mut s = state();
        s.profiles.get_mut(&name).unwrap().api_url = "http://127.0.0.1:8080".into();
        assert!(s.profile(None).is_ok());
    }

    fn state() -> CloudState {
        let mut s = CloudState {
            default: "work".into(),
            ..Default::default()
        };
        s.profiles.insert("work".into(), sample("w"));
        s.profiles.insert("home".into(), sample("h"));
        s
    }

    #[test]
    fn round_trip_preserves_every_field_and_mode_is_0600() {
        let dir = tempfile::tempdir().unwrap();
        let s = state();
        s.save_to(dir.path()).unwrap();
        assert_eq!(CloudState::load_from(dir.path()).unwrap(), s);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(dir.path().join(".quome/cloud.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(m & 0o777, 0o600);
        }
    }

    #[test]
    fn json_shape_matches_spec() {
        let v = serde_json::to_value(state()).unwrap();
        assert_eq!(v["default"], "work");
        assert_eq!(v["profiles"]["work"]["edition"], "cloud");
        assert_eq!(v["profiles"]["work"]["apps_domain"], "apps.quome.cloud");
    }

    #[test]
    fn load_missing_file_is_empty_state() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            CloudState::load_from(dir.path()).unwrap(),
            CloudState::default()
        );
    }

    #[test]
    fn save_overwrites_atomically_keeping_mode() {
        let dir = tempfile::tempdir().unwrap();
        state().save_to(dir.path()).unwrap();
        let mut s = state();
        s.default = "home".into();
        s.save_to(dir.path()).unwrap();
        assert_eq!(CloudState::load_from(dir.path()).unwrap().default, "home");
        let n = std::fs::read_dir(dir.path().join(".quome"))
            .unwrap()
            .count();
        assert_eq!(n, 1, "no temp files left behind");
    }

    #[test]
    fn profile_selection() {
        let s = state();
        assert_eq!(s.profile(None).unwrap().agent_name, "agent-w");
        assert_eq!(s.profile(Some("home")).unwrap().agent_name, "agent-h");
        assert!(s.profile(Some("nope")).is_err());
        let e = CloudState::default().profile(None).unwrap_err().to_string();
        assert!(e.contains("`quome connect`"), "{e}");
        assert!(!e.contains("cloud connect"), "{e}");
    }
}
