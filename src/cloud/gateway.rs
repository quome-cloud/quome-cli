//! Client for the org's MCP gateway: a hosted MCP server per org that the CLI
//! forwards tool calls to. The platform brokers a short-lived token; the
//! gateway speaks MCP over Streamable HTTP at the URL that comes with it.
//!
//! The token lives only in memory and is scrubbed from every error. Whatever
//! the gateway sends back is untrusted data: it is passed through unchanged,
//! but everything read from it is size-bounded.
use super::api::{CloudApi, FrontDoor, McpToken};
use super::auth::CONNECT_TIMEOUT;
use super::auth_instance::PROTOCOL_VERSION;
use crate::errors::QuomeError;
use futures_util::StreamExt;
use rmcp::model::{CallToolResult, GetPromptResult, Prompt, Tool};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, Mutex};
use tokio::time::Instant;

/// Longest total wait while the gateway starts.
const START_BUDGET: Duration = Duration::from_secs(60);
/// The wait when the platform sends no usable `Retry-After`.
const DEFAULT_RETRY: Duration = Duration::from_secs(5);
/// The shortest wait between two token requests while the gateway starts.
const MIN_RETRY: Duration = Duration::from_secs(1);
/// Longest wait for an ordinary request; tool calls may run to the gateway's own limit.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const CALL_TIMEOUT: Duration = Duration::from_secs(600);
/// Longest piece of an unexpected answer quoted in an error.
const SNIPPET: usize = 300;
/// Most of an error body that is read at all.
const ERROR_BODY_MAX: usize = 4 * 1024;
/// Largest plain JSON answer.
const BODY_MAX: usize = 16 * 1024 * 1024;
/// Longest line of an event stream, and most data in one event.
const SSE_LINE_MAX: usize = 64 * 1024;
const SSE_EVENT_MAX: usize = 4 * 1024 * 1024;
/// How long the notification stream may stay silent before it is reopened.
const LISTEN_IDLE: Duration = Duration::from_secs(300);
/// Backoff bounds for reopening the notification stream.
const LISTEN_BACKOFF_MIN: Duration = Duration::from_secs(1);
const LISTEN_BACKOFF_MAX: Duration = Duration::from_secs(60);
/// A token is refreshed this long before it expires (less for short-lived ones).
const SKEW: Duration = Duration::from_secs(30);
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
type Clock = Arc<dyn Fn() -> std::time::Instant + Send + Sync>;

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
    /// The gateway no longer knows this session; connect again.
    SessionLost,
    /// The gateway answered with an HTTP error.
    Http { status: u16, body: String },
    /// The gateway answered with a JSON-RPC error.
    Rpc {
        code: i64,
        message: String,
        /// The gateway's name for the error (`data.code`), when it sent a
        /// plain one.
        reason: Option<String>,
    },
    /// The gateway could not be reached or the answer broke off.
    Transport(String),
    /// The gateway sent something that is not MCP, or too much of it.
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
            GatewayError::SessionLost => {
                write!(f, "the MCP gateway session ended; reconnect")
            }
            GatewayError::Http { status, body } => {
                write!(f, "The org's MCP gateway answered {status}: {body}")
            }
            GatewayError::Rpc { code, message, .. } => {
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
pub(crate) struct Options {
    clock: Option<Clock>,
    listen_idle: Option<Duration>,
}

impl Options {
    pub(crate) fn clock(mut self, clock: Clock) -> Self {
        self.clock = Some(clock);
        self
    }

    /// How long the notification stream may stay silent before it is reopened.
    pub(crate) fn listen_idle(mut self, idle: Duration) -> Self {
        self.listen_idle = Some(idle);
        self
    }
}

/// The current token, when it was fetched, and a counter that changes with it.
struct Held {
    token: Option<(McpToken, std::time::Instant)>,
    generation: u64,
}

struct Inner {
    api: Arc<CloudApi>,
    http: reqwest::Client,
    /// For the notification stream: no total timeout.
    stream_http: reqwest::Client,
    url: String,
    /// Held across a refresh so concurrent callers share one fetch.
    held: Mutex<Held>,
    session: std::sync::Mutex<Option<String>>,
    next_id: AtomicU64,
    notes: broadcast::Sender<Value>,
    clock: Clock,
    /// The tools the CLI serves itself, declared again with every new session.
    local_tools: Vec<String>,
    /// Held while a session is opened, so concurrent callers open one.
    opening: Mutex<()>,
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

/// The front door named by `QUOME_MCP_FRONT_DOOR` (`direct` or `gateway`),
/// which wins over what the platform says. Anything else is ignored.
pub fn front_door_override() -> Option<FrontDoor> {
    parse_front_door(std::env::var("QUOME_MCP_FRONT_DOOR").ok().as_deref()?)
}

fn parse_front_door(v: &str) -> Option<FrontDoor> {
    match v.trim().to_ascii_lowercase().as_str() {
        "direct" => Some(FrontDoor::Direct),
        "gateway" => Some(FrontDoor::Gateway),
        _ => None,
    }
}

/// Replace every occurrence of `secret`, and the value of any `Bearer`
/// header-style credential, in `text`.
fn scrub(text: &str, secret: &str) -> String {
    let mut out = if secret.is_empty() {
        text.to_string()
    } else {
        text.replace(secret, "<redacted>")
    };
    let mut from = 0;
    while let Some(i) = out[from..].to_ascii_lowercase().find("bearer ") {
        let start = from + i + "bearer ".len();
        let end = out[start..]
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';'))
            .map_or(out.len(), |n| start + n);
        out.replace_range(start..end, "<redacted>");
        from = start + "<redacted>".len();
    }
    out
}

/// `text` with the credential scrubbed first, then cut to a quotable length.
fn snippet(text: &str, secret: &str) -> String {
    scrub(text.trim(), secret).chars().take(SNIPPET).collect()
}

/// Ask the platform for a gateway token, waiting through `mcp_starting`.
async fn fetch_token(
    api: &CloudApi,
    progress: Option<&ProgressSink>,
) -> Result<McpToken, GatewayError> {
    wait_for_token(|| api.mcp_token(&["mcp_call"]), START_BUDGET, progress).await
}

/// Make token requests with `attempt` until one succeeds, waiting through
/// `mcp_starting`. Each wait is at least 1 s, and the time attempts take
/// counts against `budget`, which is never passed.
async fn wait_for_token<F, Fut>(
    mut attempt: F,
    budget: Duration,
    progress: Option<&ProgressSink>,
) -> Result<McpToken, GatewayError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<McpToken, QuomeError>>,
{
    let start = Instant::now();
    let mut announced = false;
    loop {
        let remaining = budget.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            return Err(GatewayError::StillStarting);
        }
        let result = match tokio::time::timeout(remaining, attempt()).await {
            Ok(r) => r,
            Err(_) => return Err(GatewayError::StillStarting),
        };
        match result {
            Ok(t) => return Ok(t),
            Err(QuomeError::RetryLater {
                message,
                retry_after,
            }) if message.contains("mcp_starting") => {
                let wait = retry_after
                    .map_or(DEFAULT_RETRY, Duration::from_secs)
                    .max(MIN_RETRY);
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

/// How long a token is used: until less than half its life, or 30 s, remains
/// (and for at least a second, so a very short one is not refetched per call).
fn usable_for(expires_in: u64) -> Duration {
    let life = Duration::from_secs(expires_in);
    life.saturating_sub(SKEW.min(life / 2))
        .max(Duration::from_secs(1))
}

/// A gateway address is acceptable when it passes the URL policy, and a plain
/// http one only when the platform itself is on a loopback address.
fn check_gateway_url(raw: &str, api: &CloudApi, secret: &str) -> Result<String, GatewayError> {
    let url = crate::url_policy::validate_api_url(raw, "the MCP gateway address")
        .map_err(|e| GatewayError::InsecureUrl(snippet(&e.to_string(), secret)))?;
    if crate::url_policy::is_loopback_http(&url)
        && !crate::url_policy::is_loopback_http(api.base_url())
    {
        return Err(GatewayError::InsecureUrl(
            "plain http is only allowed when the platform is on this computer".into(),
        ));
    }
    Ok(url)
}

/// Read at most `max` bytes of a body. `None` when the body is longer.
async fn read_capped(
    resp: reqwest::Response,
    max: usize,
    token: &str,
) -> Result<Option<Vec<u8>>, GatewayError> {
    let mut out = Vec::new();
    let mut chunks = resp.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk
            .map_err(|e| GatewayError::Transport(scrub(&e.without_url().to_string(), token)))?;
        if out.len() + chunk.len() > max {
            return Ok(None);
        }
        out.extend_from_slice(&chunk);
    }
    Ok(Some(out))
}

/// The start of an error body: at most `ERROR_BODY_MAX` bytes are read.
async fn read_error_body(resp: reqwest::Response, token: &str) -> String {
    let mut out = Vec::new();
    let mut chunks = resp.bytes_stream();
    while let Some(Ok(chunk)) = chunks.next().await {
        let room = ERROR_BODY_MAX - out.len();
        out.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if out.len() >= ERROR_BODY_MAX {
            break;
        }
    }
    snippet(&String::from_utf8_lossy(&out), token)
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

    pub(crate) async fn connect_with(
        api: Arc<CloudApi>,
        local_tools: &[&str],
        progress: Option<ProgressSink>,
        opts: Options,
    ) -> Result<Self, GatewayError> {
        let clock: Clock = opts
            .clock
            .unwrap_or_else(|| Arc::new(std::time::Instant::now));
        let fetched = clock();
        let token = fetch_token(&api, progress.as_ref()).await?;
        let url = check_gateway_url(&token.url, &api, &token.access_token)?;
        // Never follow a redirect: the token must only go to the address it came with.
        let listen_idle = opts.listen_idle.unwrap_or(LISTEN_IDLE);
        let build = |idle: bool| {
            let mut b = reqwest::Client::builder()
                .https_only(!crate::url_policy::is_loopback_http(&url))
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(CONNECT_TIMEOUT);
            if idle {
                b = b.read_timeout(listen_idle);
            }
            b.build()
                .map_err(|e| GatewayError::Transport(e.without_url().to_string()))
        };
        let (notes, _) = broadcast::channel(64);
        let inner = Arc::new(Inner {
            api,
            http: build(false)?,
            stream_http: build(true)?,
            url,
            held: Mutex::new(Held {
                token: Some((token, fetched)),
                generation: 1,
            }),
            session: std::sync::Mutex::new(None),
            next_id: AtomicU64::new(0),
            notes,
            clock,
            local_tools: local_tools.iter().map(|t| t.to_string()).collect(),
            opening: Mutex::new(()),
        });
        inner.open_session().await?;
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
        let mut progress_token = None;
        if progress.is_some() {
            let t = uuid::Uuid::new_v4().to_string();
            params["_meta"] = json!({"progressToken": t});
            progress_token = Some(t);
        }
        let v = self
            .inner
            .request(
                "tools/call",
                params,
                progress.as_ref().zip(progress_token.as_deref()),
                CALL_TIMEOUT,
            )
            .await?;
        self.inner.decode(v)
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
        self.inner.decode(v)
    }

    /// Notifications the gateway sends outside any call. A receiver gets the
    /// events that arrive after it was made; earlier ones are not buffered,
    /// and a slow receiver may miss some.
    pub fn notifications(&self) -> broadcast::Receiver<Value> {
        self.inner.notes.subscribe()
    }

    #[cfg(test)]
    async fn token_debug(&self) -> String {
        format!("{:?}", self.inner.held.lock().await.token)
    }
}

/// The delay before reopening the notification stream for the `failures`-th
/// time in a row, with `jitter` in `0..=255` taking up to half of it off.
fn backoff(failures: u32, jitter: u8) -> Duration {
    let base = LISTEN_BACKOFF_MIN
        .saturating_mul(1u32 << failures.min(6))
        .min(LISTEN_BACKOFF_MAX);
    base - base.mul_f64(f64::from(jitter) / 512.0)
}

impl Inner {
    /// Decode a result; the error never quotes the gateway's data.
    fn decode<T: DeserializeOwned>(&self, v: Value) -> Result<T, GatewayError> {
        serde_json::from_value(v)
            .map_err(|_| GatewayError::Protocol("a result of the wrong shape".into()))
    }

    /// A valid token and the generation it belongs to, refreshed before it expires.
    async fn token(&self) -> Result<(String, u64), GatewayError> {
        let mut held = self.held.lock().await;
        if let Some((t, fetched)) = &held.token {
            if (self.clock)() < *fetched + usable_for(t.expires_in) {
                return Ok((t.access_token.clone(), held.generation));
            }
        }
        let fetched = (self.clock)();
        let t = fetch_token(&self.api, None).await?;
        // A refresh must not move the token to another address.
        let url = check_gateway_url(&t.url, &self.api, &t.access_token)?;
        if url != self.url {
            return Err(GatewayError::InsecureUrl(
                "the gateway address changed; connect again".into(),
            ));
        }
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

    fn headers(
        &self,
        b: reqwest::RequestBuilder,
        token: &str,
        session: Option<&str>,
        accept: &str,
    ) -> reqwest::RequestBuilder {
        let b = b
            .bearer_auth(token)
            .header("MCP-Protocol-Version", PROTOCOL_VERSION)
            .header(reqwest::header::ACCEPT, accept);
        match session {
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

    /// Open a session: `initialize`, then the `initialized` notification.
    async fn open_session(&self) -> Result<(), GatewayError> {
        self.request_once(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "quome-cli", "version": env!("CARGO_PKG_VERSION")},
                "_meta": {LOCAL_TOOLS_META: self.local_tools},
            }),
            None,
            REQUEST_TIMEOUT,
        )
        .await?;
        self.notify("notifications/initialized").await
    }

    /// Replace the session `lost`, unless another caller already did.
    async fn reopen_session(&self, lost: Option<String>) -> Result<(), GatewayError> {
        let _opening = self.opening.lock().await;
        if self.session() != lost {
            return Ok(());
        }
        if let Ok(mut s) = self.session.lock() {
            *s = None;
        }
        self.open_session().await
    }

    /// One JSON-RPC request; returns the `result`. A gateway that lost the
    /// session (it may have scaled to zero) gets a new one, once, and the
    /// request is sent again. `progress` is a sink and the progress token its
    /// reports carry.
    async fn request(
        &self,
        method: &str,
        params: Value,
        progress: Option<(&ProgressSink, &str)>,
        timeout: Duration,
    ) -> Result<Value, GatewayError> {
        let session = self.session();
        match self
            .request_once(method, params.clone(), progress, timeout)
            .await
        {
            Err(GatewayError::SessionLost) => {
                self.reopen_session(session).await?;
                self.request_once(method, params, progress, timeout).await
            }
            other => other,
        }
    }

    async fn request_once(
        &self,
        method: &str,
        params: Value,
        progress: Option<(&ProgressSink, &str)>,
        timeout: Duration,
    ) -> Result<Value, GatewayError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        match self.post(&body, Some(id), progress, timeout).await? {
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
            out.extend(self.decode::<Vec<T>>(items)?);
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
        progress: Option<(&ProgressSink, &str)>,
        timeout: Duration,
    ) -> Result<Option<Value>, GatewayError> {
        let mut refreshed = false;
        loop {
            let (token, generation) = self.token().await?;
            let session = self.session();
            let resp = self
                .headers(
                    self.http.post(&self.url),
                    &token,
                    session.as_deref(),
                    "application/json, text/event-stream",
                )
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
            if status == 404 && session.is_some() {
                return Err(GatewayError::SessionLost);
            }
            if !resp.status().is_success() {
                return Err(GatewayError::Http {
                    status,
                    body: read_error_body(resp, &token).await,
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
                let bytes = read_capped(resp, BODY_MAX, &token)
                    .await?
                    .ok_or_else(|| GatewayError::Protocol("the answer is too large".into()))?;
                let v: Value = serde_json::from_slice(&bytes).map_err(|_| {
                    GatewayError::Protocol(snippet(&String::from_utf8_lossy(&bytes), &token))
                })?;
                if v["id"] == json!(id) {
                    v
                } else {
                    return Err(GatewayError::Protocol("answer to another request".into()));
                }
            };
            return answer_result(answer, &token).map(Some);
        }
    }

    /// Handle one event of a call's stream: `Some` when it is the answer to `id`.
    fn on_event(
        &self,
        data: &str,
        id: u64,
        progress: Option<(&ProgressSink, &str)>,
    ) -> Option<Value> {
        let v = serde_json::from_str::<Value>(data).ok()?;
        if v["id"] == json!(id) && (v.get("result").is_some() || v.get("error").is_some()) {
            return Some(v);
        }
        if v["method"] == "notifications/progress" {
            if let Some((sink, token)) = progress {
                if v["params"]["progressToken"].as_str() == Some(token) {
                    sink(ProgressUpdate {
                        progress: v["params"]["progress"].as_f64().unwrap_or(0.0),
                        total: v["params"]["total"].as_f64(),
                        message: v["params"]["message"].as_str().map(str::to_string),
                    });
                }
            }
        } else if v.get("method").is_some() {
            let _ = self.notes.send(v);
        }
        None
    }

    /// Read events until the answer to `id`.
    async fn read_stream(
        &self,
        resp: reqwest::Response,
        id: u64,
        progress: Option<(&ProgressSink, &str)>,
        token: &str,
    ) -> Result<Value, GatewayError> {
        let mut events = SseEvents::default();
        let mut chunks = resp.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk
                .map_err(|e| GatewayError::Transport(scrub(&e.without_url().to_string(), token)))?;
            for data in events.push(&chunk)? {
                if let Some(v) = self.on_event(&data, id, progress) {
                    return Ok(v);
                }
            }
        }
        if let Some(data) = events.finish() {
            if let Some(v) = self.on_event(&data, id, progress) {
                return Ok(v);
            }
        }
        Err(GatewayError::Transport(
            "the answer broke off before it finished".into(),
        ))
    }

    /// Follow the gateway's notification stream. It is reopened after every
    /// close or error, with a growing delay that resets only once an event
    /// was received. A stream that goes quiet past the idle limit is reopened
    /// at once and does not count as a failure. A lost session is opened again
    /// once. It ends for good when the gateway has no stream (404 without a
    /// session, 405), when a 401 survives a refresh, or when the client is
    /// dropped.
    async fn listen(self: Arc<Self>) {
        let mut failures = 0u32;
        let mut refreshed = false;
        let mut reopened = false;
        loop {
            let Ok((token, generation)) = self.token().await else {
                failures += 1;
                tokio::time::sleep(backoff(failures, jitter())).await;
                continue;
            };
            let session = self.session();
            let resp = self
                .headers(
                    self.stream_http.get(&self.url),
                    &token,
                    session.as_deref(),
                    "text/event-stream",
                )
                .send()
                .await;
            let mut idle = false;
            match resp {
                Ok(r) if r.status().as_u16() == 401 => {
                    if refreshed {
                        return;
                    }
                    refreshed = true;
                    self.invalidate(generation).await;
                    continue;
                }
                Ok(r) => {
                    // Only two 401s in a row, with nothing between, end the listener.
                    refreshed = false;
                    let status = r.status().as_u16();
                    if status == 404 && session.is_some() {
                        if reopened {
                            return;
                        }
                        reopened = true;
                        if self.reopen_session(session).await.is_err() {
                            failures += 1;
                            tokio::time::sleep(backoff(failures, jitter())).await;
                        }
                        continue;
                    }
                    if matches!(status, 404 | 405) {
                        return;
                    }
                    if r.status().is_success() {
                        reopened = false;
                        let mut events = SseEvents::default();
                        let mut chunks = r.bytes_stream();
                        let mut got_event = false;
                        'read: while let Some(chunk) = chunks.next().await {
                            let chunk = match chunk {
                                Ok(c) => c,
                                Err(e) => {
                                    idle = is_idle(&e);
                                    break 'read;
                                }
                            };
                            let Ok(datas) = events.push(&chunk) else {
                                break 'read;
                            };
                            for data in datas {
                                if let Ok(v) = serde_json::from_str::<Value>(&data) {
                                    got_event = true;
                                    let _ = self.notes.send(v);
                                }
                            }
                        }
                        if got_event {
                            failures = 0;
                        } else if !idle {
                            failures += 1;
                        }
                    } else {
                        failures += 1;
                    }
                }
                Err(e) if is_idle(&e) => idle = true,
                Err(_) => failures += 1,
            }
            if idle {
                continue;
            }
            tokio::time::sleep(backoff(failures, jitter())).await;
        }
    }
}

/// True when the stream went quiet for longer than the idle limit (not a
/// connection that could not be made).
fn is_idle(e: &reqwest::Error) -> bool {
    e.is_timeout() && !e.is_connect()
}

fn jitter() -> u8 {
    uuid::Uuid::new_v4().as_bytes()[0]
}

/// The `result` of a JSON-RPC answer, or its error (scrubbed of `token`).
fn answer_result(v: Value, token: &str) -> Result<Value, GatewayError> {
    if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
        return Err(GatewayError::Rpc {
            code: err["code"].as_i64().unwrap_or(0),
            message: snippet(err["message"].as_str().unwrap_or("unknown error"), token),
            reason: err["data"]["code"]
                .as_str()
                .filter(|r| {
                    !r.is_empty()
                        && r.len() <= 64
                        && r.chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                })
                .map(str::to_string),
        });
    }
    v.get("result")
        .cloned()
        .ok_or_else(|| GatewayError::Protocol("no result".into()))
}

/// Splits a server-sent event stream into the `data` of each event, within
/// size limits.
#[derive(Default)]
struct SseEvents {
    line: Vec<u8>,
    data: Vec<String>,
    data_len: usize,
}

impl SseEvents {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, GatewayError> {
        let mut out = Vec::new();
        for &b in chunk {
            if b != b'\n' {
                self.line.push(b);
                if self.line.len() > SSE_LINE_MAX {
                    return Err(GatewayError::Protocol("an event line is too long".into()));
                }
                continue;
            }
            let line = String::from_utf8_lossy(&self.line).into_owned();
            self.line.clear();
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                if !self.data.is_empty() {
                    out.push(self.data.join("\n"));
                    self.data.clear();
                    self.data_len = 0;
                }
            } else if let Some(d) = line.strip_prefix("data:") {
                self.data_len += d.len() + 1;
                if self.data_len > SSE_EVENT_MAX {
                    return Err(GatewayError::Protocol("an event is too large".into()));
                }
                self.data.push(d.strip_prefix(' ').unwrap_or(d).to_string());
            }
        }
        Ok(out)
    }

    /// The event left open when the stream ended, if any.
    fn finish(&mut self) -> Option<String> {
        if !self.line.is_empty() {
            let line = String::from_utf8_lossy(&self.line).into_owned();
            self.line.clear();
            if let Some(d) = line.trim_end_matches('\r').strip_prefix("data:") {
                self.data.push(d.strip_prefix(' ').unwrap_or(d).to_string());
            }
        }
        if self.data.is_empty() {
            return None;
        }
        let d = self.data.join("\n");
        self.data.clear();
        Some(d)
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
        Mock::given(method("POST"))
            .and(path("/gw"))
            .and(body_partial_json(json!({"method": "tools/call"})))
            .respond_with(|req: &Request| {
                let b: Value = serde_json::from_slice(&req.body).unwrap();
                let tok = b["params"]["_meta"]["progressToken"].clone();
                let id = b["id"].clone();
                let note = |t: Value, msg: &str| {
                    json!({"jsonrpc":"2.0","method":"notifications/progress",
                           "params":{"progressToken":t,"progress":1,"total":2,"message":msg}})
                };
                let sse = format!(
                    "event: message\ndata: {}\n\ndata: not json\n\ndata: {}\n\ndata: {}\n\n",
                    note(tok, "half"),
                    note(json!("someone-else"), "foreign"),
                    json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":"done"}],"isError":false}}),
                );
                ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream")
            })
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

    /// A JWT-shaped token of about 900 characters.
    fn long_token() -> String {
        let seg = |n: usize, c: &str| c.repeat(n);
        format!(
            "{}.{}.{}",
            seg(40, "eyJhbGciOiJSUzI1NiIs"),
            seg(30, "eyJzdWIiOiJ1c2VyIiw"),
            seg(15, "c2lnbmF0dXJlc2ln")
        )
    }

    fn windows_leaked(text: &str, token: &str) -> bool {
        token
            .as_bytes()
            .windows(16)
            .any(|w| text.contains(std::str::from_utf8(w).unwrap()))
    }

    #[tokio::test]
    async fn token_never_in_error_text() {
        let tok = long_token();
        assert!(tok.len() > 800);
        let (s, api) = setup().await;
        mount_token(&s, &tok, 3600, None).await;
        mount_handshake(&s).await;
        // Http: the body echoes the header, and a bare copy
        Mock::given(method("POST"))
            .and(path("/gw"))
            .and(body_partial_json(json!({"method": "tools/list"})))
            .respond_with(
                ResponseTemplate::new(500)
                    .set_body_string(format!("bad: Authorization: Bearer {tok} echoed {tok}")),
            )
            .mount(&s)
            .await;
        // Rpc: the message echoes it
        let rpc_tok = tok.clone();
        Mock::given(method("POST"))
            .and(path("/gw"))
            .and(body_partial_json(json!({"method": "tools/call"})))
            .respond_with(move |req: &Request| {
                let id = serde_json::from_slice::<Value>(&req.body).unwrap()["id"].clone();
                ResponseTemplate::new(200).set_body_json(json!({"jsonrpc": "2.0", "id": id,
                    "error": {"code": -32000, "message": format!("nope {rpc_tok}")}}))
            })
            .mount(&s)
            .await;
        // Protocol: a 200 that is not JSON and echoes it
        let proto_tok = tok.clone();
        Mock::given(method("POST"))
            .and(path("/gw"))
            .and(body_partial_json(json!({"method": "prompts/list"})))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(format!("<html>{proto_tok}</html>")),
            )
            .mount(&s)
            .await;
        let c = GatewayClient::connect(api, &[]).await.unwrap();
        let errs = [
            c.list_tools().await.err().unwrap(),
            c.call_tool("x", json!({}), None).await.err().unwrap(),
            c.list_prompts().await.err().unwrap(),
        ];
        assert!(matches!(errs[0], GatewayError::Http { .. }));
        assert!(matches!(errs[1], GatewayError::Rpc { .. }));
        assert!(matches!(errs[2], GatewayError::Protocol(_)));
        for e in errs {
            for text in [e.to_string(), format!("{e:?}")] {
                assert!(!windows_leaked(&text, &tok), "{text}");
                assert!(text.contains("<redacted>"), "{text}");
            }
        }
        assert!(!windows_leaked(&c.token_debug().await, &tok));
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

    #[test]
    fn scrub_hides_bearer_values_and_the_token() {
        let t = scrub("a Bearer abc.def ghi tok-1 x", "tok-1");
        assert_eq!(t, "a Bearer <redacted> ghi <redacted> x");
        let long = format!("Bearer {}", "x".repeat(2000));
        assert!(!snippet(&long, "").contains("xxxxxxxx"));
    }

    #[test]
    fn sse_limits_and_final_event() {
        let mut e = SseEvents::default();
        assert!(e.push(&vec![b'a'; SSE_LINE_MAX + 1]).is_err());
        let mut e = SseEvents::default();
        let line = format!("data: {}\n", "a".repeat(SSE_LINE_MAX - 10));
        let mut err = false;
        for _ in 0..(SSE_EVENT_MAX / SSE_LINE_MAX + 2) {
            err |= e.push(line.as_bytes()).is_err();
        }
        assert!(err, "an event without an end must hit the size limit");
        let mut e = SseEvents::default();
        assert!(e.push(b"data: {\"id\":1}").unwrap().is_empty());
        assert_eq!(e.finish().as_deref(), Some("{\"id\":1}"));
    }

    #[test]
    fn backoff_grows_and_is_capped() {
        assert_eq!(backoff(0, 0), Duration::from_secs(1));
        assert_eq!(backoff(3, 0), Duration::from_secs(8));
        assert_eq!(backoff(30, 0), Duration::from_secs(60));
        assert!(backoff(30, 255) >= Duration::from_secs(30));
    }

    #[test]
    fn short_tokens_are_usable_for_a_moment() {
        assert_eq!(usable_for(3600), Duration::from_secs(3570));
        assert_eq!(usable_for(60), Duration::from_secs(30));
        assert_eq!(usable_for(10), Duration::from_secs(5));
        assert_eq!(usable_for(0), Duration::from_secs(1));
    }

    fn starting(retry_after: Option<u64>) -> QuomeError {
        QuomeError::RetryLater {
            message: "503 mcp_starting".into(),
            retry_after,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn zero_retry_after_does_not_hammer_the_platform() {
        let calls = std::cell::Cell::new(0u32);
        let t0 = Instant::now();
        let r = wait_for_token(
            || {
                calls.set(calls.get() + 1);
                std::future::ready(Err(starting(Some(0))))
            },
            START_BUDGET,
            None,
        )
        .await;
        assert!(matches!(r, Err(GatewayError::StillStarting)));
        assert!(calls.get() <= 61, "{} requests", calls.get());
        assert!(t0.elapsed() <= START_BUDGET);
    }

    #[tokio::test(start_paused = true)]
    async fn a_missing_retry_after_waits_five_seconds() {
        let calls = std::cell::Cell::new(0u32);
        let r = wait_for_token(
            || {
                calls.set(calls.get() + 1);
                std::future::ready(Err(starting(None)))
            },
            START_BUDGET,
            None,
        )
        .await;
        assert!(matches!(r, Err(GatewayError::StillStarting)));
        // attempts at 0, 5, ... 55 s; none starts once the budget is spent
        assert_eq!(calls.get(), 12);
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_attempt_counts_against_the_budget() {
        let r = wait_for_token(
            || async {
                tokio::time::sleep(Duration::from_secs(500)).await;
                Err(starting(Some(1)))
            },
            START_BUDGET,
            None,
        )
        .await;
        assert!(matches!(r, Err(GatewayError::StillStarting)));
    }

    #[tokio::test]
    async fn an_oversize_answer_is_refused() {
        let (s, api) = setup().await;
        mount_token(&s, "GWTOK", 3600, None).await;
        mount_handshake(&s).await;
        // an event line longer than the limit
        Mock::given(method("POST"))
            .and(path("/gw"))
            .and(body_partial_json(json!({"method": "tools/call"})))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!("data: {}\n\n", "a".repeat(SSE_LINE_MAX + 10)),
                "text/event-stream",
            ))
            .mount(&s)
            .await;
        // a plain body over the limit
        Mock::given(method("POST"))
            .and(path("/gw"))
            .and(body_partial_json(json!({"method": "tools/list"})))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(vec![b' '; BODY_MAX + 1], "application/json"),
            )
            .mount(&s)
            .await;
        // an error body is only read at its start
        Mock::given(method("POST"))
            .and(path("/gw"))
            .and(body_partial_json(json!({"method": "prompts/list"})))
            .respond_with(ResponseTemplate::new(500).set_body_string("e".repeat(1_000_000)))
            .mount(&s)
            .await;
        let c = GatewayClient::connect(api, &[]).await.unwrap();
        let e = c.call_tool("x", json!({}), None).await.err().unwrap();
        assert!(
            matches!(&e, GatewayError::Protocol(m) if m.contains("too long")),
            "{e}"
        );
        let e = c.list_tools().await.err().unwrap();
        assert!(
            matches!(&e, GatewayError::Protocol(m) if m.contains("too large")),
            "{e}"
        );
        match c.list_prompts().await.err().unwrap() {
            GatewayError::Http { status: 500, body } => assert!(body.len() <= SNIPPET),
            e => panic!("{e}"),
        }
    }

    #[tokio::test]
    async fn the_last_event_without_a_blank_line_is_used() {
        let (s, api) = setup().await;
        mount_token(&s, "GWTOK", 3600, None).await;
        mount_handshake(&s).await;
        Mock::given(method("POST"))
            .and(path("/gw"))
            .and(body_partial_json(json!({"method": "tools/list"})))
            .respond_with(|req: &Request| {
                let id = serde_json::from_slice::<Value>(&req.body).unwrap()["id"].clone();
                let r = json!({"jsonrpc":"2.0","id":id,"result":{"tools":[]}});
                ResponseTemplate::new(200).set_body_raw(format!("data: {r}"), "text/event-stream")
            })
            .mount(&s)
            .await;
        let c = GatewayClient::connect(api, &[]).await.unwrap();
        assert!(c.list_tools().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_lost_session_is_its_own_error() {
        let (s, api) = setup().await;
        mount_token(&s, "GWTOK", 3600, None).await;
        mount_handshake(&s).await;
        Mock::given(method("POST"))
            .and(path("/gw"))
            .and(body_partial_json(json!({"method": "tools/list"})))
            .respond_with(ResponseTemplate::new(404))
            .mount(&s)
            .await;
        let c = GatewayClient::connect(api, &[]).await.unwrap();
        let e = c.list_tools().await.err().unwrap();
        assert!(matches!(e, GatewayError::SessionLost), "{e}");
        assert!(e.to_string().contains("reconnect"));
    }

    #[tokio::test]
    async fn a_failed_refresh_is_an_error_and_the_next_call_retries() {
        let (s, api) = setup().await;
        mount_token(&s, "OLD", 60, Some(1)).await;
        Mock::given(method("POST"))
            .and(path(TOKEN_PATH))
            .respond_with(
                ResponseTemplate::new(403).set_body_json(json!({"reason": "capability_missing"})),
            )
            .mount(&s)
            .await;
        mount_handshake(&s).await;
        mount_rpc(&s, "tools/list", tools_result()).await;
        let skew = Arc::new(AtomicU64::new(0));
        let clock = {
            let skew = skew.clone();
            let base = std::time::Instant::now();
            Arc::new(move || base + Duration::from_secs(skew.load(Ordering::Relaxed)))
        };
        let c = GatewayClient::connect_with(api, &[], None, Options::default().clock(clock))
            .await
            .unwrap();
        skew.store(100, Ordering::Relaxed);
        let e = c.list_tools().await.err().unwrap();
        assert!(matches!(e, GatewayError::MissingCapability), "{e}");
        let e = c.list_tools().await.err().unwrap();
        assert!(matches!(e, GatewayError::MissingCapability), "{e}");
    }

    #[tokio::test]
    async fn a_refresh_may_not_move_the_gateway() {
        let (s, api) = setup().await;
        mount_token(&s, "OLD", 60, Some(1)).await;
        Mock::given(method("POST"))
            .and(path(TOKEN_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"access_token": "NEW", "expires_in": 60, "url": "http://127.0.0.1:9/elsewhere"}),
            ))
            .mount(&s)
            .await;
        mount_handshake(&s).await;
        let skew = Arc::new(AtomicU64::new(0));
        let clock = {
            let skew = skew.clone();
            let base = std::time::Instant::now();
            Arc::new(move || base + Duration::from_secs(skew.load(Ordering::Relaxed)))
        };
        let c = GatewayClient::connect_with(api, &[], None, Options::default().clock(clock))
            .await
            .unwrap();
        skew.store(100, Ordering::Relaxed);
        let e = c.list_tools().await.err().unwrap();
        assert!(matches!(e, GatewayError::InsecureUrl(_)), "{e}");
    }

    #[tokio::test]
    async fn a_short_lived_token_is_not_refetched_per_request() {
        let (s, api) = setup().await;
        mount_token(&s, "SHORT", 10, None).await;
        mount_handshake(&s).await;
        mount_rpc(&s, "tools/list", tools_result()).await;
        let c = GatewayClient::connect(api, &[]).await.unwrap();
        c.list_tools().await.unwrap();
        c.list_tools().await.unwrap();
        assert_eq!(requests_to(&s, TOKEN_PATH).await.len(), 1);
    }

    #[tokio::test]
    async fn notifications_arrive_from_the_gateway_stream() {
        let (s, api) = setup().await;
        mount_token(&s, "GWTOK", 3600, None).await;
        mount_handshake(&s).await;
        Mock::given(method("GET"))
            .and(path("/gw"))
            .and(header("mcp-session-id", "sess-1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(300))
                    .set_body_raw(
                        "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\n",
                        "text/event-stream",
                    ),
            )
            .mount(&s)
            .await;
        let c = GatewayClient::connect(api, &[]).await.unwrap();
        let mut rx = c.notifications();
        let v = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(v["method"], "notifications/tools/list_changed");
    }

    #[tokio::test]
    async fn the_stream_listener_stops_when_the_gateway_has_none() {
        let (s, api) = setup().await;
        mount_token(&s, "GWTOK", 3600, None).await;
        mount_handshake(&s).await;
        Mock::given(method("GET"))
            .and(path("/gw"))
            .respond_with(ResponseTemplate::new(405))
            .mount(&s)
            .await;
        let c = GatewayClient::connect(api, &[]).await.unwrap();
        let h = c.stream.as_ref().unwrap();
        for _ in 0..50 {
            if h.is_finished() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(h.is_finished());
    }

    /// A mock whose first answer to `method_name` is a 404 (a gateway that
    /// scaled to zero and lost its sessions), and whose handshake hands out
    /// a new session id each time.
    async fn mount_handshake_counting(s: &MockServer) {
        let n = Arc::new(AtomicU64::new(0));
        Mock::given(method("POST"))
            .and(path("/gw"))
            .and(body_partial_json(json!({"method": "initialize"})))
            .respond_with(move |req: &Request| {
                let id = serde_json::from_slice::<Value>(&req.body).unwrap()["id"].clone();
                let k = n.fetch_add(1, Ordering::SeqCst) + 1;
                ResponseTemplate::new(200)
                    .insert_header("mcp-session-id", format!("sess-{k}").as_str())
                    .set_body_json(json!({"jsonrpc": "2.0", "id": id, "result": {
                        "protocolVersion": "2025-06-18", "capabilities": {},
                        "serverInfo": {"name": "gw", "version": "1"}}}))
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

    async fn count_method(s: &MockServer, m: &str) -> usize {
        requests_to(s, "/gw")
            .await
            .iter()
            .filter(|r| body(r)["method"] == m)
            .count()
    }

    #[tokio::test]
    async fn a_lost_session_is_opened_again_and_the_request_retried() {
        let (s, api) = setup().await;
        mount_token(&s, "GWTOK", 3600, None).await;
        mount_handshake_counting(&s).await;
        Mock::given(method("POST"))
            .and(path("/gw"))
            .and(body_partial_json(json!({"method": "tools/list"})))
            .and(header("mcp-session-id", "sess-1"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&s)
            .await;
        mount_rpc(&s, "tools/list", tools_result()).await;
        let c = GatewayClient::connect_with(
            api,
            &["exec"],
            None,
            Options::default().listen_idle(Duration::from_secs(30)),
        )
        .await
        .unwrap();
        let tools = c.list_tools().await.unwrap();
        assert_eq!(tools[0].name, "x");
        assert_eq!(count_method(&s, "initialize").await, 2);
        let last = requests_to(&s, "/gw").await.into_iter().last().unwrap();
        assert_eq!(last.headers.get("mcp-session-id").unwrap(), "sess-2");
        // the new session declares the same local tools
        let inits: Vec<_> = requests_to(&s, "/gw")
            .await
            .iter()
            .map(body)
            .filter(|b| b["method"] == "initialize")
            .collect();
        assert_eq!(
            inits[1]["params"]["_meta"]["cloud.quome/local_tools"],
            json!(["exec"])
        );
    }

    #[tokio::test]
    async fn a_lost_session_is_opened_again_only_once_per_request() {
        let (s, api) = setup().await;
        mount_token(&s, "GWTOK", 3600, None).await;
        mount_handshake_counting(&s).await;
        Mock::given(method("POST"))
            .and(path("/gw"))
            .and(body_partial_json(json!({"method": "tools/list"})))
            .respond_with(ResponseTemplate::new(404))
            .mount(&s)
            .await;
        let c = GatewayClient::connect(api, &[]).await.unwrap();
        let e = c.list_tools().await.err().unwrap();
        assert!(matches!(e, GatewayError::SessionLost), "{e}");
        assert_eq!(count_method(&s, "initialize").await, 2);
        assert_eq!(count_method(&s, "tools/list").await, 2);
    }

    #[tokio::test]
    async fn the_listener_survives_401_then_500_then_401() {
        let (s, api) = setup().await;
        mount_token(&s, "GWTOK", 3600, None).await;
        mount_handshake(&s).await;
        for (status, n) in [(401, 1), (500, 1), (401, 1)] {
            Mock::given(method("GET"))
                .and(path("/gw"))
                .respond_with(ResponseTemplate::new(status))
                .up_to_n_times(n)
                .with_priority(1)
                .mount(&s)
                .await;
        }
        Mock::given(method("GET"))
            .and(path("/gw"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\n",
                "text/event-stream",
            ))
            .with_priority(5)
            .mount(&s)
            .await;
        let c = GatewayClient::connect(api, &[]).await.unwrap();
        let mut rx = c.notifications();
        let v = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("the listener gave up")
            .unwrap();
        assert_eq!(v["method"], "notifications/tools/list_changed");
    }

    #[tokio::test]
    async fn a_quiet_stream_timing_out_is_not_a_failure() {
        let (s, api) = setup().await;
        mount_token(&s, "GWTOK", 3600, None).await;
        mount_handshake(&s).await;
        // The first stream says nothing for longer than the idle limit.
        Mock::given(method("GET"))
            .and(path("/gw"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(400))
                    .set_body_raw("", "text/event-stream"),
            )
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/gw"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\n",
                "text/event-stream",
            ))
            .with_priority(5)
            .mount(&s)
            .await;
        let t0 = Instant::now();
        let c = GatewayClient::connect_with(
            api,
            &[],
            None,
            Options::default().listen_idle(Duration::from_millis(100)),
        )
        .await
        .unwrap();
        let mut rx = c.notifications();
        tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .unwrap()
            .unwrap();
        // a failure would have waited at least a second before reopening
        assert!(
            t0.elapsed() < Duration::from_millis(900),
            "{:?}",
            t0.elapsed()
        );
    }

    #[tokio::test]
    async fn the_notification_stream_sends_one_accept_header() {
        let (s, api) = setup().await;
        mount_token(&s, "GWTOK", 3600, None).await;
        mount_handshake(&s).await;
        Mock::given(method("GET"))
            .and(path("/gw"))
            .respond_with(ResponseTemplate::new(405))
            .mount(&s)
            .await;
        let c = GatewayClient::connect(api, &[]).await.unwrap();
        let h = c.stream.as_ref().unwrap();
        for _ in 0..50 {
            if h.is_finished() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let gets: Vec<_> = requests_to(&s, "/gw")
            .await
            .into_iter()
            .filter(|r| r.method.as_str() == "GET")
            .collect();
        assert!(!gets.is_empty());
        let accepts: Vec<_> = gets[0].headers.get_all("accept").iter().collect();
        assert_eq!(accepts.len(), 1, "{accepts:?}");
        assert_eq!(accepts[0], "text/event-stream");
    }

    #[tokio::test(start_paused = true)]
    async fn the_start_wait_never_passes_the_budget() {
        let t0 = tokio::time::Instant::now();
        let calls = Arc::new(AtomicU64::new(0));
        let c2 = calls.clone();
        let r = wait_for_token(
            move || {
                let n = c2.fetch_add(1, Ordering::SeqCst);
                async move {
                    if n == 0 {
                        tokio::time::sleep(Duration::from_millis(58_500)).await;
                        Err(starting(Some(1)))
                    } else {
                        std::future::pending().await
                    }
                }
            },
            START_BUDGET,
            None,
        )
        .await;
        assert!(matches!(r, Err(GatewayError::StillStarting)));
        assert!(t0.elapsed() <= START_BUDGET, "{:?}", t0.elapsed());
    }

    #[tokio::test]
    async fn an_rpc_error_keeps_its_code_name() {
        let (s, api) = setup().await;
        mount_token(&s, "GWTOK", 3600, None).await;
        mount_handshake(&s).await;
        Mock::given(method("POST"))
            .and(path("/gw"))
            .and(body_partial_json(json!({"method": "prompts/get"})))
            .respond_with(|req: &Request| {
                let id = serde_json::from_slice::<Value>(&req.body).unwrap()["id"].clone();
                ResponseTemplate::new(200).set_body_json(json!({"jsonrpc": "2.0", "id": id,
                    "error": {"code": -32602, "message": "no such prompt",
                              "data": {"code": "prompt_unknown"}}}))
            })
            .mount(&s)
            .await;
        let c = GatewayClient::connect(api, &[]).await.unwrap();
        let e = c.get_prompt("x", BTreeMap::new()).await.err().unwrap();
        match e {
            GatewayError::Rpc { code, reason, .. } => {
                assert_eq!(code, -32602);
                assert_eq!(reason.as_deref(), Some("prompt_unknown"));
            }
            other => panic!("{other}"),
        }
    }

    #[test]
    fn the_front_door_override_names_one_of_two() {
        assert_eq!(parse_front_door("gateway"), Some(FrontDoor::Gateway));
        assert_eq!(parse_front_door(" Direct "), Some(FrontDoor::Direct));
        assert_eq!(parse_front_door("relay"), None);
        assert_eq!(parse_front_door(""), None);
    }
}
