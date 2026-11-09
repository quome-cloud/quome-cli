//! Client for the org's MCP gateway: a hosted MCP server per org that the CLI
//! forwards tool calls to. The platform brokers a short-lived token; the
//! gateway speaks MCP over Streamable HTTP at the URL that comes with it.
//!
//! The token lives only in memory and is scrubbed from every error. Whatever
//! the gateway sends back is untrusted data and is passed through unchanged.
use super::api::{CloudApi, McpToken};
use super::auth::CONNECT_TIMEOUT;
use super::auth_instance::PROTOCOL_VERSION;
use crate::errors::QuomeError;
use futures_util::StreamExt;
use rmcp::model::{CallToolResult, GetPromptResult, Prompt, Tool};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, Mutex};

/// Refresh the token this long before it expires.
const SKEW: Duration = Duration::from_secs(30);
/// Longest total wait while the gateway starts.
const START_BUDGET: Duration = Duration::from_secs(60);
/// The wait when the platform sends no usable `Retry-After`.
const DEFAULT_RETRY: Duration = Duration::from_secs(5);
/// Longest wait for an ordinary request; tool calls may run to the gateway's own limit.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const CALL_TIMEOUT: Duration = Duration::from_secs(600);
/// Longest piece of an unexpected answer quoted in an error.
const SNIPPET: usize = 300;
/// Most pages followed when listing tools or prompts.
const MAX_PAGES: usize = 50;
const START_MESSAGE: &str = "starting the org's MCP gateway";
const LOCAL_TOOLS_META: &str = "cloud.quome/local_tools";
const SESSION_HEADER: &str = "mcp-session-id";

/// A progress report from the gateway (or from starting it).
#[derive(Debug, Clone, PartialEq)]
pub struct ProgressUpdate {
    pub progress: f64,
    pub total: Option<f64>,
    pub message: Option<String>,
}

/// Receives progress while a call runs.
pub type ProgressSink = Arc<dyn Fn(ProgressUpdate) + Send + Sync>;
type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// Why a gateway call failed. No variant ever carries the token.
#[derive(Debug)]
pub enum GatewayError {
    /// The platform refused or failed the token request.
    Platform(QuomeError),
    /// The agent's connection lacks the capability to use the gateway.
    MissingCapability,
    /// The gateway did not come up within the wait.
    StillStarting,
    /// The gateway's URL is not one the CLI will send a token to.
    InsecureUrl(String),
    /// The gateway refused the token, even after a refresh.
    Unauthorized,
    /// The gateway answered with an HTTP error.
    Http { status: u16, body: String },
    /// The gateway answered with a JSON-RPC error.
    Rpc { code: i64, message: String },
    /// The gateway could not be reached or the answer broke off.
    Transport(String),
    /// The gateway sent something that is not MCP.
    Protocol(String),
}

impl std::fmt::Display for GatewayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GatewayError::Platform(e) => write!(f, "{e}"),
            GatewayError::MissingCapability => write!(
                f,
                "This agent is not allowed to use the org's MCP gateway: run `quome connect` again."
            ),
            GatewayError::StillStarting => write!(
                f,
                "The org's MCP gateway is still starting; try again in a minute."
            ),
            GatewayError::InsecureUrl(why) => {
                write!(f, "Refusing the org's MCP gateway address: {why}")
            }
            GatewayError::Unauthorized => write!(
                f,
                "The org's MCP gateway rejected this agent's token; run `quome connect` again."
            ),
            GatewayError::Http { status, body } => {
                write!(f, "The org's MCP gateway answered {status}: {body}")
            }
            GatewayError::Rpc { code, message } => {
                write!(
                    f,
                    "The org's MCP gateway reported an error ({code}): {message}"
                )
            }
            GatewayError::Transport(m) => {
                write!(f, "The org's MCP gateway could not be reached: {m}")
            }
            GatewayError::Protocol(m) => {
                write!(f, "The org's MCP gateway sent an unexpected answer: {m}")
            }
        }
    }
}

impl std::error::Error for GatewayError {}

/// Knobs for tests.
#[derive(Default)]
pub struct Options {
    clock: Option<Clock>,
    budget: Option<Duration>,
}

impl Options {
    pub fn clock(mut self, clock: Clock) -> Self {
        self.clock = Some(clock);
        self
    }
    pub fn budget(mut self, budget: Duration) -> Self {
        self.budget = Some(budget);
        self
    }
}

/// The current token, when it was fetched, and a counter that changes with it.
struct Held {
    token: Option<(McpToken, Instant)>,
    generation: u64,
}

struct Inner {
    api: Arc<CloudApi>,
    http: reqwest::Client,
    url: String,
    /// Held across a refresh so concurrent callers share one fetch.
    held: Mutex<Held>,
    session: std::sync::Mutex<Option<String>>,
    next_id: AtomicU64,
    notes: broadcast::Sender<Value>,
    clock: Clock,
    budget: Duration,
}

pub struct GatewayClient {
    inner: Arc<Inner>,
    stream: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for GatewayClient {
    fn drop(&mut self) {
        if let Some(h) = self.stream.take() {
            h.abort();
        }
    }
}

/// Replace every occurrence of `secret` in `text`.
fn scrub(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        return text.to_string();
    }
    text.replace(secret, "<redacted>")
}

fn snippet(text: &str, secret: &str) -> String {
    scrub(
        &text.trim().chars().take(SNIPPET).collect::<String>(),
        secret,
    )
}

/// Ask the platform for a gateway token, waiting through `mcp_starting`.
async fn fetch_token(
    api: &CloudApi,
    budget: Duration,
    progress: Option<&ProgressSink>,
) -> Result<McpToken, GatewayError> {
    let start = Instant::now();
    let mut announced = false;
    loop {
        match api.mcp_token(&["mcp_call"]).await {
            Ok(t) => return Ok(t),
            Err(QuomeError::RetryLater {
                message,
                retry_after,
            }) if message.contains("mcp_starting") => {
                let wait = retry_after.map_or(DEFAULT_RETRY, Duration::from_secs);
                if wait > budget.saturating_sub(start.elapsed()) {
                    return Err(GatewayError::StillStarting);
                }
                if !announced {
                    announced = true;
                    if let Some(p) = progress {
                        p(ProgressUpdate {
                            progress: 0.0,
                            total: None,
                            message: Some(START_MESSAGE.into()),
                        });
                    }
                }
                tokio::time::sleep(wait).await;
            }
            Err(QuomeError::ApiError(m))
                if m.starts_with("403") && m.contains("capability_missing") =>
            {
                return Err(GatewayError::MissingCapability)
            }
            Err(e) => return Err(GatewayError::Platform(e)),
        }
    }
}

impl GatewayClient {
    /// Get a token, open a session with the org's MCP gateway and start
    /// listening for its notifications. `local_tools` names the tools the CLI
    /// serves itself.
    pub async fn connect(api: Arc<CloudApi>, local_tools: &[&str]) -> Result<Self, GatewayError> {
        Self::connect_with(api, local_tools, None, Options::default()).await
    }

    /// `connect`, reporting progress while the gateway starts.
    pub async fn connect_with_progress(
        api: Arc<CloudApi>,
        local_tools: &[&str],
        progress: Option<ProgressSink>,
    ) -> Result<Self, GatewayError> {
        Self::connect_with(api, local_tools, progress, Options::default()).await
    }

    pub async fn connect_with(
        api: Arc<CloudApi>,
        local_tools: &[&str],
        progress: Option<ProgressSink>,
        opts: Options,
    ) -> Result<Self, GatewayError> {
        let budget = opts.budget.unwrap_or(START_BUDGET);
        let clock: Clock = opts.clock.unwrap_or_else(|| Arc::new(Instant::now));
        let fetched = clock();
        let token = fetch_token(&api, budget, progress.as_ref()).await?;
        let url = crate::url_policy::validate_api_url(&token.url, "the MCP gateway address")
            .map_err(|e| GatewayError::InsecureUrl(snippet(&e.to_string(), &token.access_token)))?;
        // Never follow a redirect: the token must only go to the address it came with.
        let http = reqwest::Client::builder()
            .https_only(!crate::url_policy::is_loopback_http(&url))
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|e| GatewayError::Transport(e.without_url().to_string()))?;
        let (notes, _) = broadcast::channel(64);
        let inner = Arc::new(Inner {
            api,
            http,
            url,
            held: Mutex::new(Held {
                token: Some((token, fetched)),
                generation: 1,
            }),
            session: std::sync::Mutex::new(None),
            next_id: AtomicU64::new(0),
            notes,
            clock,
            budget,
        });
        inner
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "quome-cli", "version": env!("CARGO_PKG_VERSION")},
                    "_meta": {LOCAL_TOOLS_META: local_tools},
                }),
                None,
                REQUEST_TIMEOUT,
            )
            .await?;
        inner.notify("notifications/initialized").await?;
        let stream = tokio::spawn(inner.clone().listen());
        Ok(GatewayClient {
            inner,
            stream: Some(stream),
        })
    }

    /// The gateway's tools.
    pub async fn list_tools(&self) -> Result<Vec<Tool>, GatewayError> {
        self.inner.list("tools/list", "tools").await
    }

    /// Call a tool. The result is the gateway's, unchanged.
    pub async fn call_tool(
        &self,
        name: &str,
        args: Value,
        progress: Option<ProgressSink>,
    ) -> Result<CallToolResult, GatewayError> {
        let mut params = json!({"name": name, "arguments": args});
        if progress.is_some() {
            params["_meta"] = json!({"progressToken": uuid::Uuid::new_v4().to_string()});
        }
        let v = self
            .inner
            .request("tools/call", params, progress.as_ref(), CALL_TIMEOUT)
            .await?;
        decode(v)
    }

    /// The gateway's prompts.
    pub async fn list_prompts(&self) -> Result<Vec<Prompt>, GatewayError> {
        self.inner.list("prompts/list", "prompts").await
    }

    pub async fn get_prompt(
        &self,
        name: &str,
        args: BTreeMap<String, String>,
    ) -> Result<GetPromptResult, GatewayError> {
        let v = self
            .inner
            .request(
                "prompts/get",
                json!({"name": name, "arguments": args}),
                None,
                REQUEST_TIMEOUT,
            )
            .await?;
        decode(v)
    }

    /// Notifications the gateway sends outside any call.
    pub fn notifications(&self) -> broadcast::Receiver<Value> {
        self.inner.notes.subscribe()
    }

    #[cfg(test)]
    async fn token_debug(&self) -> String {
        format!("{:?}", self.inner.held.lock().await.token)
    }
}

fn decode<T: DeserializeOwned>(v: Value) -> Result<T, GatewayError> {
    serde_json::from_value(v).map_err(|e| GatewayError::Protocol(format!("bad result ({e})")))
}

impl Inner {
    /// A valid token and the generation it belongs to, refreshed within 30 s of expiry.
    async fn token(&self) -> Result<(String, u64), GatewayError> {
        let mut held = self.held.lock().await;
        if let Some((t, fetched)) = &held.token {
            let ttl = Duration::from_secs(t.expires_in).saturating_sub(SKEW);
            if (self.clock)() < *fetched + ttl {
                return Ok((t.access_token.clone(), held.generation));
            }
        }
        let fetched = (self.clock)();
        let t = fetch_token(&self.api, self.budget, None).await?;
        let secret = t.access_token.clone();
        held.token = Some((t, fetched));
        held.generation += 1;
        Ok((secret, held.generation))
    }

    /// Drop the token of `generation`, unless a newer one already replaced it.
    async fn invalidate(&self, generation: u64) {
        let mut held = self.held.lock().await;
        if held.generation == generation {
            held.token = None;
        }
    }

    fn session(&self) -> Option<String> {
        self.session.lock().ok().and_then(|s| s.clone())
    }

    fn headers(&self, b: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
        let b = b
            .bearer_auth(token)
            .header("MCP-Protocol-Version", PROTOCOL_VERSION)
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            );
        match self.session() {
            Some(sid) => b.header("Mcp-Session-Id", sid),
            None => b,
        }
    }

    /// Send a notification (no answer expected).
    async fn notify(&self, method: &str) -> Result<(), GatewayError> {
        let body = json!({"jsonrpc": "2.0", "method": method});
        self.post(&body, None, None, REQUEST_TIMEOUT)
            .await
            .map(|_| ())
    }

    /// One JSON-RPC request; returns the `result`.
    async fn request(
        &self,
        method: &str,
        params: Value,
        progress: Option<&ProgressSink>,
        timeout: Duration,
    ) -> Result<Value, GatewayError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let v = self.post(&body, Some(id), progress, timeout).await?;
        match v {
            Some(v) => Ok(v),
            None => Err(GatewayError::Protocol("no answer".into())),
        }
    }

    async fn list<T: DeserializeOwned>(
        &self,
        method: &str,
        key: &str,
    ) -> Result<Vec<T>, GatewayError> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let v = self.request(method, params, None, REQUEST_TIMEOUT).await?;
            let items = v.get(key).cloned().unwrap_or(Value::Null);
            out.extend(decode::<Vec<T>>(items)?);
            cursor = v["nextCursor"].as_str().map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        Ok(out)
    }

    /// POST `body`. A 401 refreshes the token and retries once. With `want`
    /// set, reads the answer to that id (from JSON or an event stream,
    /// forwarding progress and other notifications on the way).
    async fn post(
        &self,
        body: &Value,
        want: Option<u64>,
        progress: Option<&ProgressSink>,
        timeout: Duration,
    ) -> Result<Option<Value>, GatewayError> {
        let mut refreshed = false;
        loop {
            let (token, generation) = self.token().await?;
            let resp = self
                .headers(self.http.post(&self.url), &token)
                .timeout(timeout)
                .json(body)
                .send()
                .await
                .map_err(|e| {
                    GatewayError::Transport(scrub(&e.without_url().to_string(), &token))
                })?;
            let status = resp.status().as_u16();
            if status == 401 {
                if refreshed {
                    return Err(GatewayError::Unauthorized);
                }
                refreshed = true;
                self.invalidate(generation).await;
                continue;
            }
            if !resp.status().is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(GatewayError::Http {
                    status,
                    body: snippet(&text, &token),
                });
            }
            if let Some(sid) = resp
                .headers()
                .get(SESSION_HEADER)
                .and_then(|v| v.to_str().ok())
            {
                if let Ok(mut s) = self.session.lock() {
                    *s = Some(sid.to_string());
                }
            }
            let Some(id) = want else {
                return Ok(None);
            };
            let is_stream = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|c| c.starts_with("text/event-stream"));
            let answer = if is_stream {
                self.read_stream(resp, id, progress, &token).await?
            } else {
                let text = resp.text().await.map_err(|e| {
                    GatewayError::Transport(scrub(&e.without_url().to_string(), &token))
                })?;
                let v: Value = serde_json::from_str(&text)
                    .map_err(|_| GatewayError::Protocol(snippet(&text, &token)))?;
                if v["id"] == json!(id) {
                    v
                } else {
                    return Err(GatewayError::Protocol("answer to another request".into()));
                }
            };
            return answer_result(answer, &token).map(Some);
        }
    }

    /// Read events until the answer to `id`.
    async fn read_stream(
        &self,
        resp: reqwest::Response,
        id: u64,
        progress: Option<&ProgressSink>,
        token: &str,
    ) -> Result<Value, GatewayError> {
        let mut events = SseEvents::default();
        let mut chunks = resp.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk
                .map_err(|e| GatewayError::Transport(scrub(&e.without_url().to_string(), token)))?;
            for data in events.push(&chunk) {
                let Ok(v) = serde_json::from_str::<Value>(&data) else {
                    continue;
                };
                if v["id"] == json!(id) && (v.get("result").is_some() || v.get("error").is_some()) {
                    return Ok(v);
                }
                if v["method"] == "notifications/progress" {
                    if let Some(p) = progress {
                        p(ProgressUpdate {
                            progress: v["params"]["progress"].as_f64().unwrap_or(0.0),
                            total: v["params"]["total"].as_f64(),
                            message: v["params"]["message"].as_str().map(str::to_string),
                        });
                    }
                } else if v.get("method").is_some() {
                    let _ = self.notes.send(v);
                }
            }
        }
        Err(GatewayError::Transport(
            "the answer broke off before it finished".into(),
        ))
    }

    /// Follow the gateway's notification stream until it is closed for good.
    async fn listen(self: Arc<Self>) {
        let mut failures = 0u32;
        while failures < 5 {
            let Ok((token, generation)) = self.token().await else {
                return;
            };
            let resp = self
                .headers(self.http.get(&self.url), &token)
                .header(reqwest::header::ACCEPT, "text/event-stream")
                .send()
                .await;
            match resp {
                Ok(r) if r.status().as_u16() == 401 => {
                    self.invalidate(generation).await;
                    failures += 1;
                }
                Ok(r) if r.status().is_success() => {
                    failures = 0;
                    let mut events = SseEvents::default();
                    let mut chunks = r.bytes_stream();
                    while let Some(Ok(chunk)) = chunks.next().await {
                        for data in events.push(&chunk) {
                            if let Ok(v) = serde_json::from_str::<Value>(&data) {
                                let _ = self.notes.send(v);
                            }
                        }
                    }
                }
                // The gateway has no stream for this session (405 and the like).
                Ok(_) => return,
                Err(_) => failures += 1,
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

/// The `result` of a JSON-RPC answer, or its error (scrubbed of `token`).
fn answer_result(v: Value, token: &str) -> Result<Value, GatewayError> {
    if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
        return Err(GatewayError::Rpc {
            code: err["code"].as_i64().unwrap_or(0),
            message: snippet(err["message"].as_str().unwrap_or("unknown error"), token),
        });
    }
    v.get("result")
        .cloned()
        .ok_or_else(|| GatewayError::Protocol("no result".into()))
}

/// Splits a server-sent event stream into the `data` of each event.
#[derive(Default)]
struct SseEvents {
    line: Vec<u8>,
    data: Vec<String>,
}

impl SseEvents {
    fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        for &b in chunk {
            if b != b'\n' {
                self.line.push(b);
                continue;
            }
            let line = String::from_utf8_lossy(&self.line).into_owned();
            self.line.clear();
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                if !self.data.is_empty() {
                    out.push(self.data.join("\n"));
                    self.data.clear();
                }
            } else if let Some(d) = line.strip_prefix("data:") {
                self.data.push(d.strip_prefix(' ').unwrap_or(d).to_string());
            }
        }
        out
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
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    /// A mock platform that also serves the gateway at `/gw`.
    async fn setup() -> (MockServer, Arc<CloudApi>) {
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
                json!({"access_token": "AT", "token_type": "Bearer", "expires_in": 3600}),
            ))
            .mount(&s)
            .await;
        let p = profile(&s.uri());
        let api = CloudApi::new(&p, TokenSource::new(&p, AgentKey::generate(), "vrc".into()));
        (s, Arc::new(api))
    }

    const TOKEN_PATH: &str = "/api/v1/orgs/11111111-1111-1111-1111-111111111111/mcp/token";

    fn token_body(s: &MockServer, tok: &str, expires_in: u64) -> Value {
        json!({"access_token": tok, "expires_in": expires_in, "url": format!("{}/gw", s.uri())})
    }

    async fn mount_token(s: &MockServer, tok: &str, expires_in: u64, times: Option<u64>) {
        let m = Mock::given(method("POST"))
            .and(path(TOKEN_PATH))
            .and(body_partial_json(json!({"scope": ["mcp_call"]})))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_body(s, tok, expires_in)));
        match times {
            Some(n) => m.up_to_n_times(n).mount(s).await,
            None => m.mount(s).await,
        }
    }

    /// Answers a JSON-RPC request with `result`, echoing its id.
    struct Rpc {
        result: Value,
        session: Option<&'static str>,
    }
    impl Respond for Rpc {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let id = serde_json::from_slice::<Value>(&req.body).unwrap()["id"].clone();
            let mut r = ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc": "2.0", "id": id, "result": self.result}));
            if let Some(sid) = self.session {
                r = r.insert_header("mcp-session-id", sid);
            }
            r
        }
    }

    async fn mount_rpc(s: &MockServer, rpc_method: &str, result: Value) {
        Mock::given(method("POST"))
            .and(path("/gw"))
            .and(body_partial_json(json!({"method": rpc_method})))
            .respond_with(Rpc {
                result,
                session: None,
            })
            .mount(s)
            .await;
    }

    async fn mount_handshake(s: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/gw"))
            .and(body_partial_json(json!({"method": "initialize"})))
            .respond_with(Rpc {
                result: json!({"protocolVersion": "2025-06-18", "capabilities": {},
                               "serverInfo": {"name": "gw", "version": "1"}}),
                session: Some("sess-1"),
            })
            .mount(s)
            .await;
        Mock::given(method("POST"))
            .and(path("/gw"))
            .and(body_partial_json(
                json!({"method": "notifications/initialized"}),
            ))
            .respond_with(ResponseTemplate::new(202))
            .mount(s)
            .await;
    }

    fn tools_result() -> Value {
        json!({"tools": [{"name": "x", "inputSchema": {"type": "object"}}]})
    }

    async fn requests_to(s: &MockServer, p: &str) -> Vec<Request> {
        s.received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.url.path() == p)
            .collect()
    }

    fn body(r: &Request) -> Value {
        serde_json::from_slice(&r.body).unwrap_or_default()
    }

    #[tokio::test]
    async fn connect_waits_through_starting() {
        let (s, api) = setup().await;
        Mock::given(method("POST"))
            .and(path(TOKEN_PATH))
            .respond_with(
                ResponseTemplate::new(503)
                    .insert_header("retry-after", "1")
                    .set_body_json(json!({"reason": "mcp_starting"})),
            )
            .up_to_n_times(1)
            .mount(&s)
            .await;
        mount_token(&s, "GWTOK", 3600, None).await;
        mount_handshake(&s).await;
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink: ProgressSink = {
            let seen = seen.clone();
            Arc::new(move |u| seen.lock().unwrap().extend(u.message))
        };
        let t0 = Instant::now();
        GatewayClient::connect_with_progress(api, &[], Some(sink))
            .await
            .unwrap();
        assert!(t0.elapsed() >= Duration::from_millis(900));
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["starting the org's MCP gateway".to_string()]
        );
        assert_eq!(requests_to(&s, TOKEN_PATH).await.len(), 2);
    }

    #[tokio::test]
    async fn still_starting_past_the_budget_is_a_clear_error() {
        let (s, api) = setup().await;
        Mock::given(method("POST"))
            .and(path(TOKEN_PATH))
            .respond_with(
                ResponseTemplate::new(503)
                    .insert_header("retry-after", "120")
                    .set_body_json(json!({"reason": "mcp_starting"})),
            )
            .mount(&s)
            .await;
        let e = GatewayClient::connect(api, &[]).await.err().unwrap();
        assert!(e.to_string().contains("still starting"), "{e}");
        assert_eq!(requests_to(&s, TOKEN_PATH).await.len(), 1);
    }

    #[tokio::test]
    async fn initialize_declares_local_tools() {
        let (s, api) = setup().await;
        mount_token(&s, "GWTOK", 3600, None).await;
        mount_handshake(&s).await;
        mount_rpc(&s, "tools/list", tools_result()).await;
        let c = GatewayClient::connect(api, &["exec", "read_file"])
            .await
            .unwrap();
        let init = requests_to(&s, "/gw")
            .await
            .into_iter()
            .map(|r| body(&r))
            .find(|b| b["method"] == "initialize")
            .unwrap();
        assert_eq!(
            init["params"]["_meta"]["cloud.quome/local_tools"],
            json!(["exec", "read_file"])
        );
        // the session id from initialize is sent on later requests
        let tools = c.list_tools().await.unwrap();
        assert_eq!(tools[0].name, "x");
        let reqs = requests_to(&s, "/gw").await;
        let last = reqs.last().unwrap();
        assert_eq!(last.headers.get("mcp-session-id").unwrap(), "sess-1");
        assert_eq!(last.headers.get("authorization").unwrap(), "Bearer GWTOK");
    }

    #[tokio::test]
    async fn token_refreshed_before_expiry() {
        let (s, api) = setup().await;
        mount_token(&s, "OLD", 60, Some(1)).await;
        mount_token(&s, "NEW", 60, None).await;
        mount_handshake(&s).await;
        mount_rpc(&s, "tools/list", tools_result()).await;
        let skew = Arc::new(AtomicU64::new(0));
        let clock = {
            let skew = skew.clone();
            let base = Instant::now();
            Arc::new(move || base + Duration::from_secs(skew.load(Ordering::Relaxed)))
        };
        let c = GatewayClient::connect_with(api, &[], None, Options::default().clock(clock))
            .await
            .unwrap();
        c.list_tools().await.unwrap();
        assert_eq!(requests_to(&s, TOKEN_PATH).await.len(), 1);
        skew.store(31, Ordering::Relaxed);
        // concurrent callers share one refresh
        let (a, b) = tokio::join!(c.list_tools(), c.list_tools());
        a.unwrap();
        b.unwrap();
        assert_eq!(requests_to(&s, TOKEN_PATH).await.len(), 2);
        let reqs = requests_to(&s, "/gw").await;
        assert_eq!(
            reqs.last().unwrap().headers.get("authorization").unwrap(),
            "Bearer NEW"
        );
    }

    #[tokio::test]
    async fn unauthorized_refreshes_once() {
        let (s, api) = setup().await;
        mount_token(&s, "OLD", 3600, Some(1)).await;
        mount_token(&s, "NEW", 3600, None).await;
        mount_handshake(&s).await;
        Mock::given(method("POST"))
            .and(path("/gw"))
            .and(body_partial_json(json!({"method": "tools/list"})))
            .and(header("authorization", "Bearer OLD"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&s)
            .await;
        mount_rpc(&s, "tools/list", tools_result()).await;
        let c = GatewayClient::connect(api, &[]).await.unwrap();
        c.list_tools().await.unwrap();
        assert_eq!(requests_to(&s, TOKEN_PATH).await.len(), 2);

        // a second 401 is an error, not a loop
        let (s, api) = setup().await;
        mount_token(&s, "T", 3600, None).await;
        mount_handshake(&s).await;
        Mock::given(method("POST"))
            .and(path("/gw"))
            .and(body_partial_json(json!({"method": "tools/list"})))
            .respond_with(ResponseTemplate::new(401))
            .mount(&s)
            .await;
        let c = GatewayClient::connect(api, &[]).await.unwrap();
        assert!(c.list_tools().await.is_err());
        let lists = requests_to(&s, "/gw")
            .await
            .iter()
            .filter(|r| body(r)["method"] == "tools/list")
            .count();
        assert_eq!(lists, 2);
        assert_eq!(requests_to(&s, TOKEN_PATH).await.len(), 2);
    }

    #[tokio::test]
    async fn progress_forwarded() {
        let (s, api) = setup().await;
        mount_token(&s, "GWTOK", 3600, None).await;
        mount_handshake(&s).await;
        let sse = "event: message\n\
            data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{\"progressToken\":\"t\",\"progress\":1,\"total\":2,\"message\":\"half\"}}\n\n\
            event: message\n\
            data: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"done\"}],\"isError\":false}}\n\n";
        Mock::given(method("POST"))
            .and(path("/gw"))
            .and(body_partial_json(json!({"method": "tools/call"})))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&s)
            .await;
        let c = GatewayClient::connect(api, &[]).await.unwrap();
        let seen: Arc<Mutex<Vec<ProgressUpdate>>> = Arc::default();
        let sink: ProgressSink = {
            let seen = seen.clone();
            Arc::new(move |u| seen.lock().unwrap().push(u))
        };
        let r = c.call_tool("x", json!({"a": 1}), Some(sink)).await.unwrap();
        assert_eq!(r.is_error, Some(false));
        assert_eq!(
            *seen.lock().unwrap(),
            vec![ProgressUpdate {
                progress: 1.0,
                total: Some(2.0),
                message: Some("half".into())
            }]
        );
        let call = requests_to(&s, "/gw")
            .await
            .into_iter()
            .map(|r| body(&r))
            .find(|b| b["method"] == "tools/call")
            .unwrap();
        assert!(!call["params"]["_meta"]["progressToken"].is_null());
        assert_eq!(call["params"]["name"], "x");
        assert_eq!(call["params"]["arguments"], json!({"a": 1}));
    }

    #[tokio::test]
    async fn token_never_in_error_text() {
        let (s, api) = setup().await;
        mount_token(&s, "SECRET-GW-TOKEN", 3600, None).await;
        mount_handshake(&s).await;
        Mock::given(method("POST"))
            .and(path("/gw"))
            .and(body_partial_json(json!({"method": "tools/list"})))
            .respond_with(
                ResponseTemplate::new(500).set_body_string("bad: Bearer SECRET-GW-TOKEN echoed"),
            )
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/gw"))
            .and(body_partial_json(json!({"method": "tools/call"})))
            .respond_with(|req: &Request| {
                let id = serde_json::from_slice::<Value>(&req.body).unwrap()["id"].clone();
                ResponseTemplate::new(200).set_body_json(json!({"jsonrpc": "2.0", "id": id,
                    "error": {"code": -32000, "message": "nope SECRET-GW-TOKEN"}}))
            })
            .mount(&s)
            .await;
        let c = GatewayClient::connect(api, &[]).await.unwrap();
        for e in [
            c.list_tools().await.err().unwrap(),
            c.call_tool("x", json!({}), None).await.err().unwrap(),
        ] {
            assert!(!e.to_string().contains("SECRET-GW-TOKEN"), "{e}");
            assert!(!format!("{e:?}").contains("SECRET-GW-TOKEN"), "{e:?}");
        }
        assert!(!format!("{:?}", c.token_debug().await).contains("SECRET-GW-TOKEN"));
    }

    #[tokio::test]
    async fn missing_capability_reports_reconnect_hint() {
        let (s, api) = setup().await;
        Mock::given(method("POST"))
            .and(path(TOKEN_PATH))
            .respond_with(
                ResponseTemplate::new(403).set_body_json(json!({"reason": "capability_missing"})),
            )
            .mount(&s)
            .await;
        let e = GatewayClient::connect(api, &[]).await.err().unwrap();
        assert!(e.to_string().contains("quome connect"), "{e}");
    }

    #[tokio::test]
    async fn refuses_a_plain_http_gateway_url() {
        let (s, api) = setup().await;
        Mock::given(method("POST"))
            .and(path(TOKEN_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"access_token": "T", "expires_in": 60, "url": "http://gw.example.test/mcp"}),
            ))
            .mount(&s)
            .await;
        let e = GatewayClient::connect(api, &[]).await.err().unwrap();
        assert!(e.to_string().contains("https"), "{e}");
    }

    #[tokio::test]
    async fn redirects_are_not_followed() {
        let (s, api) = setup().await;
        let other = MockServer::start().await;
        mount_token(&s, "GWTOK", 3600, None).await;
        Mock::given(method("POST"))
            .and(path("/gw"))
            .respond_with(
                ResponseTemplate::new(307)
                    .insert_header("location", format!("{}/steal", other.uri())),
            )
            .mount(&s)
            .await;
        assert!(GatewayClient::connect(api, &[]).await.is_err());
        assert!(other.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn front_door_is_read_from_the_platform() {
        use super::super::api::FrontDoor;
        let (s, api) = setup().await;
        let p = "/api/v1/orgs/11111111-1111-1111-1111-111111111111/mcp/config";
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"front_door": "gateway"})),
            )
            .up_to_n_times(1)
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"front_door": "direct"})))
            .mount(&s)
            .await;
        assert_eq!(api.mcp_config().await, Ok(FrontDoor::Gateway));
        assert_eq!(api.mcp_config().await, Ok(FrontDoor::Direct));
    }
}
