//! `quome models download quexo` through the real binary. The install, checksum, archive and
//! error cases are unit tests in `src/commands/models_download.rs`; these are the end-to-end
//! ones that need the binary or this platform.
use std::process::{Command, Output};

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn quome(home: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_quome"))
        .args(args)
        .env("HOME", home)
        .env("QUOME_NO_AUTO_UPDATE", "1")
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .unwrap()
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
#[test]
fn not_apple_silicon_exits_69() {
    let h = tempfile::tempdir().unwrap();
    let o = quome(h.path(), &["models", "download", "quexo"]);
    assert_eq!(o.status.code(), Some(69));
    assert!(text(&o.stderr).contains("The pool server runs on Apple silicon Macs only."));
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod apple_silicon {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use sha2::{Digest, Sha256};
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const BIN: &[u8] = b"#!/bin/sh\necho pool server\n";

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
                    "access_token": "AT", "token_type": "Bearer", "expires_in": 3600
                })))
                .mount(&server)
                .await;
            let t = {
                let mut b = tar::Builder::new(Vec::new());
                let mut h = tar::Header::new_gnu();
                h.set_size(BIN.len() as u64);
                h.set_mode(0o755);
                h.set_path("quexo").unwrap();
                h.set_cksum();
                b.append(&h, BIN).unwrap();
                let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
                e.write_all(&b.into_inner().unwrap()).unwrap();
                e.finish().unwrap()
            };
            let sum: String = Sha256::digest(&t)
                .iter()
                .map(|x| format!("{x:02x}"))
                .collect();
            Mock::given(method("GET"))
                .and(path("/api/v1/orgs/org-1/models/downloads/quexo"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_bytes(t)
                        .insert_header("X-Quexo-Version", "0.4.2")
                        .insert_header("X-Checksum-Sha256", sum.as_str()),
                )
                .mount(&server)
                .await;
            let home = tempfile::tempdir().unwrap();
            let q = home.path().join(".quome");
            std::fs::create_dir_all(&q).unwrap();
            let profile = serde_json::json!({
                "edition": "cloud", "api_url": server.uri(), "console_url": server.uri(),
                "issuer": server.uri(), "agent_name": "studio", "agent_principal": "p",
                "kid": "k", "jkt": "j", "delegation_id": "d",
                "expires_at": "2099-01-01T00:00:00Z", "org_id": "org-1", "org_slug": "acme",
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

        fn run(&self, extra: &[&str]) -> Output {
            let dir = self.dir();
            let mut args = vec![
                "models",
                "download",
                "quexo",
                "--dir",
                dir.to_str().unwrap(),
            ];
            args.extend_from_slice(extra);
            quome(self.home.path(), &args)
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_binary_installs_and_prints_the_path() {
        let e = Env::new().await;
        let o = e.run(&[]);
        assert!(o.status.success(), "{}", text(&o.stderr));
        assert_eq!(
            text(&o.stdout).trim(),
            e.dir().join("quexo").to_string_lossy()
        );
        let _ = &e.server;
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
    async fn running_binary_started_by_relative_path_is_still_seen() {
        let e = Env::new().await;
        let _r = start_running(&e.dir());
        // Another copy started with a relative path from inside the directory.
        let rel = Command::new("./quexo")
            .arg("120")
            .current_dir(e.dir())
            .spawn()
            .unwrap();
        let _rel = Running(rel);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(e.run(&[]).status.code(), Some(1));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn force_replaces_a_running_binary_and_says_restart() {
        let e = Env::new().await;
        let _r = start_running(&e.dir());
        let o = e.run(&["--force"]);
        assert!(o.status.success(), "{}", text(&o.stderr));
        assert!(text(&o.stdout).contains("Restart the pool server to use the new version."));
        assert_eq!(std::fs::read(e.dir().join("quexo")).unwrap(), BIN);
    }
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
    let help = |args: &[&str]| text(&quome(h.path(), args).stdout);
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
