//! The Quome Desktop app on this computer: download it through the platform, check the
//! archive, its checksum and its signature, and install it as
//! `~/Applications/Quome Desktop.app`. Then ask it about the displays.
//!
//! Every check runs before anything is moved into place: the bundle is unpacked into a
//! staging directory, verified there, and swapped in with renames, so a refusal leaves
//! the app that was installed before as it was.
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use crate::cloud::api::CloudApi;
use crate::cloud::clients::runner::{CommandRunner, RunResult};
use crate::errors::{QuomeError, Result};

/// The bundle directory, in `~/Applications` and at the top of the tarball.
pub const APP_DIR: &str = "Quome Desktop.app";
/// The bundle identifier the app must be signed as.
pub const BUNDLE_ID: &str = "cloud.quome.desktop";
/// The app's executable inside the bundle.
pub const EXECUTABLE: &str = "Contents/MacOS/quome-desktop";
/// The launchd user agent that runs the app.
pub const APP_JOB: &str = "cloud.quome.desktop.app";
/// Set to `1` to let a local (unsigned) build of quome install the app.
pub const DEV_ENV: &str = "QUOME_DESKTOP_DEV";

/// The most the download may be.
const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;
/// The most the bundle may unpack to.
const MAX_UNPACKED: u64 = 1024 * 1024 * 1024;
/// The most entries the archive may hold.
const MAX_ENTRIES: usize = 100_000;
/// What the decompressed tar stream may hold beyond the unpacked files: headers,
/// padding, long names and extended headers.
const STREAM_SLACK: u64 = 256 * 1024 * 1024;
/// A leftover temporary download or old bundle older than this is removed.
const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(3600);

const DOWNLOAD_PREFIX: &str = ".quome-desktop.download.";
const OLD_PREFIX: &str = ".quome-desktop.old.";
const STAGING: &str = ".quome-desktop.new";

pub const MISMATCH: &str = "The download did not match its checksum; nothing was installed.";
pub const NO_CHECKSUM: &str =
    "The platform sent no valid checksum for the download; nothing was installed.";
pub const NO_VERSION: &str =
    "The platform sent no valid version for the download; nothing was installed.";
pub const TOO_BIG: &str =
    "The download is larger than the desktop app can be; nothing was installed.";
pub const BAD_ARCHIVE: &str =
    "The download is not the Quome Desktop app alone; nothing was installed.";
pub const BAD_SIGNATURE: &str =
    "The desktop app's signature did not check out; nothing was installed.";
pub const FOREIGN_TEAM: &str =
    "The desktop app is not signed by the same developer as quome; nothing was installed.";
pub const WRONG_ID: &str =
    "The desktop app is not signed as cloud.quome.desktop; nothing was installed.";
pub const UNSIGNED: &str = "This quome is not a signed release; install the app by hand to test.";
pub const NOT_AVAILABLE: &str = "The platform has no desktop app to download for this computer.";
pub const LATER: &str = "The desktop app is not available to download yet; try again later.";
pub const PROBE_FAILED: &str = "The desktop app could not read this computer's displays.";
/// No main display: the app has nothing to show agents (spike R8).
pub const NO_DISPLAY: &str =
    "No display is attached. Attach a monitor or an HDMI dummy plug, then run this again.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledApp {
    pub path: PathBuf,
    pub version: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub displays: u32,
    /// Width, height and scale of the main display.
    pub main_display: Option<(u32, u32, f64)>,
}

fn refuse(msg: &str) -> QuomeError {
    QuomeError::Usage(msg.into())
}

/// `~/Applications`.
pub fn applications(home: &Path) -> PathBuf {
    home.join("Applications")
}

/// `~/Applications/Quome Desktop.app`.
pub fn app_path(home: &Path) -> PathBuf {
    applications(home).join(APP_DIR)
}

/// What an install depends on beyond the platform and the runner; tests set each part.
#[derive(Debug, Clone)]
pub struct InstallCtx {
    /// `QUOME_DESKTOP_DEV=1`: a quome with no Team ID may install the app.
    pub dev: bool,
    /// This quome, whose Team ID the app must share.
    pub self_exe: PathBuf,
    /// The user whose launchd domain runs the app.
    pub uid: u32,
    pub max_download: u64,
    pub max_unpacked: u64,
}

impl InstallCtx {
    pub fn real(env: &dyn Fn(&str) -> Option<String>) -> Result<InstallCtx> {
        Ok(InstallCtx {
            dev: env(DEV_ENV).as_deref() == Some("1"),
            self_exe: std::env::current_exe()?,
            uid: current_uid(),
            max_download: MAX_DOWNLOAD,
            max_unpacked: MAX_UNPACKED,
        })
    }
}

#[cfg(target_os = "macos")]
fn current_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// Only a Mac runs launchd; `quome desktop` refuses anywhere else before this is used.
#[cfg(not(target_os = "macos"))]
fn current_uid() -> u32 {
    0
}

/// Download, check and install the app into `~/Applications`. `env` is read for
/// `QUOME_DESKTOP_DEV`.
pub async fn install_app(
    api: &CloudApi,
    home: &Path,
    run: &dyn CommandRunner,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<InstalledApp> {
    let ctx = InstallCtx::real(env)?;
    install_app_with(api, home, run, &ctx).await
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

/// The platform's refusal of the download, without its body.
fn download_error(e: QuomeError) -> QuomeError {
    match e {
        QuomeError::NotFound(_) => refuse(NOT_AVAILABLE),
        QuomeError::RetryLater { .. } => refuse(LATER),
        QuomeError::ApiRefused { status: 503, .. } => refuse(LATER),
        QuomeError::ApiRefused { status, reason, .. } => {
            let reason: String = reason
                .chars()
                .filter(|c| c.is_ascii_lowercase() || *c == '_')
                .take(64)
                .collect();
            QuomeError::Usage(
                format!("The platform refused the desktop app download ({status} {reason}).")
                    .trim_end()
                    .replace(" )", ")"),
            )
        }
        e => e,
    }
}

/// Removes the file when dropped.
struct FileGuard(PathBuf);
impl Drop for FileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Removes the directory tree when dropped.
struct DirGuard(PathBuf);
impl Drop for DirGuard {
    fn drop(&mut self) {
        remove_any(&self.0);
    }
}

/// Remove a file, link or directory tree at `p`, whichever it is.
fn remove_any(p: &Path) {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => {
            let _ = std::fs::remove_dir_all(p);
        }
        Ok(_) => {
            let _ = std::fs::remove_file(p);
        }
        Err(_) => {}
    }
}

/// An install interrupted between moving the old bundle aside and moving the new one in
/// leaves no app and an `.old.*` bundle: put the newest such bundle back. The caller
/// restarts the app's job afterwards, as after any install.
fn restore_interrupted(apps: &Path, target: &Path) {
    if std::fs::symlink_metadata(target).is_ok() {
        return;
    }
    let Ok(rd) = std::fs::read_dir(apps) else {
        return;
    };
    let newest = rd
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(OLD_PREFIX))
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| e.path().join(EXECUTABLE).is_file())
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
    if let Some(e) = newest {
        let _ = std::fs::rename(e.path(), target);
    }
}

/// Create `dir` (its parent exists), failing if anything is already there, then make it
/// 0700 whatever the umask.
fn make_new_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new().mode(0o700).create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir(dir)
    }
}

/// Reads at most `cap` bytes from `inner`; past that it fails and sets `tripped`.
struct CappedReader<R> {
    inner: R,
    left: u64,
    tripped: std::rc::Rc<std::cell::Cell<bool>>,
}

impl<R: Read> Read for CappedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n as u64 > self.left {
            self.tripped.set(true);
            return Err(std::io::Error::other("the archive is too large"));
        }
        self.left -= n as u64;
        Ok(n)
    }
}

/// Delete temporary downloads and old bundles an earlier, interrupted run left behind.
fn sweep_stale(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let ours = name.starts_with(DOWNLOAD_PREFIX) || name.starts_with(OLD_PREFIX);
        let stale = ours
            && e.metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > STALE_AFTER);
        if stale {
            remove_any(&e.path());
        }
    }
}

#[cfg(unix)]
fn create_file(path: &Path, mode: u32) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
}

#[cfg(not(unix))]
fn create_file(path: &Path, _mode: u32) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

fn make_dir(dir: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(mode)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
        std::fs::create_dir_all(dir)
    }
}

/// `install_app` with every outside dependency given.
pub async fn install_app_with(
    api: &CloudApi,
    home: &Path,
    run: &dyn CommandRunner,
    ctx: &InstallCtx,
) -> Result<InstalledApp> {
    let apps = applications(home);
    let target = app_path(home);
    restore_interrupted(&apps, &target);

    // A local build cannot vouch for the app's developer: refuse before downloading.
    let self_team = codesign_info(run, &ctx.self_exe).team;
    if self_team.is_none() && !ctx.dev {
        return Err(refuse(UNSIGNED));
    }

    let mut resp = api.desktop_download().await.map_err(download_error)?;
    let expected = resp
        .headers()
        .get("x-checksum-sha256")
        .and_then(|v| v.to_str().ok())
        .map(|c| c.to_ascii_lowercase())
        .filter(|c| is_hex_sha256(c))
        .ok_or_else(|| refuse(NO_CHECKSUM))?;
    let version = resp
        .headers()
        .get("x-quome-desktop-version")
        .and_then(|v| v.to_str().ok())
        .and_then(clean_version)
        .ok_or_else(|| refuse(NO_VERSION))?;
    if resp.content_length().is_some_and(|n| n > ctx.max_download) {
        return Err(refuse(TOO_BIG));
    }

    make_dir(&apps, 0o755)?;
    sweep_stale(&apps);

    // 1. Stream the body to a private file while hashing it.
    let tmp = apps.join(format!("{DOWNLOAD_PREFIX}{}", std::process::id()));
    remove_any(&tmp);
    let tmp_guard = FileGuard(tmp.clone());
    let mut hasher = Sha256::new();
    {
        let mut f = create_file(&tmp, 0o600)?;
        let mut total = 0u64;
        while let Some(chunk) = resp.chunk().await? {
            total += chunk.len() as u64;
            if total > ctx.max_download {
                return Err(refuse(TOO_BIG));
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
        return Err(refuse(MISMATCH));
    }

    // 2. Unpack into a fresh staging directory.
    let staging = apps.join(STAGING);
    remove_any(&staging);
    make_new_private_dir(&staging)?;
    let _staging_guard = DirGuard(staging.clone());
    let max_stream = ctx.max_unpacked.saturating_add(STREAM_SLACK);
    unpack_bundle(&tmp, &staging, ctx.max_unpacked, max_stream)?;
    drop(tmp_guard);
    let new_app = staging.join(APP_DIR);

    // 3. The signature, its developer and its identifier.
    check_signature(run, &new_app, self_team.as_deref())?;

    // 4. Swap it in.
    swap_in(run, &apps, &new_app, &target, ctx.uid)?;
    Ok(InstalledApp {
        path: target,
        version,
        sha256: got,
    })
}

/// An archive path as a relative path inside the bundle directory, or `None` when it
/// is anything else (absolute, `..`, `.`, outside the bundle, not UTF-8).
fn bundle_path(raw: &[u8]) -> Option<PathBuf> {
    let s = std::str::from_utf8(raw).ok()?;
    if s.is_empty() || s.starts_with('/') || s.contains('\0') {
        return None;
    }
    let mut out = PathBuf::new();
    for (i, c) in Path::new(s).components().enumerate() {
        match c {
            Component::Normal(n) if i > 0 || n == APP_DIR => out.push(n),
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// Unpack the bundle in `tarball` into `staging`. Refuses any entry that is not a
/// regular file or a directory inside `Quome Desktop.app/`, and an unpacked size over
/// `max_unpacked`; the decompressed stream as a whole may not pass `max_stream`.
fn unpack_bundle(tarball: &Path, staging: &Path, max_unpacked: u64, max_stream: u64) -> Result<()> {
    let tripped = std::rc::Rc::new(std::cell::Cell::new(false));
    let r = unpack_entries(tarball, staging, max_unpacked, max_stream, tripped.clone());
    if tripped.get() {
        return Err(refuse(TOO_BIG));
    }
    r
}

fn unpack_entries(
    tarball: &Path,
    staging: &Path,
    max_unpacked: u64,
    max_stream: u64,
    tripped: std::rc::Rc<std::cell::Cell<bool>>,
) -> Result<()> {
    let bad = || refuse(BAD_ARCHIVE);
    let gz = flate2::read::GzDecoder::new(std::fs::File::open(tarball)?);
    let mut ar = tar::Archive::new(CappedReader {
        inner: gz,
        left: max_stream,
        tripped,
    });
    let mut total = 0u64;
    let mut count = 0usize;
    for entry in ar.entries().map_err(|_| bad())? {
        let mut entry = entry.map_err(|_| bad())?;
        count += 1;
        if count > MAX_ENTRIES {
            return Err(bad());
        }
        let kind = entry.header().entry_type();
        let is_dir = kind == tar::EntryType::Directory;
        if !is_dir && kind != tar::EntryType::Regular {
            return Err(bad());
        }
        let rel = bundle_path(&entry.path_bytes()).ok_or_else(bad)?;
        let dest = staging.join(&rel);
        if is_dir {
            if entry.header().size().map_err(|_| bad())? != 0 {
                return Err(bad());
            }
            make_dir(&dest, 0o755).map_err(|_| bad())?;
            continue;
        }
        if rel.components().count() < 2 {
            return Err(bad()); // the bundle itself must be a directory
        }
        let size = entry.header().size().map_err(|_| bad())?;
        if total.saturating_add(size) > max_unpacked {
            return Err(refuse(TOO_BIG));
        }
        if let Some(parent) = dest.parent() {
            make_dir(parent, 0o755).map_err(|_| bad())?;
        }
        let mode = if entry.header().mode().unwrap_or(0) & 0o111 != 0 {
            0o755
        } else {
            0o644
        };
        // `create_new`: a second entry for the same path is refused, not merged.
        let mut out = create_file(&dest, mode).map_err(|_| bad())?;
        let n = std::io::copy(&mut (&mut entry).take(size), &mut out).map_err(|_| bad())?;
        total += n;
        if n != size {
            return Err(bad());
        }
        out.flush()?;
    }
    let app = staging.join(APP_DIR);
    let exe_ok = std::fs::symlink_metadata(app.join(EXECUTABLE)).is_ok_and(|m| m.is_file());
    if !app.is_dir() || !exe_ok {
        return Err(bad());
    }
    Ok(())
}

/// What `codesign -dv` says of a signed file.
#[derive(Debug, Default, PartialEq, Eq)]
struct SigningInfo {
    identifier: Option<String>,
    team: Option<String>,
}

/// Read `Identifier=` and `TeamIdentifier=` lines (codesign prints them on stderr).
fn parse_codesign(text: &str) -> SigningInfo {
    let mut info = SigningInfo::default();
    for line in text.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("Identifier=") {
            info.identifier = Some(v.trim().to_string()).filter(|v| !v.is_empty());
        } else if let Some(v) = line.strip_prefix("TeamIdentifier=") {
            let v = v.trim();
            // A Team ID is a short run of letters and digits; anything else (including
            // `not set`) is no Team ID, so it can never reach the requirement below.
            let shaped =
                !v.is_empty() && v.len() <= 20 && v.bytes().all(|b| b.is_ascii_alphanumeric());
            info.team = shaped.then(|| v.to_string());
        }
    }
    info
}

/// The signing details of `path`; empty when it is not signed or codesign cannot run.
fn codesign_info(run: &dyn CommandRunner, path: &Path) -> SigningInfo {
    let args = ["-dv".into(), "--verbose=2".into(), path_arg(path)];
    match run.run("codesign", &args) {
        RunResult::Ran {
            ok: true,
            stdout,
            stderr,
        } => parse_codesign(&format!("{stdout}\n{stderr}")),
        _ => SigningInfo::default(),
    }
}

fn path_arg(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// The designated requirement the app must meet: an Apple-issued certificate chain whose
/// leaf names `team`, and our bundle identifier. `team` has been checked to be letters and
/// digits only.
fn requirement(team: &str) -> String {
    format!(
        "anchor apple generic and identifier \"{BUNDLE_ID}\" and certificate leaf[subject.OU] = \"{team}\""
    )
}

/// The staged app must pass a strict deep verify, carry the bundle identifier, and be
/// signed by quome's own team (any team when `self_team` is `None`, the dev case). With a
/// team, the verify also tests `requirement(team)`: the `TeamIdentifier=` that `-dv`
/// prints is only a field of the signature, which a self-made certificate could fill in.
fn check_signature(run: &dyn CommandRunner, app: &Path, self_team: Option<&str>) -> Result<()> {
    let mut verify: Vec<String> = vec!["--verify".into(), "--strict".into(), "--deep".into()];
    if let Some(team) = self_team {
        verify.push(format!("-R={}", requirement(team)));
    }
    verify.push(path_arg(app));
    if !matches!(
        run.run("codesign", &verify),
        RunResult::Ran { ok: true, .. }
    ) {
        return Err(refuse(BAD_SIGNATURE));
    }
    let info = codesign_info(run, app);
    if let Some(want) = self_team {
        if info.team.as_deref() != Some(want) {
            return Err(refuse(FOREIGN_TEAM));
        }
    }
    if info.identifier.as_deref() != Some(BUNDLE_ID) {
        return Err(refuse(WRONG_ID));
    }
    Ok(())
}

/// Stop the app's job if it is loaded, move the old bundle aside, move the new one in,
/// then remove the old one. A failed move puts the old bundle back.
fn swap_in(
    run: &dyn CommandRunner,
    apps: &Path,
    new_app: &Path,
    target: &Path,
    uid: u32,
) -> Result<()> {
    let job = format!("gui/{uid}/{APP_JOB}");
    if matches!(
        run.run("launchctl", &["print".into(), job.clone()]),
        RunResult::Ran { ok: true, .. }
    ) {
        let _ = run.run("launchctl", &["bootout".into(), job]);
    }
    let old = apps.join(format!("{OLD_PREFIX}{}", std::process::id()));
    remove_any(&old);
    let had_old = std::fs::symlink_metadata(target).is_ok();
    if had_old {
        std::fs::rename(target, &old)?;
    }
    if let Err(e) = std::fs::rename(new_app, target) {
        if had_old {
            let _ = std::fs::rename(&old, target);
        }
        return Err(e.into());
    }
    if had_old {
        remove_any(&old);
    }
    Ok(())
}

#[derive(Deserialize)]
struct RawProbe {
    displays: u32,
    main_display: Option<RawDisplay>,
}

#[derive(Deserialize)]
struct RawDisplay {
    w: u32,
    h: u32,
    scale: f64,
}

/// The app's `probe --json` output.
pub fn parse_probe(text: &str) -> Result<Probe> {
    let raw: RawProbe = serde_json::from_str(text.trim()).map_err(|_| refuse(PROBE_FAILED))?;
    let main_display = match raw.main_display {
        None => None,
        Some(d) if d.w > 0 && d.h > 0 && d.scale.is_finite() && d.scale > 0.0 => {
            Some((d.w, d.h, d.scale))
        }
        Some(_) => return Err(refuse(PROBE_FAILED)),
    };
    Ok(Probe {
        displays: raw.displays,
        main_display,
    })
}

/// Run `<app>/Contents/MacOS/quome-desktop probe --json`.
pub fn probe(app: &Path, run: &dyn CommandRunner) -> Result<Probe> {
    let exe = app.join(EXECUTABLE);
    match run.run(&path_arg(&exe), &["probe".into(), "--json".into()]) {
        RunResult::Ran {
            ok: true, stdout, ..
        } => parse_probe(&stdout),
        _ => Err(refuse(PROBE_FAILED)),
    }
}

/// The main display, or the no-display refusal.
pub fn main_display(p: &Probe) -> Result<(u32, u32, f64)> {
    p.main_display.ok_or_else(|| refuse(NO_DISPLAY))
}

/// Built tarballs and a fake Mac (codesign, launchctl, the app's probe) for tests.
#[cfg(test)]
pub mod fixture {
    use super::*;
    use crate::cloud::clients::runner::fake::Fake;

    pub const TEAM: &str = "ABCDE12345";
    pub const EXE_BYTES: &[u8] = b"#!/bin/sh\necho quome-desktop\n";

    pub fn gz(tar_bytes: Vec<u8>) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(&tar_bytes).unwrap();
        e.finish().unwrap()
    }

    /// A header with `name` written raw, so a name tar's own setter would refuse
    /// (`..`, absolute) can be tested.
    pub fn header(name: &str, kind: tar::EntryType, size: usize, mode: u32) -> tar::Header {
        let mut h = tar::Header::new_ustar();
        h.set_entry_type(kind);
        h.set_size(size as u64);
        h.set_mode(mode);
        let raw = h.as_ustar_mut().unwrap();
        raw.name = [0; 100];
        raw.name[..name.len()].copy_from_slice(name.as_bytes());
        if kind == tar::EntryType::Symlink || kind == tar::EntryType::Link {
            raw.linkname[..7].copy_from_slice(b"/bin/sh");
        }
        h.set_cksum();
        h
    }

    /// One archive entry: name, type, contents, mode.
    pub type Entry<'a> = (&'a str, tar::EntryType, &'a [u8], u32);

    pub fn tarball(entries: &[Entry]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (name, kind, bytes, mode) in entries {
            b.append(&header(name, *kind, bytes.len(), *mode), *bytes)
                .unwrap();
        }
        gz(b.into_inner().unwrap())
    }

    /// The entries of a good bundle.
    pub fn bundle_entries() -> Vec<Entry<'static>> {
        use tar::EntryType::{Directory, Regular};
        vec![
            ("Quome Desktop.app/", Directory, b"", 0o755),
            ("Quome Desktop.app/Contents/", Directory, b"", 0o755),
            (
                "Quome Desktop.app/Contents/Info.plist",
                Regular,
                b"<plist/>",
                0o644,
            ),
            ("Quome Desktop.app/Contents/MacOS/", Directory, b"", 0o755),
            (
                "Quome Desktop.app/Contents/MacOS/quome-desktop",
                Regular,
                EXE_BYTES,
                0o755,
            ),
        ]
    }

    pub fn app_tarball() -> Vec<u8> {
        tarball(&bundle_entries())
    }

    /// The bundle plus `extra`.
    pub fn app_tarball_with(extra: &[Entry]) -> Vec<u8> {
        let mut e = bundle_entries();
        e.extend_from_slice(extra);
        tarball(&e)
    }

    pub fn sha(b: &[u8]) -> String {
        Sha256::digest(b)
            .iter()
            .map(|x| format!("{x:02x}"))
            .collect()
    }

    /// What the fake Mac answers.
    #[derive(Clone)]
    pub struct FakeMac {
        pub computer_name: String,
        /// This quome's Team ID (`None`: a local build).
        pub self_team: Option<String>,
        pub app_team: String,
        pub app_id: String,
        pub verify_ok: bool,
        /// The app's certificate chains to Apple; false: a self-made certificate, which
        /// fails any `-R=anchor apple generic ...` test.
        pub anchor_apple: bool,
        pub job_loaded: bool,
        pub probe: String,
    }

    impl Default for FakeMac {
        fn default() -> Self {
            FakeMac {
                computer_name: String::new(),
                self_team: Some(TEAM.into()),
                app_team: TEAM.into(),
                app_id: BUNDLE_ID.into(),
                verify_ok: true,
                anchor_apple: true,
                job_loaded: false,
                probe: r#"{"displays": 1, "main_display": {"w": 3024, "h": 1890, "scale": 2.0}}"#
                    .into(),
            }
        }
    }

    fn signed(id: &str, team: Option<&str>) -> RunResult {
        RunResult::Ran {
            ok: true,
            stdout: String::new(),
            stderr: format!(
                "Executable=/x\nIdentifier={id}\nFormat=app bundle with Mach-O thin (arm64)\n\
                 Authority=Developer ID Application\nTeamIdentifier={}\n",
                team.unwrap_or("not set")
            ),
        }
    }

    impl FakeMac {
        pub fn runner(&self) -> Fake {
            use crate::cloud::clients::runner::fake::{fail, ok};
            let m = self.clone();
            Fake::new(move |program, args| {
                let last = args.last().map(String::as_str).unwrap_or("");
                match program {
                    "scutil" => ok(&m.computer_name),
                    "id" => ok("501\n"),
                    "open" => ok(""),
                    "codesign" if args.first().map(String::as_str) == Some("--verify") => {
                        let tests_anchor = args.iter().any(|a| a.starts_with("-R=anchor apple"));
                        if m.verify_ok && (m.anchor_apple || !tests_anchor) {
                            ok("")
                        } else {
                            fail("a sealed resource is missing or invalid")
                        }
                    }
                    "codesign" if last.ends_with(APP_DIR) => {
                        signed(&m.app_id, Some(m.app_team.as_str()))
                    }
                    "codesign" => match &m.self_team {
                        Some(t) => signed("quome", Some(t)),
                        None => fail("code object is not signed at all"),
                    },
                    "launchctl" if args.first().map(String::as_str) == Some("print") => {
                        if m.job_loaded {
                            ok("state = running")
                        } else {
                            fail("Could not find service")
                        }
                    }
                    "launchctl" => ok(""),
                    p if p.ends_with("/Contents/MacOS/quome-desktop") => ok(&m.probe),
                    _ => RunResult::NotFound,
                }
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::*;
    use super::*;
    use crate::cloud::auth::TokenSource;
    use crate::cloud::keys::AgentKey;
    use tar::EntryType::{Block, Char, Fifo, Link, Regular, Symlink};
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TOKEN: &str = "AT-SECRET-TOKEN-1234";
    const ROUTE: &str = "/api/v1/orgs/org-1/desktops/downloads/quome-desktop";

    fn profile(uri: &str) -> crate::cloud::state::Profile {
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
            org_id: "org-1".into(),
            org_slug: "acme".into(),
            apps_domain: "x".into(),
        }
    }

    struct Env {
        home: tempfile::TempDir,
        server: MockServer,
        api: CloudApi,
        mac: FakeMac,
        dev: bool,
        max_download: u64,
        max_unpacked: u64,
    }

    impl Env {
        async fn new() -> Env {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/.well-known/openid-configuration"))
                .respond_with(ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({"token_endpoint": format!("{}/oauth/token", server.uri())}),
                ))
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/oauth/token"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "access_token": TOKEN, "token_type": "Bearer", "expires_in": 3600
                })))
                .mount(&server)
                .await;
            let p = profile(&server.uri());
            let key = AgentKey::from_seed([7u8; 32]);
            let api = CloudApi::new(&p, TokenSource::new(&p, key, "vrc".into()));
            Env {
                home: tempfile::tempdir().unwrap(),
                server,
                api,
                mac: FakeMac::default(),
                dev: false,
                max_download: MAX_DOWNLOAD,
                max_unpacked: MAX_UNPACKED,
            }
        }

        async fn serve(&self, body: Vec<u8>, checksum: Option<String>) {
            let mut r = ResponseTemplate::new(200)
                .set_body_bytes(body)
                .insert_header("Content-Type", "application/gzip")
                .insert_header("X-Quome-Desktop-Version", "0.1.0");
            if let Some(c) = checksum {
                r = r.insert_header("X-Checksum-Sha256", c.as_str());
            }
            Mock::given(method("GET"))
                .and(path(ROUTE))
                .and(query_param("os", "macos"))
                .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
                .respond_with(r)
                .mount(&self.server)
                .await;
        }

        async fn serve_good(&self, t: Vec<u8>) {
            let c = sha(&t);
            self.serve(t, Some(c)).await;
        }

        async fn install_with(
            &self,
            run: &crate::cloud::clients::runner::fake::Fake,
        ) -> Result<InstalledApp> {
            let ctx = InstallCtx {
                dev: self.dev,
                self_exe: PathBuf::from("/usr/local/bin/quome"),
                uid: 501,
                max_download: self.max_download,
                max_unpacked: self.max_unpacked,
            };
            install_app_with(&self.api, self.home.path(), run, &ctx).await
        }

        async fn install(&self) -> Result<InstalledApp> {
            self.install_with(&self.mac.runner()).await
        }

        fn apps(&self) -> PathBuf {
            applications(self.home.path())
        }

        /// What is in `~/Applications`, sorted.
        fn leftovers(&self) -> Vec<String> {
            let mut v: Vec<String> = match std::fs::read_dir(self.apps()) {
                Ok(rd) => rd
                    .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect(),
                Err(_) => vec![],
            };
            v.sort();
            v
        }

        fn nothing_installed(&self) {
            assert!(self.leftovers().is_empty(), "{:?}", self.leftovers());
        }

        async fn download_requests(&self) -> usize {
            self.server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .filter(|r| r.url.path() == ROUTE)
                .count()
        }
    }

    async fn refused_with(t: Vec<u8>, want: &str) {
        let e = Env::new().await;
        e.serve_good(t).await;
        let err = e.install().await.unwrap_err();
        assert_eq!(err.to_string(), want);
        e.nothing_installed();
    }

    #[test]
    fn checksum_and_version_shapes() {
        assert!(is_hex_sha256(&"a".repeat(64)));
        assert!(!is_hex_sha256(&"A".repeat(64)));
        assert!(!is_hex_sha256(&"a".repeat(63)));
        assert_eq!(clean_version("0.1.0").as_deref(), Some("0.1.0"));
        assert_eq!(clean_version("0.1.0 rm -rf"), None);
        assert_eq!(clean_version(""), None);
    }

    #[test]
    fn bundle_paths() {
        let ok = |s: &str| bundle_path(s.as_bytes());
        assert_eq!(ok("Quome Desktop.app/"), Some(PathBuf::from(APP_DIR)));
        assert_eq!(
            ok("Quome Desktop.app/Contents/MacOS/quome-desktop"),
            Some(PathBuf::from(APP_DIR).join(EXECUTABLE))
        );
        for bad in [
            "",
            "/Quome Desktop.app/x",
            "./Quome Desktop.app/x",
            "../Quome Desktop.app/x",
            "Quome Desktop.app/../x",
            "Quome Desktop.app/Contents/../../x",
            "Quome Desktop.appx/x",
            "quome desktop.app/x",
            "README",
            "Other.app/Contents",
        ] {
            assert_eq!(ok(bad), None, "{bad}");
        }
        assert_eq!(bundle_path(b"Quome Desktop.app/\xff"), None);
    }

    #[test]
    fn codesign_output_is_read() {
        let i = parse_codesign(
            "Executable=/x\nIdentifier=cloud.quome.desktop\nTeamIdentifier=ABCDE12345\n",
        );
        assert_eq!(i.identifier.as_deref(), Some("cloud.quome.desktop"));
        assert_eq!(i.team.as_deref(), Some("ABCDE12345"));
        let i = parse_codesign("Identifier=quome\nTeamIdentifier=not set\n");
        assert_eq!(i.team, None);
    }

    #[tokio::test]
    async fn installs_bundle_and_reports_version() {
        let e = Env::new().await;
        let t = app_tarball();
        let c = sha(&t);
        e.serve(t, Some(c.clone())).await;
        let run = e.mac.runner();
        let done = e.install_with(&run).await.unwrap();
        let want = e.home.path().join("Applications").join("Quome Desktop.app");
        assert_eq!(done.path, want);
        assert_eq!(done.version, "0.1.0");
        assert_eq!(done.sha256, c);
        let exe = want.join("Contents/MacOS/quome-desktop");
        assert_eq!(std::fs::read(&exe).unwrap(), EXE_BYTES);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&exe), 0o755);
            assert_eq!(mode(&want.join("Contents/Info.plist")), 0o644);
        }
        assert_eq!(e.leftovers(), vec![APP_DIR.to_string()]);
        let staged = e.apps().join(".quome-desktop.new").join(APP_DIR);
        let calls = run.calls();
        assert!(
            calls.contains(&format!(
                "codesign --verify --strict --deep -R=anchor apple generic and identifier \"cloud.quome.desktop\" and certificate leaf[subject.OU] = \"ABCDE12345\" {}",
                staged.display()
            )),
            "{calls:?}"
        );
        assert!(calls.contains(&"codesign -dv --verbose=2 /usr/local/bin/quome".to_string()));
        assert!(calls.contains(&format!("codesign -dv --verbose=2 {}", staged.display())));
        assert!(calls.contains(&"launchctl print gui/501/cloud.quome.desktop.app".to_string()));
        assert!(!calls.iter().any(|c| c.contains("bootout")), "not loaded");
    }

    #[tokio::test]
    async fn a_loaded_app_job_is_stopped_and_the_old_app_replaced() {
        let mut e = Env::new().await;
        e.mac.job_loaded = true;
        let old = app_path(e.home.path());
        std::fs::create_dir_all(old.join("Contents")).unwrap();
        std::fs::write(old.join("Contents/old-marker"), b"old").unwrap();
        e.serve_good(app_tarball()).await;
        let run = e.mac.runner();
        e.install_with(&run).await.unwrap();
        assert!(run
            .calls()
            .contains(&"launchctl bootout gui/501/cloud.quome.desktop.app".to_string()));
        assert!(!old.join("Contents/old-marker").exists());
        assert!(old.join(EXECUTABLE).is_file());
        assert_eq!(e.leftovers(), vec![APP_DIR.to_string()]);
    }

    #[tokio::test]
    async fn checksum_mismatch_installs_nothing() {
        let e = Env::new().await;
        e.serve(app_tarball(), Some("0".repeat(64))).await;
        let err = e.install().await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "The download did not match its checksum; nothing was installed."
        );
        e.nothing_installed();
    }

    #[tokio::test]
    async fn missing_checksum_header_installs_nothing() {
        for c in [None, Some("abc".to_string()), Some("z".repeat(64))] {
            let e = Env::new().await;
            e.serve(app_tarball(), c).await;
            let err = e.install().await.unwrap_err();
            assert_eq!(err.to_string(), NO_CHECKSUM);
            assert!(!e.apps().exists(), "nothing written");
        }
        // An uppercase checksum is the same checksum.
        let e = Env::new().await;
        let t = app_tarball();
        let c = sha(&t).to_uppercase();
        e.serve(t, Some(c)).await;
        e.install().await.unwrap();
    }

    #[tokio::test]
    async fn tar_with_extra_top_level_entry_refused() {
        refused_with(
            app_tarball_with(&[("README", Regular, b"hi", 0o644)]),
            BAD_ARCHIVE,
        )
        .await;
        refused_with(
            app_tarball_with(&[("Other.app/Contents/x", Regular, b"hi", 0o644)]),
            BAD_ARCHIVE,
        )
        .await;
        // The bundle as a file, not a directory.
        refused_with(
            tarball(&[("Quome Desktop.app", Regular, b"x", 0o644)]),
            BAD_ARCHIVE,
        )
        .await;
        // No executable in it.
        refused_with(
            tarball(&[(
                "Quome Desktop.app/Contents/Info.plist",
                Regular,
                b"x",
                0o644,
            )]),
            BAD_ARCHIVE,
        )
        .await;
        // The same file twice.
        refused_with(
            app_tarball_with(&[(
                "Quome Desktop.app/Contents/MacOS/quome-desktop",
                Regular,
                b"evil",
                0o755,
            )]),
            BAD_ARCHIVE,
        )
        .await;
        // Not a gzip tarball at all.
        refused_with(b"not a tarball".to_vec(), BAD_ARCHIVE).await;
    }

    #[tokio::test]
    async fn tar_with_parent_path_refused() {
        for name in [
            "Quome Desktop.app/../evil",
            "Quome Desktop.app/Contents/../../evil",
            "../evil",
            "/tmp/evil",
            "./Quome Desktop.app/Contents/x",
        ] {
            let e = Env::new().await;
            e.serve_good(app_tarball_with(&[(name, Regular, b"evil", 0o644)]))
                .await;
            assert_eq!(e.install().await.unwrap_err().to_string(), BAD_ARCHIVE);
            e.nothing_installed();
            assert!(!e.home.path().join("evil").exists());
        }
    }

    #[tokio::test]
    async fn tar_with_symlink_refused() {
        for kind in [Symlink, Link, Char, Block, Fifo] {
            refused_with(
                app_tarball_with(&[("Quome Desktop.app/Contents/link", kind, b"", 0o644)]),
                BAD_ARCHIVE,
            )
            .await;
        }
    }

    #[tokio::test]
    async fn oversize_download_and_unpack_refused() {
        // By the Content-Length.
        let mut e = Env::new().await;
        e.max_download = 16;
        e.serve_good(app_tarball()).await;
        assert_eq!(e.install().await.unwrap_err().to_string(), TOO_BIG);
        e.nothing_installed();
        // By what it unpacks to.
        let mut e = Env::new().await;
        e.max_unpacked = 8;
        e.serve_good(app_tarball()).await;
        assert_eq!(e.install().await.unwrap_err().to_string(), TOO_BIG);
        e.nothing_installed();
    }

    #[tokio::test]
    async fn a_directory_entry_with_contents_is_refused() {
        refused_with(
            app_tarball_with(&[(
                "Quome Desktop.app/Contents/Resources/",
                tar::EntryType::Directory,
                b"hidden data",
                0o755,
            )]),
            BAD_ARCHIVE,
        )
        .await;
    }

    #[test]
    fn the_decompressed_stream_is_capped() {
        // A long name (GNU long-name data) adds to the stream, not to the files' sizes.
        let long = format!("Quome Desktop.app/Contents/{}x", "d123456789/".repeat(60));
        let tar_of = |with_long: bool| {
            let mut b = tar::Builder::new(Vec::new());
            for (name, kind, bytes, mode) in bundle_entries() {
                b.append(
                    &super::fixture::header(name, kind, bytes.len(), mode),
                    bytes,
                )
                .unwrap();
            }
            if with_long {
                let mut h = tar::Header::new_gnu();
                h.set_size(1);
                h.set_mode(0o644);
                h.set_entry_type(Regular);
                b.append_data(&mut h, &long, &b"x"[..]).unwrap();
            }
            b.into_inner().unwrap()
        };
        let (short, with_long) = (tar_of(false), tar_of(true));
        // Past the good bundle's entries (the last 1024 bytes are the end marker) and the
        // long name's header, partway into the long name itself.
        let cap = (short.len() - 1024 + 512 + 100) as u64;
        let whole = with_long.len() as u64;
        let d = tempfile::tempdir().unwrap();
        let fresh = |n: &str| {
            let s = d.path().join(n);
            std::fs::create_dir(&s).unwrap();
            s
        };
        let f = d.path().join("t.tar.gz");
        std::fs::write(&f, gz(with_long)).unwrap();
        // Under a cap it fits, it unpacks (the long name is a real path).
        unpack_bundle(&f, &fresh("a"), 1 << 20, whole).unwrap();
        assert!(d.path().join("a").join(&long).is_file());
        let err = unpack_bundle(&f, &fresh("b"), 1 << 20, cap).unwrap_err();
        assert_eq!(err.to_string(), TOO_BIG);
    }

    #[test]
    fn the_staging_directory_must_be_new_and_private() {
        let d = tempfile::tempdir().unwrap();
        let s = d.path().join(".quome-desktop.new");
        make_new_private_dir(&s).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&s).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }
        assert!(make_new_private_dir(&s).is_err());
    }

    #[tokio::test]
    async fn an_interrupted_swap_puts_the_old_app_back() {
        let e = Env::new().await;
        // The old bundle was moved aside; the new one never arrived.
        let aside = e.apps().join(".quome-desktop.old.4242");
        std::fs::create_dir_all(aside.join("Contents/MacOS")).unwrap();
        std::fs::write(aside.join(EXECUTABLE), b"old").unwrap();
        e.serve(app_tarball(), Some("0".repeat(64))).await;
        assert_eq!(e.install().await.unwrap_err().to_string(), MISMATCH);
        let target = app_path(e.home.path());
        assert_eq!(std::fs::read(target.join(EXECUTABLE)).unwrap(), b"old");
        assert_eq!(e.leftovers(), vec![APP_DIR.to_string()]);
        // With an app in place, an `.old.*` bundle is left for the sweep.
        let aside = e.apps().join(".quome-desktop.old.4243");
        std::fs::create_dir_all(aside.join("Contents/MacOS")).unwrap();
        std::fs::write(aside.join(EXECUTABLE), b"older").unwrap();
        restore_interrupted(&e.apps(), &target);
        assert_eq!(std::fs::read(target.join(EXECUTABLE)).unwrap(), b"old");
    }

    #[tokio::test]
    async fn foreign_team_id_refused() {
        let mut e = Env::new().await;
        e.mac.app_team = "ZZZZZ99999".into();
        e.serve_good(app_tarball()).await;
        assert_eq!(e.install().await.unwrap_err().to_string(), FOREIGN_TEAM);
        e.nothing_installed();
    }

    #[tokio::test]
    async fn a_failed_strict_verify_is_refused() {
        let mut e = Env::new().await;
        e.mac.verify_ok = false;
        e.serve_good(app_tarball()).await;
        assert_eq!(e.install().await.unwrap_err().to_string(), BAD_SIGNATURE);
        e.nothing_installed();
    }

    #[tokio::test]
    async fn the_verify_tests_apples_anchor_our_identifier_and_our_team() {
        let e = Env::new().await;
        e.serve_good(app_tarball()).await;
        let run = e.mac.runner();
        e.install_with(&run).await.unwrap();
        let verify: Vec<String> = run
            .calls()
            .into_iter()
            .filter(|c| c.starts_with("codesign --verify"))
            .collect();
        assert_eq!(verify.len(), 1);
        // One argument, as codesign gets it (the fake joins arguments with spaces).
        let want = r#"-R=anchor apple generic and identifier "cloud.quome.desktop" and certificate leaf[subject.OU] = "ABCDE12345""#;
        assert!(verify[0].contains(want), "{}", verify[0]);
        assert_eq!(requirement("ABCDE12345"), &want[3..]);
    }

    #[test]
    fn the_requirement_is_one_argument() {
        let mac = FakeMac::default();
        let seen = std::rc::Rc::new(std::cell::RefCell::new(vec![]));
        struct Spy(
            crate::cloud::clients::runner::fake::Fake,
            std::rc::Rc<std::cell::RefCell<Vec<Vec<String>>>>,
        );
        impl CommandRunner for Spy {
            fn run(&self, program: &str, args: &[String]) -> RunResult {
                self.1.borrow_mut().push(args.to_vec());
                self.0.run(program, args)
            }
        }
        let spy = Spy(mac.runner(), seen.clone());
        check_signature(&spy, Path::new("/x/Quome Desktop.app"), Some(TEAM)).unwrap();
        let args = seen.borrow()[0].clone();
        assert_eq!(
            args,
            [
                "--verify".to_string(),
                "--strict".into(),
                "--deep".into(),
                format!("-R={}", requirement(TEAM)),
                "/x/Quome Desktop.app".into()
            ]
        );
    }

    #[tokio::test]
    async fn a_self_made_certificate_claiming_our_team_is_refused() {
        let mut e = Env::new().await;
        e.mac.anchor_apple = false; // TeamIdentifier= still reads as ours
        e.serve_good(app_tarball()).await;
        assert_eq!(e.install().await.unwrap_err().to_string(), BAD_SIGNATURE);
        e.nothing_installed();
    }

    #[test]
    fn a_team_id_that_is_not_letters_and_digits_is_no_team_id() {
        let i = parse_codesign("TeamIdentifier=AB\" or true or \"\n");
        assert_eq!(i.team, None);
    }

    #[tokio::test]
    async fn wrong_bundle_identifier_refused() {
        let mut e = Env::new().await;
        e.mac.app_id = "com.example.desktop".into();
        e.serve_good(app_tarball()).await;
        assert_eq!(e.install().await.unwrap_err().to_string(), WRONG_ID);
        e.nothing_installed();
    }

    #[tokio::test]
    async fn unsigned_quome_refused_without_dev_env() {
        let mut e = Env::new().await;
        e.mac.self_team = None;
        e.serve_good(app_tarball()).await;
        let err = e.install().await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "This quome is not a signed release; install the app by hand to test."
        );
        assert_eq!(e.download_requests().await, 0, "refused before downloading");
        e.nothing_installed();
        // QUOME_DESKTOP_DEV=1: a local build installs an app signed by any team.
        e.dev = true;
        e.mac.app_team = "ZZZZZ99999".into();
        e.install().await.unwrap();
        // ... but still only the right app.
        e.mac.app_id = "com.example.desktop".into();
        assert_eq!(e.install().await.unwrap_err().to_string(), WRONG_ID);
        assert!(
            InstallCtx::real(&|k| (k == DEV_ENV).then(|| "1".into()))
                .unwrap()
                .dev
        );
        assert!(!InstallCtx::real(&|_| None).unwrap().dev);
    }

    #[tokio::test]
    async fn old_app_untouched_on_failure() {
        /// What fails, how the fake is set, the body (default: a good bundle) and the
        /// checksum (default: the body's).
        type Case = (
            &'static str,
            Box<dyn Fn(&mut Env)>,
            Option<Vec<u8>>,
            Option<String>,
        );
        let cases: Vec<Case> = vec![
            ("mismatch", Box::new(|_| {}), None, Some("0".repeat(64))),
            (
                "archive",
                Box::new(|_| {}),
                Some(app_tarball_with(&[("README", Regular, b"x", 0o644)])),
                None,
            ),
            (
                "team",
                Box::new(|e| e.mac.app_team = "ZZZZZ99999".into()),
                None,
                None,
            ),
            (
                "identifier",
                Box::new(|e| e.mac.app_id = "x".into()),
                None,
                None,
            ),
            ("verify", Box::new(|e| e.mac.verify_ok = false), None, None),
            ("unsigned", Box::new(|e| e.mac.self_team = None), None, None),
        ];
        for (what, edit, body, checksum) in cases {
            let mut e = Env::new().await;
            edit(&mut e);
            let t = body.unwrap_or_else(app_tarball);
            let c = checksum.unwrap_or_else(|| sha(&t));
            e.serve(t, Some(c)).await;
            let old = app_path(e.home.path());
            std::fs::create_dir_all(old.join("Contents/MacOS")).unwrap();
            std::fs::write(old.join(EXECUTABLE), b"old").unwrap();
            assert!(e.install().await.is_err(), "{what}");
            assert_eq!(
                std::fs::read(old.join(EXECUTABLE)).unwrap(),
                b"old",
                "{what}"
            );
            assert_eq!(e.leftovers(), vec![APP_DIR.to_string()], "{what}");
        }
    }

    #[tokio::test]
    async fn a_stale_temporary_download_is_swept() {
        let e = Env::new().await;
        std::fs::create_dir_all(e.apps()).unwrap();
        let stale = e.apps().join(".quome-desktop.download.1");
        std::fs::write(&stale, b"x").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
        std::fs::File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(old)
            .unwrap();
        e.serve_good(app_tarball()).await;
        e.install().await.unwrap();
        assert_eq!(e.leftovers(), vec![APP_DIR.to_string()]);
    }

    #[tokio::test]
    async fn the_platforms_refusals_read_plainly() {
        for (status, body, want) in [
            (
                404,
                r#"{"detail":"x","reason":"unsupported_os"}"#,
                NOT_AVAILABLE,
            ),
            (
                503,
                r#"{"detail":"x","reason":"download_unavailable"}"#,
                LATER,
            ),
            (
                403,
                r#"{"detail":"x","reason":"capability_missing"}"#,
                "The platform refused the desktop app download (403 capability_missing).",
            ),
        ] {
            let e = Env::new().await;
            Mock::given(method("GET"))
                .and(path(ROUTE))
                .respond_with(ResponseTemplate::new(status).set_body_string(body))
                .mount(&e.server)
                .await;
            assert_eq!(e.install().await.unwrap_err().to_string(), want);
            e.nothing_installed();
        }
    }

    #[test]
    fn probe_parses_json() {
        let p =
            parse_probe(r#"{"displays": 2, "main_display": {"w": 2560, "h": 1440, "scale": 1.0}}"#)
                .unwrap();
        assert_eq!(
            p,
            Probe {
                displays: 2,
                main_display: Some((2560, 1440, 1.0))
            }
        );
        let p = parse_probe("{\"displays\": 0, \"main_display\": null}\n").unwrap();
        assert_eq!(
            p,
            Probe {
                displays: 0,
                main_display: None
            }
        );
        for bad in [
            "",
            "not json",
            r#"{"main_display": null}"#,
            r#"{"displays": 1, "main_display": {"w": 0, "h": 1, "scale": 1.0}}"#,
            r#"{"displays": 1, "main_display": {"w": 1, "h": 1, "scale": -1.0}}"#,
        ] {
            assert_eq!(
                parse_probe(bad).unwrap_err().to_string(),
                PROBE_FAILED,
                "{bad}"
            );
        }

        let mac = FakeMac::default();
        let run = mac.runner();
        let app = PathBuf::from("/Users/x/Applications/Quome Desktop.app");
        let p = probe(&app, &run).unwrap();
        assert_eq!(p.main_display, Some((3024, 1890, 2.0)));
        assert_eq!(
            run.calls(),
            ["/Users/x/Applications/Quome Desktop.app/Contents/MacOS/quome-desktop probe --json"]
        );
        let failing = crate::cloud::clients::runner::fake::Fake::new(|_, _| {
            crate::cloud::clients::runner::fake::fail("boom")
        });
        assert_eq!(probe(&app, &failing).unwrap_err().to_string(), PROBE_FAILED);
    }

    #[test]
    fn no_display_message() {
        let p = parse_probe(r#"{"displays": 0, "main_display": null}"#).unwrap();
        assert_eq!(
            main_display(&p).unwrap_err().to_string(),
            "No display is attached. Attach a monitor or an HDMI dummy plug, then run this again."
        );
        let p =
            parse_probe(r#"{"displays": 1, "main_display": {"w": 1920, "h": 1080, "scale": 1.0}}"#)
                .unwrap();
        assert_eq!(main_display(&p).unwrap(), (1920, 1080, 1.0));
    }

    /// Every file under `dir`, recursively.
    fn files_under(dir: &Path) -> Vec<PathBuf> {
        let mut out = vec![];
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push(p);
                }
            }
        }
        out
    }

    #[tokio::test]
    async fn token_never_in_output_or_logs() {
        // A refusal whose body echoes the token: the error does not carry the body.
        let e = Env::new().await;
        Mock::given(method("GET"))
            .and(path(ROUTE))
            .respond_with(ResponseTemplate::new(403).set_body_string(format!(
                r#"{{"detail":"bad token {TOKEN}","reason":"capability_missing"}}"#
            )))
            .mount(&e.server)
            .await;
        let err = e.install().await.unwrap_err();
        assert!(!format!("{err} {err:?}").contains(TOKEN), "{err:?}");

        // A good install and each refusal: nothing written or run carries it.
        for checksum_ok in [true, false] {
            let e = Env::new().await;
            let t = app_tarball();
            let c = if checksum_ok { sha(&t) } else { "0".repeat(64) };
            e.serve(t, Some(c)).await;
            let run = e.mac.runner();
            let r = e.install_with(&run).await;
            if let Err(err) = &r {
                assert!(!format!("{err} {err:?}").contains(TOKEN));
            } else {
                assert!(!format!("{:?}", r.unwrap()).contains(TOKEN));
            }
            for c in run.calls() {
                assert!(!c.contains(TOKEN), "{c}");
            }
            for f in files_under(e.home.path()) {
                let bytes = std::fs::read(&f).unwrap();
                let text = String::from_utf8_lossy(&bytes);
                assert!(!text.contains(TOKEN), "{}", f.display());
            }
        }
    }
}
