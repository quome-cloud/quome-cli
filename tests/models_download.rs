//! `quome models download quexo`, driven through the real binary against a mock platform.
#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "AT-SECRET-TOKEN-1234";
const ORG: &str = "org-1";
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

/// A .tar.gz of the given (name, bytes) regular files.
fn tarball(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    for (name, bytes) in files {
        b.append(&file_header(name, bytes.len()), *bytes).unwrap();
    }
    gz(b.into_inner().unwrap())
}

fn symlink_tarball() -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::Symlink);
    h.set_size(0);
    h.set_path("quexo").unwrap();
    h.set_link_name("/bin/sh").unwrap();
    h.set_cksum();
    b.append(&h, std::io::empty()).unwrap();
    gz(b.into_inner().unwrap())
}

struct Env {
    home: tempfile::TempDir,
    server: MockServer,
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
        let home = tempfile::tempdir().unwrap();
        let q = home.path().join(".quome");
        std::fs::create_dir_all(&q).unwrap();
        let profile = serde_json::json!({
            "edition": "cloud", "api_url": server.uri(), "console_url": server.uri(),
            "issuer": server.uri(), "agent_name": "studio", "agent_principal": "p",
            "kid": "k", "jkt": "j", "delegation_id": "d",
            "expires_at": "2099-01-01T00:00:00Z", "org_id": ORG, "org_slug": "acme",
            "apps_domain": "x",
        });
        std::fs::write(
            q.join("cloud.json"),
            serde_json::json!({"default": "", "profiles": {"models": profile}}).to_string(),
        )
        .unwrap();
        let key = serde_json::json!({"seed": URL_SAFE_NO_PAD.encode([7u8; 32]), "vrc": "vrc"});
        std::fs::write(q.join("cloud-models.key"), key.to_string()).unwrap();
        Env { home, server }
    }

    fn dir(&self) -> PathBuf {
        self.home.path().join("bin")
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

    fn run(&self, extra: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_quome"))
            .args(["models", "download", "quexo", "--dir"])
            .arg(self.dir())
            .args(extra)
            .env("HOME", self.home.path())
            .env("QUOME_NO_AUTO_UPDATE", "1")
            .env_remove("XDG_CONFIG_HOME")
            .output()
            .unwrap()
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn leftovers(dir: &Path) -> Vec<String> {
    match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => vec![],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_installs_0755_and_prints_path() {
    let e = Env::new().await;
    e.serve_ok().await;
    let o = e.run(&[]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let installed = e.dir().join("quexo");
    assert_eq!(text(&o.stdout).trim(), installed.to_string_lossy());
    assert_eq!(std::fs::read(&installed).unwrap(), BIN);
    let mode = std::fs::metadata(&installed).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755);
    assert_eq!(leftovers(&e.dir()), vec!["quexo".to_string()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_json_shape() {
    let e = Env::new().await;
    let t = tarball(&[("quexo", BIN)]);
    let c = sha(&t);
    e.serve(t, Some(c.clone())).await;
    let o = e.run(&["--json"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(
        v,
        serde_json::json!({
            "path": e.dir().join("quexo").to_string_lossy(),
            "version": "0.4.2",
            "sha256": c,
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn header_checksum_mismatch_installs_nothing() {
    let e = Env::new().await;
    let t = tarball(&[("quexo", BIN)]);
    e.serve(t, Some("0".repeat(64))).await;
    let o = e.run(&[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        text(&o.stderr).contains("The download did not match its checksum; nothing was installed.")
    );
    assert!(leftovers(&e.dir()).is_empty(), "{:?}", leftovers(&e.dir()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_checksum_header_installs_nothing() {
    let e = Env::new().await;
    e.serve(tarball(&[("quexo", BIN)]), None).await;
    let o = e.run(&[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(leftovers(&e.dir()).is_empty(), "{:?}", leftovers(&e.dir()));
}

async fn refused_tarball(t: Vec<u8>) {
    let e = Env::new().await;
    let c = sha(&t);
    e.serve(t, Some(c)).await;
    let o = e.run(&[]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o.stderr));
    assert!(text(&o.stderr).contains("nothing was installed"));
    assert!(leftovers(&e.dir()).is_empty(), "{:?}", leftovers(&e.dir()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tar_with_parent_path_is_refused() {
    refused_tarball(tarball(&[("../quexo", BIN)])).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tar_with_two_entries_is_refused() {
    refused_tarball(tarball(&[("quexo", BIN), ("other", b"x")])).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tar_with_symlink_is_refused() {
    refused_tarball(symlink_tarball()).await;
}

/// A copy of a system binary installed as `<dir>/quexo` and left running.
struct Running(std::process::Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_running(dir: &Path) -> Running {
    std::fs::create_dir_all(dir).unwrap();
    let target = dir.join("quexo");
    std::fs::copy("/bin/sleep", &target).unwrap();
    // A copied system binary needs its own ad hoc signature to run.
    let signed = Command::new("codesign")
        .args(["-s", "-", "--force"])
        .arg(&target)
        .output()
        .unwrap();
    assert!(signed.status.success());
    let child = Command::new(&target).arg("120").spawn().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    Running(child)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_binary_refuses_without_force() {
    let e = Env::new().await;
    e.serve_ok().await;
    let _r = start_running(&e.dir());
    let before = std::fs::read(e.dir().join("quexo")).unwrap();
    let o = e.run(&[]);
    assert_eq!(o.status.code(), Some(1));
    let err = text(&o.stderr);
    assert!(err.contains("The pool server is running from"), "{err}");
    assert!(err.contains("Stop it first, or pass --force."), "{err}");
    assert_eq!(std::fs::read(e.dir().join("quexo")).unwrap(), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn force_replaces_a_running_binary_and_says_restart() {
    let e = Env::new().await;
    e.serve_ok().await;
    let _r = start_running(&e.dir());
    let o = e.run(&["--force"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert!(text(&o.stdout).contains("Restart the pool server to use the new version."));
    assert_eq!(std::fs::read(e.dir().join("quexo")).unwrap(), BIN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uses_the_models_profile_token_when_present() {
    // The mock only answers a request carrying the models profile's exchanged token.
    let e = Env::new().await;
    e.serve_ok().await;
    let o = e.run(&[]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let seen = e.server.received_requests().await.unwrap();
    let dl = seen.iter().find(|r| r.url.path() == ROUTE).unwrap();
    assert_eq!(
        dl.headers.get("authorization").unwrap().to_str().unwrap(),
        format!("Bearer {TOKEN}")
    );
}

async fn refusal(status: u16, body: serde_json::Value, want: &str) {
    let e = Env::new().await;
    Mock::given(method("GET"))
        .and(path(ROUTE))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(&e.server)
        .await;
    let o = e.run(&[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains(want), "{}", text(&o.stderr));
    assert!(leftovers(&e.dir()).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn capability_missing_says_connect_again() {
    refusal(
        403,
        serde_json::json!({"detail": "forbidden", "reason": "capability_missing"}),
        "Run `quome models connect` again.",
    )
    .await;
    refusal(
        403,
        serde_json::json!({"error": {"code": "capability_missing"}}),
        "Run `quome models connect` again.",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn not_enabled_404_message() {
    refusal(
        404,
        serde_json::json!({"detail": "not_found"}),
        "Machines are not enabled for this org.",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn token_never_in_output_or_logs() {
    let e = Env::new().await;
    e.serve_ok().await;
    for extra in [&[][..], &["--json"][..]] {
        let o = e.run(extra);
        let all = format!("{}{}", text(&o.stdout), text(&o.stderr));
        assert!(!all.contains(TOKEN), "{all}");
    }
    let e = Env::new().await;
    Mock::given(method("GET"))
        .and(path(ROUTE))
        .respond_with(ResponseTemplate::new(500).set_body_string(format!("echo {TOKEN}")))
        .mount(&e.server)
        .await;
    let o = e.run(&[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(!text(&o.stderr).contains(TOKEN), "a body is never printed");
}

const MODELS_HELP_BEFORE: &str = "Serve this computer's local models to your organization

Usage: quome models <COMMAND>

Commands:
  connect     Pair this computer with an organization to serve its local models
  status      Show this machine's connection, settings and what the platform sees
  disconnect  Revoke this machine and forget its connection
  serve       Serve this machine's models to the organization
  help        Print this message or the help of the given subcommand(s)

Options:
  -h, --help  Print help
";

#[test]
fn existing_models_commands_unchanged() {
    let h = tempfile::tempdir().unwrap();
    let help = |args: &[&str]| {
        let o = Command::new(env!("CARGO_BIN_EXE_quome"))
            .args(args)
            .env("HOME", h.path())
            .env("QUOME_NO_AUTO_UPDATE", "1")
            .output()
            .unwrap();
        text(&o.stdout)
    };
    let now = help(&["models", "--help"]);
    let without: String = now
        .lines()
        .filter(|l| !l.trim_start().starts_with("download "))
        .map(|l| format!("{l}\n"))
        .collect();
    assert_eq!(without, MODELS_HELP_BEFORE);
    assert!(now.contains("download"));
    assert!(help(&["models", "serve", "--help"])
        .contains("--install    Install it as a background service"));
}
