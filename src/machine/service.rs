//! Where the machine's background service is installed.
use std::path::{Path, PathBuf};

use crate::cloud::clients::runner::{CommandRunner, RunResult};
use crate::errors::QuomeError;

pub const LAUNCHD_LABEL: &str = "cloud.quome.models";
pub const SYSTEMD_UNIT: &str = "quome-models.service";

/// `~/Library/LaunchAgents/cloud.quome.models.plist`.
pub fn launchd_plist_path(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist"))
}

/// `~/.config/systemd/user/quome-models.service`.
pub fn systemd_unit_path(home: &Path) -> PathBuf {
    home.join(".config/systemd/user").join(SYSTEMD_UNIT)
}

/// True when this platform's service definition exists.
pub fn installed(home: &Path) -> bool {
    if cfg!(target_os = "macos") {
        launchd_plist_path(home).exists()
    } else {
        systemd_unit_path(home).exists()
    }
}

/// The path the unit should start. A PATH entry (for example a Homebrew `bin`
/// symlink) that resolves to this binary survives package upgrades, so it wins
/// over the versioned real path.
pub fn service_program(exe: &Path, on_path: Option<&Path>) -> PathBuf {
    let canon = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
    if let Some(p) = on_path {
        if std::fs::canonicalize(p).ok().as_deref() == Some(canon.as_path()) {
            return p.to_path_buf();
        }
    }
    canon
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The launchd user agent: restarts only after a failure.
pub fn launchd_plist(program: &Path, log_dir: &Path) -> String {
    let log = xml(&log_dir.join("models.log").to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LAUNCHD_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{program}</string>
    <string>models</string>
    <string>serve</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>10</integer>
  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>QUOME_NO_AUTO_UPDATE</key>
    <string>1</string>
  </dict>
</dict>
</plist>
"#,
        program = xml(&program.to_string_lossy()),
    )
}

/// The systemd user unit: restarts only after a failure.
pub fn systemd_unit(program: &Path) -> String {
    let exe = program.to_string_lossy();
    let exe = if exe.contains(char::is_whitespace) {
        format!("\"{}\"", exe.replace('"', "\\\""))
    } else {
        exe.into_owned()
    };
    format!(
        "[Unit]\nDescription=Quome models machine agent\nAfter=network-online.target\n\n\
         [Service]\nExecStart={exe} models serve\nRestart=on-failure\nRestartSec=10\n\
         Environment=QUOME_NO_AUTO_UPDATE=1\n\n[Install]\nWantedBy=default.target\n"
    )
}

fn fail(what: &str, r: &RunResult) -> QuomeError {
    match r {
        RunResult::NotFound => QuomeError::Usage(format!("{what}: command not found")),
        RunResult::Ran { stderr, .. } => {
            QuomeError::Usage(format!("{what} failed: {}", stderr.trim()))
        }
    }
}

fn call(run: &dyn CommandRunner, program: &str, args: &[&str]) -> RunResult {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    run.run(program, &args)
}

fn must(
    run: &dyn CommandRunner,
    what: &str,
    program: &str,
    args: &[&str],
) -> Result<(), QuomeError> {
    match call(run, program, args) {
        RunResult::Ran { ok: true, .. } => Ok(()),
        r => Err(fail(what, &r)),
    }
}

/// How long to wait for launchd to finish removing a job.
pub struct Waits {
    /// Between `launchctl print` polls after a `bootout`.
    pub poll: std::time::Duration,
    pub max_polls: u32,
    /// Before the one `bootstrap` retry.
    pub retry: std::time::Duration,
}

impl Waits {
    pub const REAL: Waits = Waits {
        poll: std::time::Duration::from_millis(100),
        max_polls: 50,
        retry: std::time::Duration::from_secs(1),
    };
}

pub fn install_macos(
    home: &Path,
    run: &dyn CommandRunner,
    program: &Path,
    uid: u32,
) -> Result<(), QuomeError> {
    install_macos_waiting(home, run, program, uid, &Waits::REAL)
}

pub fn install_macos_waiting(
    home: &Path,
    run: &dyn CommandRunner,
    program: &Path,
    uid: u32,
    waits: &Waits,
) -> Result<(), QuomeError> {
    let plist = launchd_plist_path(home);
    let log_dir = home.join("Library/Logs/quome");
    let domain = format!("gui/{uid}");
    let target = format!("gui/{uid}/{LAUNCHD_LABEL}");
    // A previous copy may or may not be loaded. `bootout` can return before
    // launchd has removed the job, so wait for `print` to stop finding it.
    if let RunResult::Ran { ok: true, .. } = call(run, "launchctl", &["bootout", &target]) {
        for _ in 0..waits.max_polls {
            if !matches!(
                call(run, "launchctl", &["print", &target]),
                RunResult::Ran { ok: false, .. }
            ) {
                std::thread::sleep(waits.poll);
            } else {
                break;
            }
        }
    }
    std::fs::create_dir_all(&log_dir)?;
    std::fs::create_dir_all(plist.parent().expect("plist has a parent"))?;
    std::fs::write(&plist, launchd_plist(program, &log_dir))?;
    let plist_s = plist.to_string_lossy();
    let bootstrap = ["bootstrap", &domain, &plist_s];
    if must(run, "launchctl bootstrap", "launchctl", &bootstrap).is_err() {
        std::thread::sleep(waits.retry);
        must(run, "launchctl bootstrap", "launchctl", &bootstrap)?;
    }
    must(
        run,
        "launchctl kickstart",
        "launchctl",
        &["kickstart", "-k", &target],
    )
}

pub fn uninstall_macos(home: &Path, run: &dyn CommandRunner, uid: u32) -> Result<(), QuomeError> {
    let target = format!("gui/{uid}/{LAUNCHD_LABEL}");
    let _ = call(run, "launchctl", &["bootout", &target]);
    match std::fs::remove_file(launchd_plist_path(home)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

pub fn install_linux(
    home: &Path,
    run: &dyn CommandRunner,
    program: &Path,
) -> Result<(), QuomeError> {
    let unit = systemd_unit_path(home);
    std::fs::create_dir_all(unit.parent().expect("unit has a parent"))?;
    std::fs::write(&unit, systemd_unit(program))?;
    must(
        run,
        "systemctl daemon-reload",
        "systemctl",
        &["--user", "daemon-reload"],
    )?;
    must(
        run,
        "systemctl enable",
        "systemctl",
        &["--user", "enable", "--now", SYSTEMD_UNIT],
    )
}

pub fn uninstall_linux(home: &Path, run: &dyn CommandRunner) -> Result<(), QuomeError> {
    let _ = call(
        run,
        "systemctl",
        &["--user", "disable", "--now", SYSTEMD_UNIT],
    );
    match std::fs::remove_file(systemd_unit_path(home)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    let _ = call(run, "systemctl", &["--user", "daemon-reload"]);
    Ok(())
}

/// Install and start the background service for this platform.
pub fn install(
    home: &Path,
    run: &dyn CommandRunner,
    program: &Path,
    uid: u32,
) -> Result<(), QuomeError> {
    if cfg!(target_os = "macos") {
        install_macos(home, run, program, uid)
    } else {
        install_linux(home, run, program)
    }
}

/// Stop and remove the background service for this platform.
pub fn uninstall(home: &Path, run: &dyn CommandRunner, uid: u32) -> Result<(), QuomeError> {
    if cfg!(target_os = "macos") {
        uninstall_macos(home, run, uid)
    } else {
        uninstall_linux(home, run)
    }
}

/// The user id, from `id -u`.
pub fn current_uid(run: &dyn CommandRunner) -> Result<u32, QuomeError> {
    match call(run, "id", &["-u"]) {
        RunResult::Ran {
            ok: true, stdout, ..
        } => stdout
            .trim()
            .parse()
            .map_err(|_| QuomeError::Usage("could not read the user id".into())),
        r => Err(fail("id -u", &r)),
    }
}

/// The user name for the linger hint, from `id -un`.
pub fn current_user(run: &dyn CommandRunner) -> String {
    match call(run, "id", &["-un"]) {
        RunResult::Ran {
            ok: true, stdout, ..
        } if !stdout.trim().is_empty() => stdout.trim().to_string(),
        _ => "$USER".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::clients::runner::fake::{fail, ok, Fake};

    const PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>cloud.quome.models</string>
  <key>ProgramArguments</key>
  <array>
    <string>/opt/homebrew/bin/quome</string>
    <string>models</string>
    <string>serve</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>10</integer>
  <key>StandardOutPath</key>
  <string>/Users/a/Library/Logs/quome/models.log</string>
  <key>StandardErrorPath</key>
  <string>/Users/a/Library/Logs/quome/models.log</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>QUOME_NO_AUTO_UPDATE</key>
    <string>1</string>
  </dict>
</dict>
</plist>
"#;

    #[test]
    fn plist_golden() {
        let p = launchd_plist(
            Path::new("/opt/homebrew/bin/quome"),
            Path::new("/Users/a/Library/Logs/quome"),
        );
        assert_eq!(p, PLIST);
        let p = launchd_plist(Path::new("/a&b/quome"), Path::new("/l&m"));
        assert!(p.contains("<string>/a&amp;b/quome</string>"), "{p}");
        assert!(p.contains("<string>/l&amp;m/models.log</string>"), "{p}");
    }

    #[test]
    fn unit_golden() {
        assert_eq!(
            systemd_unit(Path::new("/home/a/.cargo/bin/quome")),
            "[Unit]\nDescription=Quome models machine agent\nAfter=network-online.target\n\n\
             [Service]\nExecStart=/home/a/.cargo/bin/quome models serve\nRestart=on-failure\n\
             RestartSec=10\nEnvironment=QUOME_NO_AUTO_UPDATE=1\n\n\
             [Install]\nWantedBy=default.target\n"
        );
        assert!(
            systemd_unit(Path::new("/a b/quome")).contains("ExecStart=\"/a b/quome\" models serve")
        );
    }

    #[test]
    fn install_runs_launchctl_in_order() {
        let h = tempfile::tempdir().unwrap();
        let run = Fake::new(|_, a| {
            if a[0] == "bootout" {
                fail("not loaded")
            } else {
                ok("")
            }
        });
        install_macos(h.path(), &run, Path::new("/opt/homebrew/bin/quome"), 501).unwrap();
        let plist = launchd_plist_path(h.path());
        assert_eq!(
            run.calls(),
            vec![
                "launchctl bootout gui/501/cloud.quome.models".to_string(),
                format!("launchctl bootstrap gui/501 {}", plist.display()),
                "launchctl kickstart -k gui/501/cloud.quome.models".to_string(),
            ]
        );
        assert!(plist.exists());
        assert!(h.path().join("Library/Logs/quome").is_dir());
        if cfg!(target_os = "macos") {
            assert!(installed(h.path()));
        }
        let run = Fake::new(|_, _| ok(""));
        uninstall_macos(h.path(), &run, 501).unwrap();
        assert_eq!(
            run.calls(),
            vec!["launchctl bootout gui/501/cloud.quome.models".to_string()]
        );
        assert!(!plist.exists());
        // Already gone: still fine.
        uninstall_macos(h.path(), &run, 501).unwrap();
    }

    #[test]
    fn install_runs_systemctl_in_order() {
        let h = tempfile::tempdir().unwrap();
        let run = Fake::new(|_, _| ok(""));
        install_linux(h.path(), &run, Path::new("/usr/bin/quome")).unwrap();
        assert_eq!(
            run.calls(),
            vec![
                "systemctl --user daemon-reload".to_string(),
                "systemctl --user enable --now quome-models.service".to_string(),
            ]
        );
        assert!(systemd_unit_path(h.path()).exists());
        let run = Fake::new(|_, _| ok(""));
        uninstall_linux(h.path(), &run).unwrap();
        assert!(!systemd_unit_path(h.path()).exists());
        assert_eq!(
            run.calls()[0],
            "systemctl --user disable --now quome-models.service"
        );
    }

    const ZERO: Waits = Waits {
        poll: std::time::Duration::ZERO,
        max_polls: 5,
        retry: std::time::Duration::ZERO,
    };

    #[test]
    fn install_waits_for_bootout_then_retries_bootstrap_once() {
        let h = tempfile::tempdir().unwrap();
        let prints = std::cell::Cell::new(0);
        let boots = std::cell::Cell::new(0);
        let run = Fake::new(move |_, a| match a[0].as_str() {
            "print" => {
                prints.set(prints.get() + 1);
                if prints.get() < 3 {
                    ok("still loaded")
                } else {
                    fail("not found")
                }
            }
            "bootstrap" => {
                boots.set(boots.get() + 1);
                if boots.get() == 1 {
                    fail("Bootstrap failed: 5: Input/output error")
                } else {
                    ok("")
                }
            }
            _ => ok(""),
        });
        install_macos_waiting(h.path(), &run, Path::new("/x/quome"), 501, &ZERO).unwrap();
        let calls = run.calls();
        let t = "gui/501/cloud.quome.models";
        assert_eq!(calls[0], format!("launchctl bootout {t}"));
        assert_eq!(calls[1..4], vec![format!("launchctl print {t}"); 3][..]);
        assert_eq!(calls.iter().filter(|c| c.contains("bootstrap")).count(), 2);
        assert!(calls.last().unwrap().contains("kickstart"));
    }

    #[test]
    fn a_failed_bootstrap_is_an_error() {
        let h = tempfile::tempdir().unwrap();
        let run = Fake::new(|_, a| {
            if a[0] == "bootstrap" {
                fail("denied")
            } else {
                ok("")
            }
        });
        let e =
            install_macos_waiting(h.path(), &run, Path::new("/x/quome"), 501, &ZERO).unwrap_err();
        assert!(
            e.to_string().contains("launchctl bootstrap failed: denied"),
            "{e}"
        );
    }

    #[test]
    fn service_program_prefers_the_path_symlink() {
        let d = tempfile::tempdir().unwrap();
        let exe = d.path().join("Cellar/quome/0.3.0/bin/quome");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, "x").unwrap();
        let link = d.path().join("bin/quome");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&exe, &link).unwrap();
        #[cfg(unix)]
        assert_eq!(service_program(&exe, Some(&link)), link);
        // A PATH entry that is a different binary is ignored.
        let other = d.path().join("other");
        std::fs::write(&other, "y").unwrap();
        assert_eq!(
            service_program(&exe, Some(&other)),
            std::fs::canonicalize(&exe).unwrap()
        );
        assert_eq!(
            service_program(&exe, None),
            std::fs::canonicalize(&exe).unwrap()
        );
    }

    #[test]
    fn uid_comes_from_id() {
        let run = Fake::new(|_, _| ok("501\n"));
        assert_eq!(current_uid(&run).unwrap(), 501);
        let run = Fake::new(|_, _| fail("nope"));
        assert!(current_uid(&run).is_err());
    }

    #[test]
    fn installed_follows_the_platform_file() {
        let h = tempfile::tempdir().unwrap();
        assert!(!installed(h.path()));
        let p = if cfg!(target_os = "macos") {
            launchd_plist_path(h.path())
        } else {
            systemd_unit_path(h.path())
        };
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "x").unwrap();
        assert!(installed(h.path()));
    }
}
