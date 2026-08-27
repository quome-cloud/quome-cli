//! `quome host …` — run a Quome sandbox host on this computer.
//!
//! The heavy lifting (Lima VM lifecycle, provisioning, enrollment, the
//! agent's signed self-update) lives in `quome-host`, a Go binary published
//! per environment by the control plane's signed distribution channel. This
//! command is a thin wrapper: it installs `quome-host` from the control plane
//! you are logged in to, verifies it, and passes your subcommand through.
//!
//! The install mirrors the control plane's `host.sh` one-liner byte for byte
//! in what it trusts: the binary's sha256 is checked against the published
//! `SHA256SUMS`, then the freshly downloaded binary verifies that manifest's
//! detached Ed25519 signature against the control plane's public key
//! (`verify-sums`). That first install is trust-on-first-use over TLS to the
//! control plane; from then on `quome-host` verifies every update against the
//! key compiled into it, which is the control that actually fails closed.
//!
//! No API key is needed or sent: the download route is public and the
//! enrollment code is the credential. Nothing here needs root.

use clap::{Args, Subcommand};
use colored::Colorize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::errors::{QuomeError, Result};
use crate::settings::Settings;
use crate::ui;

/// Path under the control plane where the signed artifacts live.
const DOWNLOADS_PATH: &str = "/api/v1/downloads/sandbox-host";
/// Where `host.sh` installs too, so the two paths never fight over a binary.
const DEFAULT_INSTALL_SUBDIR: &str = ".quome/bin";
const BINARY_NAME: &str = "quome-host";
const USER_AGENT: &str = concat!("quome-cli/", env!("CARGO_PKG_VERSION"));

#[derive(Subcommand)]
pub enum HostCommands {
    /// Start (or create) the host VM and provision it; --enroll joins an org
    Up(UpArgs),
    /// Redeem an enrollment code on an already-running host
    Enroll(EnrollArgs),
    /// VM state plus the local agent's own health answer
    Status(PassthroughArgs),
    /// The host agent's journal
    Logs(LogsArgs),
    /// Re-provision, which reinstalls the verified current agent
    Update(PassthroughArgs),
    /// Stop the VM (the sandboxes on it stop with it)
    Down(PassthroughArgs),
    /// Download and verify quome-host without running it
    Install(InstallArgs),
}

#[derive(Args)]
pub struct UpArgs {
    /// Enrollment code from the Quome dashboard (Sandboxes → Fleet → Add device)
    #[arg(long)]
    enroll: Option<String>,

    /// Provision THIS machine instead of a VM (Linux, root — a dedicated host, not a laptop)
    #[arg(long)]
    native: bool,

    /// Re-download quome-host even if it is already installed
    #[arg(long)]
    refresh: bool,

    /// Any further quome-host flags (--cpus, --memory, --disk, --vm-type, --dry-run)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    extra: Vec<String>,
}

#[derive(Args)]
pub struct EnrollArgs {
    /// Enrollment code from the Quome dashboard
    code: String,

    /// Enroll THIS machine (no VM)
    #[arg(long)]
    native: bool,
}

#[derive(Args)]
pub struct LogsArgs {
    /// Follow the log
    #[arg(short = 'f', long)]
    follow: bool,
}

#[derive(Args)]
pub struct PassthroughArgs {
    /// Any further quome-host flags
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    extra: Vec<String>,
}

#[derive(Args)]
pub struct InstallArgs {
    /// Re-download even if quome-host is already installed
    #[arg(long)]
    refresh: bool,
}

pub async fn execute(command: HostCommands) -> Result<()> {
    let api_url = control_plane_url()?;

    match command {
        HostCommands::Install(args) => {
            let bin = ensure_installed(&api_url, args.refresh).await?;
            println!(
                "{} quome-host is installed at {}",
                "✓".green(),
                bin.display()
            );
            Ok(())
        }
        HostCommands::Up(args) => {
            let bin = ensure_installed(&api_url, args.refresh).await?;
            let mut argv = up_argv(&api_url, args.enroll.as_deref(), args.native);
            argv.extend(args.extra);
            run(&bin, &argv)
        }
        HostCommands::Enroll(args) => {
            let bin = ensure_installed(&api_url, false).await?;
            let mut argv = vec![
                "enroll".to_string(),
                "--control-plane-url".to_string(),
                api_url.clone(),
            ];
            if args.native {
                argv.push("--native".to_string());
            }
            argv.push(args.code);
            run(&bin, &argv)
        }
        HostCommands::Status(args) => {
            let bin = ensure_installed(&api_url, false).await?;
            run(&bin, &with_extra("status", args.extra))
        }
        HostCommands::Logs(args) => {
            let bin = ensure_installed(&api_url, false).await?;
            let mut argv = vec!["logs".to_string()];
            if args.follow {
                argv.push("-f".to_string());
            }
            run(&bin, &argv)
        }
        HostCommands::Update(args) => {
            let bin = ensure_installed(&api_url, false).await?;
            let mut argv = vec![
                "update".to_string(),
                "--control-plane-url".to_string(),
                api_url.clone(),
            ];
            argv.extend(args.extra);
            run(&bin, &argv)
        }
        HostCommands::Down(args) => {
            let bin = ensure_installed(&api_url, false).await?;
            run(&bin, &with_extra("down", args.extra))
        }
    }
}

/// The control plane the CLI is configured for, without a trailing slash so
/// it can be joined to `DOWNLOADS_PATH` and handed to `--control-plane-url`.
fn control_plane_url() -> Result<String> {
    let settings = Settings::load().unwrap_or_default();
    let url = settings.get_api_url();
    let url = url.trim().trim_end_matches('/').to_string();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(QuomeError::ApiError(format!(
            "API URL must be an http(s) URL, got {url:?} (check QUOME_API_URL or ~/.quome/settings.json)"
        )));
    }
    Ok(url)
}

/// `quome-host up` argv. `--control-plane-url` goes FIRST so `up` knows which
/// control plane installed it; a later `--control-plane-url` in the user's
/// extra flags still wins because Go's flag package takes the last occurrence.
fn up_argv(api_url: &str, enroll: Option<&str>, native: bool) -> Vec<String> {
    let mut argv = vec![
        "up".to_string(),
        "--control-plane-url".to_string(),
        api_url.to_string(),
    ];
    if let Some(code) = enroll {
        argv.push("--enroll".to_string());
        argv.push(code.to_string());
    }
    if native {
        argv.push("--native".to_string());
    }
    argv
}

fn with_extra(sub: &str, extra: Vec<String>) -> Vec<String> {
    let mut argv = vec![sub.to_string()];
    argv.extend(extra);
    argv
}

/// Run quome-host with inherited stdio and mirror its exit code. `up` runs an
/// interactive VM bring-up, so this must not capture output.
fn run(bin: &Path, argv: &[String]) -> Result<()> {
    let status = Command::new(bin)
        .args(argv)
        .status()
        .map_err(|e| QuomeError::ApiError(format!("could not run {}: {e}", bin.display())))?;
    if status.success() {
        return Ok(());
    }
    // quome-host uses 2 for "your machine is missing something" (Lima, root)
    // and 1 for "the thing you asked for failed"; keep that distinction.
    std::process::exit(status.code().unwrap_or(1));
}

// ---------------------------------------------------------------------------
// Installer
// ---------------------------------------------------------------------------

/// The installed binary, downloading and verifying it first if it is missing
/// (or `refresh` is set).
async fn ensure_installed(api_url: &str, refresh: bool) -> Result<PathBuf> {
    let dir = install_dir()?;
    let bin = dir.join(BINARY_NAME);
    if bin.is_file() && !refresh {
        return Ok(bin);
    }

    let artifact = artifact_name(std::env::consts::OS, std::env::consts::ARCH)?;
    let version = std::env::var("QUOME_HOST_VERSION").unwrap_or_else(|_| "latest".to_string());
    let dl = Downloader::new(api_url, &version)?;

    let sp = ui::spinner(&format!("Downloading {artifact} ({version})..."));
    let binary = dl.fetch(&artifact).await?;
    let sums = String::from_utf8(dl.fetch("SHA256SUMS").await?)
        .map_err(|_| QuomeError::ApiError("SHA256SUMS is not UTF-8".into()))?;
    sp.finish_and_clear();

    let want = sums_entry(&sums, &artifact).ok_or_else(|| {
        QuomeError::ApiError(format!(
            "{artifact} has no SHA256SUMS entry; refusing to install"
        ))
    })?;
    let got = sha256_hex(&binary);
    if got != want {
        return Err(QuomeError::ApiError(format!(
            "checksum mismatch for {artifact}; refusing to install (want {want}, got {got})"
        )));
    }

    // Stage inside the install dir so the final rename is atomic and never
    // crosses a filesystem boundary.
    std::fs::create_dir_all(&dir)?;
    let staging = dir.join(format!(".{BINARY_NAME}.{}.tmp", std::process::id()));
    let _cleanup = RemoveOnDrop(staging.clone());
    std::fs::create_dir_all(&staging)?;
    let candidate = staging.join(BINARY_NAME);
    std::fs::write(&candidate, &binary)?;
    make_executable(&candidate)?;
    let sums_path = staging.join("SHA256SUMS");
    std::fs::write(&sums_path, &sums)?;

    // sh cannot verify Ed25519 portably and neither should this CLI reinvent
    // it: the binary that will verify its own future updates verifies the
    // manifest it was just checksummed against. Same key, same channel as
    // host.sh — see the module docs for exactly what that does and does not
    // protect against.
    match dl.signing_public_key().await? {
        Some(pubkey) => {
            let sig_path = staging.join("SHA256SUMS.sig");
            std::fs::write(&sig_path, dl.fetch("SHA256SUMS.sig").await?)?;
            verify_sums(&candidate, &sums_path, &sig_path, &pubkey)?;
        }
        None => {
            eprintln!(
                "{} this control plane publishes no signing key; installing on checksum verification alone",
                "warning:".yellow().bold()
            );
        }
    }

    std::fs::rename(&candidate, &bin)?;
    println!(
        "{} installed {} ({}, sha256 {}…)",
        "✓".green(),
        bin.display(),
        version,
        &got[..12]
    );
    Ok(bin)
}

/// `$QUOME_HOST_INSTALL_DIR`, else `~/.quome/bin` — the same rule host.sh uses.
fn install_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("QUOME_HOST_INSTALL_DIR") {
        if !dir.trim().is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    let home = dirs::home_dir().ok_or_else(|| {
        QuomeError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Could not find home directory",
        ))
    })?;
    Ok(home.join(DEFAULT_INSTALL_SUBDIR))
}

/// The artifact name the control plane publishes for this platform.
fn artifact_name(os: &str, arch: &str) -> Result<String> {
    let os = match os {
        "macos" => "darwin",
        "linux" => "linux",
        other => {
            return Err(QuomeError::ApiError(format!(
                "quome host is not supported on {other} (macOS and Linux only)"
            )))
        }
    };
    let arch = match arch {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => {
            return Err(QuomeError::ApiError(format!(
                "quome host is not supported on {other} (arm64 and amd64 only)"
            )))
        }
    };
    Ok(format!("{BINARY_NAME}-{os}-{arch}"))
}

/// The sha256 recorded for `name` in a `sha256sum`-style manifest. Tolerates
/// the `*name` binary-mode marker and ignores everything else.
fn sums_entry(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let sum = parts.next()?;
        let entry = parts.next()?.trim_start_matches('*');
        (entry == name && sum.len() == 64).then(|| sum.to_ascii_lowercase())
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// The control plane publishes its signing public key inside the `host.sh`
/// installer it renders (`PUBKEY="<64 hex>"`, empty when unsigned). Reading it
/// from there keeps this CLI on exactly the same trust path as the one-liner.
fn pubkey_from_host_sh(script: &str) -> Result<Option<String>> {
    let line = script
        .lines()
        .find(|l| l.starts_with("PUBKEY="))
        .ok_or_else(|| QuomeError::ApiError("host.sh has no PUBKEY line".into()))?;
    let value = line["PUBKEY=".len()..]
        .trim()
        .trim_matches('"')
        .to_ascii_lowercase();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() != 64 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(QuomeError::ApiError(
            "host.sh carries a malformed signing key; refusing to install".into(),
        ));
    }
    Ok(Some(value))
}

/// Delegate the Ed25519 check to the just-downloaded binary. A non-zero exit
/// is a hard failure: either the publish is broken or the key is wrong.
fn verify_sums(bin: &Path, sums: &Path, sig: &Path, pubkey: &str) -> Result<()> {
    let out = Command::new(bin)
        .args(["verify-sums", "--sums"])
        .arg(sums)
        .arg("--sig")
        .arg(sig)
        .arg("--pubkey")
        .arg(pubkey)
        .output()
        .map_err(|e| QuomeError::ApiError(format!("could not run verify-sums: {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(QuomeError::ApiError(format!(
        "signature verification failed; refusing to install: {}",
        stderr.trim()
    )))
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Anonymous downloads from the control plane's signed artifact route. This
/// is deliberately NOT `QuomeClient`: the route is public, and the control
/// plane answers with a redirect to a signed bucket URL, so a client carrying
/// the user's API key would forward it off-platform.
struct Downloader {
    http: reqwest::Client,
    base: String,
    version: String,
}

impl Downloader {
    fn new(api_url: &str, version: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(300))
            .build()?;
        Ok(Self {
            http,
            base: format!("{api_url}{DOWNLOADS_PATH}"),
            version: version.to_string(),
        })
    }

    async fn fetch(&self, artifact: &str) -> Result<Vec<u8>> {
        let url = format!("{}/{artifact}?version={}", self.base, self.version);
        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            return Err(QuomeError::ApiError(format!(
                "GET {url} returned {}",
                resp.status()
            )));
        }
        Ok(resp.bytes().await?.to_vec())
    }

    async fn signing_public_key(&self) -> Result<Option<String>> {
        let script = String::from_utf8(self.fetch("host.sh").await?)
            .map_err(|_| QuomeError::ApiError("host.sh is not UTF-8".into()))?;
        pubkey_from_host_sh(&script)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_names_follow_the_control_plane_convention() {
        assert_eq!(
            artifact_name("macos", "aarch64").unwrap(),
            "quome-host-darwin-arm64"
        );
        assert_eq!(
            artifact_name("linux", "x86_64").unwrap(),
            "quome-host-linux-amd64"
        );
        assert!(artifact_name("windows", "x86_64").is_err());
        assert!(artifact_name("linux", "riscv64").is_err());
    }

    #[test]
    fn sums_entry_reads_sha256sum_output_with_or_without_binary_marker() {
        let sums = "\
0000000000000000000000000000000000000000000000000000000000000001  quome-host-linux-amd64
ABCDEF0000000000000000000000000000000000000000000000000000000002 *quome-host-darwin-arm64
not-a-sum quome-host-darwin-amd64
";
        assert_eq!(
            sums_entry(sums, "quome-host-linux-amd64").as_deref(),
            Some("0000000000000000000000000000000000000000000000000000000000000001")
        );
        // Lower-cased and the `*` marker stripped.
        assert_eq!(
            sums_entry(sums, "quome-host-darwin-arm64").as_deref(),
            Some("abcdef0000000000000000000000000000000000000000000000000000000002")
        );
        // A malformed digest never matches.
        assert_eq!(sums_entry(sums, "quome-host-darwin-amd64"), None);
        assert_eq!(sums_entry(sums, "quome-host-windows-amd64"), None);
    }

    #[test]
    fn sha256_hex_matches_a_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn pubkey_is_read_from_the_rendered_host_sh() {
        let key = "ab".repeat(32);
        let script = format!(
            "#!/bin/sh\nset -eu\n\nCP=\"https://cp\"\nPUBKEY=\"{}\"\n",
            key.to_uppercase()
        );
        assert_eq!(
            pubkey_from_host_sh(&script).unwrap().as_deref(),
            Some(key.as_str())
        );
        // Unsigned control plane: empty key, checksum-only install.
        assert_eq!(pubkey_from_host_sh("PUBKEY=\"\"\n").unwrap(), None);
        // Anything else is a broken render, not a downgrade.
        assert!(pubkey_from_host_sh("PUBKEY=\"abc\"\n").is_err());
        assert!(pubkey_from_host_sh("CP=\"https://cp\"\n").is_err());
    }

    #[test]
    fn up_argv_puts_the_control_plane_first_and_the_rest_after() {
        assert_eq!(
            up_argv("https://cp", Some("qh_code"), false),
            vec![
                "up",
                "--control-plane-url",
                "https://cp",
                "--enroll",
                "qh_code"
            ]
        );
        assert_eq!(
            up_argv("https://cp", None, true),
            vec!["up", "--control-plane-url", "https://cp", "--native"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn verify_sums_delegates_to_the_binary_and_fails_closed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("quome-host-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("quome-host");
        // A stand-in that only accepts the exact argv shape the wrapper sends.
        std::fs::write(
            &fake,
            "#!/bin/sh\n[ \"$1\" = verify-sums ] && [ \"$2\" = --sums ] && [ \"$4\" = --sig ] && [ \"$6\" = --pubkey ] && [ \"$7\" = goodkey ] && exit 0\necho bad signature >&2; exit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let sums = dir.join("SHA256SUMS");
        let sig = dir.join("SHA256SUMS.sig");
        std::fs::write(&sums, "").unwrap();
        std::fs::write(&sig, "").unwrap();

        assert!(verify_sums(&fake, &sums, &sig, "goodkey").is_ok());
        let err = verify_sums(&fake, &sums, &sig, "otherkey").unwrap_err();
        assert!(err.to_string().contains("bad signature"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
