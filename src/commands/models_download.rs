//! `quome models download quexo`: fetch the pool server through the platform, check it
//! against the checksum the platform sends, and install it as one executable file.
use clap::{Args, ValueEnum};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::cloud::api::CloudApi;
use crate::cloud::auth::TokenSource;
use crate::cloud::keys::{platform_store, AgentKey, KeyStore};
use crate::cloud::state::CloudState;
use crate::errors::{QuomeError, Result};
use crate::machine::config::{self, PROFILE};

/// The platform name the download route takes.
const PLATFORM: &str = "darwin-arm64";
const BINARY: &str = "quexo";
/// The most the one file may unpack to.
const MAX_UNPACKED: u64 = 4 * 1024 * 1024 * 1024;
/// Exit code when this computer cannot run the pool server (EX_UNAVAILABLE).
const EXIT_UNSUPPORTED: i32 = 69;

#[derive(Clone, Copy, ValueEnum)]
pub enum DownloadWhat {
    /// The pool server that pools several Macs behind one connection
    Quexo,
}

#[derive(Args)]
pub struct DownloadArgs {
    /// What to download
    what: DownloadWhat,
    /// Install into this directory (default: ~/.quome/bin)
    #[arg(long)]
    dir: Option<PathBuf>,
    /// Replace the file even while the pool server is running from it
    #[arg(long)]
    force: bool,
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Debug)]
pub struct Installed {
    pub path: PathBuf,
    pub version: Option<String>,
    pub sha256: String,
    /// The pool server was running from the replaced file.
    pub was_running: bool,
}

fn refuse(msg: &str) -> QuomeError {
    QuomeError::Usage(msg.into())
}

fn mismatch() -> QuomeError {
    refuse("The download did not match its checksum; nothing was installed.")
}

fn bad_archive() -> QuomeError {
    refuse("The download is not a single pool server file; nothing was installed.")
}

/// The machine's `models` profile when present, else the default profile.
fn resolve_api(
    home: &Path,
    machine_store: &dyn KeyStore,
    default_store: &dyn Fn() -> Result<Box<dyn KeyStore>>,
) -> Result<CloudApi> {
    let st = CloudState::load_from(home)?;
    let (name, store): (String, Box<dyn KeyStore + '_>) = if st.profiles.contains_key(PROFILE) {
        // Borrowed store: wrap by reference.
        struct Ref<'a>(&'a dyn KeyStore);
        impl KeyStore for Ref<'_> {
            fn put(&self, p: &str, s: &str) -> Result<()> {
                self.0.put(p, s)
            }
            fn get(&self, p: &str) -> Result<String> {
                self.0.get(p)
            }
            fn delete(&self, p: &str) -> Result<()> {
                self.0.delete(p)
            }
        }
        (PROFILE.into(), Box::new(Ref(machine_store)))
    } else {
        if !st.has_user_profiles() {
            return Err(refuse(crate::commands::cloud::NOT_CONNECTED));
        }
        (st.default.clone(), default_store()?)
    };
    let p = st.profile(Some(&name))?.clone();
    let (key, vrc) = AgentKey::load_from(store.as_ref(), &name)?;
    Ok(CloudApi::new(&p, TokenSource::new(&p, key, vrc)))
}

fn is_hex_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A version string the platform sent, kept only when it is plainly a version.
fn clean_version(s: &str) -> Option<String> {
    let ok = !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+' | b'_'));
    ok.then(|| s.to_string())
}

/// Removes the file when dropped unless kept.
struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(unix)]
fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

fn make_dir(dir: &Path, private: bool) -> Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        return Ok(());
    }
    let _ = private;
    std::fs::create_dir_all(dir)?;
    Ok(())
}

/// Unpack the one file of the tarball at `tarball` to `dest` (mode 0755). Refuses anything
/// but exactly one regular file named `quexo`; `dest` does not remain on a refusal.
fn unpack_single(tarball: &Path, dest: &Path) -> Result<()> {
    let guard = Cleanup(dest.to_path_buf());
    let gz = flate2::read::GzDecoder::new(std::fs::File::open(tarball)?);
    let mut ar = tar::Archive::new(gz);
    let mut seen = 0usize;
    for entry in ar.entries().map_err(|_| bad_archive())? {
        let entry = entry.map_err(|_| bad_archive())?;
        seen += 1;
        if seen > 1 || entry.header().entry_type() != tar::EntryType::Regular {
            return Err(bad_archive());
        }
        let p = entry.path().map_err(|_| bad_archive())?.into_owned();
        let mut parts = p
            .components()
            .filter(|c| *c != std::path::Component::CurDir);
        let named = matches!(parts.next(), Some(std::path::Component::Normal(n)) if n == BINARY);
        if !named || parts.next().is_some() || p.is_absolute() {
            return Err(bad_archive());
        }
        let mut out = create_private(dest)?;
        let n = std::io::copy(&mut entry.take(MAX_UNPACKED + 1), &mut out)
            .map_err(|_| bad_archive())?;
        if n > MAX_UNPACKED {
            return Err(bad_archive());
        }
        out.flush()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    if seen != 1 {
        return Err(bad_archive());
    }
    std::mem::forget(guard);
    Ok(())
}

/// Whether a running process's executable is `target`, read from the process table.
fn is_running(target: &Path) -> bool {
    let Ok(want) = target.canonicalize() else {
        return false; // no file, nothing can be running from it
    };
    #[cfg(target_os = "linux")]
    {
        let Ok(rd) = std::fs::read_dir("/proc") else {
            return false;
        };
        rd.flatten().any(|e| {
            std::fs::read_link(e.path().join("exe"))
                .map(|exe| {
                    let s = exe.to_string_lossy();
                    Path::new(s.trim_end_matches(" (deleted)")) == want
                })
                .unwrap_or(false)
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let Ok(out) = std::process::Command::new("ps")
            .args(["-axo", "comm="])
            .output()
        else {
            return false;
        };
        String::from_utf8_lossy(&out.stdout).lines().any(|l| {
            let p = Path::new(l.trim());
            p == want || p.canonicalize().map(|c| c == want).unwrap_or(false)
        })
    }
}

/// Download, verify and install into `dir`.
pub async fn install(
    api: &CloudApi,
    dir: &Path,
    force: bool,
    private_dir: bool,
) -> Result<Installed> {
    let target = dir.join(BINARY);
    let was_running = is_running(&target);
    if was_running && !force {
        return Err(QuomeError::Usage(format!(
            "The pool server is running from {}. Stop it first, or pass --force.",
            target.display()
        )));
    }

    let mut resp = api.models_download_quexo(PLATFORM).await?;
    let expected = resp
        .headers()
        .get("x-checksum-sha256")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .filter(|c| is_hex_sha256(c))
        .ok_or_else(|| {
            refuse("The platform sent no valid checksum for the download; nothing was installed.")
        })?;
    let version = resp
        .headers()
        .get("x-quexo-version")
        .and_then(|v| v.to_str().ok())
        .and_then(clean_version);

    make_dir(dir, private_dir)?;
    let tmp = dir.join(format!(".quexo.download.{}", std::process::id()));
    let _tmp_guard = Cleanup(tmp.clone());
    let mut hasher = Sha256::new();
    {
        let mut f = create_private(&tmp)?;
        while let Some(chunk) = resp.chunk().await? {
            hasher.update(&chunk);
            f.write_all(&chunk)?;
        }
        f.flush()?;
    }
    let got: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if got != expected {
        return Err(mismatch());
    }

    let staged = dir.join(format!("{BINARY}.new"));
    let _staged_guard = Cleanup(staged.clone());
    let _ = std::fs::remove_file(&staged);
    unpack_single(&tmp, &staged)?;
    std::fs::rename(&staged, &target)?;
    Ok(Installed {
        path: target,
        version,
        sha256: got,
        was_running,
    })
}

fn home() -> Result<PathBuf> {
    dirs::home_dir().ok_or_else(|| QuomeError::CloudState("cannot find home directory".into()))
}

fn apple_silicon() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
}

pub async fn execute(a: DownloadArgs) -> Result<()> {
    let DownloadWhat::Quexo = a.what;
    if !apple_silicon() {
        eprintln!("The pool server runs on Apple silicon Macs only.");
        std::process::exit(EXIT_UNSUPPORTED);
    }
    let home = home()?;
    let (dir, private_dir) = match a.dir {
        Some(d) => (d, false),
        None => (crate::cloud::state::quome_dir(&home).join("bin"), true),
    };
    let machine_store = config::key_store(&home);
    let api = resolve_api(&home, &machine_store, &platform_store)?;
    let done = install(&api, &dir, a.force, private_dir).await?;
    if a.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "path": done.path, "version": done.version, "sha256": done.sha256,
            }))?
        );
        if done.was_running {
            eprintln!("Restart the pool server to use the new version.");
        }
    } else {
        println!("{}", done.path.display());
        if done.was_running {
            println!("Restart the pool server to use the new version.");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::keys::FileStore;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn checksum_and_version_shapes() {
        assert!(is_hex_sha256(&"a".repeat(64)));
        assert!(!is_hex_sha256(&"A".repeat(64)));
        assert!(!is_hex_sha256("abc"));
        assert_eq!(clean_version("0.4.2").as_deref(), Some("0.4.2"));
        assert_eq!(clean_version("bad value"), None);
    }

    #[tokio::test]
    async fn falls_back_to_the_default_profile() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"token_endpoint": format!("{}/oauth/token", s.uri())}),
            ))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "DEFAULT-TOKEN", "token_type": "Bearer", "expires_in": 3600
            })))
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/orgs/org-d/models/downloads/quexo"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"x".to_vec()))
            .mount(&s)
            .await;
        let home = tempfile::tempdir().unwrap();
        let store = FileStore {
            dir: crate::cloud::state::quome_dir(home.path()),
        };
        let p = crate::cloud::state::Profile {
            edition: "cloud".into(),
            api_url: s.uri(),
            console_url: s.uri(),
            issuer: s.uri(),
            agent_name: "a".into(),
            agent_principal: "p".into(),
            kid: "k".into(),
            jkt: "j".into(),
            delegation_id: "d".into(),
            expires_at: "2099-01-01T00:00:00Z".into(),
            org_id: "org-d".into(),
            org_slug: "d".into(),
            apps_domain: "x".into(),
        };
        let mut st = CloudState {
            default: "cloud".into(),
            ..Default::default()
        };
        st.profiles.insert("cloud".into(), p);
        st.save_to(home.path()).unwrap();
        AgentKey::generate()
            .store_in(&store, "cloud", "vrc")
            .unwrap();

        let machine = config::key_store(home.path());
        let default: &dyn Fn() -> Result<Box<dyn KeyStore>> = &|| {
            Ok(Box::new(FileStore {
                dir: crate::cloud::state::quome_dir(home.path()),
            }))
        };
        let api = resolve_api(home.path(), &machine, default).unwrap();
        api.models_download_quexo(PLATFORM).await.unwrap();
        let seen = s.received_requests().await.unwrap();
        let dl = seen
            .iter()
            .find(|r| r.url.path().ends_with("/downloads/quexo"))
            .unwrap();
        assert_eq!(
            dl.headers.get("authorization").unwrap().to_str().unwrap(),
            "Bearer DEFAULT-TOKEN"
        );
    }
}
