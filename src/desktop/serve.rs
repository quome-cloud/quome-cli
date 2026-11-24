//! `quome desktop serve`: the link between this computer and the platform. It asks
//! the platform for a session (where the desktop app dials and the key it dials
//! with), writes it to `session.json` for the app, and watches the app's
//! `state.json`. After five failed dials it asks for a new session, which also
//! wakes the engine, at most once per `new_session_every`.
//!
//! Exit codes are `quome models serve`'s: 0 stops for good (not linked, unlinked,
//! the screen fixed otherwise, a stop signal); 3 means the program was upgraded and
//! launchd starts the new one. Transient failures back off in-process. No log line
//! carries a token, the session's key or a response body.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]
use futures_util::future::BoxFuture;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::{sleep, Instant};

use super::files::{self, SessionFile, Size};
use crate::cloud::api::Session;
use crate::cloud::keys::KeyStore;
use crate::errors::{QuomeError, Result};

/// Said, and exit 0, when there is no link to serve.
pub const NOT_LINKED: &str = "This computer is not linked. Run `quome desktop connect`.";
/// Said, and exit 0, once the platform no longer knows this computer.
pub const UNLINKED: &str =
    "This computer is no longer linked. Run `quome desktop connect --replace` again.";
/// Said, and exit 0, when the platform fixed another screen size for this computer.
pub const SCREEN_FIXED: &str = "The screen size in desktop.toml no longer matches this \
     computer's link. Run `quome desktop connect --replace`.";

#[derive(Clone, Debug)]
pub struct Timing {
    /// How often `state.json` is read (and the program checked for an upgrade).
    pub state_every: Duration,
    /// The least time between two sessions asked for because the app's dials fail.
    pub new_session_every: Duration,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            state_every: Duration::from_secs(2),
            new_session_every: Duration::from_secs(30),
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
        }
    }
}

/// How `quome desktop serve` ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Served {
    NotConnected,
    Unlinked,
    ScreenFixed,
    Upgraded,
    Shutdown,
}

impl Served {
    /// What to tell the person, if anything.
    pub fn message(&self) -> Option<&'static str> {
        match self {
            Served::NotConnected => Some(NOT_LINKED),
            Served::Unlinked => Some(UNLINKED),
            Served::ScreenFixed => Some(SCREEN_FIXED),
            Served::Upgraded | Served::Shutdown => None,
        }
    }
}

/// 0 = stop for good (launchd leaves it stopped); 3 = the program was upgraded, so
/// launchd starts the new one. As `quome models serve`.
pub fn serve_exit_code(s: &Served) -> i32 {
    match s {
        Served::Upgraded => 3,
        _ => 0,
    }
}

pub type SessionFn = Arc<dyn Fn() -> BoxFuture<'static, Result<Session>> + Send + Sync>;
pub type LogFn = Arc<dyn Fn(&str) + Send + Sync>;

/// What the link loop runs on; tests give each part.
pub struct Link {
    pub session: SessionFn,
    /// The runtime directory.
    pub dir: PathBuf,
    pub timing: Timing,
    pub program: Option<PathBuf>,
    pub shutdown: watch::Receiver<bool>,
    pub log: LogFn,
}

/// The app's failed dials in a row after which it gets a new session.
const NEW_SESSION_AFTER: u32 = 5;
/// The wait for a starting engine whose answer names none.
const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(5);

/// What a refused session call means.
#[derive(Debug, PartialEq, Eq)]
enum Refusal {
    /// The link is gone: forget the session and stop for good.
    Unlinked,
    ScreenFixed,
    /// The engine is starting: ask again after this long.
    Wait(Duration),
    /// Anything else: back off. The text is safe to log (no body, no token).
    Transient(String),
}

fn classify(e: &QuomeError, max_wait: Duration) -> Refusal {
    // A revoked or expired delegation, or one without the capability, as `models serve`.
    if crate::machine::tunnel::permanent(e).is_some() {
        return Refusal::Unlinked;
    }
    match e {
        // `computer_not_found`, or the feature turned off.
        QuomeError::NotFound(_) => Refusal::Unlinked,
        QuomeError::ApiRefused { status, reason, .. } => match (*status, reason.as_str()) {
            (403, "approver_not_admin") => Refusal::Unlinked,
            (409, "screen_is_fixed") => Refusal::ScreenFixed,
            (s, "") => Refusal::Transient(format!("status={s}")),
            (s, r) => Refusal::Transient(format!("status={s} reason={r}")),
        },
        QuomeError::RetryLater { retry_after, .. } => Refusal::Wait(
            retry_after
                .map(Duration::from_secs)
                .unwrap_or(DEFAULT_RETRY_AFTER)
                .min(max_wait),
        ),
        QuomeError::Http(e) if e.is_timeout() => Refusal::Transient("error=timed_out".into()),
        QuomeError::Http(e) if e.is_connect() => Refusal::Transient("error=connect".into()),
        QuomeError::Http(_) => Refusal::Transient("error=network".into()),
        QuomeError::RateLimited => Refusal::Transient("status=429".into()),
        // Messages the CLI wrote itself, or that hold only a status line.
        QuomeError::ApiError(_) => Refusal::Transient("error=api".into()),
        other => Refusal::Transient(format!("error={}", error_kind(other))),
    }
}

/// A name for an error, never its text (which may quote an answer).
fn error_kind(e: &QuomeError) -> &'static str {
    match e {
        QuomeError::CloudState(_) => "cloud_state",
        QuomeError::CloudKey(_) => "key",
        QuomeError::InvalidResponse => "invalid_response",
        QuomeError::Usage(_) => "local",
        _ => "other",
    }
}

/// 1 s doubling to 60 s; a session written starts it over.
struct Backoff {
    min: Duration,
    max: Duration,
    next: Duration,
}

impl Backoff {
    fn new(min: Duration, max: Duration) -> Self {
        Backoff {
            min,
            max,
            next: min,
        }
    }

    fn wait(&mut self) -> Duration {
        let w = self.next;
        self.next = self.next.saturating_mul(2).min(self.max);
        w
    }

    fn reset(&mut self) {
        self.next = self.min;
    }
}

/// When the app's failed dials ask for a new session: five in a row in a fresh
/// state, and at least `every` since the last session was written.
fn wants_new_session(
    state: Option<&files::AppState>,
    since_last: Duration,
    every: Duration,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    state.is_some_and(|s| s.dial_failures >= NEW_SESSION_AFTER && files::fresh(s, now))
        && since_last >= every
}

/// Ask for sessions and keep `session.json` current until a permanent stop.
pub async fn run(l: Link) -> Served {
    use crate::machine::tunnel::{stopped, Program};
    let program = l.program.clone().map(Program::new);
    let upgraded = || program.as_ref().is_some_and(Program::changed);
    let mut shutdown = l.shutdown.clone();
    let mut backoff = Backoff::new(l.timing.backoff_min, l.timing.backoff_max);
    let log = |m: &str| (l.log)(m);
    loop {
        if *shutdown.borrow() {
            return Served::Shutdown;
        }
        if upgraded() {
            return Served::Upgraded;
        }
        let got = tokio::select! {
            r = (l.session)() => r,
            _ = stopped(&mut shutdown) => return Served::Shutdown,
        };
        let wait = match got {
            Ok(s) => {
                let file = SessionFile {
                    v: files::VERSION,
                    url: s.url,
                    desktop: s.desktop,
                    key: s.key,
                    screen: Size {
                        w: s.screen.w,
                        h: s.screen.h,
                    },
                    written_at: chrono::Utc::now()
                        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                };
                match files::write_session(&l.dir, &file) {
                    Err(e) => {
                        log(&format!("session_write_failed error={:?}", e.kind()));
                        backoff.wait()
                    }
                    Ok(()) => {
                        backoff.reset();
                        log(&format!("session desktop={}", file.desktop));
                        let written = Instant::now();
                        // Watch the app until it needs a new session.
                        loop {
                            tokio::select! {
                                _ = sleep(l.timing.state_every) => {}
                                _ = stopped(&mut shutdown) => return Served::Shutdown,
                            }
                            if upgraded() {
                                return Served::Upgraded;
                            }
                            let state = files::read_state(&l.dir);
                            if wants_new_session(
                                state.as_ref(),
                                written.elapsed(),
                                l.timing.new_session_every,
                                chrono::Utc::now(),
                            ) {
                                let n = state.map_or(0, |s| s.dial_failures);
                                log(&format!("new_session dial_failures={n}"));
                                break;
                            }
                        }
                        Duration::ZERO
                    }
                }
            }
            Err(e) => match classify(&e, l.timing.backoff_max) {
                Refusal::Unlinked => {
                    files::remove_session(&l.dir);
                    log("unlinked");
                    return Served::Unlinked;
                }
                Refusal::ScreenFixed => {
                    // The key is for a link that no longer matches: the app stops dialing.
                    files::remove_session(&l.dir);
                    log("screen_is_fixed");
                    return Served::ScreenFixed;
                }
                Refusal::Wait(d) => {
                    log(&format!("desktops_starting retry_after={}", d.as_secs()));
                    d
                }
                Refusal::Transient(what) => {
                    let w = backoff.wait();
                    log(&format!("session_failed {what} retry_in={}", w.as_secs()));
                    w
                }
            },
        };
        if !wait.is_zero() {
            tokio::select! {
                _ = sleep(wait) => {}
                _ = stopped(&mut shutdown) => return Served::Shutdown,
            }
        }
    }
}

/// `CFBundleShortVersionString` from an XML `Info.plist`.
fn plist_version(text: &str) -> Option<String> {
    let after = text.split("<key>CFBundleShortVersionString</key>").nth(1)?;
    let v = after.trim_start().strip_prefix("<string>")?;
    let v = v.split("</string>").next()?.trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// The installed app's version: its bundle's, else what it last reported, else 0.0.0.
pub fn app_version(home: &Path) -> String {
    let plist = crate::desktop::app::app_path(home).join("Contents/Info.plist");
    std::fs::read_to_string(plist)
        .ok()
        .and_then(|t| plist_version(&t))
        .or_else(|| files::read_state(&files::runtime_dir(home)).and_then(|s| s.version))
        .unwrap_or_else(|| "0.0.0".into())
}

/// One line on stderr, for launchd's log file: `desktop: <event> k=v`.
fn log_stderr(m: &str) {
    eprintln!("desktop: {m}");
}

/// Serve this computer's link until a permanent stop.
pub async fn serve(
    home: &Path,
    store: &dyn KeyStore,
    timing: Timing,
    program: Option<PathBuf>,
    shutdown: watch::Receiver<bool>,
) -> Result<Served> {
    use crate::cloud::api::{CloudApi, Screen, SessionBody};
    use crate::cloud::auth::TokenSource;
    use crate::cloud::keys::AgentKey;
    use crate::cloud::state::CloudState;
    use crate::desktop::config::{self, PROFILE};

    let st = CloudState::load_from(home)?;
    let Some(p) = st.profiles.get(PROFILE).cloned() else {
        return Ok(Served::NotConnected);
    };
    let Some((w, h)) = config::load(home)?.and_then(|c| c.screen) else {
        return Ok(Served::NotConnected);
    };
    // A profile without its key is as good as not linked: nothing a restart could fix.
    let (key, vrc) = match AgentKey::load_from(store, PROFILE) {
        Ok(k) => k,
        // Only a key that is not there is "not linked". A Keychain that cannot be read
        // (locked at login) is an error: exit non-zero so launchd tries again.
        Err(e @ QuomeError::CloudKey(_)) => {
            return match store.has(PROFILE) {
                Ok(false) => Ok(Served::NotConnected),
                _ => Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    let api = Arc::new(CloudApi::new(&p, TokenSource::new(&p, key, vrc)));
    let body = SessionBody {
        os: "macos".into(),
        screen: Screen { w, h },
        app_version: app_version(home),
    };
    log_stderr(&format!(
        "serving org={} screen={w}x{h} app_version={}",
        p.org_slug, body.app_version
    ));
    let session: SessionFn = Arc::new(move || {
        let (api, body) = (api.clone(), body.clone());
        Box::pin(async move { api.desktop_session(&body).await })
    });
    Ok(run(Link {
        session,
        dir: files::runtime_dir(home),
        timing,
        program,
        shutdown,
        log: Arc::new(log_stderr),
    })
    .await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::api::Screen;
    use crate::cloud::keys::{AgentKey, FileStore};
    use crate::cloud::state::{CloudState, Profile};
    use crate::desktop::config::{self, DesktopConfig, PROFILE};
    use std::sync::Mutex;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const KEY: &str = "qpk_TOPSECRET_desk_key";
    const SESSION_PATH: &str = "/api/v1/orgs/org-1/desktops/computers/me/session";

    fn good_session(n: usize) -> Session {
        Session {
            desktop: "d-1".into(),
            name: "Studio".into(),
            url: "wss://desktops.example.test/acme/v1/daemon".into(),
            key: format!("{KEY}_{n}"),
            screen: Screen { w: 1920, h: 1200 },
        }
    }

    /// The link's fakes: a session answer per call, the calls' times, and the log.
    struct Fake {
        dir: tempfile::TempDir,
        calls: Arc<Mutex<Vec<Instant>>>,
        logged: Arc<Mutex<Vec<String>>>,
        stop: watch::Sender<bool>,
    }

    /// `answer(n)` is the n-th (0-based) session call's answer.
    fn fake(
        answer: impl Fn(usize) -> Result<Session> + Send + Sync + 'static,
        timing: Timing,
        program: Option<PathBuf>,
    ) -> (Fake, Link) {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(vec![]));
        let logged = Arc::new(Mutex::new(vec![]));
        let (stop, shutdown) = watch::channel(false);
        let c = calls.clone();
        let answer = Arc::new(answer);
        let session: SessionFn = Arc::new(move || {
            let n = {
                let mut c = c.lock().unwrap();
                c.push(Instant::now());
                c.len() - 1
            };
            let a = answer.clone();
            Box::pin(async move { a(n) })
        });
        let l = logged.clone();
        let link = Link {
            session,
            dir: dir.path().join("run"),
            timing,
            program,
            shutdown,
            log: Arc::new(move |m: &str| l.lock().unwrap().push(m.to_string())),
        };
        (
            Fake {
                dir,
                calls,
                logged,
                stop,
            },
            link,
        )
    }

    fn write_state(dir: &Path, dial_failures: u32) {
        write_state_at(dir, dial_failures, &chrono::Utc::now().to_rfc3339());
    }

    fn write_state_at(dir: &Path, dial_failures: u32, updated_at: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("state.json"),
            serde_json::json!({"v": 1, "version": "0.1.0", "connected": false,
                "dial_failures": dial_failures, "updated_at": updated_at})
            .to_string(),
        )
        .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_stale_state_asks_for_no_new_session() {
        let (f, link) = fake(|n| Ok(good_session(n)), Timing::default(), None);
        let old = (chrono::Utc::now() - chrono::Duration::seconds(61)).to_rfc3339();
        write_state_at(&link.dir, 5, &old);
        let running = tokio::spawn(run(link));
        sleep(Duration::from_secs(300)).await;
        assert_eq!(f.calls.lock().unwrap().len(), 1);
        f.stop.send(true).unwrap();
        assert_eq!(running.await.unwrap(), Served::Shutdown);
    }

    #[test]
    fn a_state_counts_only_while_fresh() {
        let now = chrono::Utc::now();
        let state = |updated_at: Option<String>| files::AppState {
            v: 1,
            dial_failures: 5,
            updated_at,
            ..Default::default()
        };
        let every = Duration::from_secs(30);
        let at = |secs: i64| Some((now - chrono::Duration::seconds(secs)).to_rfc3339());
        assert!(wants_new_session(Some(&state(at(10))), every, every, now));
        assert!(wants_new_session(Some(&state(at(60))), every, every, now));
        assert!(!wants_new_session(Some(&state(at(61))), every, every, now));
        assert!(!wants_new_session(Some(&state(None)), every, every, now));
        assert!(!wants_new_session(
            Some(&state(Some("yesterday".into()))),
            every,
            every,
            now
        ));
        assert!(!wants_new_session(
            Some(&state(at(10))),
            Duration::from_secs(29),
            every,
            now
        ));
        assert!(!wants_new_session(None, every, every, now));
    }

    fn refused(status: u16, reason: &str) -> QuomeError {
        QuomeError::ApiRefused {
            message: format!("{status} {reason}"),
            status,
            reason: reason.into(),
        }
    }

    #[test]
    fn exit_codes_and_messages() {
        assert_eq!(serve_exit_code(&Served::Upgraded), 3);
        for s in [
            Served::NotConnected,
            Served::Unlinked,
            Served::ScreenFixed,
            Served::Shutdown,
        ] {
            assert_eq!(serve_exit_code(&s), 0, "{s:?}");
        }
        assert_eq!(
            Served::NotConnected.message(),
            Some("This computer is not linked. Run `quome desktop connect`.")
        );
        assert_eq!(
            Served::Unlinked.message(),
            Some("This computer is no longer linked. Run `quome desktop connect --replace` again.")
        );
        assert_eq!(
            Served::ScreenFixed.message(),
            Some(
                "The screen size in desktop.toml no longer matches this computer's link. \
                 Run `quome desktop connect --replace`."
            )
        );
        assert_eq!(Served::Shutdown.message(), None);
        assert_eq!(Served::Upgraded.message(), None);
    }

    #[test]
    fn app_version_reads_the_bundle_then_the_state() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(app_version(home.path()), "0.0.0");
        write_state(&files::runtime_dir(home.path()), 0);
        assert_eq!(app_version(home.path()), "0.1.0");
        let contents = crate::desktop::app::app_path(home.path()).join("Contents");
        std::fs::create_dir_all(&contents).unwrap();
        std::fs::write(
            contents.join("Info.plist"),
            "<?xml version=\"1.0\"?><plist><dict>\n  <key>CFBundleIdentifier</key>\n  \
             <string>cloud.quome.desktop</string>\n  <key>CFBundleShortVersionString</key>\n  \
             <string>0.2.3</string>\n</dict></plist>",
        )
        .unwrap();
        assert_eq!(app_version(home.path()), "0.2.3");
    }

    #[tokio::test(start_paused = true)]
    async fn five_failures_new_session_rate_limited() {
        let (f, link) = fake(|n| Ok(good_session(n)), Timing::default(), None);
        write_state(&link.dir, 5);
        let running = tokio::spawn(run(link));
        sleep(Duration::from_secs(61)).await;
        let calls = f.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 3, "the first session and exactly two new ones");
        for w in calls.windows(2) {
            assert!(w[1] - w[0] >= Duration::from_secs(30), "{:?}", w[1] - w[0]);
        }
        // The newest key is what the app reads.
        let text = std::fs::read_to_string(f.dir.path().join("run/session.json")).unwrap();
        assert!(text.contains(&format!("{KEY}_2")), "{text}");
        f.stop.send(true).unwrap();
        assert_eq!(running.await.unwrap(), Served::Shutdown);
    }

    #[tokio::test(start_paused = true)]
    async fn fewer_than_five_failures_keep_the_session() {
        let (f, link) = fake(|n| Ok(good_session(n)), Timing::default(), None);
        write_state(&link.dir, 4);
        let running = tokio::spawn(run(link));
        sleep(Duration::from_secs(300)).await;
        assert_eq!(f.calls.lock().unwrap().len(), 1);
        f.stop.send(true).unwrap();
        assert_eq!(running.await.unwrap(), Served::Shutdown);
    }

    #[tokio::test(start_paused = true)]
    async fn starting_503_waits_retry_after() {
        let (f, link) = fake(
            |n| match n {
                0 => Err(QuomeError::RetryLater {
                    message: "503 desktops_starting".into(),
                    retry_after: Some(7),
                }),
                1 => Err(QuomeError::RetryLater {
                    message: "503 desktops_starting".into(),
                    retry_after: None,
                }),
                n => Ok(good_session(n)),
            },
            Timing::default(),
            None,
        );
        let dir = link.dir.clone();
        let running = tokio::spawn(run(link));
        sleep(Duration::from_secs(20)).await;
        let calls = f.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[1] - calls[0], Duration::from_secs(7));
        assert_eq!(
            calls[2] - calls[1],
            Duration::from_secs(5),
            "the default wait"
        );
        assert!(dir.join("session.json").is_file());
        f.stop.send(true).unwrap();
        assert_eq!(running.await.unwrap(), Served::Shutdown);
    }

    #[tokio::test(start_paused = true)]
    async fn network_errors_back_off_doubling_to_a_minute() {
        let (f, link) = fake(
            |_| Err(QuomeError::Usage("connection refused".into())),
            Timing::default(),
            None,
        );
        let running = tokio::spawn(run(link));
        sleep(Duration::from_secs(400)).await;
        let calls = f.calls.lock().unwrap().clone();
        let gaps: Vec<u64> = calls.windows(2).map(|w| (w[1] - w[0]).as_secs()).collect();
        assert_eq!(&gaps[..8], [1, 2, 4, 8, 16, 32, 60, 60]);
        f.stop.send(true).unwrap();
        assert_eq!(running.await.unwrap(), Served::Shutdown);
    }

    #[tokio::test(start_paused = true)]
    async fn session_file_never_logged() {
        let (f, link) = fake(
            |n| match n {
                // An error whose text carries the key must not reach the log either.
                0 => Err(QuomeError::ApiRefused {
                    message: format!("500 boom: {KEY}"),
                    status: 500,
                    reason: "boom".into(),
                }),
                n => Ok(good_session(n)),
            },
            Timing::default(),
            None,
        );
        let dir = link.dir.clone();
        let running = tokio::spawn(run(link));
        sleep(Duration::from_secs(5)).await;
        f.stop.send(true).unwrap();
        assert_eq!(running.await.unwrap(), Served::Shutdown);
        let text = std::fs::read_to_string(dir.join("session.json")).unwrap();
        assert!(text.contains(&format!("{KEY}_1")));
        let logged = f.logged.lock().unwrap().clone();
        assert!(logged.len() >= 2, "{logged:?}");
        for line in &logged {
            assert!(!line.contains(KEY), "{line}");
            assert!(!line.contains("boom:"), "no body text: {line}");
        }
        assert!(
            logged.iter().any(|l| l.contains("status=500")),
            "{logged:?}"
        );
        let s = files::SessionFile {
            v: 1,
            url: "u".into(),
            desktop: "d".into(),
            key: KEY.into(),
            screen: Size { w: 1, h: 1 },
            written_at: "t".into(),
        };
        assert!(!format!("{s:?}").contains(KEY));
    }

    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn upgrade_exits_3() {
        let bin = tempfile::tempdir().unwrap();
        let (a, b, link_path) = (
            bin.path().join("quome-1"),
            bin.path().join("quome-2"),
            bin.path().join("quome"),
        );
        std::fs::write(&a, "1").unwrap();
        std::fs::write(&b, "2").unwrap();
        std::os::unix::fs::symlink(&a, &link_path).unwrap();
        let (f, link) = fake(
            |n| Ok(good_session(n)),
            Timing::default(),
            Some(link_path.clone()),
        );
        let running = tokio::spawn(run(link));
        sleep(Duration::from_secs(3)).await;
        assert!(!running.is_finished());
        std::fs::remove_file(&link_path).unwrap();
        std::os::unix::fs::symlink(&b, &link_path).unwrap();
        let out = tokio::time::timeout(Duration::from_secs(10), running)
            .await
            .expect("an upgrade ends serve")
            .unwrap();
        assert_eq!(out, Served::Upgraded);
        assert_eq!(serve_exit_code(&out), 3);
        // The app keeps its session through the restart.
        assert!(f.dir.path().join("run/session.json").is_file());
    }

    // ------------------------------------------------- against a mock platform

    struct Home {
        home: tempfile::TempDir,
        store: FileStore,
    }

    /// A linked computer: the `desktop` profile and key, and `desktop.toml` with a screen.
    fn linked(s: &MockServer) -> Home {
        let home = tempfile::tempdir().unwrap();
        let mut st = CloudState::default();
        st.profiles.insert(
            PROFILE.into(),
            Profile {
                edition: "cloud".into(),
                api_url: s.uri(),
                console_url: "https://console.example.test".into(),
                issuer: s.uri(),
                agent_name: "Studio".into(),
                agent_principal: "agent-principal".into(),
                kid: "k".into(),
                jkt: "j".into(),
                delegation_id: "del-1".into(),
                expires_at: "2099-01-01T00:00:00Z".into(),
                org_id: "org-1".into(),
                org_slug: "acme".into(),
                apps_domain: "apps.example.test".into(),
            },
        );
        st.save_to(home.path()).unwrap();
        let store = FileStore {
            dir: home.path().join("keys"),
        };
        AgentKey::generate()
            .store_in(&store, PROFILE, "VRC-1")
            .unwrap();
        config::save(
            home.path(),
            &DesktopConfig {
                api: s.uri(),
                org: "acme".into(),
                screen: Some((1920, 1200)),
                small_frames: false,
                panes_shown: true,
            },
        )
        .unwrap();
        Home { home, store }
    }

    async fn mount_auth(s: &MockServer, token: ResponseTemplate) {
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"token_endpoint": format!("{}/oauth/token", s.uri())}),
            ))
            .mount(s)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(token)
            .mount(s)
            .await;
    }

    fn token_ok() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "AT", "token_type": "Bearer", "expires_in": 3600
        }))
    }

    async fn mount_session(s: &MockServer, answer: ResponseTemplate) {
        Mock::given(method("POST"))
            .and(path(SESSION_PATH))
            .respond_with(answer)
            .mount(s)
            .await;
    }

    fn seed_session(h: &Home) -> PathBuf {
        let dir = files::runtime_dir(h.home.path());
        files::write_session(
            &dir,
            &SessionFile {
                v: 1,
                url: "wss://x".into(),
                desktop: "d-1".into(),
                key: "qpk_old".into(),
                screen: Size { w: 1920, h: 1200 },
                written_at: "t".into(),
            },
        )
        .unwrap();
        dir.join("session.json")
    }

    async fn serve_once(h: &Home) -> Served {
        let (_tx, rx) = watch::channel(false);
        tokio::time::timeout(
            Duration::from_secs(10),
            serve(h.home.path(), &h.store, Timing::default(), None, rx),
        )
        .await
        .expect("serve ends")
        .unwrap()
    }

    #[tokio::test]
    async fn not_linked_exits_0() {
        let s = MockServer::start().await;
        let h = Home {
            home: tempfile::tempdir().unwrap(),
            store: FileStore {
                dir: std::env::temp_dir(),
            },
        };
        assert_eq!(serve_once(&h).await, Served::NotConnected);
        // A profile but no screen picked yet (or no desktop.toml) is not linked either.
        let h = linked(&s);
        std::fs::remove_file(config::config_path(h.home.path())).unwrap();
        assert_eq!(serve_once(&h).await, Served::NotConnected);
        // A profile without its key.
        let h = linked(&s);
        h.store.delete(PROFILE).unwrap();
        assert_eq!(serve_once(&h).await, Served::NotConnected);
        assert!(s.received_requests().await.unwrap().is_empty());
    }

    /// A Keychain that holds a key but cannot be read (locked at login).
    struct LockedStore;
    impl KeyStore for LockedStore {
        fn put(&self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
        fn get(&self, _: &str) -> Result<String> {
            Err(QuomeError::CloudKey("keychain is locked".into()))
        }
        fn has(&self, _: &str) -> Result<bool> {
            Ok(true)
        }
        fn delete(&self, _: &str) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn an_unreadable_keychain_is_an_error_not_not_linked() {
        let s = MockServer::start().await;
        let h = linked(&s);
        let (_tx, rx) = watch::channel(false);
        let r = serve(h.home.path(), &LockedStore, Timing::default(), None, rx).await;
        assert!(matches!(r, Err(QuomeError::CloudKey(_))), "{r:?}");
    }

    #[tokio::test]
    async fn serve_writes_session_0600_in_0700_dir() {
        let s = MockServer::start().await;
        mount_auth(&s, token_ok()).await;
        Mock::given(method("POST"))
            .and(path(SESSION_PATH))
            .and(body_json(serde_json::json!({
                "os": "macos", "screen": {"w": 1920, "h": 1200}, "app_version": "0.0.0"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "desktop": "d-1", "name": "Studio",
                "url": "wss://desktops.apps.example.test/acme/v1/daemon",
                "key": KEY, "screen": {"w": 1920, "h": 1200}
            })))
            .expect(1)
            .mount(&s)
            .await;
        let h = linked(&s);
        let dir = files::runtime_dir(h.home.path());
        let file = dir.join("session.json");
        let (tx, rx) = watch::channel(false);
        let stopper = async {
            while !file.exists() {
                sleep(Duration::from_millis(10)).await;
            }
            tx.send(true).unwrap();
        };
        let (out, ()) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(
                serve(h.home.path(), &h.store, Timing::default(), None, rx),
                stopper
            )
        })
        .await
        .expect("a session is written and a stop ends serve");
        assert_eq!(out.unwrap(), Served::Shutdown);
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(v["v"], 1);
        assert_eq!(v["url"], "wss://desktops.apps.example.test/acme/v1/daemon");
        assert_eq!(v["desktop"], "d-1");
        assert_eq!(v["key"], KEY);
        assert_eq!(v["screen"], serde_json::json!({"w": 1920, "h": 1200}));
        assert!(chrono::DateTime::parse_from_rfc3339(v["written_at"].as_str().unwrap()).is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&dir), 0o700);
            assert_eq!(mode(&file), 0o600);
        }
    }

    #[tokio::test]
    async fn unlinked_404_removes_session_and_exits_0() {
        let s = MockServer::start().await;
        mount_auth(&s, token_ok()).await;
        mount_session(
            &s,
            ResponseTemplate::new(404)
                .set_body_json(serde_json::json!({"reason": "computer_not_found"})),
        )
        .await;
        let h = linked(&s);
        let file = seed_session(&h);
        let out = serve_once(&h).await;
        assert_eq!(out, Served::Unlinked);
        assert_eq!(serve_exit_code(&out), 0);
        assert!(!file.exists());

        // The feature turned off answers a bare 404: the same.
        let s = MockServer::start().await;
        mount_auth(&s, token_ok()).await;
        mount_session(
            &s,
            ResponseTemplate::new(404).set_body_json(serde_json::json!({"detail": "Not Found"})),
        )
        .await;
        let h = linked(&s);
        let file = seed_session(&h);
        assert_eq!(serve_once(&h).await, Served::Unlinked);
        assert!(!file.exists());
    }

    #[tokio::test]
    async fn revoked_token_exits_0() {
        for token in [
            ResponseTemplate::new(400).set_body_json(serde_json::json!({"error": "invalid_grant"})),
            ResponseTemplate::new(401)
                .set_body_json(serde_json::json!({"reason": "delegation_revoked"})),
        ] {
            let s = MockServer::start().await;
            mount_auth(&s, token).await;
            let h = linked(&s);
            let file = seed_session(&h);
            let out = serve_once(&h).await;
            assert_eq!(out, Served::Unlinked);
            assert_eq!(serve_exit_code(&out), 0);
            assert!(!file.exists());
        }
        // The session route itself saying the delegation is revoked.
        let s = MockServer::start().await;
        mount_auth(&s, token_ok()).await;
        mount_session(
            &s,
            ResponseTemplate::new(401)
                .set_body_json(serde_json::json!({"reason": "delegation_revoked"})),
        )
        .await;
        let h = linked(&s);
        let file = seed_session(&h);
        assert_eq!(serve_once(&h).await, Served::Unlinked);
        assert!(!file.exists());
    }

    #[tokio::test]
    async fn approver_not_admin_exits_0() {
        let s = MockServer::start().await;
        mount_auth(&s, token_ok()).await;
        mount_session(
            &s,
            ResponseTemplate::new(403)
                .set_body_json(serde_json::json!({"reason": "approver_not_admin"})),
        )
        .await;
        let h = linked(&s);
        let file = seed_session(&h);
        let out = serve_once(&h).await;
        assert_eq!(out, Served::Unlinked);
        assert_eq!(serve_exit_code(&out), 0);
        assert!(!file.exists());
    }

    #[tokio::test]
    async fn screen_is_fixed_exits_0_with_message() {
        let s = MockServer::start().await;
        mount_auth(&s, token_ok()).await;
        mount_session(
            &s,
            ResponseTemplate::new(409)
                .set_body_json(serde_json::json!({"reason": "screen_is_fixed"})),
        )
        .await;
        let h = linked(&s);
        let file = seed_session(&h);
        let out = serve_once(&h).await;
        assert_eq!(out, Served::ScreenFixed);
        assert!(!file.exists(), "the app must stop dialing with the old key");
        assert_eq!(serve_exit_code(&out), 0);
        assert_eq!(out.message(), Some(SCREEN_FIXED));
    }

    #[tokio::test(start_paused = true)]
    async fn other_refusals_are_transient() {
        let (f, link) = fake(
            |n| match n {
                0 => Err(refused(422, "screen_not_allowed")),
                1 => Err(refused(500, "")),
                n => Ok(good_session(n)),
            },
            Timing::default(),
            None,
        );
        let running = tokio::spawn(run(link));
        sleep(Duration::from_secs(10)).await;
        assert_eq!(f.calls.lock().unwrap().len(), 3);
        f.stop.send(true).unwrap();
        assert_eq!(running.await.unwrap(), Served::Shutdown);
    }
}
