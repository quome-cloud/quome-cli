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
/// The most the download itself may be.
const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;
/// A leftover temporary download older than this is removed.
const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(3600);
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

fn too_big() -> QuomeError {
    refuse("The download is larger than a pool server file can be; nothing was installed.")
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
struct Cleanup(Option<PathBuf>);
impl Cleanup {
    fn new(p: &Path) -> Cleanup {
        Cleanup(Some(p.to_path_buf()))
    }
    fn keep(mut self) {
        self.0 = None;
    }
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(p) = &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Delete temporary downloads an earlier, interrupted run left behind.
fn sweep_stale(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let stale = e
            .file_name()
            .to_string_lossy()
            .starts_with(".quexo.download.")
            && e.metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > STALE_AFTER);
        if stale {
            let _ = std::fs::remove_file(e.path());
        }
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
    let guard = Cleanup::new(dest);
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
    guard.keep();
    Ok(())
}

/// Whether a running process's executable is `target`, read from the process table.
fn is_running(target: &Path) -> bool {
    let Ok(want) = target.canonicalize() else {
        return false; // no file, nothing can be running from it
    };
    running_paths().contains(&want)
}

/// The absolute, resolved executable path of each process this user can inspect.
#[cfg(target_os = "linux")]
fn running_paths() -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return vec![];
    };
    rd.flatten()
        .filter_map(|e| std::fs::read_link(e.path().join("exe")).ok())
        .map(|exe| {
            let s = exe.to_string_lossy().into_owned();
            PathBuf::from(s.trim_end_matches(" (deleted)"))
        })
        .collect()
}

/// macOS: each pid from `ps`, then the kernel's own path for it (`proc_pidpath`), which is
/// absolute however the process was started. Processes of other users are not visible, so
/// the check covers this user's processes.
#[cfg(target_os = "macos")]
fn running_paths() -> Vec<PathBuf> {
    let Ok(out) = std::process::Command::new("ps")
        .args(["-axo", "pid="])
        .output()
    else {
        return vec![];
    };
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .filter_map(|p| p.parse::<i32>().ok())
        .filter_map(|pid| {
            let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
            // SAFETY: the buffer is valid for the length passed.
            let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
            if n <= 0 {
                return None;
            }
            buf.truncate(n as usize);
            let path = PathBuf::from(String::from_utf8_lossy(&buf).into_owned());
            Some(path.canonicalize().unwrap_or(path))
        })
        .collect()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn running_paths() -> Vec<PathBuf> {
    vec![]
}

/// Download, verify and install into `dir`.
pub async fn install(
    api: &CloudApi,
    dir: &Path,
    force: bool,
    private_dir: bool,
    running: &dyn Fn(&Path) -> bool,
) -> Result<Installed> {
    install_capped(api, dir, force, private_dir, running, MAX_DOWNLOAD).await
}

/// `install` with the download cap given, so a test can use a small one.
async fn install_capped(
    api: &CloudApi,
    dir: &Path,
    force: bool,
    private_dir: bool,
    running: &dyn Fn(&Path) -> bool,
    max_download: u64,
) -> Result<Installed> {
    let target = dir.join(BINARY);
    let running_error = || {
        QuomeError::Usage(format!(
            "The pool server is running from {}. Stop it first, or pass --force.",
            target.display()
        ))
    };
    let was_running = running(&target);
    if was_running && !force {
        return Err(running_error());
    }

    let mut resp = api.models_download_quexo(PLATFORM).await?;
    let expected = resp
        .headers()
        .get("x-checksum-sha256")
        .and_then(|v| v.to_str().ok())
        .map(|c| c.to_ascii_lowercase())
        .filter(|c| is_hex_sha256(c))
        .ok_or_else(|| {
            refuse("The platform sent no valid checksum for the download; nothing was installed.")
        })?;
    let version = resp
        .headers()
        .get("x-quexo-version")
        .and_then(|v| v.to_str().ok())
        .and_then(clean_version);

    if resp.content_length().is_some_and(|n| n > max_download) {
        return Err(too_big());
    }
    make_dir(dir, private_dir)?;
    sweep_stale(dir);
    let tmp = dir.join(format!(".quexo.download.{}", std::process::id()));
    let _tmp_guard = Cleanup::new(&tmp);
    let mut hasher = Sha256::new();
    {
        let mut f = create_private(&tmp)?;
        let mut total = 0u64;
        while let Some(chunk) = resp.chunk().await? {
            total += chunk.len() as u64;
            if total > max_download {
                return Err(too_big());
            }
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
    let _staged_guard = Cleanup::new(&staged);
    let _ = std::fs::remove_file(&staged);
    unpack_single(&tmp, &staged)?;
    let still_running = running(&target);
    if still_running && !force {
        return Err(running_error());
    }
    std::fs::rename(&staged, &target)?;
    Ok(Installed {
        path: target,
        version,
        sha256: got,
        was_running: was_running || still_running,
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
    let mut done = install(&api, &dir, a.force, private_dir, &is_running).await?;
    // Show where it really is, whatever relative or linked `--dir` was given.
    if let Ok(abs) = done.path.canonicalize() {
        done.path = abs;
    }
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
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use std::os::unix::fs::PermissionsExt;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TOKEN: &str = "AT-SECRET-TOKEN-1234";
    const ROUTE: &str = "/api/v1/orgs/org-1/models/downloads/quexo";
    const BIN: &[u8] = b"#!/bin/sh\necho pool server\n";

    fn sha(b: &[u8]) -> String {
        Sha256::digest(b)
            .iter()
            .map(|x| format!("{x:02x}"))
            .collect()
    }

    fn gz(tar_bytes: Vec<u8>) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(&tar_bytes).unwrap();
        e.finish().unwrap()
    }

    fn file_header(name: &str, size: usize) -> tar::Header {
        let mut h = tar::Header::new_gnu();
        h.set_size(size as u64);
        h.set_mode(0o755);
        // Written raw so a name tar's own setter would refuse (`..`) can be tested.
        let raw = h.as_old_mut();
        raw.name = [0; 100];
        raw.name[..name.len()].copy_from_slice(name.as_bytes());
        h.set_cksum();
        h
    }

    fn tarball(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (name, bytes) in files {
            b.append(&file_header(name, bytes.len()), *bytes).unwrap();
        }
        gz(b.into_inner().unwrap())
    }

    fn typed_tarball(kind: tar::EntryType) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(kind);
        h.set_size(0);
        h.set_path("quexo").unwrap();
        if kind == tar::EntryType::Symlink {
            h.set_link_name("/bin/sh").unwrap();
        }
        h.set_cksum();
        b.append(&h, std::io::empty()).unwrap();
        gz(b.into_inner().unwrap())
    }

    fn profile(uri: &str, org: &str) -> crate::cloud::state::Profile {
        crate::cloud::state::Profile {
            edition: "cloud".into(),
            api_url: uri.into(),
            console_url: uri.into(),
            issuer: uri.into(),
            agent_name: "studio".into(),
            agent_principal: "p".into(),
            kid: "k".into(),
            jkt: "j".into(),
            delegation_id: "d".into(),
            expires_at: "2099-01-01T00:00:00Z".into(),
            org_id: org.into(),
            org_slug: "acme".into(),
            apps_domain: "x".into(),
        }
    }

    async fn mount_auth(s: &MockServer, token: &str) {
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"token_endpoint": format!("{}/oauth/token", s.uri())}),
            ))
            .mount(s)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": token, "token_type": "Bearer", "expires_in": 3600
            })))
            .mount(s)
            .await;
    }

    struct Env {
        dir: tempfile::TempDir,
        server: MockServer,
        api: CloudApi,
    }

    impl Env {
        async fn new() -> Env {
            let server = MockServer::start().await;
            mount_auth(&server, TOKEN).await;
            let p = profile(&server.uri(), "org-1");
            let key = AgentKey::from_seed([7u8; 32]);
            let api = CloudApi::new(&p, TokenSource::new(&p, key, "vrc".into()));
            Env {
                dir: tempfile::tempdir().unwrap(),
                server,
                api,
            }
        }

        fn bin_dir(&self) -> PathBuf {
            self.dir.path().join("bin")
        }

        async fn serve(&self, body: Vec<u8>, checksum: Option<String>) {
            let mut r = ResponseTemplate::new(200)
                .set_body_bytes(body)
                .insert_header("X-Quexo-Version", "0.4.2");
            if let Some(c) = checksum {
                r = r.insert_header("X-Checksum-Sha256", c.as_str());
            }
            Mock::given(method("GET"))
                .and(path(ROUTE))
                .and(query_param("platform", "darwin-arm64"))
                .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
                .respond_with(r)
                .mount(&self.server)
                .await;
        }

        async fn serve_ok(&self) {
            let t = tarball(&[("quexo", BIN)]);
            let c = sha(&t);
            self.serve(t, Some(c)).await;
        }

        async fn install(&self, force: bool, running: bool) -> Result<Installed> {
            install(&self.api, &self.bin_dir(), force, true, &move |_| running).await
        }

        fn leftovers(&self) -> Vec<String> {
            match std::fs::read_dir(self.bin_dir()) {
                Ok(rd) => rd
                    .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect(),
                Err(_) => vec![],
            }
        }
    }

    #[test]
    fn checksum_and_version_shapes() {
        assert!(is_hex_sha256(&"a".repeat(64)));
        assert!(!is_hex_sha256(&"A".repeat(64)));
        assert!(!is_hex_sha256("abc"));
        assert_eq!(clean_version("0.4.2").as_deref(), Some("0.4.2"));
        assert_eq!(clean_version("bad value"), None);
    }

    #[tokio::test]
    async fn download_installs_0755_and_returns_the_path() {
        let e = Env::new().await;
        let t = tarball(&[("quexo", BIN)]);
        let c = sha(&t);
        e.serve(t, Some(c.clone())).await;
        let done = e.install(false, false).await.unwrap();
        assert_eq!(done.path, e.bin_dir().join("quexo"));
        assert_eq!(done.version.as_deref(), Some("0.4.2"));
        assert_eq!(done.sha256, c);
        assert_eq!(std::fs::read(&done.path).unwrap(), BIN);
        let mode = std::fs::metadata(&done.path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
        assert_eq!(e.leftovers(), vec!["quexo".to_string()]);
        let dir_mode = std::fs::metadata(e.bin_dir()).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
    }

    #[tokio::test]
    async fn an_uppercase_checksum_header_is_accepted() {
        let e = Env::new().await;
        let t = tarball(&[("quexo", BIN)]);
        let c = sha(&t).to_uppercase();
        e.serve(t, Some(c)).await;
        e.install(false, false).await.unwrap();
    }

    #[tokio::test]
    async fn header_checksum_mismatch_installs_nothing() {
        let e = Env::new().await;
        e.serve(tarball(&[("quexo", BIN)]), Some("0".repeat(64)))
            .await;
        let err = e.install(false, false).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "The download did not match its checksum; nothing was installed."
        );
        assert!(e.leftovers().is_empty(), "{:?}", e.leftovers());
    }

    #[tokio::test]
    async fn missing_or_malformed_checksum_header_installs_nothing() {
        for c in [None, Some("abc".to_string())] {
            let e = Env::new().await;
            e.serve(tarball(&[("quexo", BIN)]), c).await;
            assert!(e.install(false, false).await.is_err());
            assert!(e.leftovers().is_empty(), "{:?}", e.leftovers());
        }
    }

    async fn refused(t: Vec<u8>) {
        let e = Env::new().await;
        let c = sha(&t);
        e.serve(t, Some(c)).await;
        let err = e.install(false, false).await.unwrap_err();
        assert!(err.to_string().contains("nothing was installed"), "{err}");
        assert!(e.leftovers().is_empty(), "{:?}", e.leftovers());
    }

    #[tokio::test]
    async fn tar_with_parent_path_is_refused() {
        refused(tarball(&[("../quexo", BIN)])).await;
    }

    #[tokio::test]
    async fn tar_with_a_nested_or_absolute_name_is_refused() {
        refused(tarball(&[("bin/quexo", BIN)])).await;
        refused(tarball(&[("/quexo", BIN)])).await;
    }

    #[tokio::test]
    async fn tar_with_two_entries_is_refused() {
        refused(tarball(&[("quexo", BIN), ("other", b"x")])).await;
    }

    #[tokio::test]
    async fn tar_with_symlink_or_directory_is_refused() {
        refused(typed_tarball(tar::EntryType::Symlink)).await;
        refused(typed_tarball(tar::EntryType::Directory)).await;
    }

    #[tokio::test]
    async fn an_existing_install_survives_a_refused_download() {
        let e = Env::new().await;
        std::fs::create_dir_all(e.bin_dir()).unwrap();
        std::fs::write(e.bin_dir().join("quexo"), b"old").unwrap();
        // A valid checksum, so the refusal comes from the archive itself.
        let t = tarball(&[("quexo", BIN), ("x", b"y")]);
        let c = sha(&t);
        e.serve(t, Some(c)).await;
        let err = e.install(false, false).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "The download is not a single pool server file; nothing was installed."
        );
        assert_eq!(std::fs::read(e.bin_dir().join("quexo")).unwrap(), b"old");
        assert_eq!(e.leftovers(), vec!["quexo".to_string()]);
    }

    #[tokio::test]
    async fn a_stale_temporary_download_is_swept() {
        let e = Env::new().await;
        std::fs::create_dir_all(e.bin_dir()).unwrap();
        let stale = e.bin_dir().join(".quexo.download.1");
        std::fs::write(&stale, b"x").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
        std::fs::File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(old)
            .unwrap();
        e.serve_ok().await;
        e.install(false, false).await.unwrap();
        assert_eq!(e.leftovers(), vec!["quexo".to_string()]);
    }

    #[tokio::test]
    async fn a_running_binary_is_refused_without_force_and_replaced_with_it() {
        let e = Env::new().await;
        e.serve_ok().await;
        std::fs::create_dir_all(e.bin_dir()).unwrap();
        std::fs::write(e.bin_dir().join("quexo"), b"old").unwrap();
        let err = e.install(false, true).await.unwrap_err();
        let m = err.to_string();
        assert!(m.contains("The pool server is running from"), "{m}");
        assert!(m.contains("Stop it first, or pass --force."), "{m}");
        assert_eq!(std::fs::read(e.bin_dir().join("quexo")).unwrap(), b"old");
        let done = e.install(true, true).await.unwrap();
        assert!(done.was_running);
        assert_eq!(std::fs::read(e.bin_dir().join("quexo")).unwrap(), BIN);
    }

    #[tokio::test]
    async fn a_process_that_starts_during_the_download_is_caught_before_the_rename() {
        let e = Env::new().await;
        e.serve_ok().await;
        std::fs::create_dir_all(e.bin_dir()).unwrap();
        std::fs::write(e.bin_dir().join("quexo"), b"old").unwrap();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let r = install(&e.api, &e.bin_dir(), false, true, &|_| {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0
        })
        .await;
        assert!(r.unwrap_err().to_string().contains("is running from"));
        assert_eq!(std::fs::read(e.bin_dir().join("quexo")).unwrap(), b"old");
        assert_eq!(e.leftovers(), vec!["quexo".to_string()]);
    }

    #[tokio::test]
    async fn a_download_over_the_cap_is_refused_by_its_content_length() {
        let e = Env::new().await;
        let t = tarball(&[("quexo", BIN)]);
        let c = sha(&t);
        assert!(t.len() > 16);
        e.serve(t, Some(c)).await;
        let r = install_capped(&e.api, &e.bin_dir(), false, true, &|_| false, 16).await;
        assert_eq!(r.unwrap_err().to_string(), too_big().to_string());
        assert!(e.leftovers().is_empty(), "{:?}", e.leftovers());
    }

    /// A server that answers the download with a chunked body (no Content-Length), so only
    /// the count of bytes received can stop it. Auth is the mock server's.
    async fn chunked_server(body: Vec<u8>, checksum: String) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                let (body, checksum) = (body.clone(), checksum.clone());
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = s.read(&mut buf).await;
                    let head = format!(
                        "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nx-checksum-sha256: {checksum}\r\nconnection: close\r\n\r\n"
                    );
                    let _ = s.write_all(head.as_bytes()).await;
                    for c in body.chunks(8) {
                        let _ = s.write_all(format!("{:x}\r\n", c.len()).as_bytes()).await;
                        let _ = s.write_all(c).await;
                        let _ = s.write_all(b"\r\n").await;
                    }
                    let _ = s.write_all(b"0\r\n\r\n").await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn a_streamed_download_over_the_cap_is_refused_and_leaves_nothing() {
        let e = Env::new().await;
        let t = tarball(&[("quexo", BIN)]);
        let base = chunked_server(t.clone(), sha(&t)).await;
        let mut p = profile(&e.server.uri(), "org-1");
        p.api_url = base;
        let key = AgentKey::from_seed([7u8; 32]);
        let api = CloudApi::new(&p, TokenSource::new(&p, key, "vrc".into()));
        // Cap below the body: only the streamed count can refuse it.
        let r = install_capped(&api, &e.bin_dir(), false, true, &|_| false, 16).await;
        assert_eq!(r.unwrap_err().to_string(), too_big().to_string());
        assert!(e.leftovers().is_empty(), "{:?}", e.leftovers());
        // And the same body under a generous cap installs, so the server is sound.
        let done = install_capped(&api, &e.bin_dir(), false, true, &|_| false, 1 << 20)
            .await
            .unwrap();
        assert_eq!(std::fs::read(done.path).unwrap(), BIN);
    }

    #[test]
    fn a_missing_file_is_not_running() {
        assert!(!is_running(Path::new("/nonexistent/quexo")));
    }

    /// Run only by `a_spawned_copy_is_seen_in_the_process_table`, as a long-lived child.
    #[test]
    #[ignore = "a helper process for the process-table test"]
    fn sleeper() {
        std::thread::sleep(std::time::Duration::from_secs(30));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_spawned_copy_is_seen_in_the_process_table() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("quexo");
        // Copy this test binary (not /bin/sleep: on some images that is a multi-call binary that
        // exits at once when started under another name) and run only the ignored sleeper test.
        std::fs::copy(std::env::current_exe().unwrap(), &target).unwrap();
        // A just-copied file can still be open for writing: retry on "Text file busy".
        let mut tries = 0;
        let mut child = loop {
            match std::process::Command::new(&target)
                .args([
                    "--ignored",
                    "--exact",
                    "commands::models_download::tests::sleeper",
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(c) => break c,
                Err(e) if e.raw_os_error() == Some(26) && tries < 10 => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(e) => panic!("{e}"),
            }
        };
        std::thread::sleep(std::time::Duration::from_millis(200));
        let seen = is_running(&target);
        let _ = child.kill();
        let _ = child.wait();
        assert!(seen);
        assert!(!is_running(&target));
    }

    async fn refusal(status: u16, body: serde_json::Value, want: &str) {
        let e = Env::new().await;
        Mock::given(method("GET"))
            .and(path(ROUTE))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(&e.server)
            .await;
        let err = e.install(false, false).await.unwrap_err();
        assert_eq!(err.to_string(), want);
        assert!(e.leftovers().is_empty());
    }

    #[tokio::test]
    async fn capability_missing_says_connect_again() {
        for body in [
            serde_json::json!({"detail": "forbidden", "reason": "capability_missing"}),
            serde_json::json!({"error": {"code": "capability_missing"}}),
            serde_json::json!({"error": {"type": "capability_missing"}}),
        ] {
            refusal(403, body, "Run `quome models connect` again.").await;
        }
    }

    #[tokio::test]
    async fn not_enabled_404_message() {
        refusal(
            404,
            serde_json::json!({"detail": "not_found"}),
            "Machines are not enabled for this org.",
        )
        .await;
    }

    #[tokio::test]
    async fn server_refusals_have_plain_messages() {
        refusal(
            502,
            serde_json::json!({"detail": "bad_gateway", "reason": "download_integrity"}),
            "The pool server download failed its integrity check on the server; try again later.",
        )
        .await;
        refusal(
            503,
            serde_json::json!({"detail": "unavailable"}),
            "The pool server download is not available yet.",
        )
        .await;
    }

    #[tokio::test]
    async fn token_never_in_errors() {
        let e = Env::new().await;
        Mock::given(method("GET"))
            .and(path(ROUTE))
            .respond_with(ResponseTemplate::new(500).set_body_string(format!("echo {TOKEN}")))
            .mount(&e.server)
            .await;
        let err = e.install(false, false).await.unwrap_err().to_string();
        assert!(!err.contains(TOKEN), "{err}");
    }

    #[tokio::test]
    async fn the_models_profile_token_is_used_when_present() {
        let e = Env::new().await;
        e.serve_ok().await;
        let home = tempfile::tempdir().unwrap();
        let q = crate::cloud::state::quome_dir(home.path());
        std::fs::create_dir_all(&q).unwrap();
        let mut st = CloudState::default();
        st.profiles
            .insert(PROFILE.into(), profile(&e.server.uri(), "org-1"));
        st.save_to(home.path()).unwrap();
        let key = serde_json::json!({"seed": URL_SAFE_NO_PAD.encode([7u8; 32]), "vrc": "vrc"});
        std::fs::write(q.join("cloud-models.key"), key.to_string()).unwrap();
        let machine = config::key_store(home.path());
        let never: &dyn Fn() -> Result<Box<dyn KeyStore>> =
            &|| Err(QuomeError::Usage("default store must not be read".into()));
        let api = resolve_api(home.path(), &machine, never).unwrap();
        install(&api, &e.bin_dir(), false, true, &|_| false)
            .await
            .unwrap();
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
