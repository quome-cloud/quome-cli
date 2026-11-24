//! This computer's desktop link: its profile name, its scope and its settings file.
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::{Path, PathBuf};

use crate::cloud::state::write_private;
use crate::errors::QuomeError;

/// The profile (and Keychain account) a computer links under.
pub const PROFILE: &str = "desktop";
/// The one capability a computer's delegation carries.
pub const SCOPE: &str = "desktops_serve";

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct DesktopConfig {
    /// The platform's API URL.
    pub api: String,
    /// The organization (slug) this computer serves.
    pub org: String,
    /// The size agents see, fixed at the first session; unset until picked.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "ser_screen",
        deserialize_with = "de_screen"
    )]
    pub screen: Option<(u16, u16)>,
    /// Agents get the smaller size of the display's shape.
    #[serde(default)]
    pub small_frames: bool,
    /// Set once the agents are installed and the permission panes were shown.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub panes_shown: bool,
}

/// `~/.config/quome/desktop.toml`.
pub fn config_path(home: &Path) -> PathBuf {
    home.join(".config").join("quome").join("desktop.toml")
}

/// A missing file is `None`.
#[allow(dead_code)] // read by the commands that run after connect
pub fn load(home: &Path) -> Result<Option<DesktopConfig>, QuomeError> {
    let path = config_path(home);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    toml::from_str(&text)
        .map(Some)
        .map_err(|e| QuomeError::Usage(format!("{} is not valid: {e}", path.display())))
}

/// Written with mode 0600.
pub fn save(home: &Path, c: &DesktopConfig) -> Result<(), QuomeError> {
    let text = toml::to_string_pretty(c)
        .map_err(|e| QuomeError::Usage(format!("cannot write the desktop config: {e}")))?;
    write_private(&config_path(home), text.as_bytes())
}

/// `"1920x1200"` as `(1920, 1200)`, when it is a size agents may have.
fn parse_screen(s: &str) -> Option<(u16, u16)> {
    let (w, h) = s.split_once('x')?;
    let size = (w.parse().ok()?, h.parse().ok()?);
    super::size::ALLOWED.contains(&size).then_some(size)
}

fn ser_screen<S: Serializer>(v: &Option<(u16, u16)>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some((w, h)) => s.serialize_str(&format!("{w}x{h}")),
        None => s.serialize_none(),
    }
}

fn de_screen<'de, D: Deserializer<'de>>(d: D) -> Result<Option<(u16, u16)>, D::Error> {
    let s = String::deserialize(d)?;
    parse_screen(&s).map(Some).ok_or_else(|| {
        serde::de::Error::custom(format!(
            "screen \"{s}\" is not one of 1366x768, 1440x900, 1920x1080, 1920x1200"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trip_and_mode() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(load(d.path()).unwrap(), None);
        let c = DesktopConfig {
            api: "https://api.example.test".into(),
            org: "acme".into(),
            screen: Some((1920, 1200)),
            panes_shown: false,
            small_frames: false,
        };
        save(d.path(), &c).unwrap();
        let p = d.path().join(".config/quome/desktop.toml");
        assert_eq!(config_path(d.path()), p);
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("screen = \"1920x1200\""), "{text}");
        assert!(text.contains("small_frames = false"), "{text}");
        assert_eq!(load(d.path()).unwrap(), Some(c.clone()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(&p).unwrap().permissions().mode();
            assert_eq!(m & 0o777, 0o600);
        }
        // No size yet: the key is left out and reads back as None.
        let unpicked = DesktopConfig { screen: None, ..c };
        save(d.path(), &unpicked).unwrap();
        assert!(!std::fs::read_to_string(&p).unwrap().contains("screen"));
        assert_eq!(load(d.path()).unwrap(), Some(unpicked));
    }

    #[test]
    fn load_refuses_a_size_agents_cannot_have() {
        let d = tempfile::tempdir().unwrap();
        let p = config_path(d.path());
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        for bad in ["\"1024x768\"", "\"wide\"", "\"1920x\"", "1920"] {
            std::fs::write(
                &p,
                format!("api = \"https://a.test\"\norg = \"acme\"\nscreen = {bad}\n"),
            )
            .unwrap();
            let e = load(d.path()).unwrap_err().to_string();
            assert!(e.contains("desktop.toml"), "{bad}: {e}");
        }
    }
}
