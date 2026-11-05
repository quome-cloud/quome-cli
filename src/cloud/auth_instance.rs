//! Calls to the org's auth instance: a short-lived token brokered by the
//! platform, then one stateless JSON-RPC `tools/call` to the instance's MCP.
use super::api::{AuthToken, CloudApi};
use super::auth::{CONNECT_TIMEOUT, REQUEST_TIMEOUT};
use super::mcp::{fail, is_session_error};
use crate::errors::QuomeError;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// The MCP protocol version sent to the instance.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// Refresh a brokered token this long before it expires.
const SKEW: Duration = Duration::from_secs(30);
/// Longest piece of an unexpected HTTP answer quoted in an error.
const SNIPPET: usize = 300;

/// The current platform connection (it changes when `quome connect` is re-run).
pub type ApiSource = Arc<dyn Fn() -> Arc<CloudApi> + Send + Sync>;
type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// What a call needs to do on the instance; picks the scopes asked of the broker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Read,
    Write,
}

impl Scope {
    fn scopes(self, env: &str) -> &'static [&'static str] {
        match (self, env) {
            (Scope::Read, _) => &["auth_read"],
            (Scope::Write, "prod") => &["auth_read", "auth_prod_write"],
            (Scope::Write, _) => &["auth_read", "auth_sandbox_write"],
        }
    }
}

/// Why an auth call failed: the platform (broker) refused, or the instance did.
#[derive(Debug)]
pub enum AuthCallError {
    /// The broker call failed; a reconnect may fix some of these.
    Api(QuomeError),
    /// The instance (or this client) failed; the message already has its hint.
    Instance(String),
}

impl From<String> for AuthCallError {
    fn from(m: String) -> Self {
        AuthCallError::Instance(m)
    }
}

impl AuthCallError {
    /// True when re-reading the stored connection may help: the delegation was
    /// revoked or is invalid, or it lacks the auth scopes or role (a re-run of
    /// `quome connect` fixes that).
    pub fn needs_reconnect(&self) -> bool {
        match self {
            AuthCallError::Api(QuomeError::ApiError(m)) => {
                m.starts_with("403")
                    && (m.contains("capability_missing") || m.contains("role_required"))
            }
            AuthCallError::Api(e) => is_session_error(e),
            AuthCallError::Instance(_) => false,
        }
    }

    /// The message for the agent, with the next step.
    pub fn message(self) -> String {
        match self {
            AuthCallError::Api(e) => auth_hint(&fail(e)),
            AuthCallError::Instance(m) => m,
        }
    }
}

#[derive(Default)]
struct Cache {
    /// The connection the cached tokens were brokered through.
    api: Option<Arc<CloudApi>>,
    /// Token and when it was fetched, per environment and scope.
    tokens: HashMap<(String, Scope), (AuthToken, Instant)>,
}

/// Client for the org's auth instances (one per environment).
pub struct AuthInstances {
    api: ApiSource,
    http: reqwest::Client,
    /// Held across a broker call so concurrent callers share one fetch.
    cache: Mutex<Cache>,
    next_id: AtomicU64,
    clock: Clock,
}

impl AuthInstances {
    pub fn new(api: ApiSource) -> AuthInstances {
        AuthInstances {
            api,
            http: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .build()
                .unwrap_or_default(),
            cache: Mutex::default(),
            next_id: AtomicU64::new(0),
            clock: Arc::new(Instant::now),
        }
    }

    /// Replace the clock the token cache reads (tests).
    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: Clock) -> AuthInstances {
        self.clock = clock;
        self
    }

    /// The instance's issuer URL for `env`.
    pub async fn issuer(&self, env: &str) -> Result<String, AuthCallError> {
        check_env(env)?;
        {
            let api = (self.api)();
            let cache = self.cache.lock().await;
            if cache.api.as_ref().is_some_and(|a| Arc::ptr_eq(a, &api)) {
                let cached = cache.tokens.iter().find(|((e, _), _)| e == env);
                if let Some((_, (t, _))) = cached {
                    return Ok(t.issuer.clone());
                }
            }
        }
        Ok(self.token(env, Scope::Read).await?.issuer)
    }

    /// Call `tool` on the instance for `env`. Returns the result's
    /// `structuredContent`, else its text parsed as JSON, else `{"text": …}`.
    /// A 401 from the instance drops the cached token and retries once.
    pub async fn call(
        &self,
        env: &str,
        scope: Scope,
        tool: &str,
        args: Value,
    ) -> Result<Value, AuthCallError> {
        check_env(env)?;
        let mut retried = false;
        loop {
            let t = self.token(env, scope).await?;
            let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
            let body = json!({
                "jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": {"name": tool, "arguments": args},
            });
            let resp = self
                .http
                .post(&t.mcp_url)
                .bearer_auth(&t.access_token)
                .header("MCP-Protocol-Version", PROTOCOL_VERSION)
                .header(
                    reqwest::header::ACCEPT,
                    "application/json, text/event-stream",
                )
                .json(&body)
                .send()
                .await
                .map_err(instance_unreachable)?;
            let status = resp.status().as_u16();
            if status == 401 && !retried {
                retried = true;
                self.evict(env, scope, &t.access_token).await;
                continue;
            }
            let text = resp.text().await.map_err(instance_unreachable)?;
            if !(200..300).contains(&status) {
                let quoted: String = text.trim().chars().take(SNIPPET).collect();
                return Err(AuthCallError::Instance(auth_hint(
                    format!("The org's auth instance answered {status}: {quoted}").trim(),
                )));
            }
            return Ok(parse_answer(&text)?);
        }
    }

    /// A valid token for `env` and `scope`, brokered anew within 30 s of expiry.
    async fn token(&self, env: &str, scope: Scope) -> Result<AuthToken, AuthCallError> {
        let api = (self.api)();
        let mut cache = self.cache.lock().await;
        if !cache.api.as_ref().is_some_and(|a| Arc::ptr_eq(a, &api)) {
            cache.tokens.clear();
            cache.api = Some(api.clone());
        }
        let key = (env.to_string(), scope);
        if let Some((t, fetched)) = cache.tokens.get(&key) {
            let ttl = Duration::from_secs(t.expires_in).saturating_sub(SKEW);
            if (self.clock)() < *fetched + ttl {
                return Ok(t.clone());
            }
        }
        let fetched = (self.clock)();
        let t = api
            .auth_token(env, scope.scopes(env))
            .await
            .map_err(AuthCallError::Api)?;
        cache.tokens.insert(key, (t.clone(), fetched));
        Ok(t)
    }

    /// Drop the cached token for `env` and `scope` if it is still `token`.
    async fn evict(&self, env: &str, scope: Scope, token: &str) {
        let mut cache = self.cache.lock().await;
        let key = (env.to_string(), scope);
        if matches!(cache.tokens.get(&key), Some((t, _)) if t.access_token == token) {
            cache.tokens.remove(&key);
        }
    }
}

fn check_env(env: &str) -> Result<(), String> {
    match env {
        "sandbox" | "prod" => Ok(()),
        _ => Err(format!("env must be \"sandbox\" or \"prod\", not {env:?}.")),
    }
}

fn instance_unreachable(e: reqwest::Error) -> String {
    format!("The org's auth instance could not be reached: {e}")
}

/// The JSON-RPC response in a plain JSON body or, from a server that streams,
/// the last `data:` event that carries one.
fn rpc_response(text: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        return Some(v);
    }
    text.lines()
        .rev()
        .filter_map(|l| l.strip_prefix("data:"))
        .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
        .find(|v| v.get("result").is_some() || v.get("error").is_some())
}

/// A `tools/call` answer as the tool's value, or its error with a hint. A tool
/// error's `structuredContent` (such as secrets minted before a failure) is
/// kept, after the message.
fn parse_answer(text: &str) -> Result<Value, String> {
    let unexpected = || "The org's auth instance sent an unexpected answer.".to_string();
    let v = rpc_response(text).ok_or_else(unexpected)?;
    if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
        let msg = err["message"].as_str().unwrap_or("unknown error");
        return Err(auth_hint(msg));
    }
    let r = v
        .get("result")
        .filter(|r| r.is_object())
        .ok_or_else(unexpected)?;
    let text = r["content"][0]["text"].as_str().unwrap_or("");
    let structured = r.get("structuredContent").filter(|s| !s.is_null());
    if r["isError"].as_bool().unwrap_or(false) {
        let msg = auth_hint(text);
        return Err(match structured {
            Some(s) => format!(
                "{msg}\n{}",
                serde_json::to_string_pretty(s).unwrap_or_default()
            ),
            None => msg,
        });
    }
    if let Some(s) = structured {
        return Ok(s.clone());
    }
    Ok(serde_json::from_str(text).unwrap_or_else(|_| json!({ "text": text })))
}

/// The next step for an auth tool error.
pub fn auth_hint(e: &str) -> String {
    let has = |r: &str| e.contains(r);
    if has("403") && (has("capability_missing") || has("role_required")) {
        "Run `quome connect` again and allow auth access (prod changes need an org owner or admin)."
            .into()
    } else if has("auth_not_started") {
        // A platform 404 arrives as "Not found: auth_not_started" (no status).
        "This org's prod auth hasn't started: link an app or promote first.".into()
    } else if has("503") && has("auth_starting") {
        "The auth instance is waking up; try again in a few seconds.".into()
    } else if has("sandbox_only") {
        "This tool works only on the sandbox environment.".into()
    } else {
        e.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::super::api::CloudApi;
    use super::super::auth::tests::profile;
    use super::super::auth::TokenSource;
    use super::super::keys::AgentKey;
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};
    use wiremock::matchers::{body_json, body_partial_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ORG: &str = "11111111-1111-1111-1111-111111111111";

    /// A mock platform (which also serves the instance's `/mcp`), the client,
    /// and the milliseconds its clock is moved forward by.
    async fn setup() -> (MockServer, AuthInstances, Arc<AtomicU64>) {
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
        let auth =
            AuthInstances::new(Arc::new(move || api.clone())).with_clock(Arc::new(move || {
                base + Duration::from_millis(o.load(Ordering::SeqCst))
            }));
        (s, auth, offset)
    }

    fn broker_path() -> String {
        format!("/api/v1/orgs/{ORG}/auth/token")
    }

    async fn mount_broker(s: &MockServer, expires_in: u64, times: u64) {
        Mock::given(method("POST"))
            .and(path(broker_path()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "T1", "token_type": "Bearer", "expires_in": expires_in,
                "issuer": s.uri(), "mcp_url": format!("{}/mcp", s.uri())
            })))
            .expect(times)
            .mount(s)
            .await;
    }

    fn result(r: serde_json::Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":1,"result": r}))
    }

    #[tokio::test]
    async fn call_sends_jsonrpc_with_bearer_and_version() {
        let (s, auth, _) = setup().await;
        Mock::given(method("POST"))
            .and(path(broker_path()))
            .and(query_param("env", "sandbox"))
            .and(body_json(json!({"scope": ["auth_read"]})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "T1", "token_type": "Bearer", "expires_in": 300,
                "issuer": s.uri(), "mcp_url": format!("{}/mcp", s.uri())
            })))
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(header("authorization", "Bearer T1"))
            .and(header("mcp-protocol-version", "2025-06-18"))
            .and(header("content-type", "application/json"))
            .and(body_partial_json(json!({
                "jsonrpc": "2.0", "method": "tools/call",
                "params": {"name": "status", "arguments": {}}
            })))
            .respond_with(result(json!({
                "content": [{"type": "text", "text": "{\"mode\":\"sandbox\"}"}],
                "structuredContent": {"mode": "sandbox"},
                "isError": false
            })))
            .expect(1)
            .mount(&s)
            .await;
        let v = auth
            .call("sandbox", Scope::Read, "status", json!({}))
            .await
            .unwrap();
        assert_eq!(v, json!({"mode": "sandbox"}));
    }

    #[tokio::test]
    async fn call_prefers_structured_then_text_json() {
        let (s, auth, _) = setup().await;
        mount_broker(&s, 300, 1).await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(body_partial_json(json!({"params": {"name": "a"}})))
            .respond_with(result(json!({
                "content": [{"type": "text", "text": "{\"a\":1}"}], "isError": false
            })))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(body_partial_json(json!({"params": {"name": "b"}})))
            .respond_with(result(json!({
                "content": [{"type": "text", "text": "plain words"}], "isError": false
            })))
            .mount(&s)
            .await;
        let a = auth.call("sandbox", Scope::Read, "a", json!({})).await;
        assert_eq!(a.unwrap(), json!({"a": 1}));
        let b = auth.call("sandbox", Scope::Read, "b", json!({})).await;
        assert_eq!(b.unwrap(), json!({"text": "plain words"}));
    }

    #[tokio::test]
    async fn call_maps_tool_error_through_hint() {
        let (s, auth, _) = setup().await;
        mount_broker(&s, 300, 1).await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(result(json!({
                "content": [{"type": "text", "text": "sandbox_only: test tokens are sandbox-only"}],
                "isError": true
            })))
            .mount(&s)
            .await;
        let e = auth
            .call("prod", Scope::Write, "test_token", json!({}))
            .await
            .unwrap_err()
            .message();
        assert!(e.contains("only on the sandbox"), "{e}");
    }

    #[tokio::test]
    async fn tool_error_keeps_structured_content() {
        let (s, auth, _) = setup().await;
        mount_broker(&s, 300, 1).await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(result(json!({
                "content": [{"type": "text", "text": "partial_apply: 1 of 2 changes applied"}],
                "structuredContent": {"minted": [{"client_id": "c1", "client_secret": "s1"}]},
                "isError": true
            })))
            .mount(&s)
            .await;
        let e = auth
            .call("sandbox", Scope::Write, "apply", json!({}))
            .await
            .unwrap_err()
            .message();
        assert!(e.contains("partial_apply: 1 of 2 changes applied"), "{e}");
        assert!(e.contains("\"client_secret\": \"s1\""), "{e}");
    }

    #[tokio::test]
    async fn jsonrpc_error_is_returned() {
        let (s, auth, _) = setup().await;
        mount_broker(&s, 300, 1).await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0", "id": 1,
                "error": {"code": -32602, "message": "unknown tool: nope"}
            })))
            .mount(&s)
            .await;
        let e = auth
            .call("sandbox", Scope::Read, "nope", json!({}))
            .await
            .unwrap_err()
            .message();
        assert!(e.contains("unknown tool: nope"), "{e}");
    }

    #[tokio::test]
    async fn streamed_answer_is_read() {
        let (s, auth, _) = setup().await;
        mount_broker(&s, 300, 1).await;
        let event = json!({"jsonrpc":"2.0","id":1,"result":{
            "content": [{"type": "text", "text": "{\"ok\":true}"}]}});
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(format!("event: message\ndata: {event}\n\n")),
            )
            .mount(&s)
            .await;
        let v = auth.call("sandbox", Scope::Read, "status", json!({})).await;
        assert_eq!(v.unwrap(), json!({"ok": true}));
    }

    #[tokio::test]
    async fn token_cached_until_skew_then_refreshed() {
        let (s, auth, offset) = setup().await;
        mount_broker(&s, 31, 2).await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(result(json!({"content": [{"type": "text", "text": "{}"}]})))
            .expect(3)
            .mount(&s)
            .await;
        auth.call("sandbox", Scope::Read, "status", json!({}))
            .await
            .unwrap();
        auth.call("sandbox", Scope::Read, "status", json!({}))
            .await
            .unwrap();
        // 31 s minus the 30 s skew: the token is good for 1 s.
        offset.store(1_001, Ordering::SeqCst);
        auth.call("sandbox", Scope::Read, "status", json!({}))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn token_cached_per_env_and_scope() {
        let (s, auth, _) = setup().await;
        mount_broker(&s, 300, 3).await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(result(json!({"content": [{"type": "text", "text": "{}"}]})))
            .mount(&s)
            .await;
        for (env, scope) in [
            ("sandbox", Scope::Read),
            ("sandbox", Scope::Write),
            ("prod", Scope::Read),
            ("sandbox", Scope::Read),
        ] {
            auth.call(env, scope, "status", json!({})).await.unwrap();
        }
    }

    #[tokio::test]
    async fn instance_401_evicts_and_retries_once() {
        let (s, auth, _) = setup().await;
        // The broker hands out T1, then T2; the instance refuses T1.
        for (tok, n) in [("T1", 1u64), ("T2", 1)] {
            Mock::given(method("POST"))
                .and(path(broker_path()))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "access_token": tok, "token_type": "Bearer", "expires_in": 300,
                    "issuer": s.uri(), "mcp_url": format!("{}/mcp", s.uri())
                })))
                .up_to_n_times(n)
                .expect(1)
                .mount(&s)
                .await;
        }
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(header("authorization", "Bearer T1"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(header("authorization", "Bearer T2"))
            .respond_with(result(
                json!({"content": [{"type": "text", "text": "{\"ok\":true}"}]}),
            ))
            .expect(1)
            .mount(&s)
            .await;
        let v = auth
            .call("sandbox", Scope::Read, "status", json!({}))
            .await
            .unwrap();
        assert_eq!(v, json!({"ok": true}));
    }

    #[tokio::test]
    async fn instance_401_twice_fails() {
        let (s, auth, _) = setup().await;
        mount_broker(&s, 300, 2).await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(ResponseTemplate::new(401))
            .expect(2)
            .mount(&s)
            .await;
        let e = auth
            .call("sandbox", Scope::Read, "status", json!({}))
            .await
            .unwrap_err()
            .message();
        assert!(e.contains("401"), "{e}");
    }

    #[tokio::test]
    async fn broker_403_capability_missing_hint() {
        let (s, auth, _) = setup().await;
        Mock::given(method("POST"))
            .and(path(broker_path()))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_json(json!({"detail":"forbidden","reason":"capability_missing"})),
            )
            .mount(&s)
            .await;
        let e = auth
            .call("sandbox", Scope::Read, "status", json!({}))
            .await
            .unwrap_err()
            .message();
        assert!(e.contains("quome connect"), "{e}");
    }

    #[tokio::test]
    async fn issuer_comes_from_the_token_response() {
        let (s, auth, _) = setup().await;
        mount_broker(&s, 300, 1).await;
        assert_eq!(auth.issuer("sandbox").await.unwrap(), s.uri());
        assert_eq!(auth.issuer("sandbox").await.unwrap(), s.uri());
    }

    #[tokio::test]
    async fn bad_env_is_refused_without_a_request() {
        let (_s, auth, _) = setup().await;
        let e = auth
            .call("staging", Scope::Read, "status", json!({}))
            .await
            .unwrap_err()
            .message();
        assert!(e.contains("sandbox"), "{e}");
    }

    #[test]
    fn hints() {
        let connect = "Run `quome connect` again";
        assert!(auth_hint("API error: 403 forbidden: capability_missing").contains(connect));
        assert!(auth_hint("API error: 403 forbidden: role_required").contains(connect));
        assert!(auth_hint("Not found: auth_not_started").contains("hasn't started"));
        assert!(auth_hint("API error: 503 starting: auth_starting").contains("waking up"));
        assert!(auth_hint("sandbox_only: nope").contains("only on the sandbox"));
        assert_eq!(auth_hint("API error: 500 boom"), "API error: 500 boom");
    }
}
