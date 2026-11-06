//! Calls to the org's git server: a short-lived token brokered by the
//! platform, sent as HTTP Basic auth, with waiting while the server starts.
// `GitError::Server` carries the whole answer body on purpose; errors are rare.
#![allow(clippy::result_large_err)]

use super::api::{is_timeout, CloudApi, GitToken, GIT_REQUEST_TIMEOUT};
use super::auth::CONNECT_TIMEOUT;
use super::mcp::{fail, is_session_error};
use super::state::Profile;
use crate::errors::QuomeError;
use bytes::Bytes;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::Method;
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Refresh a brokered token this long before it expires.
const SKEW: Duration = Duration::from_secs(30);
/// Longest piece of an unexpected HTTP answer quoted in an error.
const SNIPPET: usize = 300;
/// Pause between attempts when the server names no `Retry-After`.
const DEFAULT_PAUSE: Duration = Duration::from_secs(5);
/// What a caller is told when the wait ran out.
const STILL_STARTING: &str = "Your Git server is still starting; try again in a few seconds.";

/// The current platform connection and profile (they change when
/// `quome connect` is re-run).
pub type GitConn = Arc<dyn Fn() -> (Arc<CloudApi>, Arc<Profile>) + Send + Sync>;
type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// How long to wait for the server to start, and the longest single pause.
#[derive(Debug, Clone, Copy)]
pub struct GitTiming {
    pub wait: Duration,
    pub retry_cap: Duration,
}

impl Default for GitTiming {
    fn default() -> Self {
        GitTiming {
            wait: Duration::from_secs(60),
            retry_cap: Duration::from_secs(60),
        }
    }
}

/// What a call needs to do on the server; picks the scopes asked of the broker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Access {
    Read,
    Write,
}

impl Access {
    fn scopes(self) -> &'static [&'static str] {
        match self {
            Access::Read => &["repos_read"],
            Access::Write => &["repos_read", "repos_write"],
        }
    }
}

/// Told when a call is waiting for the server to start.
pub trait Waiting: Sync {
    fn note<'a>(&'a self, message: &'a str) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

/// A `Waiting` that says nothing.
pub struct NoProgress;

impl Waiting for NoProgress {
    fn note<'a>(&'a self, _message: &'a str) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
}

/// Why a git call failed.
#[derive(Debug)]
pub enum GitError {
    /// The platform (broker) refused; a reconnect may fix some of these.
    Api(QuomeError),
    /// The git server answered with an error.
    Server {
        status: u16,
        code: String,
        message: String,
        /// The whole answer body, for callers that read more than the envelope.
        body: Value,
        retry_after: Option<Duration>,
    },
    /// The git server could not be reached.
    Unreachable(String),
    /// A problem with the request or the wait; the message says what to do.
    Usage(String),
}

impl GitError {
    /// True when re-reading the stored connection may help: the delegation was
    /// revoked or is invalid, or it lacks the Git scopes or role (a re-run of
    /// `quome connect` fixes that).
    pub fn needs_reconnect(&self) -> bool {
        match self {
            GitError::Api(QuomeError::ApiError(m)) => {
                m.starts_with("403")
                    && (m.contains("capability_missing") || m.contains("role_required"))
            }
            GitError::Api(e) => is_session_error(e),
            _ => false,
        }
    }

    /// The message for the agent, with the next step.
    pub fn message(self) -> String {
        match self {
            GitError::Api(e) => git_hint(&fail(e)),
            GitError::Server {
                status,
                code,
                message,
                ..
            } => {
                git_hint(format!("The org's git server answered {status} {code}: {message}").trim())
            }
            GitError::Unreachable(m) | GitError::Usage(m) => m,
        }
    }

    /// True when the server is starting or briefly unavailable.
    fn is_starting(&self) -> bool {
        match self {
            GitError::Api(QuomeError::RetryLater { message, .. }) => {
                message.contains("git_starting") || message.contains("git_unavailable")
            }
            GitError::Api(e) => is_timeout(e),
            GitError::Server { status, code, .. } => *status == 503 && code == "auth_unavailable",
            _ => false,
        }
    }

    fn retry_after(&self) -> Option<Duration> {
        match self {
            GitError::Api(QuomeError::RetryLater { retry_after, .. }) => {
                retry_after.map(Duration::from_secs)
            }
            GitError::Server { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

/// The next step for a git error.
pub fn git_hint(e: &str) -> String {
    let has = |r: &str| e.contains(r);
    if has("403") && (has("capability_missing") || has("role_required")) {
        "Run `quome connect` again and allow Git access.".into()
    } else if has("not_a_member") {
        "You are not a member of this org.".into()
    } else if has("broker_refused") {
        "Quome Cloud could not issue a Git token; try again in a few seconds.".into()
    } else if has("image_unpinned") {
        "Git is not available for this org yet.".into()
    } else {
        e.to_string()
    }
}

/// `https://git.{apps_domain}/{org_slug}/-/v1`, the org's git server API.
pub fn server_base(p: &Profile) -> String {
    format!("https://{}/{}/-/v1", git_host(p), p.org_slug)
}

/// `git.{apps_domain}`, the org's git server host.
pub fn git_host(p: &Profile) -> String {
    format!("git.{}", p.apps_domain)
}

/// A request to the git server.
pub(crate) struct ServerReq {
    pub method: Method,
    /// After the base, such as `/repos/acme/api/tree`.
    pub path: String,
    pub query: Vec<(&'static str, String)>,
    pub body: Option<Value>,
}

/// A 2xx answer from the git server.
pub(crate) struct ServerAnswer {
    pub status: u16,
    pub headers: HeaderMap,
    pub bytes: Bytes,
}

#[derive(Default)]
struct Cache {
    /// The connection the cached tokens were brokered through.
    api: Option<Arc<CloudApi>>,
    /// Token and when it was fetched, per access level.
    tokens: HashMap<Access, (GitToken, Instant)>,
}

/// Client for the org's git server.
pub struct GitClient {
    conn: GitConn,
    http: reqwest::Client,
    /// Held across a broker call so concurrent callers share one fetch.
    cache: Mutex<Cache>,
    clock: Clock,
    timing: GitTiming,
    base: Option<String>,
}

/// One call's budget for waiting out a start-up.
struct Patience<'a> {
    deadline: tokio::time::Instant,
    cap: Duration,
    w: &'a dyn Waiting,
}

impl Patience<'_> {
    /// Pause before the next attempt if `e` is a start-up error and time is
    /// left; otherwise return the error (a `Usage` when the time ran out).
    async fn pause(&self, e: GitError) -> Result<(), GitError> {
        if !e.is_starting() {
            return Err(e);
        }
        let left = self
            .deadline
            .saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return Err(GitError::Usage(STILL_STARTING.into()));
        }
        let pause = e
            .retry_after()
            .unwrap_or(DEFAULT_PAUSE)
            .min(self.cap)
            .min(left);
        self.w.note("Your Git server is starting").await;
        tokio::time::sleep(pause).await;
        Ok(())
    }
}

impl GitClient {
    pub fn new(conn: GitConn) -> GitClient {
        GitClient {
            conn,
            http: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(GIT_REQUEST_TIMEOUT)
                .build()
                .unwrap_or_default(),
            cache: Mutex::default(),
            clock: Arc::new(Instant::now),
            timing: GitTiming::default(),
            base: None,
        }
    }

    /// Send server requests to `base` instead of the org's git host (an override).
    pub(crate) fn with_server_base(mut self, base: String) -> Self {
        self.base = Some(base);
        self
    }

    /// Replace the wait timing (tests).
    #[cfg(test)]
    pub(crate) fn with_timing(mut self, t: GitTiming) -> Self {
        self.timing = t;
        self
    }

    /// Replace the clock the token cache reads (tests).
    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    fn patience<'a>(&self, w: &'a dyn Waiting) -> Patience<'a> {
        Patience {
            deadline: tokio::time::Instant::now() + self.timing.wait,
            cap: self.timing.retry_cap,
            w,
        }
    }

    /// Run a platform call, waiting while the git server starts.
    pub(crate) async fn platform<T, F, Fut>(&self, w: &dyn Waiting, f: F) -> Result<T, GitError>
    where
        F: Fn(Arc<CloudApi>) -> Fut,
        Fut: Future<Output = Result<T, QuomeError>>,
    {
        let patience = self.patience(w);
        loop {
            let (api, _) = (self.conn)();
            match f(api).await {
                Ok(v) => return Ok(v),
                Err(e) => patience.pause(GitError::Api(e)).await?,
            }
        }
    }

    /// Send `req` to the git server with a brokered token. A 401, or an answer
    /// that did not come from the server (an edge miss), gets one re-mint and
    /// resend per call; a starting server is waited for.
    pub(crate) async fn server(
        &self,
        access: Access,
        req: ServerReq,
        w: &dyn Waiting,
    ) -> Result<ServerAnswer, GitError> {
        let patience = self.patience(w);
        let mut reminted = false;
        loop {
            let token = match self.token(access).await {
                Ok(t) => t,
                Err(e) => {
                    patience.pause(e).await?;
                    continue;
                }
            };
            let url = format!("{}{}", self.base_url(), req.path);
            let mut b = self
                .http
                .request(req.method.clone(), &url)
                .basic_auth("quome", Some(&token.access_token));
            if !req.query.is_empty() {
                b = b.query(&req.query);
            }
            if let Some(body) = &req.body {
                b = b.json(body);
            }
            let resp = b.send().await.map_err(unreachable)?;
            let status = resp.status().as_u16();
            let headers = resp.headers().clone();
            let bytes = resp.bytes().await.map_err(unreachable)?;
            if (200..300).contains(&status) {
                return Ok(ServerAnswer {
                    status,
                    headers,
                    bytes,
                });
            }
            let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            let enveloped = body["error"].is_object();
            let edge_miss = !enveloped && matches!(status, 404 | 502 | 503 | 504);
            if (status == 401 || edge_miss) && !reminted {
                reminted = true;
                self.evict(access, &token.access_token).await;
                continue;
            }
            let (code, message) = if enveloped {
                (
                    body["error"]["code"].as_str().unwrap_or("").to_string(),
                    body["error"]["message"].as_str().unwrap_or("").to_string(),
                )
            } else {
                let text = String::from_utf8_lossy(&bytes);
                (String::new(), text.trim().chars().take(SNIPPET).collect())
            };
            let retry_after = headers
                .get(RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(Duration::from_secs);
            let e = GitError::Server {
                status,
                code,
                message,
                body,
                retry_after,
            };
            patience.pause(e).await?;
        }
    }

    fn base_url(&self) -> String {
        match &self.base {
            Some(b) => b.clone(),
            None => server_base(&(self.conn)().1),
        }
    }

    /// A valid token for `access`, brokered anew within 30 s of expiry.
    async fn token(&self, access: Access) -> Result<GitToken, GitError> {
        let (api, _) = (self.conn)();
        let mut cache = self.cache.lock().await;
        if !cache.api.as_ref().is_some_and(|a| Arc::ptr_eq(a, &api)) {
            cache.tokens.clear();
            cache.api = Some(api.clone());
        }
        if let Some((t, fetched)) = cache.tokens.get(&access) {
            let ttl = Duration::from_secs(t.expires_in).saturating_sub(SKEW);
            if (self.clock)() < *fetched + ttl {
                return Ok(t.clone());
            }
        }
        let fetched = (self.clock)();
        let t = api
            .git_token(access.scopes())
            .await
            .map_err(GitError::Api)?;
        cache.tokens.insert(access, (t.clone(), fetched));
        Ok(t)
    }

    /// Drop the cached token for `access` if it is still `token`.
    async fn evict(&self, access: Access, token: &str) {
        let mut cache = self.cache.lock().await;
        if matches!(cache.tokens.get(&access), Some((t, _)) if t.access_token == token) {
            cache.tokens.remove(&access);
        }
    }
}

fn unreachable(e: reqwest::Error) -> GitError {
    GitError::Unreachable(format!("The org's git server could not be reached: {e}"))
}

#[cfg(test)]
mod tests {
    use super::super::auth::tests::profile;
    use super::super::auth::TokenSource;
    use super::super::keys::AgentKey;
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ORG: &str = "11111111-1111-1111-1111-111111111111";

    /// A mock platform (which also serves the git server), the client, and
    /// the milliseconds its clock is moved forward by.
    async fn setup() -> (MockServer, GitClient, Arc<AtomicU64>) {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"token_endpoint": format!("{}/oauth/token", s.uri())})),
            )
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"access_token":"AT","token_type":"Bearer","expires_in":3600}),
            ))
            .mount(&s)
            .await;
        let p = Arc::new(profile(&s.uri()));
        let api = Arc::new(CloudApi::new(
            &p,
            TokenSource::new(&p, AgentKey::generate(), "vrc".into()),
        ));
        let offset = Arc::new(AtomicU64::new(0));
        let base = Instant::now();
        let o = offset.clone();
        let git = GitClient::new(Arc::new(move || (api.clone(), p.clone())))
            .with_server_base(format!("{}/acme/-/v1", s.uri()))
            .with_timing(GitTiming {
                wait: Duration::from_millis(300),
                retry_cap: Duration::from_millis(5),
            })
            .with_clock(Arc::new(move || {
                base + Duration::from_millis(o.load(Ordering::SeqCst))
            }));
        (s, git, offset)
    }

    fn broker_path() -> String {
        format!("/api/v1/orgs/{ORG}/git/token")
    }

    fn token_body(tok: &str, expires_in: u64) -> serde_json::Value {
        json!({"access_token": tok, "token_type": "Bearer", "expires_in": expires_in})
    }

    async fn mount_broker(s: &MockServer, tok: &str, expires_in: u64, times: u64) {
        Mock::given(method("POST"))
            .and(path(broker_path()))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_body(tok, expires_in)))
            .expect(times)
            .mount(s)
            .await;
    }

    fn refs() -> ServerReq {
        ServerReq {
            method: Method::GET,
            path: "/repos/acme/api/refs".into(),
            query: vec![],
            body: None,
        }
    }

    const REFS_PATH: &str = "/acme/-/v1/repos/acme/api/refs";

    fn ok() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({"refs": []}))
    }

    #[tokio::test]
    async fn server_sends_basic_auth() {
        let (s, git, _) = setup().await;
        mount_broker(&s, "G1", 300, 1).await;
        Mock::given(method("GET"))
            .and(path(REFS_PATH))
            .and(header("authorization", "Basic cXVvbWU6RzE="))
            .respond_with(ok())
            .expect(1)
            .mount(&s)
            .await;
        let a = git.server(Access::Read, refs(), &NoProgress).await.unwrap();
        assert_eq!(a.status, 200);
    }

    #[tokio::test]
    async fn token_cached_until_30s_before_expiry() {
        let (s, git, offset) = setup().await;
        Mock::given(method("POST"))
            .and(path(broker_path()))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_body("G1", 300)))
            .expect(2)
            .mount(&s)
            .await;
        git.token(Access::Read).await.unwrap();
        git.token(Access::Read).await.unwrap();
        offset.store(271_000, Ordering::SeqCst);
        git.token(Access::Read).await.unwrap();
    }

    #[tokio::test]
    async fn read_and_write_scopes_are_cached_separately() {
        let (s, git, _) = setup().await;
        Mock::given(method("POST"))
            .and(path(broker_path()))
            .and(body_json(json!({"scope": ["repos_read"]})))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_body("R", 300)))
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path(broker_path()))
            .and(body_json(json!({"scope": ["repos_read", "repos_write"]})))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_body("W", 300)))
            .expect(1)
            .mount(&s)
            .await;
        for a in [Access::Read, Access::Write, Access::Read, Access::Write] {
            git.token(a).await.unwrap();
        }
        assert_eq!(git.token(Access::Write).await.unwrap().access_token, "W");
    }

    #[tokio::test]
    async fn unauthorized_remints_once() {
        let (s, git, _) = setup().await;
        mount_broker(&s, "G1", 300, 2).await;
        Mock::given(method("GET"))
            .and(path(REFS_PATH))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_json(json!({"error": {"code": "invalid_token", "message": "no"}})),
            )
            .expect(2)
            .mount(&s)
            .await;
        match git.server(Access::Read, refs(), &NoProgress).await {
            Err(GitError::Server { status: 401, .. }) => {}
            other => panic!("{:?}", other.err()),
        }
    }

    #[tokio::test]
    async fn edge_miss_remints_and_retries_once() {
        let (s, git, _) = setup().await;
        mount_broker(&s, "G1", 300, 2).await;
        Mock::given(method("GET"))
            .and(path(REFS_PATH))
            .respond_with(ResponseTemplate::new(502).set_body_string("Bad Gateway"))
            .up_to_n_times(1)
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path(REFS_PATH))
            .respond_with(ok())
            .mount(&s)
            .await;
        git.server(Access::Read, refs(), &NoProgress).await.unwrap();
    }

    #[tokio::test]
    async fn enveloped_404_is_not_retried() {
        let (s, git, _) = setup().await;
        mount_broker(&s, "G1", 300, 1).await;
        Mock::given(method("GET"))
            .and(path(REFS_PATH))
            .respond_with(
                ResponseTemplate::new(404)
                    .set_body_json(json!({"error": {"code": "not_found", "message": "no repo"}})),
            )
            .expect(1)
            .mount(&s)
            .await;
        match git.server(Access::Read, refs(), &NoProgress).await {
            Err(GitError::Server {
                status: 404, code, ..
            }) => assert_eq!(code, "not_found"),
            other => panic!("{:?}", other.err()),
        }
    }

    #[tokio::test]
    async fn waits_through_git_starting() {
        let (s, git, _) = setup().await;
        Mock::given(method("POST"))
            .and(path(broker_path()))
            .respond_with(
                ResponseTemplate::new(503)
                    .insert_header("retry-after", "5")
                    .set_body_json(json!({"error": "unavailable", "reason": "git_starting"})),
            )
            .up_to_n_times(2)
            .expect(2)
            .mount(&s)
            .await;
        mount_broker(&s, "G1", 300, 1).await;
        Mock::given(method("GET"))
            .and(path(REFS_PATH))
            .respond_with(ok())
            .mount(&s)
            .await;
        git.server(Access::Read, refs(), &NoProgress).await.unwrap();
    }

    #[tokio::test]
    async fn waits_through_auth_unavailable() {
        let (s, git, _) = setup().await;
        mount_broker(&s, "G1", 300, 1).await;
        Mock::given(method("GET"))
            .and(path(REFS_PATH))
            .respond_with(ResponseTemplate::new(503).set_body_json(
                json!({"error": {"code": "auth_unavailable", "message": "starting"}}),
            ))
            .up_to_n_times(1)
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path(REFS_PATH))
            .respond_with(ok())
            .mount(&s)
            .await;
        git.server(Access::Read, refs(), &NoProgress).await.unwrap();
    }

    #[tokio::test]
    async fn gives_up_after_wait() {
        let (s, git, _) = setup().await;
        Mock::given(method("POST"))
            .and(path(broker_path()))
            .respond_with(
                ResponseTemplate::new(503)
                    .set_body_json(json!({"error": "unavailable", "reason": "git_starting"})),
            )
            .mount(&s)
            .await;
        let e = git
            .server(Access::Read, refs(), &NoProgress)
            .await
            .err()
            .unwrap();
        assert_eq!(
            e.message(),
            "Your Git server is still starting; try again in a few seconds."
        );
    }

    #[tokio::test]
    async fn platform_waits_and_notes() {
        struct Count(AtomicU64);
        impl Waiting for Count {
            fn note<'a>(&'a self, m: &'a str) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
                assert_eq!(m, "Your Git server is starting");
                self.0.fetch_add(1, Ordering::SeqCst);
                Box::pin(async {})
            }
        }
        let (s, git, _) = setup().await;
        Mock::given(method("POST"))
            .and(path(broker_path()))
            .respond_with(
                ResponseTemplate::new(503)
                    .set_body_json(json!({"error": "unavailable", "reason": "git_unavailable"})),
            )
            .up_to_n_times(1)
            .mount(&s)
            .await;
        mount_broker(&s, "G1", 300, 1).await;
        let w = Count(AtomicU64::new(0));
        let t = git
            .platform(
                &w,
                |api| async move { api.git_token(&["repos_read"]).await },
            )
            .await
            .unwrap();
        assert_eq!(t.access_token, "G1");
        assert_eq!(w.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn needs_reconnect_on_capability_missing() {
        let e = GitError::Api(QuomeError::ApiError(
            "403 forbidden: capability_missing".into(),
        ));
        assert!(e.needs_reconnect());
        assert_eq!(
            e.message(),
            "Run `quome connect` again and allow Git access."
        );
        assert!(
            !GitError::Api(QuomeError::ApiError("403 forbidden: not_a_member".into()))
                .needs_reconnect()
        );
    }

    #[test]
    fn hints() {
        assert_eq!(
            git_hint("API error: 403 forbidden: not_a_member"),
            "You are not a member of this org."
        );
        assert!(git_hint("502 broker_refused").contains("could not issue"));
        assert!(git_hint("image_unpinned").contains("not available"));
        assert_eq!(git_hint("boom"), "boom");
    }

    #[test]
    fn server_base_and_host() {
        let mut p = profile("https://api.example");
        p.apps_domain = "apps.example".into();
        p.org_slug = "acme".into();
        assert_eq!(server_base(&p), "https://git.apps.example/acme/-/v1");
        assert_eq!(git_host(&p), "git.apps.example");
    }
}
