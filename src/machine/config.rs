//! This machine's settings: which local server it fronts and how hard to drive it.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::cloud::keys::FileStore;
use crate::cloud::state::{quome_dir, write_private};
use crate::errors::QuomeError;

/// The profile (and key file name) a machine connects under.
pub const PROFILE: &str = "models";
pub const MAX_CONCURRENCY: u32 = 64;
pub const DEFAULT_CONCURRENCY: u32 = 4;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ServerKind {
    #[default]
    #[serde(rename = "ollama")]
    Ollama,
    #[serde(rename = "vllm")]
    Vllm,
    #[serde(rename = "llama.cpp")]
    LlamaCpp,
    #[serde(rename = "mlx")]
    Mlx,
    #[serde(rename = "quexo")]
    Quexo,
}

impl ServerKind {
    pub fn name(self) -> &'static str {
        match self {
            ServerKind::Ollama => "ollama",
            ServerKind::Vllm => "vllm",
            ServerKind::LlamaCpp => "llama.cpp",
            ServerKind::Mlx => "mlx",
            ServerKind::Quexo => "quexo",
        }
    }

    pub fn parse(s: &str) -> Result<ServerKind, String> {
        [
            ServerKind::Ollama,
            ServerKind::Vllm,
            ServerKind::LlamaCpp,
            ServerKind::Mlx,
            ServerKind::Quexo,
        ]
        .into_iter()
        .find(|k| k.name() == s)
        .ok_or_else(|| format!("unknown server \"{s}\"; use ollama, vllm, llama.cpp, mlx or quexo"))
    }

    pub fn default_url(self) -> &'static str {
        match self {
            ServerKind::Ollama => "http://127.0.0.1:11434",
            ServerKind::Vllm => "http://127.0.0.1:8000",
            ServerKind::LlamaCpp | ServerKind::Mlx => "http://127.0.0.1:8080",
            ServerKind::Quexo => "http://127.0.0.1:52415",
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct MachineConfig {
    pub server: ServerKind,
    pub url: String,
    pub max_concurrency: u32,
    /// Read at start; sent to the local server only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_file: Option<PathBuf>,
}

impl Default for MachineConfig {
    fn default() -> Self {
        let server = ServerKind::default();
        MachineConfig {
            server,
            url: server.default_url().into(),
            max_concurrency: DEFAULT_CONCURRENCY,
            api_key_file: None,
        }
    }
}

/// `$XDG_CONFIG_HOME/quome/models.toml`, else `~/.config/quome/models.toml` (on every OS).
pub fn config_path(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    let base = env("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    base.join("quome").join("models.toml")
}

/// A missing file is the defaults.
pub fn load(path: &Path) -> Result<MachineConfig, QuomeError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(MachineConfig::default()),
        Err(e) => return Err(e.into()),
    };
    let c: MachineConfig = toml::from_str(&text)
        .map_err(|e| QuomeError::Usage(format!("{} is not valid: {e}", path.display())))?;
    check(&c)?;
    Ok(c)
}

fn check(c: &MachineConfig) -> Result<(), QuomeError> {
    if !(1..=MAX_CONCURRENCY).contains(&c.max_concurrency) {
        return Err(QuomeError::Usage(format!(
            "max_concurrency must be between 1 and {MAX_CONCURRENCY}"
        )));
    }
    validate_local_url(&c.url)?;
    Ok(())
}

/// Written with mode 0600.
pub fn save(path: &Path, c: &MachineConfig) -> Result<(), QuomeError> {
    check(c)?;
    let text = toml::to_string_pretty(c)
        .map_err(|e| QuomeError::Usage(format!("cannot write the machine config: {e}")))?;
    write_private(path, text.as_bytes())
}

/// The machine key lives in `~/.quome` on every OS, never the Keychain.
pub fn key_store(home: &Path) -> FileStore {
    FileStore {
        dir: quome_dir(home),
    }
}

/// The local server's URL: plain http only to this computer, https anywhere.
pub fn validate_local_url(u: &str) -> Result<url::Url, QuomeError> {
    let parsed = url::Url::parse(u.trim())
        .map_err(|e| QuomeError::Usage(format!("the server URL {u:?} is not a valid URL ({e})")))?;
    match parsed.scheme() {
        "https" => Ok(parsed),
        "http" if crate::url_policy::is_loopback_http(parsed.as_str()) => Ok(parsed),
        "http" => Err(QuomeError::Usage(format!(
            "the server URL {u:?} uses plain http:// to another computer; \
             use https:// or a loopback address"
        ))),
        other => Err(QuomeError::Usage(format!(
            "the server URL {u:?} uses {other}://; use http:// (loopback only) or https://"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_and_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("quome/models.toml");
        let want = MachineConfig {
            server: ServerKind::Ollama,
            url: "http://127.0.0.1:11434".into(),
            max_concurrency: 4,
            api_key_file: None,
        };
        assert_eq!(load(&p).unwrap(), want);
        let c = MachineConfig {
            server: ServerKind::LlamaCpp,
            url: "https://gpu.lan:8000".into(),
            max_concurrency: 9,
            api_key_file: Some("/k/file".into()),
        };
        save(&p, &c).unwrap();
        assert_eq!(load(&p).unwrap(), c);
        assert!(std::fs::read_to_string(&p).unwrap().contains("llama.cpp"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(&p).unwrap().permissions().mode();
            assert_eq!(m & 0o777, 0o600);
        }
        let home = Path::new("/home/u");
        assert_eq!(
            config_path(home, &|k| (k == "XDG_CONFIG_HOME")
                .then(|| "/x".to_string())),
            Path::new("/x/quome/models.toml")
        );
        assert_eq!(
            config_path(home, &|_| None),
            Path::new("/home/u/.config/quome/models.toml")
        );
    }

    #[test]
    fn load_refuses_out_of_range_concurrency() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("models.toml");
        std::fs::write(
            &p,
            "server = \"vllm\"\nurl = \"http://127.0.0.1:8000\"\nmax_concurrency = 65\n",
        )
        .unwrap();
        assert!(load(&p).is_err());
    }

    #[test]
    fn local_url_policy() {
        for ok in [
            "http://127.0.0.1:11434",
            "http://[::1]:8000",
            "https://gpu.lan:8000",
        ] {
            validate_local_url(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
        for bad in ["http://192.168.1.5:8000", "ftp://127.0.0.1/x"] {
            assert!(validate_local_url(bad).is_err(), "{bad}");
        }
    }
}
