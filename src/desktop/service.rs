//! The two launchd user agents of a linked desktop: the link (`quome desktop serve`)
//! and the app itself, plus the permission panes the app needs turned on.
use std::path::{Path, PathBuf};

use crate::cloud::clients::runner::CommandRunner;
use crate::desktop::app::{self, BUNDLE_ID, EXECUTABLE};
use crate::errors::Result;
use crate::machine::service::{install_job, uninstall_job, xml, Waits};

pub const SERVE_LABEL: &str = "cloud.quome.desktop.serve";
pub const APP_LABEL: &str = "cloud.quome.desktop.app";

pub const SCREEN_PANE: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";
pub const ACCESSIBILITY_PANE: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
pub const PANES_SAID: &str = "Turn on Quome Desktop in both lists. If macOS asks for an \
     administrator's name and password, an administrator must type it.";

pub fn plist_path(home: &Path, label: &str) -> PathBuf {
    home.join("Library/LaunchAgents")
        .join(format!("{label}.plist"))
}

pub fn log_dir(home: &Path) -> PathBuf {
    home.join("Library/Logs/quome")
}

/// The link's agent: `quome desktop serve`, restarted only after a failure.
pub fn serve_plist(program: &Path, log_dir: &Path) -> String {
    let log = xml(&log_dir.join("desktop.log").to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{SERVE_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{program}</string>
    <string>desktop</string>
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

/// The app's agent, run in the person's graphical session so it can see the screen.
/// Written as one function: the app's executable is run directly, associated with its
/// bundle. Provisional until checked on hardware; another way would change only this text.
pub fn app_plist(app: &Path, log_dir: &Path) -> String {
    let log = xml(&log_dir.join("desktop-app.log").to_string_lossy());
    let exe = xml(&app.join(EXECUTABLE).to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{APP_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
  </array>
  <key>LimitLoadToSessionType</key>
  <string>Aqua</string>
  <key>AssociatedBundleIdentifiers</key>
  <string>{BUNDLE_ID}</string>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#
    )
}

/// Install and start both agents. Safe to run again.
pub fn install_services(
    home: &Path,
    run: &dyn CommandRunner,
    program: &Path,
    uid: u32,
    waits: &Waits,
) -> Result<()> {
    let logs = log_dir(home);
    install_job(
        run,
        SERVE_LABEL,
        &plist_path(home, SERVE_LABEL),
        &serve_plist(program, &logs),
        &logs,
        uid,
        waits,
    )?;
    install_job(
        run,
        APP_LABEL,
        &plist_path(home, APP_LABEL),
        &app_plist(&app::app_path(home), &logs),
        &logs,
        uid,
        waits,
    )?;
    Ok(())
}

/// Stop both agents and remove their plists.
pub fn uninstall_services(home: &Path, run: &dyn CommandRunner, uid: u32) -> Result<()> {
    let mut first_error = None;
    for label in [APP_LABEL, SERVE_LABEL] {
        if let Err(e) = uninstall_job(run, label, &plist_path(home, label), uid) {
            first_error.get_or_insert(e);
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// Open the two permission panes. A failure to open one is not an error: the person
/// can open System Settings themselves.
pub fn open_panes(run: &dyn CommandRunner) {
    for url in [SCREEN_PANE, ACCESSIBILITY_PANE] {
        let _ = run.run("open", &[url.to_string()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::clients::runner::fake::{fail, ok, Fake};

    const ZERO: Waits = Waits {
        poll: std::time::Duration::ZERO,
        max_polls: 5,
        retry: std::time::Duration::ZERO,
    };

    #[test]
    fn serve_plist_exact() {
        let want = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>cloud.quome.desktop.serve</string>
  <key>ProgramArguments</key>
  <array>
    <string>/opt/homebrew/bin/quome</string>
    <string>desktop</string>
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
  <string>/Users/a/Library/Logs/quome/desktop.log</string>
  <key>StandardErrorPath</key>
  <string>/Users/a/Library/Logs/quome/desktop.log</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>QUOME_NO_AUTO_UPDATE</key>
    <string>1</string>
  </dict>
</dict>
</plist>
"#;
        let got = serve_plist(
            Path::new("/opt/homebrew/bin/quome"),
            Path::new("/Users/a/Library/Logs/quome"),
        );
        assert_eq!(got, want);
    }

    #[test]
    fn app_plist_exact() {
        let want = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>cloud.quome.desktop.app</string>
  <key>ProgramArguments</key>
  <array>
    <string>/Users/a/Applications/Quome Desktop.app/Contents/MacOS/quome-desktop</string>
  </array>
  <key>LimitLoadToSessionType</key>
  <string>Aqua</string>
  <key>AssociatedBundleIdentifiers</key>
  <string>cloud.quome.desktop</string>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>StandardOutPath</key>
  <string>/Users/a/Library/Logs/quome/desktop-app.log</string>
  <key>StandardErrorPath</key>
  <string>/Users/a/Library/Logs/quome/desktop-app.log</string>
</dict>
</plist>
"#;
        let got = app_plist(
            Path::new("/Users/a/Applications/Quome Desktop.app"),
            Path::new("/Users/a/Library/Logs/quome"),
        );
        assert_eq!(got, want);
    }

    #[test]
    fn install_is_idempotent() {
        let h = tempfile::tempdir().unwrap();
        let loaded = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = loaded.clone();
        let run = Fake::new(move |_, a| match a[0].as_str() {
            "bootout" if !seen.load(std::sync::atomic::Ordering::SeqCst) => fail("not loaded"),
            "print" => fail("gone"),
            _ => ok(""),
        });
        let exe = Path::new("/opt/homebrew/bin/quome");
        install_services(h.path(), &run, exe, 501, &ZERO).unwrap();
        loaded.store(true, std::sync::atomic::Ordering::SeqCst);
        install_services(h.path(), &run, exe, 501, &ZERO).unwrap();
        let calls = run.calls();
        let serve = plist_path(h.path(), SERVE_LABEL);
        let app = plist_path(h.path(), APP_LABEL);
        let one = |s: &str, p: &Path| {
            vec![
                format!("launchctl bootout gui/501/{s}"),
                format!("launchctl bootstrap gui/501 {}", p.display()),
                format!("launchctl kickstart -k gui/501/{s}"),
            ]
        };
        let mut want = one(SERVE_LABEL, &serve);
        want.extend(one(APP_LABEL, &app));
        assert_eq!(calls[..6], want[..]);
        // Second run: bootout found the job, the wait polled, then bootstrap again.
        assert!(calls[6..].contains(&format!("launchctl bootstrap gui/501 {}", serve.display())));
        assert!(calls[6..].contains(&format!("launchctl bootstrap gui/501 {}", app.display())));
        assert!(serve.exists() && app.exists());
    }

    #[test]
    fn uninstall_removes_both_plists() {
        let h = tempfile::tempdir().unwrap();
        let run = Fake::new(|_, _| ok(""));
        install_services(h.path(), &run, Path::new("/q"), 501, &ZERO).unwrap();
        let run = Fake::new(|_, _| ok(""));
        uninstall_services(h.path(), &run, 501).unwrap();
        assert!(!plist_path(h.path(), SERVE_LABEL).exists());
        assert!(!plist_path(h.path(), APP_LABEL).exists());
        assert_eq!(
            run.calls(),
            vec![
                format!("launchctl bootout gui/501/{APP_LABEL}"),
                format!("launchctl bootout gui/501/{SERVE_LABEL}"),
            ]
        );
        uninstall_services(h.path(), &run, 501).unwrap();
    }

    #[test]
    fn uninstall_tries_both_and_reports_the_first_error() {
        let h = tempfile::tempdir().unwrap();
        let run = Fake::new(|_, _| ok(""));
        install_services(h.path(), &run, Path::new("/q"), 501, &ZERO).unwrap();
        // A directory where the app's plist was: removing it fails (not NotFound).
        let app = plist_path(h.path(), APP_LABEL);
        std::fs::remove_file(&app).unwrap();
        std::fs::create_dir(&app).unwrap();
        std::fs::write(app.join("x"), "").unwrap();
        assert!(uninstall_services(h.path(), &run, 501).is_err());
        assert!(!plist_path(h.path(), SERVE_LABEL).exists());
    }

    #[test]
    fn panes_open_with_open() {
        let run = Fake::new(|_, _| ok(""));
        open_panes(&run);
        assert_eq!(
            run.calls(),
            vec![
                format!("open {SCREEN_PANE}"),
                format!("open {ACCESSIBILITY_PANE}")
            ]
        );
    }
}
