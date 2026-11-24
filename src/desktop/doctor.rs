//! `quome desktop doctor`: what on this Mac would stop agents using it, one sentence
//! per problem with its fix. No check needs `sudo`; every check reads only.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]
use serde::Serialize;
use std::path::Path;

use crate::cloud::api::ComputerSelf;
use crate::cloud::clients::runner::{CommandRunner, RunResult};
use crate::desktop::files::AppState;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub id: &'static str,
    pub ok: bool,
    pub say: String,
}

fn check(id: &'static str, ok: bool, say: impl Into<String>) -> Check {
    Check {
        id,
        ok,
        say: say.into(),
    }
}

/// `Some(stdout)` when the command ran and succeeded.
fn out(run: &dyn CommandRunner, program: &str, args: &[&str]) -> Option<String> {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    match run.run(program, &args) {
        RunResult::Ran {
            ok: true, stdout, ..
        } => Some(stdout),
        _ => None,
    }
}

/// `id -Gn`: the account's groups; an administrator is in `admin`.
pub fn is_admin(groups: &str) -> bool {
    groups.split_whitespace().any(|g| g == "admin")
}

/// `fdesetup status`: "FileVault is On." / "FileVault is Off."
pub fn filevault_on(text: &str) -> Option<bool> {
    let t = text.trim();
    if t.starts_with("FileVault is On") {
        Some(true)
    } else if t.starts_with("FileVault is Off") {
        Some(false)
    } else {
        None
    }
}

/// `pmset -g`: the idle-sleep minutes (the `sleep` line; text after the number is ignored).
pub fn sleep_minutes(text: &str) -> Option<u32> {
    text.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        (it.next()? == "sleep").then(|| it.next()?.parse().ok())?
    })
}

/// Provisional (spike R6 has not run): the screen saver's `askForPassword`, "1" meaning
/// a password is asked when the screen locks.
pub fn lock_on(text: &str) -> Option<bool> {
    match text.trim() {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

fn standard_account(run: &dyn CommandRunner) -> Check {
    match out(run, "id", &["-Gn"]) {
        Some(g) if is_admin(&g) => check(
            "standard_account",
            false,
            "This account is an administrator. Use a standard account for agents.",
        ),
        Some(_) => check("standard_account", true, "This is a standard account."),
        None => check(
            "standard_account",
            true,
            "Could not check whether this account is an administrator.",
        ),
    }
}

fn auto_login(run: &dyn CommandRunner) -> Check {
    let me = out(run, "id", &["-un"]).map(|s| s.trim().to_string());
    let set = out(
        run,
        "defaults",
        &[
            "read",
            "/Library/Preferences/com.apple.loginwindow",
            "autoLoginUser",
        ],
    )
    .map(|s| s.trim().to_string());
    match (set, me) {
        (Some(u), Some(m)) if !u.is_empty() && u == m => {
            check("auto_login", true, "This account signs in automatically.")
        }
        _ => check(
            "auto_login",
            false,
            "This account does not sign in automatically. Turn on automatic login for it in \
             System Settings > Users & Groups.",
        ),
    }
}

fn filevault(run: &dyn CommandRunner) -> Check {
    match out(run, "fdesetup", &["status"]).as_deref().and_then(filevault_on) {
        Some(true) => check(
            "filevault",
            true,
            "FileVault is on: after a restart a person must unlock this Mac once before agents can use it.",
        ),
        Some(false) => check("filevault", true, "FileVault is off."),
        None => check("filevault", true, "Could not check FileVault."),
    }
}

fn screen_lock(run: &dyn CommandRunner) -> Check {
    let text = out(
        run,
        "defaults",
        &[
            "-currentHost",
            "read",
            "com.apple.screensaver",
            "askForPassword",
        ],
    );
    match text.as_deref().and_then(lock_on) {
        Some(true) => check(
            "screen_lock",
            false,
            "The screen asks for a password when it locks. Turn that off in System Settings > Lock Screen.",
        ),
        Some(false) => check("screen_lock", true, "The screen does not ask for a password."),
        None => check(
            "screen_lock",
            true,
            "Could not check the screen lock; make sure it is off in System Settings > Lock Screen.",
        ),
    }
}

fn sleep(run: &dyn CommandRunner) -> Check {
    match out(run, "pmset", &["-g"]).as_deref().and_then(sleep_minutes) {
        Some(0) => check("sleep", true, "This Mac does not sleep."),
        Some(_) => check(
            "sleep",
            false,
            "This Mac goes to sleep. Set it never to sleep when the display is off in System Settings > Energy (or Battery > Options).",
        ),
        None => check("sleep", true, "Could not check the sleep setting."),
    }
}

const NOT_REPORTING: &str =
    "The app is not reporting. Run `quome desktop connect` to start it, then run this again.";

fn displays(state: Option<&AppState>) -> Check {
    match state.map(|s| s.displays) {
        None => check("displays", false, NOT_REPORTING),
        Some(0) => check(
            "displays",
            false,
            "No display is attached. Attach a monitor or an HDMI dummy plug.",
        ),
        Some(1) => check("displays", true, "One display is shared."),
        Some(_) => check("displays", true, "Only the main display is shared."),
    }
}

fn permissions(state: Option<&AppState>) -> Check {
    let Some(s) = state else {
        return check("permissions", false, NOT_REPORTING);
    };
    let mut missing = vec![];
    if !s.permissions.screen_recording {
        missing.push("Screen & System Audio Recording");
    }
    if !s.permissions.accessibility {
        missing.push("Accessibility");
    }
    if missing.is_empty() {
        return check("permissions", true, "The app has the permissions it needs.");
    }
    check(
        "permissions",
        false,
        format!(
            "Allow Quome Desktop under Privacy & Security in System Settings: {}.",
            missing.join(" and ")
        ),
    )
}

fn app_check(run: &dyn CommandRunner, app: &Path, state: Option<&AppState>) -> Check {
    if !app.exists() {
        return check(
            "app",
            false,
            "The desktop app is not installed. Run `quome desktop connect`.",
        );
    }
    let path = app.to_string_lossy().into_owned();
    if out(run, "codesign", &["--verify", "--strict", "--deep", &path]).is_none() {
        return check(
            "app",
            false,
            "The desktop app's signature does not check out. Run `quome desktop connect --replace` to install it again.",
        );
    }
    match state.and_then(|s| s.version.as_deref()) {
        Some(v) => check(
            "app",
            true,
            format!("The desktop app is installed and signed (version {v})."),
        ),
        None => check("app", true, "The desktop app is installed and signed."),
    }
}

fn link(me: Option<&ComputerSelf>) -> Check {
    match me {
        None => check(
            "link",
            false,
            "The platform did not answer for this computer. Check the network, or run `quome desktop connect --replace`.",
        ),
        Some(c) if !c.online => check(
            "link",
            false,
            "The platform shows this computer offline. Run `quome desktop status`, and `quome desktop serve --install` if the agents are stopped.",
        ),
        Some(_) => check("link", true, "The platform sees this computer online."),
    }
}

/// Every check, in a fixed order. `app` is the installed bundle's path.
pub fn checks(
    run: &dyn CommandRunner,
    app: &Path,
    state: Option<&AppState>,
    me: Option<&ComputerSelf>,
) -> Vec<Check> {
    vec![
        standard_account(run),
        auto_login(run),
        filevault(run),
        screen_lock(run),
        sleep(run),
        displays(state),
        permissions(state),
        app_check(run, app, state),
        link(me),
    ]
}

/// Text (or JSON) to print and the exit code: 0 when every check passes, else 1.
pub fn render(list: &[Check], json: bool) -> (String, i32) {
    let code = i32::from(list.iter().any(|c| !c.ok));
    if json {
        let s = serde_json::to_string_pretty(list).unwrap_or_else(|_| "[]".into());
        return (s, code);
    }
    let text = list
        .iter()
        .map(|c| {
            format!(
                "{}  {}: {}",
                if c.ok { "ok  " } else { "FAIL" },
                c.id,
                c.say
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    (text, code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::clients::runner::fake::{fail, ok, Fake};
    use crate::desktop::files::Permissions;

    // Outputs recorded read-only on a real Mac, with the user name replaced.
    const GROUPS_ADMIN: &str =
        "staff everyone localaccounts admin _appstore _lpadmin _developer com.apple.access_ssh\n";
    const GROUPS_STANDARD: &str = "staff everyone localaccounts _appstore com.apple.access_ssh\n";
    const NO_AUTOLOGIN: &str =
        "Error: Could not find key 'autoLoginUser' in domain 'com.apple.loginwindow'.";
    const FV_ON: &str = "FileVault is On.\n";
    const PMSET: &str = "System-wide power settings:\nCurrently in use:\n standby              1\n \
        powernap             0\n disksleep            0\n sleep                0 (sleep prevented by powerd, caffeinate)\n \
        displaysleep         0\n womp                 1\n";

    struct Mac {
        groups: &'static str,
        auto: Option<&'static str>,
        fv: &'static str,
        lock: Option<&'static str>,
        pmset: String,
    }

    impl Mac {
        fn good() -> Mac {
            Mac {
                groups: GROUPS_STANDARD,
                auto: Some("agent\n"),
                fv: FV_ON,
                lock: Some("0\n"),
                pmset: PMSET.into(),
            }
        }
        fn runner(self) -> Fake {
            Fake::new(move |program, args| match program {
                "id" if args == ["-Gn"] => ok(self.groups),
                "id" => ok("agent\n"),
                "defaults" if args.contains(&"autoLoginUser".to_string()) => match self.auto {
                    Some(u) => ok(u),
                    None => fail(NO_AUTOLOGIN),
                },
                "defaults" => match self.lock {
                    Some(v) => ok(v),
                    None => fail("Error: Could not find key 'askForPassword'"),
                },
                "fdesetup" => ok(self.fv),
                "pmset" => ok(&self.pmset),
                "codesign" => ok(""),
                _ => fail("unexpected"),
            })
        }
    }

    fn state() -> AppState {
        AppState {
            v: 1,
            version: Some("0.1.0".into()),
            permissions: Permissions {
                screen_recording: true,
                accessibility: true,
            },
            displays: 1,
            ..Default::default()
        }
    }

    fn me(online: bool) -> ComputerSelf {
        serde_json::from_value(serde_json::json!({"id": "c1", "name": "desk", "online": online}))
            .unwrap()
    }

    fn app_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn run_all(mac: Mac, st: Option<&AppState>, m: Option<&ComputerSelf>) -> Vec<Check> {
        let d = app_dir();
        checks(&mac.runner(), d.path(), st, m)
    }

    fn get(list: &[Check], id: &str) -> Check {
        list.iter().find(|c| c.id == id).cloned().unwrap()
    }

    #[test]
    fn standard_account_check() {
        let c = get(&run_all(Mac::good(), None, None), "standard_account");
        assert!(c.ok);
        let mut m = Mac::good();
        m.groups = GROUPS_ADMIN;
        let c = get(&run_all(m, None, None), "standard_account");
        assert!(!c.ok);
        assert_eq!(
            c.say,
            "This account is an administrator. Use a standard account for agents."
        );
    }

    #[test]
    fn auto_login_check() {
        assert!(get(&run_all(Mac::good(), None, None), "auto_login").ok);
        let mut m = Mac::good();
        m.auto = None;
        assert!(!get(&run_all(m, None, None), "auto_login").ok);
        let mut m = Mac::good();
        m.auto = Some("someone-else\n");
        assert!(!get(&run_all(m, None, None), "auto_login").ok);
    }

    #[test]
    fn filevault_is_a_note() {
        let c = get(&run_all(Mac::good(), None, None), "filevault");
        assert!(c.ok);
        assert_eq!(
            c.say,
            "FileVault is on: after a restart a person must unlock this Mac once before agents can use it."
        );
        let mut m = Mac::good();
        m.fv = "FileVault is Off.\n";
        assert!(get(&run_all(m, None, None), "filevault").ok);
    }

    #[test]
    fn screen_lock_check() {
        assert!(get(&run_all(Mac::good(), None, None), "screen_lock").ok);
        let mut m = Mac::good();
        m.lock = Some("1\n");
        assert!(!get(&run_all(m, None, None), "screen_lock").ok);
        // The key is missing on a Mac that never set it: a note, not a failure.
        let mut m = Mac::good();
        m.lock = None;
        let c = get(&run_all(m, None, None), "screen_lock");
        assert!(c.ok && c.say.starts_with("Could not check"));
    }

    #[test]
    fn sleep_check() {
        assert!(get(&run_all(Mac::good(), None, None), "sleep").ok);
        let mut m = Mac::good();
        m.pmset = PMSET.replace("sleep                0 (", "sleep                10 (");
        assert!(!get(&run_all(m, None, None), "sleep").ok);
        assert_eq!(sleep_minutes(" sleep   0\n displaysleep 5"), Some(0));
        assert_eq!(sleep_minutes(" displaysleep 5"), None);
    }

    #[test]
    fn displays_check() {
        let mut s = state();
        assert!(get(&run_all(Mac::good(), Some(&s), None), "displays").ok);
        s.displays = 0;
        let c = get(&run_all(Mac::good(), Some(&s), None), "displays");
        assert!(!c.ok && c.say.contains("dummy plug"));
        s.displays = 2;
        let c = get(&run_all(Mac::good(), Some(&s), None), "displays");
        assert!(c.ok && c.say == "Only the main display is shared.");
        assert!(!get(&run_all(Mac::good(), None, None), "displays").ok);
    }

    #[test]
    fn permissions_check() {
        let mut s = state();
        assert!(get(&run_all(Mac::good(), Some(&s), None), "permissions").ok);
        s.permissions.accessibility = false;
        let c = get(&run_all(Mac::good(), Some(&s), None), "permissions");
        assert!(!c.ok && c.say.contains("Accessibility") && !c.say.contains("Recording"));
        s.permissions.screen_recording = false;
        let c = get(&run_all(Mac::good(), Some(&s), None), "permissions");
        assert!(c.say.contains("Recording") && c.say.contains("Accessibility"));
    }

    #[test]
    fn app_check_cases() {
        let d = app_dir();
        let s = state();
        let c = app_check(&Mac::good().runner(), d.path(), Some(&s));
        assert!(c.ok && c.say.contains("0.1.0"));
        let missing = d.path().join("nope.app");
        assert!(!app_check(&Mac::good().runner(), &missing, Some(&s)).ok);
        let bad = Fake::new(|_, _| fail("invalid"));
        assert!(!app_check(&bad, d.path(), Some(&s)).ok);
    }

    #[test]
    fn link_check() {
        assert!(get(&run_all(Mac::good(), None, Some(&me(true))), "link").ok);
        assert!(!get(&run_all(Mac::good(), None, Some(&me(false))), "link").ok);
        assert!(!get(&run_all(Mac::good(), None, None), "link").ok);
    }

    #[test]
    fn doctor_json_shape() {
        let list = vec![check("a", true, "fine"), check("b", false, "Fix it.")];
        let (out, _) = render(&list, true);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v,
            serde_json::json!([
                {"id": "a", "ok": true, "say": "fine"},
                {"id": "b", "ok": false, "say": "Fix it."}
            ])
        );
    }

    #[test]
    fn doctor_exit_codes() {
        let s = state();
        let m = me(true);
        let list = run_all(Mac::good(), Some(&s), Some(&m));
        assert_eq!(list.len(), 9);
        // Notes (FileVault on) do not fail.
        assert_eq!(render(&list, false).1, 0);
        let mut bad = Mac::good();
        bad.groups = GROUPS_ADMIN;
        let list = run_all(bad, Some(&s), Some(&m));
        let (text, code) = render(&list, false);
        assert_eq!(code, 1);
        assert!(text.contains("FAIL  standard_account"));
    }
}
