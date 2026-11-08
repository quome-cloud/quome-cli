//! Client for the org's credentials service: a brokered, cached token and
//! calls to the service's `/_v1/*` routes.
#![allow(clippy::result_large_err)]

use super::api::{CloudApi, CredentialsToken, GIT_REQUEST_TIMEOUT};
use super::git::{Patience, Waiting};
use super::mcp::{is_session_error, Reconnect};
use crate::errors::QuomeError;
use reqwest::header::RETRY_AFTER;
use reqwest::{Method, StatusCode};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Refresh a brokered token this long before it expires.
const SKEW: Duration = Duration::from_secs(30);
/// Longest piece of an unexpected HTTP answer quoted in an error.
const SNIPPET: usize = 300;
/// What a caller is told when the wait ran out.
const STILL_STARTING: &str =
    "Your credentials service is still starting; try again in a few seconds.";

/// The current platform connection (it changes when `quome connect` is re-run).
pub type CredConn = Arc<dyn Fn() -> Arc<CloudApi> + Send + Sync>;
type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// What a call needs to do; picks the scopes asked of the broker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CredAccess {
    Use,
    Admin,
}

impl CredAccess {
    fn scopes(self) -> &'static [&'static str] {
        match self {
            CredAccess::Use => &["credentials_use"],
            CredAccess::Admin => &["credentials_use", "credentials_admin"],
        }
    }
}

/// Why a credentials call failed.
#[derive(Debug)]
pub enum CredError {
    /// The platform (or an unparsable service answer) failed the call.
    Api(QuomeError),
    /// The credentials service answered with an error envelope.
    Refused {
        code: String,
        message: String,
        /// From the `Retry-After` header, in seconds.
        retry_after: Option<u64>,
        /// The whole `error` object, so callers can read extra fields.
        detail: Value,
    },
}

impl Reconnect for CredError {
    fn needs_reconnect(&self) -> bool {
        match self {
            CredError::Api(QuomeError::ApiError(m)) => {
                m.starts_with("403")
                    && (m.contains("capability_missing") || m.contains("role_required"))
            }
            CredError::Api(e) => is_session_error(e),
            // Only a 403 carries these codes.
            CredError::Refused { code, .. } => {
                code == "capability_missing" || code == "role_required"
            }
        }
    }
    fn message(self) -> String {
        cred_hint(self)
    }
}

impl CredError {
    /// True while the platform is waking the service.
    fn is_starting(&self) -> bool {
        matches!(self, CredError::Api(QuomeError::RetryLater { message, .. })
            if message.contains("credentials_starting"))
    }

    fn retry_after(&self) -> Option<Duration> {
        match self {
            CredError::Api(QuomeError::RetryLater { retry_after, .. }) => {
                retry_after.map(Duration::from_secs)
            }
            _ => None,
        }
    }
}

/// The next step for a credentials error. Never includes a token.
pub fn cred_hint(e: CredError) -> String {
    let (code, text) = match e {
        CredError::Api(e) => {
            let m = e.to_string();
            let code = if m.contains("capability_missing") {
                "capability_missing"
            } else if m.contains("role_required") {
                "role_required"
            } else {
                return super::mcp::fail(e);
            };
            (code.to_string(), m)
        }
        CredError::Refused { code, message, .. } => {
            let text = if message.is_empty() {
                format!("The credentials service answered {code}.")
            } else {
                format!("The credentials service answered {code}: {message}")
            };
            (code, text)
        }
    };
    match code.as_str() {
        "capability_missing" => "Run `quome connect` again and allow Credentials access.".into(),
        "role_required" => "An org Owner or Admin must have connected this agent with Credentials admin access; ask one to run `quome connect` again and allow it.".into(),
        "grant_missing" => "Ask for access with credentials_request.".into(),
        _ => text,
    }
}

#[derive(Default)]
struct Cache {
    api: Option<Arc<CloudApi>>,
    tokens: HashMap<CredAccess, (CredentialsToken, Instant)>,
}

/// Client for the org's credentials service.
pub struct CredentialsClient {
    conn: CredConn,
    http: reqwest::Client,
    /// Held across a broker call so concurrent callers share one fetch.
    cache: Mutex<Cache>,
    clock: Clock,
    wait: Duration,
    retry_cap: Duration,
}

impl CredentialsClient {
    pub fn new(conn: CredConn) -> CredentialsClient {
        CredentialsClient {
            conn,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(super::auth::CONNECT_TIMEOUT)
                .timeout(GIT_REQUEST_TIMEOUT)
                .build()
                .unwrap_or_default(),
            cache: Mutex::default(),
            clock: Arc::new(Instant::now),
            wait: Duration::from_secs(60),
            retry_cap: Duration::from_secs(60),
        }
    }

    /// Replace the wait timing (tests).
    #[cfg(test)]
    fn with_timing(mut self, wait: Duration, retry_cap: Duration) -> Self {
        self.wait = wait;
        self.retry_cap = retry_cap;
        self
    }

    /// Replace the clock the token cache reads (tests).
    #[cfg(test)]
    fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    fn patience<'a>(&self, w: &'a dyn Waiting) -> Patience<'a> {
        Patience::new(self.wait, self.retry_cap, w)
    }

    /// GET a service path and return the JSON answer.
    pub async fn get(
        &self,
        access: CredAccess,
        path: &str,
        w: &dyn Waiting,
    ) -> Result<Value, CredError> {
        Ok(self
            .send(access, Method::GET, path, &Value::Null, w)
            .await?
            .1)
    }

    /// Send a request to a service path; any 2xx answer is a success.
    pub async fn send(
        &self,
        access: CredAccess,
        method: Method,
        path: &str,
        body: &Value,
        w: &dyn Waiting,
    ) -> Result<(StatusCode, Value), CredError> {
        let patience = self.patience(w);
        let mut reminted = false;
        loop {
            let api = (self.conn)();
            let token = match self.token(access, &api).await {
                Ok(t) => t,
                Err(e) if e.is_starting() => {
                    self.nap(&patience, &e).await?;
                    continue;
                }
                Err(e) => return Err(e),
            };
            let url = format!(
                "{}/{}",
                token.url.trim_end_matches('/'),
                path.trim_start_matches('/')
            );
            let mut b = self
                .http
                .request(method.clone(), &url)
                .bearer_auth(&token.access_token);
            if !body.is_null() {
                b = b.json(body);
            }
            let resp = b.send().await.map_err(|e| {
                CredError::Api(QuomeError::Usage(format!(
                    "The credentials service could not be reached: {}",
                    e.without_url()
                )))
            })?;
            let status = resp.status();
            let retry_after = resp
                .headers()
                .get(RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok());
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| CredError::Api(QuomeError::Usage(e.without_url().to_string())))?;
            let parsed: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            if status.is_success() {
                return Ok((status, parsed));
            }
            if status.as_u16() == 401 && !reminted {
                reminted = true;
                self.evict(access, &token.access_token).await;
                continue;
            }
            if parsed["error"].is_object() {
                let err = &parsed["error"];
                return Err(CredError::Refused {
                    code: err["code"].as_str().unwrap_or("").to_string(),
                    message: err["message"].as_str().unwrap_or("").to_string(),
                    retry_after,
                    detail: err.clone(),
                });
            }
            let detail = parsed["detail"].as_str().unwrap_or("");
            let reason = parsed["reason"].as_str().unwrap_or("");
            let text = if detail.is_empty() && reason.is_empty() {
                String::from_utf8_lossy(&bytes)
                    .trim()
                    .chars()
                    .take(SNIPPET)
                    .collect()
            } else {
                [detail, reason]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(": ")
            };
            return Err(CredError::Api(QuomeError::ApiError(
                format!("{} {text}", status.as_u16()).trim().to_string(),
            )));
        }
    }

    /// A token for plain git, same as a `Use` token.
    pub async fn git_token(&self) -> Result<CredentialsToken, CredError> {
        let patience = self.patience(&super::git::NoProgress);
        loop {
            let api = (self.conn)();
            match self.token(CredAccess::Use, &api).await {
                Err(e) if e.is_starting() => self.nap(&patience, &e).await?,
                other => return other,
            }
        }
    }

    async fn nap(&self, patience: &Patience<'_>, e: &CredError) -> Result<(), CredError> {
        if patience
            .nap(e.retry_after(), "Your credentials service is starting")
            .await
        {
            Ok(())
        } else {
            Err(CredError::Api(QuomeError::Usage(STILL_STARTING.into())))
        }
    }

    /// A valid token for `access` from `api`, brokered anew within 30 s of
    /// expiry. A different connection than the cached tokens came from drops them.
    async fn token(
        &self,
        access: CredAccess,
        api: &Arc<CloudApi>,
    ) -> Result<CredentialsToken, CredError> {
        let mut cache = self.cache.lock().await;
        if !cache.api.as_ref().is_some_and(|a| Arc::ptr_eq(a, api)) {
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
            .credentials_token(access.scopes())
            .await
            .map_err(CredError::Api)?;
        cache.tokens.insert(access, (t.clone(), fetched));
        Ok(t)
    }

    /// Drop the cached token for `access` if it is still `token`.
    async fn evict(&self, access: CredAccess, token: &str) {
        let mut cache = self.cache.lock().await;
        if matches!(cache.tokens.get(&access), Some((t, _)) if t.access_token == token) {
            cache.tokens.remove(&access);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::auth::tests::profile;
    use super::super::auth::TokenSource;
    use super::super::git::NoProgress;
    use super::super::keys::AgentKey;
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ORG: &str = "11111111-1111-1111-1111-111111111111";

    async fn setup() -> (MockServer, CredentialsClient, Arc<AtomicU64>) {
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
        let p = profile(&s.uri());
        let api = Arc::new(CloudApi::new(
            &p,
            TokenSource::new(&p, AgentKey::generate(), "vrc".into()),
        ));
        let offset = Arc::new(AtomicU64::new(0));
        let base = Instant::now();
        let o = offset.clone();
        let c = CredentialsClient::new(Arc::new(move || api.clone()))
            .with_timing(Duration::from_secs(5), Duration::from_millis(5))
            .with_clock(Arc::new(move || {
                base + Duration::from_millis(o.load(Ordering::SeqCst))
            }));
        (s, c, offset)
    }

    fn broker() -> String {
        format!("/api/v1/orgs/{ORG}/credentials/token")
    }

    async fn mount_broker(s: &MockServer, tok: &str, expires_in: u64, times: u64) {
        Mock::given(method("POST"))
            .and(path(broker()))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"access_token": tok, "expires_in": expires_in, "url": format!("{}/acme/", s.uri())}),
            ))
            .expect(times)
            .mount(s)
            .await;
    }

    #[tokio::test]
    async fn token_route_and_scope_body() {
        let (s, c, _) = setup().await;
        Mock::given(method("POST"))
            .and(path(broker()))
            .and(header("authorization", "Bearer AT"))
            .and(body_json(json!({"scope": ["credentials_use"]})))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"access_token": "C1", "expires_in": 300, "url": format!("{}/acme", s.uri())}),
            ))
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/acme/_v1/routes"))
            .and(header("authorization", "Bearer C1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"routes": []})))
            .expect(1)
            .mount(&s)
            .await;
        let v = c
            .get(CredAccess::Use, "/_v1/routes", &NoProgress)
            .await
            .unwrap();
        assert_eq!(v, json!({"routes": []}));
    }

    #[tokio::test]
    async fn admin_scope_body() {
        let (s, c, _) = setup().await;
        Mock::given(method("POST"))
            .and(path(broker()))
            .and(body_json(
                json!({"scope": ["credentials_use", "credentials_admin"]}),
            ))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({"access_token": "C1", "expires_in": 300, "url": s.uri()}),
                ),
            )
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/_v1/x"))
            .and(body_json(json!({"a": 1})))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({"ok": true})))
            .mount(&s)
            .await;
        let (st, v) = c
            .send(
                CredAccess::Admin,
                Method::POST,
                "_v1/x",
                &json!({"a": 1}),
                &NoProgress,
            )
            .await
            .unwrap();
        assert_eq!((st.as_u16(), v), (202, json!({"ok": true})));
    }

    #[tokio::test]
    async fn token_cached_until_skew() {
        let (s, c, offset) = setup().await;
        mount_broker(&s, "C1", 100, 2).await;
        Mock::given(method("GET"))
            .and(path("/acme/_v1/routes"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&s)
            .await;
        c.get(CredAccess::Use, "/_v1/routes", &NoProgress)
            .await
            .unwrap();
        offset.store(69_000, Ordering::SeqCst);
        c.get(CredAccess::Use, "/_v1/routes", &NoProgress)
            .await
            .unwrap();
        offset.store(71_000, Ordering::SeqCst);
        c.get(CredAccess::Use, "/_v1/routes", &NoProgress)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn null_expiry_is_not_cached() {
        let (s, c, _) = setup().await;
        Mock::given(method("POST"))
            .and(path(broker()))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({"access_token": "C1", "expires_in": null, "url": s.uri()}),
                ),
            )
            .expect(2)
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/_v1/routes"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&s)
            .await;
        for _ in 0..2 {
            c.get(CredAccess::Use, "/_v1/routes", &NoProgress)
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn waits_through_starting() {
        struct Count(AtomicU64);
        impl Waiting for Count {
            fn note<'a>(
                &'a self,
                _m: &'a str,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Box::pin(async {})
            }
        }
        let (s, c, _) = setup().await;
        Mock::given(method("POST"))
            .and(path(broker()))
            .respond_with(
                ResponseTemplate::new(503)
                    .insert_header("Retry-After", "1")
                    .set_body_json(json!({"detail":"unavailable","reason":"credentials_starting"})),
            )
            .up_to_n_times(1)
            .mount(&s)
            .await;
        mount_broker(&s, "C1", 300, 1).await;
        Mock::given(method("GET"))
            .and(path("/acme/_v1/routes"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"n": 1})))
            .mount(&s)
            .await;
        let w = Count(AtomicU64::new(0));
        let v = c.get(CredAccess::Use, "/_v1/routes", &w).await.unwrap();
        assert_eq!(v, json!({"n": 1}));
        assert_eq!(w.0.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn refusal_envelope_is_parsed() {
        let (s, c, _) = setup().await;
        mount_broker(&s, "C1", 300, 1).await;
        Mock::given(method("GET"))
            .and(path("/acme/_v1/routes"))
            .respond_with(
                ResponseTemplate::new(403)
                    .insert_header("Retry-After", "7")
                    .set_body_json(json!({"error": {"code": "grant_missing", "message": "no grant", "route": "openai", "requestable": true}})),
            )
            .mount(&s)
            .await;
        let e = c
            .get(CredAccess::Use, "/_v1/routes", &NoProgress)
            .await
            .unwrap_err();
        match &e {
            CredError::Refused {
                code,
                retry_after,
                detail,
                ..
            } => {
                assert_eq!(code, "grant_missing");
                assert_eq!(*retry_after, Some(7));
                assert_eq!(detail["route"], "openai");
                assert_eq!(detail["requestable"], true);
            }
            other => panic!("{other:?}"),
        }
        assert!(!e.needs_reconnect());
        assert_eq!(e.message(), "Ask for access with credentials_request.");
    }

    #[tokio::test]
    async fn platform_403_reconnects_and_hints() {
        let (s, c, _) = setup().await;
        Mock::given(method("POST"))
            .and(path(broker()))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_json(json!({"detail":"forbidden","reason":"capability_missing"})),
            )
            .mount(&s)
            .await;
        let e = c
            .get(CredAccess::Use, "/_v1/routes", &NoProgress)
            .await
            .unwrap_err();
        assert!(e.needs_reconnect());
        assert_eq!(
            e.message(),
            "Run `quome connect` again and allow Credentials access."
        );
    }

    #[test]
    fn service_envelope_reconnect_codes() {
        let r = |code: &str| CredError::Refused {
            code: code.into(),
            message: "m".into(),
            retry_after: None,
            detail: json!({}),
        };
        assert!(r("capability_missing").needs_reconnect());
        assert!(r("role_required").needs_reconnect());
        assert!(!r("other").needs_reconnect());
        assert!(r("role_required").message().contains("Owner or Admin"));
        assert_eq!(
            r("other").message(),
            "The credentials service answered other: m"
        );
    }

    #[tokio::test]
    async fn non_envelope_error_is_api() {
        let (s, c, _) = setup().await;
        mount_broker(&s, "C1", 300, 1).await;
        Mock::given(method("GET"))
            .and(path("/acme/_v1/routes"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&s)
            .await;
        let e = c
            .get(CredAccess::Use, "/_v1/routes", &NoProgress)
            .await
            .unwrap_err();
        assert!(matches!(e, CredError::Api(QuomeError::ApiError(m)) if m == "500 boom"));
    }

    #[tokio::test]
    async fn git_token_returns_use_token() {
        let (s, c, _) = setup().await;
        mount_broker(&s, "C9", 300, 1).await;
        let t = c.git_token().await.unwrap();
        assert_eq!(t.access_token, "C9");
    }

    #[test]
    fn token_debug_is_redacted() {
        let t = CredentialsToken {
            access_token: "SECRET".into(),
            expires_in: 5,
            url: "https://x.test/o".into(),
        };
        assert!(!format!("{t:?}").contains("SECRET"));
    }
}
