//! Keeps the CLI current so nobody has to remember `brew upgrade`.
//!
//! At most once a day a normal command starts a detached copy of this binary, which compares
//! itself with the latest GitHub release. Homebrew installs upgrade themselves; any other install
//! only gets a notice, because we never replace a binary we didn't install. A release is left
//! alone for its first hours, so a bad one can be pulled before it reaches everyone. Every failure
//! is silent and the next day's check tries again.
//!
//! `QUOME_NO_AUTO_UPDATE=1` turns all of this off, and it never runs in CI.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use chrono::{DateTime, Utc};
use colored::Colorize;
use serde::{Deserialize, Serialize};

/// The hidden subcommand the background check runs as.
pub const SUBCOMMAND: &str = "__auto-update";

const RELEASES_URL: &str = "https://api.github.com/repos/quome-cloud/quome-cli/releases/latest";
const RELEASES_PAGE: &str = "https://github.com/quome-cloud/quome-cli/releases/latest";
const TAP: &str = "quome-cloud/quome";
const FORMULA: &str = "quome-cloud/quome/quome";
const STATE_FILE: &str = "update-state.json";
const CHECK_EVERY_HOURS: i64 = 24;
/// How long a release must have been out before it installs itself.
const SOAK_HOURS: i64 = 6;
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest a background check may run, so a stuck `brew` can't linger.
const RUN_TIMEOUT: Duration = Duration::from_secs(15 * 60);

const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct UpdateState {
    #[serde(default)]
    last_check: Option<DateTime<Utc>>,
    /// Set after a background upgrade, cleared when the notice is shown.
    #[serde(default)]
    installed: Option<String>,
    /// A newer version this install can't apply itself, cleared when the notice is shown.
    #[serde(default)]
    available: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    published_at: DateTime<Utc>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// What to do about the latest release.
#[derive(Debug, PartialEq)]
enum Decision {
    Nothing,
    Update(String),
}

fn disabled(env: impl Fn(&str) -> Option<String>) -> bool {
    let set = |name: &str| env(name).is_some_and(|v| !v.is_empty() && v != "0");
    set("QUOME_NO_AUTO_UPDATE") || set("CI")
}

fn disabled_here() -> bool {
    disabled(|name| std::env::var(name).ok())
}

fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.strip_prefix('v').unwrap_or(text).split('.');
    let mut next = || parts.next()?.parse::<u64>().ok();
    let version = (next()?, next()?, next()?);
    parts.next().is_none().then_some(version)
}

fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((parse_version(candidate), parse_version(current)), (Some(a), Some(b)) if a > b)
}

fn check_due(state: &UpdateState, now: DateTime<Utc>) -> bool {
    state
        .last_check
        .is_none_or(|last| now - last >= chrono::Duration::hours(CHECK_EVERY_HOURS))
}

fn decide(release: &Release, current: &str, now: DateTime<Utc>) -> Decision {
    let old_enough = now - release.published_at >= chrono::Duration::hours(SOAK_HOURS);
    if release.draft || release.prerelease || !old_enough || !is_newer(&release.tag_name, current) {
        return Decision::Nothing;
    }
    let version = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name);
    Decision::Update(version.to_string())
}

/// The Homebrew that installed this binary, or `None` if Homebrew didn't.
fn homebrew_for(exe: &Path) -> Option<PathBuf> {
    let components: Vec<_> = exe.components().collect();
    let cellar = components
        .iter()
        .position(|c| c.as_os_str() == "Cellar")
        .filter(|i| {
            components
                .get(i + 1)
                .is_some_and(|c| c.as_os_str() == "quome")
        })?;
    let prefix: PathBuf = components[..cellar].iter().collect();
    Some(prefix.join("bin").join("brew"))
}

fn state_path() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".quome").join(STATE_FILE))
}

fn load(path: &Path) -> UpdateState {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save(path: &Path, state: &UpdateState) -> std::io::Result<()> {
    let dir = path.parent().ok_or(std::io::ErrorKind::NotFound)?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{STATE_FILE}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(state)?)?;
    std::fs::rename(&tmp, path)
}

/// Starts today's check if one is due. Never fails and never waits for the check.
pub fn start_background_check() {
    if disabled_here() {
        return;
    }
    let Some(path) = state_path() else { return };
    let mut state = load(&path);
    let now = Utc::now();
    if !check_due(&state, now) {
        return;
    }
    // Recorded before starting, so a check that fails waits until tomorrow.
    state.last_check = Some(now);
    if save(&path, &state).is_ok() {
        let _ = spawn_detached();
    }
}

fn spawn_detached() -> std::io::Result<()> {
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .arg(SUBCOMMAND)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    // `quome mcp` runs for hours: reap the check when it ends.
    std::thread::spawn(move || child.wait());
    Ok(())
}

fn notice_text(state: &UpdateState, current: &str) -> Option<String> {
    if let Some(version) = state.installed.as_deref().filter(|v| *v == current) {
        return Some(format!("Updated quome to {version}"));
    }
    let version = state
        .available
        .as_deref()
        .filter(|v| is_newer(v, current))?;
    Some(format!("quome {version} is available: {RELEASES_PAGE}"))
}

/// Shows the pending update line once, on a terminal only.
pub fn print_notice() {
    if !std::io::stderr().is_terminal() {
        return;
    }
    let Some(path) = state_path() else { return };
    let mut state = load(&path);
    let Some(text) = notice_text(&state, CURRENT) else {
        return;
    };
    state.installed = None;
    state.available = None;
    if save(&path, &state).is_ok() {
        eprintln!("{}", text.dimmed());
    }
}

/// The background check itself. Failures are dropped on purpose: tomorrow's check tries again.
pub async fn run() {
    if disabled_here() {
        return;
    }
    let _ = tokio::time::timeout(RUN_TIMEOUT, check_and_update()).await;
}

async fn check_and_update() -> Option<()> {
    let release = fetch_latest(RELEASES_URL).await?;
    let Decision::Update(version) = decide(&release, CURRENT, Utc::now()) else {
        return None;
    };
    let path = state_path()?;
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    match homebrew_for(&exe).filter(|brew| brew.exists()) {
        Some(brew) => {
            brew_upgrade(&brew, Path::new("git"), &version).await?;
            let mut state = load(&path);
            state.installed = Some(version);
            state.available = None;
            save(&path, &state).ok()
        }
        None => {
            let mut state = load(&path);
            state.available = Some(version);
            save(&path, &state).ok()
        }
    }
}

async fn fetch_latest(url: &str) -> Option<Release> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("quome-cli/", env!("CARGO_PKG_VERSION")))
        .timeout(FETCH_TIMEOUT)
        .build()
        .ok()?;
    let response = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    response.json().await.ok()
}

fn quiet(program: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .env("HOMEBREW_NO_ENV_HINTS", "1");
    command
}

/// Refreshes only our tap and upgrades only `quome`, leaving the rest of Homebrew alone.
/// `Some(())` only once Homebrew reports `version` installed.
async fn brew_upgrade(brew: &Path, git: &Path, version: &str) -> Option<()> {
    let output = quiet(brew)
        .args(["--repository", TAP])
        .output()
        .await
        .ok()?;
    let tap = String::from_utf8(output.stdout).ok()?;
    let tap = tap.trim();
    if !output.status.success() || !Path::new(tap).is_dir() {
        return None;
    }
    let pulled = quiet(git)
        .args(["-C", tap, "pull", "--ff-only", "--quiet"])
        .stdout(Stdio::null())
        .status()
        .await
        .ok()?;
    let upgraded = quiet(brew)
        .args(["upgrade", FORMULA])
        .stdout(Stdio::null())
        .status()
        .await
        .ok()?;
    if !pulled.success() || !upgraded.success() {
        return None;
    }
    let listed = quiet(brew)
        .args(["list", "--versions", FORMULA])
        .output()
        .await
        .ok()?;
    String::from_utf8_lossy(&listed.stdout)
        .split_whitespace()
        .any(|word| word == version)
        .then_some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 9, hour, 0, 0).unwrap()
    }

    fn release(tag: &str, published_at: DateTime<Utc>) -> Release {
        Release {
            tag_name: tag.into(),
            published_at,
            draft: false,
            prerelease: false,
        }
    }

    fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn off_switch_and_ci_disable_it() {
        assert!(!disabled(env_of(&[])));
        assert!(disabled(env_of(&[("QUOME_NO_AUTO_UPDATE", "1")])));
        assert!(disabled(env_of(&[("CI", "true")])));
        assert!(!disabled(env_of(&[("QUOME_NO_AUTO_UPDATE", "0")])));
        assert!(!disabled(env_of(&[("CI", "")])));
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert!(is_newer("v0.2.10", "0.2.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("v0.2.31", "0.2.31"));
        assert!(!is_newer("v0.2.30", "0.2.31"));
    }

    #[test]
    fn malformed_versions_are_never_newer() {
        for bad in ["latest", "v1.2", "v1.2.3.4", "v1.2.x", "v1.2.3-rc1", ""] {
            assert!(!is_newer(bad, "0.0.1"), "{bad}");
        }
    }

    #[test]
    fn checks_at_most_daily() {
        let mut state = UpdateState::default();
        assert!(check_due(&state, at(0)));
        state.last_check = Some(at(0));
        assert!(!check_due(&state, at(23)));
        assert!(check_due(&state, at(0) + chrono::Duration::hours(24)));
    }

    #[test]
    fn new_release_waits_out_the_soak() {
        let r = release("v0.2.32", at(0));
        assert_eq!(decide(&r, "0.2.31", at(5)), Decision::Nothing);
        assert_eq!(
            decide(&r, "0.2.31", at(6)),
            Decision::Update("0.2.32".into())
        );
    }

    #[test]
    fn never_installs_an_older_draft_or_prerelease() {
        assert_eq!(
            decide(&release("v0.2.30", at(0)), "0.2.31", at(9)),
            Decision::Nothing
        );
        let mut draft = release("v0.2.32", at(0));
        draft.draft = true;
        assert_eq!(decide(&draft, "0.2.31", at(9)), Decision::Nothing);
        let mut pre = release("v0.2.32", at(0));
        pre.prerelease = true;
        assert_eq!(decide(&pre, "0.2.31", at(9)), Decision::Nothing);
    }

    #[test]
    fn finds_homebrew_only_for_homebrew_installs() {
        assert_eq!(
            homebrew_for(Path::new("/opt/homebrew/Cellar/quome/0.2.31/bin/quome")),
            Some(PathBuf::from("/opt/homebrew/bin/brew"))
        );
        assert_eq!(
            homebrew_for(Path::new("/usr/local/Cellar/quome/0.2.31/bin/quome")),
            Some(PathBuf::from("/usr/local/bin/brew"))
        );
        assert_eq!(homebrew_for(Path::new("/home/me/.cargo/bin/quome")), None);
        assert_eq!(
            homebrew_for(Path::new("/opt/homebrew/Cellar/other/1.0/bin/quome")),
            None
        );
    }

    #[test]
    fn notice_says_what_happened_and_drops_stale_entries() {
        let installed = UpdateState {
            installed: Some("0.2.32".into()),
            ..Default::default()
        };
        assert_eq!(
            notice_text(&installed, "0.2.32").as_deref(),
            Some("Updated quome to 0.2.32")
        );
        // Not running the new version yet.
        assert_eq!(notice_text(&installed, "0.2.31"), None);

        let available = UpdateState {
            available: Some("0.2.32".into()),
            ..Default::default()
        };
        assert!(notice_text(&available, "0.2.31")
            .unwrap()
            .contains("0.2.32 is available"));
        assert_eq!(notice_text(&available, "0.2.32"), None);
        assert_eq!(notice_text(&UpdateState::default(), "0.2.32"), None);
    }

    #[test]
    fn state_round_trips_and_bad_state_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(STATE_FILE);
        assert_eq!(load(&path), UpdateState::default());
        let state = UpdateState {
            last_check: Some(at(3)),
            installed: Some("0.2.32".into()),
            available: None,
        };
        save(&path, &state).unwrap();
        assert_eq!(load(&path), state);
        std::fs::write(&path, b"not json").unwrap();
        assert_eq!(load(&path), UpdateState::default());
    }

    #[tokio::test]
    async fn reads_the_release_github_returns() {
        use wiremock::matchers::{header_exists, method};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(header_exists("user-agent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "tag_name": "v0.2.32",
                "published_at": "2026-11-09T00:26:32Z",
                "draft": false,
                "prerelease": false,
                "assets": [],
            })))
            .mount(&server)
            .await;
        let got = fetch_latest(&server.uri()).await.unwrap();
        assert_eq!(got.tag_name, "v0.2.32");
        assert_eq!(
            got.published_at,
            at(0) + chrono::Duration::seconds(26 * 60 + 32)
        );
    }

    #[tokio::test]
    async fn a_failing_or_garbled_answer_is_no_release() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        assert!(fetch_latest(&server.uri()).await.is_none());

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("nope"))
            .mount(&server)
            .await;
        assert!(fetch_latest(&server.uri()).await.is_none());
    }

    #[cfg(unix)]
    mod with_a_fake_homebrew {
        use super::super::*;
        use std::os::unix::fs::PermissionsExt;

        struct Fake {
            dir: tempfile::TempDir,
            brew: PathBuf,
            git: PathBuf,
        }

        impl Fake {
            fn calls(&self) -> String {
                std::fs::read_to_string(self.dir.path().join("calls")).unwrap_or_default()
            }
        }

        fn script(path: &Path, body: &str) {
            std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        /// `installed` is what `brew list --versions` reports; `upgrade_exit` is `brew upgrade`'s.
        fn fake(installed: &str, upgrade_exit: u8) -> Fake {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().display().to_string();
            let brew = dir.path().join("brew");
            let git = dir.path().join("git");
            script(
                &brew,
                &format!(
                    r#"echo "brew $* no_auto=$HOMEBREW_NO_AUTO_UPDATE" >> {root}/calls
case "$1" in
  --repository) echo {root} ;;
  upgrade) exit {upgrade_exit} ;;
  list) echo "quome {installed}" ;;
esac"#
                ),
            );
            script(&git, &format!(r#"echo "git $*" >> {root}/calls"#));
            Fake { dir, brew, git }
        }

        #[tokio::test]
        async fn refreshes_only_our_tap_then_upgrades_only_quome() {
            let f = fake("0.2.31 0.2.32", 0);
            assert!(brew_upgrade(&f.brew, &f.git, "0.2.32").await.is_some());
            let calls = f.calls();
            let root = f.dir.path().display();
            let want = format!(
                "brew --repository quome-cloud/quome no_auto=1\n\
                 git -C {root} pull --ff-only --quiet\n\
                 brew upgrade quome-cloud/quome/quome no_auto=1\n\
                 brew list --versions quome-cloud/quome/quome no_auto=1\n"
            );
            assert_eq!(calls, want);
        }

        #[tokio::test]
        async fn a_failed_upgrade_is_not_reported_as_installed() {
            let f = fake("0.2.31", 1);
            assert!(brew_upgrade(&f.brew, &f.git, "0.2.32").await.is_none());
        }

        #[tokio::test]
        async fn an_upgrade_that_changed_nothing_is_not_reported_as_installed() {
            let f = fake("0.2.31", 0);
            assert!(brew_upgrade(&f.brew, &f.git, "0.2.32").await.is_none());
        }
    }
}
