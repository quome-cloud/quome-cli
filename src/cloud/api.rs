//! Quome Cloud API client: bearer-authenticated calls under `<api_url>/api/v1`.
use super::auth::{TokenSource, CONNECT_TIMEOUT, REQUEST_TIMEOUT};
use super::state::Profile;
use crate::errors::QuomeError;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use reqwest::{Method, RequestBuilder, Response};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// Largest body of one upload request.
pub const UPLOAD_CHUNK: usize = 8 * 1024 * 1024;
/// Largest exec timeout the CLI sends (the edge cuts requests at 30 s).
pub const MAX_EXEC_TIMEOUT: u32 = 25;
pub const MEMORY_SIZES: [f64; 8] = [0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0];
pub const RUNTIMES: [&str; 4] = ["claude", "codex", "opencode", "antigravity"];

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct OrgInfo {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub role: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Me {
    pub id: Option<String>,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub orgs: Vec<OrgInfo>,
    pub principal_kind: String,
    pub apps_domain: String,
    pub api_origin: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Port {
    pub port: u16,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Ports {
    pub detected: Vec<u16>,
    pub exposed: Vec<Port>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Sandbox {
    pub id: String,
    pub name: String,
    pub status: String,
    pub runtime: String,
    pub memory_gb: f64,
    #[serde(default)]
    pub failure_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i64,
    pub truncated: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct FileEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub size: u64,
    pub mtime: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct FileList {
    pub path: String,
    pub entries: Vec<FileEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Service {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub memory_gb: f64,
    pub status: String,
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct SourceUpload {
    pub source_key: String,
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Deploy {
    pub deploy_id: String,
    pub build_id: String,
    pub workflow_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Build {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub image_digest: Option<String>,
    #[serde(default)]
    pub log_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct CatalogApp {
    pub slug: String,
    pub name: String,
    pub summary: String,
    pub available: bool,
    #[serde(default)]
    pub installed_status: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AppInstall {
    pub slug: String,
    pub name: String,
    pub status: String,
    #[serde(default)]
    pub url: Option<String>,
    pub agents_enabled: bool,
    #[serde(default)]
    pub error: Option<String>,
    /// Billing note, present on the install response only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billing: Option<String>,
    /// The server's note (for example on uninstall: data is kept).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ToolHit {
    pub app: String,
    pub tool: String,
    pub summary: String,
    pub destructive: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ToolSearch {
    pub results: Vec<ToolHit>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ToolDetail {
    pub app: String,
    pub tool: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub destructive: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AppKeyStatus {
    pub key: String,
    pub label: String,
    pub unlocks: String,
    pub set: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AppKeys {
    pub keys: Vec<AppKeyStatus>,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// Longest single server wait for mail (the edge cuts requests at 30 s).
pub const MAX_EMAIL_WAIT: u32 = 25;
/// Per-request HTTP timeout for mail waits: longer than the edge's 30 s cut,
/// so that cut shows up as a 504 before this client gives up.
pub const EMAIL_WAIT_HTTP_TIMEOUT: Duration = Duration::from_secs(40);

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct EmailAddress {
    pub id: String,
    pub address: String,
    pub kind: String,
    #[serde(default)]
    pub label: Option<String>,
    pub status: String,
    pub forward: bool,
    pub owner_user_id: String,
    #[serde(default)]
    pub sandbox_id: Option<String>,
    #[serde(default)]
    pub created_by_actor: Option<String>,
    #[serde(default)]
    pub created_by_on_behalf_of: Option<String>,
    pub created_at: String,
    #[serde(default)]
    pub last_message_at: Option<String>,
}

/// One short server wait for mail.
#[derive(Debug)]
pub enum WaitStep {
    /// A matching message: the server's envelope, unchanged.
    Message(serde_json::Value),
    /// Nothing matched; pass `now` as the next `since`.
    NoMatch { now: String },
    /// The server (or the edge) was busy; retry with the same cursor.
    Busy,
}

/// Lowercase an address label and check it is 1-20 of `[a-z0-9-]`.
pub fn check_label(label: &str) -> Result<String, QuomeError> {
    let l = label.to_lowercase();
    let ok = (1..=20).contains(&l.len())
        && l.bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-');
    if ok {
        Ok(l)
    } else {
        Err(QuomeError::Usage(
            "Label must be 1-20 characters: lowercase letters, digits or hyphens, such as the \
             service name."
                .into(),
        ))
    }
}

/// Whether a wait error means "busy, try again": any 429, a 502/503/504, or
/// this client's HTTP timeout.
fn wait_busy(e: &QuomeError) -> bool {
    match e {
        QuomeError::RateLimited => true,
        QuomeError::ApiError(m) => ["502", "503", "504"]
            .iter()
            .any(|c| m == c || m.starts_with(&format!("{c} "))),
        QuomeError::Http(h) => h.is_timeout(),
        _ => false,
    }
}

/// The result of a size-capped download.
#[derive(Debug, PartialEq)]
pub enum Download {
    Body(Vec<u8>),
    /// Over the cap; the size when the server declared it.
    TooLarge(Option<u64>),
}

pub struct CloudApi {
    http: reqwest::Client,
    base: String,
    org: String,
    tokens: TokenSource,
}

fn valid_name(name: &str) -> bool {
    // ^[a-z][a-z0-9-]{1,30}$
    let b = name.as_bytes();
    (2..=31).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

fn check_name_size(name: &str, memory_gb: f64) -> Result<(), QuomeError> {
    if !valid_name(name) {
        return Err(QuomeError::Usage(
            "Name must be 2-31 characters: a lowercase letter, then lowercase letters, digits or \
             hyphens."
                .into(),
        ));
    }
    if !MEMORY_SIZES.contains(&memory_gb) {
        return Err(QuomeError::Usage(
            "Memory must be one of 0.5, 1, 2, 4, 8, 16, 32, 64 (GB).".into(),
        ));
    }
    Ok(())
}

/// Per-request timeout for uploads (source tars and file chunks).
pub const UPLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// Path-segment encoding: everything but unreserved characters (no `.`, so `..` cannot survive).
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_');

fn enc(s: &str) -> String {
    utf8_percent_encode(s, SEGMENT).to_string()
}

fn is_session_reason(reason: &str) -> bool {
    reason == "delegation_revoked" || reason == "agent_not_connected"
}

/// Map a non-success status and body to the right error.
/// Most of a build log handed to an agent: enough for the failing step.
pub const LOG_TAIL_MAX: usize = 6 * 1024;

/// The last `max` bytes of `log`, starting at a line boundary when one exists.
pub fn log_tail(log: &str, max: usize) -> String {
    let log = log.trim_end();
    if log.len() <= max {
        return log.to_string();
    }
    let mut start = log.len() - max;
    while !log.is_char_boundary(start) {
        start += 1;
    }
    let cut = &log[start..];
    match cut.find('\n') {
        Some(i) if i + 1 < cut.len() => cut[i + 1..].to_string(),
        _ => cut.to_string(),
    }
}

fn map_error(status: u16, text: &str) -> QuomeError {
    let v: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    let reason = v["reason"].as_str().unwrap_or("");
    let detail = match &v["detail"] {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    };
    match status {
        401 if reason == "delegation_revoked" => QuomeError::CloudDisconnected,
        401 if reason == "agent_not_connected" => QuomeError::CloudNotYetConnected,
        404 => QuomeError::NotFound(if reason.is_empty() {
            detail
        } else {
            reason.into()
        }),
        429 => QuomeError::RateLimited,
        code => {
            let message = v["message"].as_str().unwrap_or("");
            let msg = [detail.as_str(), reason, message]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(": ");
            QuomeError::ApiError(format!("{code} {msg}").trim().to_string())
        }
    }
}

async fn check(resp: Response) -> Result<Response, QuomeError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let text = resp.text().await.unwrap_or_default();
    Err(map_error(status.as_u16(), &text))
}

impl CloudApi {
    pub fn new(profile: &Profile, tokens: TokenSource) -> CloudApi {
        Self::with_timeout(profile, tokens, REQUEST_TIMEOUT)
    }

    pub(crate) fn with_timeout(
        profile: &Profile,
        tokens: TokenSource,
        timeout: Duration,
    ) -> CloudApi {
        CloudApi {
            http: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(timeout)
                .build()
                .unwrap_or_default(),
            base: format!("{}/api/v1", profile.api_url.trim_end_matches('/')),
            org: profile.org_id.clone(),
            tokens,
        }
    }

    /// Send an authenticated request. A 401 that is not a known session reason
    /// drops the cached token, re-exchanges and retries once.
    async fn send(
        &self,
        m: Method,
        path: &str,
        build: impl Fn(RequestBuilder) -> RequestBuilder,
    ) -> Result<Response, QuomeError> {
        let url = format!("{}{}", self.base, path);
        let mut retried = false;
        loop {
            let token = self.tokens.access_token().await?;
            let resp = build(self.http.request(m.clone(), &url).bearer_auth(&token))
                .send()
                .await?;
            if resp.status().as_u16() != 401 {
                return check(resp).await;
            }
            let text = resp.text().await.unwrap_or_default();
            let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
            if retried || is_session_reason(v["reason"].as_str().unwrap_or("")) {
                return Err(map_error(401, &text));
            }
            retried = true;
            self.tokens.invalidate(&token);
        }
    }

    fn org_path(&self, rest: &str) -> String {
        format!("/orgs/{}{}", enc(&self.org), rest)
    }

    async fn json<T: serde::de::DeserializeOwned>(
        &self,
        m: Method,
        path: &str,
        build: impl Fn(RequestBuilder) -> RequestBuilder,
    ) -> Result<T, QuomeError> {
        Ok(self.send(m, path, build).await?.json().await?)
    }

    pub async fn me(&self) -> Result<Me, QuomeError> {
        self.json(Method::GET, "/me", |b| b).await
    }

    pub async fn agent_guide(&self) -> Result<String, QuomeError> {
        let p = self.org_path("/agent-guide");
        Ok(self.send(Method::GET, &p, |b| b).await?.text().await?)
    }

    pub async fn list_sandboxes(&self) -> Result<Vec<Sandbox>, QuomeError> {
        self.json(Method::GET, &self.org_path("/sandboxes"), |b| b)
            .await
    }

    pub async fn get_sandbox(&self, id: &str) -> Result<Sandbox, QuomeError> {
        let p = self.org_path(&format!("/sandboxes/{}", enc(id)));
        self.json(Method::GET, &p, |b| b).await
    }

    pub async fn create_sandbox(
        &self,
        name: &str,
        memory_gb: f64,
        runtime: &str,
    ) -> Result<Sandbox, QuomeError> {
        check_name_size(name, memory_gb)?;
        if !RUNTIMES.contains(&runtime) {
            return Err(QuomeError::Usage(format!(
                "Runtime must be one of {}.",
                RUNTIMES.join(", ")
            )));
        }
        let body = serde_json::json!({"name": name, "memory_gb": memory_gb, "runtime": runtime});
        self.json(Method::POST, &self.org_path("/sandboxes"), |b| {
            b.json(&body)
        })
        .await
    }

    pub async fn stop_sandbox(&self, id: &str) -> Result<Sandbox, QuomeError> {
        let p = self.org_path(&format!("/sandboxes/{}/stop", enc(id)));
        self.json(Method::POST, &p, |b| b).await
    }

    pub async fn exec(
        &self,
        id: &str,
        command: &str,
        timeout: u32,
    ) -> Result<ExecResult, QuomeError> {
        if timeout == 0 || timeout > MAX_EXEC_TIMEOUT {
            return Err(QuomeError::Usage(format!(
                "Timeout must be 1-{MAX_EXEC_TIMEOUT} seconds."
            )));
        }
        let p = self.org_path(&format!("/sandboxes/{}/exec", enc(id)));
        let body = serde_json::json!({"command": command, "timeout": timeout, "working_dir": "/workspace"});
        self.json(Method::POST, &p, |b| b.json(&body)).await
    }

    pub async fn list_files(&self, id: &str, path: &str) -> Result<FileList, QuomeError> {
        let p = self.org_path(&format!("/sandboxes/{}/files/list", enc(id)));
        self.json(Method::GET, &p, |b| b.query(&[("path", path)]))
            .await
    }

    /// Download a file, refusing more than `max` bytes. The server sets
    /// `Content-Length`, so an oversize file is refused before its body is read;
    /// without one the read stops as soon as it passes `max`.
    pub async fn download(&self, id: &str, path: &str, max: usize) -> Result<Download, QuomeError> {
        let p = self.org_path(&format!("/sandboxes/{}/files/download", enc(id)));
        let mut r = self
            .send(Method::GET, &p, |b| b.query(&[("path", path)]))
            .await?;
        if let Some(len) = r.content_length() {
            if len > max as u64 {
                return Ok(Download::TooLarge(Some(len)));
            }
        }
        let mut body = Vec::new();
        while let Some(chunk) = r.chunk().await? {
            body.extend_from_slice(&chunk);
            if body.len() > max {
                return Ok(Download::TooLarge(None));
            }
        }
        Ok(Download::Body(body))
    }

    /// Upload `bytes` to `path`: one request up to 8 MiB, otherwise sequential
    /// chunks sharing one `upload_id`, the last marked `final=true`.
    pub async fn upload(&self, id: &str, path: &str, bytes: &[u8]) -> Result<(), QuomeError> {
        let p = self.org_path(&format!("/sandboxes/{}/workspace-files", enc(id)));
        if bytes.len() <= UPLOAD_CHUNK {
            self.send(Method::PUT, &p, |b| {
                b.query(&[("path", path)])
                    .timeout(UPLOAD_TIMEOUT)
                    .body(bytes.to_vec())
            })
            .await?;
            return Ok(());
        }
        let upload_id = uuid::Uuid::new_v4().simple().to_string();
        let total = bytes.len().div_ceil(UPLOAD_CHUNK);
        for (i, chunk) in bytes.chunks(UPLOAD_CHUNK).enumerate() {
            let offset = (i * UPLOAD_CHUNK).to_string();
            let last = if i + 1 == total { "true" } else { "false" };
            self.send(Method::PUT, &p, |b| {
                b.query(&[
                    ("path", path),
                    ("upload_id", upload_id.as_str()),
                    ("offset", offset.as_str()),
                    ("final", last),
                ])
                .timeout(UPLOAD_TIMEOUT)
                .body(chunk.to_vec())
            })
            .await?;
        }
        Ok(())
    }

    pub async fn ports(&self, id: &str) -> Result<Ports, QuomeError> {
        let p = self.org_path(&format!("/sandboxes/{}/ports", enc(id)));
        self.json(Method::GET, &p, |b| b).await
    }

    pub async fn expose_port(&self, id: &str, port: u16) -> Result<Port, QuomeError> {
        let p = self.org_path(&format!("/sandboxes/{}/ports", enc(id)));
        let body = serde_json::json!({"port": port});
        self.json(Method::POST, &p, |b| b.json(&body)).await
    }

    pub async fn list_services(&self) -> Result<Vec<Service>, QuomeError> {
        self.json(Method::GET, &self.org_path("/services"), |b| b)
            .await
    }

    pub async fn app_catalog(&self) -> Result<Vec<CatalogApp>, QuomeError> {
        self.json(Method::GET, &self.org_path("/app-catalog"), |b| b)
            .await
    }

    pub async fn install_app(&self, slug: &str) -> Result<AppInstall, QuomeError> {
        let body = serde_json::json!({"slug": slug, "accept_billing": true});
        self.json(Method::POST, &self.org_path("/app-installs"), |b| {
            b.json(&body)
        })
        .await
    }

    pub async fn list_app_installs(&self) -> Result<Vec<AppInstall>, QuomeError> {
        self.json(Method::GET, &self.org_path("/app-installs"), |b| b)
            .await
    }

    pub async fn app_install(&self, slug: &str) -> Result<AppInstall, QuomeError> {
        let p = self.org_path(&format!("/app-installs/{}", enc(slug)));
        self.json(Method::GET, &p, |b| b).await
    }

    pub async fn app_launch_url(&self, slug: &str) -> Result<String, QuomeError> {
        let p = self.org_path(&format!("/app-installs/{}/launch-url", enc(slug)));
        let v: serde_json::Value = self.json(Method::GET, &p, |b| b).await?;
        match v["url"].as_str() {
            Some(u) if !u.is_empty() => Ok(u.to_string()),
            _ => Err(QuomeError::ApiError(
                "the launch-url response had no url".into(),
            )),
        }
    }

    pub async fn find_app_tools(
        &self,
        q: Option<&str>,
        app: Option<&str>,
    ) -> Result<ToolSearch, QuomeError> {
        let mut query: Vec<(&str, &str)> = Vec::new();
        if let Some(q) = q {
            query.push(("q", q));
        }
        if let Some(app) = app {
            query.push(("app", app));
        }
        self.json(Method::GET, &self.org_path("/app-installs/tools"), |b| {
            b.query(&query)
        })
        .await
    }

    pub async fn describe_app_tool(
        &self,
        slug: &str,
        tool: &str,
    ) -> Result<ToolDetail, QuomeError> {
        let p = self.org_path(&format!("/app-installs/{}/tools/{}", enc(slug), enc(tool)));
        self.json(Method::GET, &p, |b| b).await
    }

    pub async fn call_app_tool(
        &self,
        slug: &str,
        tool: &str,
        arguments: &serde_json::Value,
    ) -> Result<serde_json::Value, QuomeError> {
        let p = self.org_path(&format!(
            "/app-installs/{}/tools/{}/call",
            enc(slug),
            enc(tool)
        ));
        let body = serde_json::json!({"arguments": arguments});
        self.json(Method::POST, &p, |b| b.json(&body)).await
    }

    pub async fn app_keys(&self, slug: &str) -> Result<AppKeys, QuomeError> {
        let p = self.org_path(&format!("/app-installs/{}/keys", enc(slug)));
        self.json(Method::GET, &p, |b| b).await
    }

    /// Stores the key; returns whether the server is delivering it to the running app.
    pub async fn set_app_key(
        &self,
        slug: &str,
        key: &str,
        value: &str,
    ) -> Result<bool, QuomeError> {
        let p = self.org_path(&format!("/app-installs/{}/keys/{}", enc(slug), enc(key)));
        let body = serde_json::json!({"value": value});
        let v: serde_json::Value = self.json(Method::PUT, &p, |b| b.json(&body)).await?;
        Ok(v["delivering"].as_bool().unwrap_or(false))
    }

    pub async fn uninstall_app(&self, slug: &str) -> Result<AppInstall, QuomeError> {
        let p = self.org_path(&format!("/app-installs/{}", enc(slug)));
        let body = serde_json::json!({"confirm": true});
        self.json(Method::DELETE, &p, |b| b.json(&body)).await
    }

    pub async fn delete_app_data(&self, slug: &str) -> Result<AppInstall, QuomeError> {
        let p = self.org_path(&format!("/app-installs/{}/delete-data", enc(slug)));
        let body = serde_json::json!({"confirm": true, "slug": slug});
        self.json(Method::POST, &p, |b| b.json(&body)).await
    }

    pub async fn email_create_address(
        &self,
        kind: &str,
        label: &str,
        sandbox_id: Option<&str>,
        forward: bool,
    ) -> Result<EmailAddress, QuomeError> {
        let label = check_label(label)?;
        let mut body = serde_json::json!({"kind": kind, "label": label, "forward": forward});
        if let Some(sb) = sandbox_id {
            body["sandbox_id"] = serde_json::Value::String(sb.into());
        }
        self.json(Method::POST, &self.org_path("/email/addresses"), |b| {
            b.json(&body)
        })
        .await
    }

    pub async fn email_list_addresses(&self) -> Result<Vec<EmailAddress>, QuomeError> {
        #[derive(Deserialize)]
        struct List {
            addresses: Vec<EmailAddress>,
        }
        let l: List = self
            .json(Method::GET, &self.org_path("/email/addresses"), |b| b)
            .await?;
        Ok(l.addresses)
    }

    pub async fn email_disable(&self, id: &str) -> Result<EmailAddress, QuomeError> {
        let p = self.org_path(&format!("/email/addresses/{}", enc(id)));
        let body = serde_json::json!({"status": "disabled"});
        self.json(Method::PATCH, &p, |b| b.json(&body)).await
    }

    /// An address's messages, newest first: the whole response body.
    pub async fn email_list_messages(
        &self,
        id: &str,
        since: Option<&str>,
        limit: Option<u32>,
    ) -> Result<serde_json::Value, QuomeError> {
        let p = self.org_path(&format!("/email/addresses/{}/messages", enc(id)));
        let limit = limit.map(|l| l.to_string());
        let mut query: Vec<(&str, &str)> = Vec::new();
        if let Some(since) = since {
            query.push(("since", since));
        }
        if let Some(l) = &limit {
            query.push(("limit", l));
        }
        self.json(Method::GET, &p, |b| b.query(&query)).await
    }

    /// One message's envelope, exactly as the server sent it (never HTML).
    pub async fn email_message(&self, id: &str) -> Result<serde_json::Value, QuomeError> {
        let p = self.org_path(&format!("/email/messages/{}", enc(id)));
        self.json(Method::GET, &p, |b| b).await
    }

    /// One server wait of `timeout_s` (1-`MAX_EMAIL_WAIT`) seconds for mail.
    pub async fn email_wait(
        &self,
        id: &str,
        since: Option<&str>,
        from_contains: Option<&str>,
        subject_contains: Option<&str>,
        timeout_s: u32,
    ) -> Result<WaitStep, QuomeError> {
        if timeout_s == 0 || timeout_s > MAX_EMAIL_WAIT {
            return Err(QuomeError::Usage(format!(
                "Wait timeout must be 1-{MAX_EMAIL_WAIT} seconds."
            )));
        }
        let p = self.org_path(&format!("/email/addresses/{}/wait", enc(id)));
        let t = timeout_s.to_string();
        let mut query: Vec<(&str, &str)> = vec![("timeout_s", t.as_str())];
        for (k, v) in [
            ("since", since),
            ("from_contains", from_contains),
            ("subject_contains", subject_contains),
        ] {
            if let Some(v) = v.filter(|v| !v.is_empty()) {
                query.push((k, v));
            }
        }
        let resp = self
            .send(Method::GET, &p, |b| {
                b.query(&query).timeout(EMAIL_WAIT_HTTP_TIMEOUT)
            })
            .await;
        let v: serde_json::Value = match resp {
            Ok(r) => match r.json().await {
                Ok(v) => v,
                Err(e) if e.is_timeout() => return Ok(WaitStep::Busy),
                Err(e) => return Err(e.into()),
            },
            Err(e) if wait_busy(&e) => return Ok(WaitStep::Busy),
            Err(e) => return Err(e),
        };
        if !v["message"].is_null() {
            return Ok(WaitStep::Message(v["message"].clone()));
        }
        match v["now"].as_str() {
            Some(now) => Ok(WaitStep::NoMatch { now: now.into() }),
            None => Err(QuomeError::InvalidResponse),
        }
    }

    pub async fn create_app(&self, name: &str, memory_gb: f64) -> Result<Service, QuomeError> {
        check_name_size(name, memory_gb)?;
        let body = serde_json::json!({"kind": "app", "name": name, "memory_gb": memory_gb});
        self.json(Method::POST, &self.org_path("/services"), |b| b.json(&body))
            .await
    }

    pub async fn upload_source(
        &self,
        app_id: &str,
        tar_path: &Path,
    ) -> Result<SourceUpload, QuomeError> {
        let bytes = tokio::fs::read(tar_path).await?;
        let p = format!(
            "/uploads/orgs/{}/apps/{}/source",
            enc(&self.org),
            enc(app_id)
        );
        self.json(Method::PUT, &p, |b| {
            b.timeout(UPLOAD_TIMEOUT).body(bytes.clone())
        })
        .await
    }

    pub async fn deploy(&self, app_id: &str, source_key: &str) -> Result<Deploy, QuomeError> {
        let p = self.org_path(&format!("/apps/{}/deploy", enc(app_id)));
        let body = serde_json::json!({"source_key": source_key});
        self.json(Method::POST, &p, |b| b.json(&body)).await
    }

    /// An app's builds, newest first (the server returns up to 100).
    pub async fn list_builds(&self, app_id: &str) -> Result<Vec<Build>, QuomeError> {
        let p = self.org_path(&format!("/apps/{}/builds", enc(app_id)));
        self.json(Method::GET, &p, |b| b).await
    }

    pub async fn build(&self, app_id: &str, build_id: &str) -> Result<Build, QuomeError> {
        let p = self.org_path(&format!("/apps/{}/builds/{}", enc(app_id), enc(build_id)));
        self.json(Method::GET, &p, |b| b).await
    }

    /// The end of a build's log (at most `LOG_TAIL_MAX` bytes, cut at a line
    /// start), or `None` when the build left no log (it failed before running).
    pub async fn build_log_tail(
        &self,
        app_id: &str,
        build_id: &str,
    ) -> Result<Option<String>, QuomeError> {
        let p = self.org_path(&format!(
            "/apps/{}/builds/{}/log",
            enc(app_id),
            enc(build_id)
        ));
        match self
            .send(Method::GET, &p, |b| b.query(&[("tail", "1")]))
            .await
        {
            Ok(r) => Ok(Some(log_tail(&r.text().await?, LOG_TAIL_MAX))),
            Err(QuomeError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn log_tail_keeps_whole_lines_from_the_end() {
        assert_eq!(super::log_tail("a\nb\n", 100), "a\nb");
        let log = "first line\nsecond line\nthird\n";
        assert_eq!(super::log_tail(log, 14), "third");
        // One long line is cut mid-line rather than dropped.
        assert_eq!(super::log_tail("0123456789", 4), "6789");
        // Never splits a multi-byte character.
        assert_eq!(super::log_tail("ééé", 3), "é");
    }

    use super::super::auth::tests::profile;
    use super::super::keys::AgentKey;
    use super::*;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ORG: &str = "11111111-1111-1111-1111-111111111111";

    async fn setup() -> (MockServer, CloudApi) {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"token_endpoint": format!("{}/oauth/token", s.uri())}),
            ))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "AT", "token_type": "Bearer", "expires_in": 3600
            })))
            .mount(&s)
            .await;
        let p = profile(&s.uri());
        let api = CloudApi::new(&p, TokenSource::new(&p, AgentKey::generate(), "vrc".into()));
        (s, api)
    }

    #[tokio::test]
    async fn me_sends_bearer() {
        let (s, api) = setup().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/me"))
            .and(header("authorization", "Bearer AT"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": null, "email": null, "display_name": null,
                "orgs": [{"id": ORG, "slug": "acme", "name": "Acme", "role": "admin"}],
                "principal_kind": "agent", "apps_domain": "apps.example", "api_origin": "https://x",
                "operator": false
            })))
            .expect(1)
            .mount(&s)
            .await;
        let me = api.me().await.unwrap();
        assert_eq!(me.orgs[0].slug, "acme");
        assert_eq!(me.apps_domain, "apps.example");
    }

    #[tokio::test]
    async fn revoked_delegation_maps_to_disconnected() {
        let (s, api) = setup().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/me"))
            .respond_with(ResponseTemplate::new(401).set_body_json(
                serde_json::json!({"detail":"unauthorized","reason":"delegation_revoked"}),
            ))
            .mount(&s)
            .await;
        assert!(matches!(
            api.me().await.unwrap_err(),
            QuomeError::CloudDisconnected
        ));
    }

    #[tokio::test]
    async fn agent_not_connected_maps_to_not_yet_connected() {
        let (s, api) = setup().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/me"))
            .respond_with(ResponseTemplate::new(401).set_body_json(
                serde_json::json!({"detail":"unauthorized","reason":"agent_not_connected"}),
            ))
            .mount(&s)
            .await;
        assert!(matches!(
            api.me().await.unwrap_err(),
            QuomeError::CloudNotYetConnected
        ));
    }

    #[tokio::test]
    async fn other_errors_carry_detail_and_reason() {
        let (s, api) = setup().await;
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/orgs/{ORG}/sandboxes")))
            .respond_with(
                ResponseTemplate::new(422)
                    .set_body_json(serde_json::json!({"detail":"invalid","reason":"invalid_name"})),
            )
            .mount(&s)
            .await;
        let e = api
            .create_sandbox("ok-name", 4.0, "claude")
            .await
            .unwrap_err();
        assert_eq!(e.to_string(), "API error: 422 invalid: invalid_name");
    }

    #[tokio::test]
    async fn create_sandbox_validates_locally_and_posts() {
        let (s, api) = setup().await;
        assert!(api.create_sandbox("Bad", 4.0, "claude").await.is_err());
        assert!(api.create_sandbox("good", 3.0, "claude").await.is_err());
        assert!(api.create_sandbox("good", 4.0, "vim").await.is_err());
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/orgs/{ORG}/sandboxes")))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id":"s1","name":"good","status":"provisioning","runtime":"codex","memory_gb":4.0
            })))
            .expect(1)
            .mount(&s)
            .await;
        let sb = api.create_sandbox("good", 4.0, "codex").await.unwrap();
        assert_eq!(sb.id, "s1");
        let reqs = s.received_requests().await.unwrap();
        let r = reqs
            .iter()
            .find(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/sandboxes"))
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(
            body,
            serde_json::json!({"name":"good","memory_gb":4.0,"runtime":"codex"})
        );
    }

    #[tokio::test]
    async fn exec_enforces_timeout_and_parses() {
        let (s, api) = setup().await;
        assert!(api.exec("s1", "ls", 26).await.is_err());
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/orgs/{ORG}/sandboxes/s1/exec")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "stdout":"hi\n","stderr":"","exit_code":0,"truncated":false
            })))
            .mount(&s)
            .await;
        let r = api.exec("s1", "echo hi", 25).await.unwrap();
        assert_eq!(r.stdout, "hi\n");
        let reqs = s.received_requests().await.unwrap();
        let r = reqs
            .iter()
            .find(|r| r.url.path().ends_with("/exec"))
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(body["timeout"], 25);
        assert_eq!(body["working_dir"], "/workspace");
    }

    #[tokio::test]
    async fn small_upload_is_single_shot() {
        let (s, api) = setup().await;
        Mock::given(method("PUT"))
            .and(path(format!(
                "/api/v1/orgs/{ORG}/sandboxes/s1/workspace-files"
            )))
            .and(query_param("path", "/workspace/a.txt"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"uploaded":true})),
            )
            .expect(1)
            .mount(&s)
            .await;
        api.upload("s1", "/workspace/a.txt", b"hello")
            .await
            .unwrap();
        let reqs = s.received_requests().await.unwrap();
        let r = reqs.iter().find(|r| r.method.as_str() == "PUT").unwrap();
        assert!(!r.url.query().unwrap().contains("upload_id"));
        assert_eq!(r.body, b"hello");
    }

    #[tokio::test]
    async fn large_upload_is_chunked_with_one_upload_id() {
        let (s, api) = setup().await;
        Mock::given(method("PUT"))
            .and(path(format!(
                "/api/v1/orgs/{ORG}/sandboxes/s1/workspace-files"
            )))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"uploaded":false})),
            )
            .mount(&s)
            .await;
        let data = vec![7u8; UPLOAD_CHUNK * 2 + 5];
        api.upload("s1", "/workspace/big.bin", &data).await.unwrap();
        let reqs = s.received_requests().await.unwrap();
        let puts: Vec<_> = reqs.iter().filter(|r| r.method.as_str() == "PUT").collect();
        assert_eq!(puts.len(), 3);
        let q = |r: &wiremock::Request| -> std::collections::HashMap<String, String> {
            r.url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect()
        };
        let ids: std::collections::HashSet<_> =
            puts.iter().map(|r| q(r)["upload_id"].clone()).collect();
        assert_eq!(ids.len(), 1);
        let id = ids.into_iter().next().unwrap();
        assert!(
            id.len() >= 8
                && id.len() <= 64
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        let offsets: Vec<_> = puts.iter().map(|r| q(r)["offset"].clone()).collect();
        assert_eq!(
            offsets,
            vec![
                "0".to_string(),
                UPLOAD_CHUNK.to_string(),
                (UPLOAD_CHUNK * 2).to_string()
            ]
        );
        let finals: Vec<_> = puts.iter().map(|r| q(r)["final"].clone()).collect();
        assert_eq!(finals, vec!["false", "false", "true"]);
        assert_eq!(puts[2].body.len(), 5);
    }

    #[tokio::test]
    async fn app_deploy_flow() {
        let (s, api) = setup().await;
        Mock::given(method("PUT"))
            .and(path(format!("/api/v1/uploads/orgs/{ORG}/apps/a1/source")))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(serde_json::json!({"source_key":"k.tar","size":3})),
            )
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/orgs/{ORG}/apps/a1/deploy")))
            .respond_with(ResponseTemplate::new(202).set_body_json(
                serde_json::json!({"deploy_id":"d","build_id":"b","workflow_id":"w"}),
            ))
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/orgs/{ORG}/apps/a1/builds/b")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id":"b","service_id":"a1","status":"failed","image_digest":null,"error":"boom",
                "started_at":null,"finished_at":null,"created_at":"2026-01-01T00:00:00Z"
            })))
            .mount(&s)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let tar = dir.path().join("src.tar");
        std::fs::write(&tar, b"tar").unwrap();
        let up = api.upload_source("a1", &tar).await.unwrap();
        assert_eq!(up.source_key, "k.tar");
        let d = api.deploy("a1", &up.source_key).await.unwrap();
        let b = api.build("a1", &d.build_id).await.unwrap();
        assert_eq!(b.error.as_deref(), Some("boom"));
        let reqs = s.received_requests().await.unwrap();
        let dep = reqs
            .iter()
            .find(|r| r.url.path().ends_with("/deploy"))
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&dep.body).unwrap(),
            serde_json::json!({"source_key":"k.tar"})
        );
    }

    #[tokio::test]
    async fn guide_ports_files_services() {
        let (s, api) = setup().await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/orgs/{ORG}/agent-guide")))
            .respond_with(ResponseTemplate::new(200).set_body_string("# Guide"))
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/orgs/{ORG}/sandboxes/s1/ports")))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"detected":[3000],"exposed":[{"port":3000,"url":"https://p"}]}),
            ))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/orgs/{ORG}/sandboxes/s1/ports")))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(serde_json::json!({"port":3000,"url":"https://p"})),
            )
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/orgs/{ORG}/sandboxes/s1/files/list")))
            .and(query_param("path", "/workspace"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"path":"/workspace","entries":[{"name":"a","type":"file","size":1,"mtime":2}],"truncated":false})))
            .mount(&s).await;
        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/orgs/{ORG}/sandboxes/s1/files/download"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1u8, 2, 3]))
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/orgs/{ORG}/services")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([{"id":"a1","kind":"app","name":"web","memory_gb":1.0,"status":"running","cpus":1,"replicas":1,"region":"r","created_by_actor":"x","created_at":"2026-01-01T00:00:00Z"}])))
            .mount(&s).await;
        assert_eq!(api.agent_guide().await.unwrap(), "# Guide");
        assert_eq!(api.ports("s1").await.unwrap().detected, vec![3000]);
        assert_eq!(api.expose_port("s1", 3000).await.unwrap().url, "https://p");
        assert_eq!(
            api.list_files("s1", "/workspace").await.unwrap().entries[0].name,
            "a"
        );
        assert_eq!(
            api.download("s1", "/workspace/a", 1 << 20).await.unwrap(),
            Download::Body(vec![1, 2, 3])
        );
        assert_eq!(api.list_services().await.unwrap()[0].name, "web");
    }

    #[tokio::test]
    async fn sandbox_get_list_stop() {
        let (s, api) = setup().await;
        let sb = |st: &str| serde_json::json!({"id":"s1","name":"n","status":st,"runtime":"claude","memory_gb":4.0});
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/orgs/{ORG}/sandboxes")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([sb("running")])),
            )
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/orgs/{ORG}/sandboxes/s1")))
            .respond_with(ResponseTemplate::new(200).set_body_json(sb("running")))
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/orgs/{ORG}/sandboxes/s1/stop")))
            .respond_with(ResponseTemplate::new(202).set_body_json(sb("stopping")))
            .expect(1)
            .mount(&s)
            .await;
        assert_eq!(api.list_sandboxes().await.unwrap()[0].id, "s1");
        assert_eq!(api.get_sandbox("s1").await.unwrap().status, "running");
        assert_eq!(api.stop_sandbox("s1").await.unwrap().status, "stopping");
    }

    async fn put_count(s: &MockServer) -> Vec<wiremock::Request> {
        s.received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.method.as_str() == "PUT")
            .collect()
    }

    #[tokio::test]
    async fn upload_exactly_8_mib_is_single_and_one_more_byte_is_chunked() {
        let (s, api) = setup().await;
        Mock::given(method("PUT"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"uploaded":true})),
            )
            .mount(&s)
            .await;
        api.upload("s1", "/workspace/a", &vec![1u8; UPLOAD_CHUNK])
            .await
            .unwrap();
        let puts = put_count(&s).await;
        assert_eq!(puts.len(), 1);
        assert!(!puts[0].url.query().unwrap().contains("upload_id"));
        api.upload("s1", "/workspace/b", &vec![1u8; UPLOAD_CHUNK + 1])
            .await
            .unwrap();
        let puts = put_count(&s).await;
        assert_eq!(puts.len(), 3);
        assert!(puts[1].url.query().unwrap().contains("final=false"));
        assert!(puts[2].url.query().unwrap().contains("final=true"));
        assert_eq!(puts[2].body.len(), 1);
    }

    #[tokio::test]
    async fn revoked_after_exchange_is_not_retried() {
        let (s, api) = setup().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/me"))
            .respond_with(ResponseTemplate::new(401).set_body_json(
                serde_json::json!({"detail":"unauthorized","reason":"delegation_revoked"}),
            ))
            .expect(1)
            .mount(&s)
            .await;
        assert!(matches!(
            api.me().await.unwrap_err(),
            QuomeError::CloudDisconnected
        ));
        let n = s
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == "/oauth/token")
            .count();
        assert_eq!(n, 1);
    }

    async fn two_token_server() -> (MockServer, CloudApi) {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"token_endpoint": format!("{}/oauth/token", s.uri())}),
            ))
            .mount(&s)
            .await;
        for tok in ["AT1", "AT2"] {
            Mock::given(method("POST")).and(path("/oauth/token"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"access_token": tok, "token_type":"Bearer","expires_in":3600})))
                .up_to_n_times(1).mount(&s).await;
        }
        let p = profile(&s.uri());
        let api = CloudApi::new(&p, TokenSource::new(&p, AgentKey::generate(), "vrc".into()));
        (s, api)
    }

    #[tokio::test]
    async fn other_401_refreshes_token_and_retries_once() {
        let (s, api) = two_token_server().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/orgs/".to_string() + ORG + "/services"))
            .and(header("authorization", "Bearer AT1"))
            .respond_with(ResponseTemplate::new(401).set_body_json(
                serde_json::json!({"detail":"unauthorized","reason":"invalid_token"}),
            ))
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/orgs/".to_string() + ORG + "/services"))
            .and(header("authorization", "Bearer AT2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .expect(1)
            .mount(&s)
            .await;
        assert!(api.list_services().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn persistent_401_is_retried_only_once() {
        let (s, api) = two_token_server().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/me"))
            .respond_with(ResponseTemplate::new(401).set_body_json(
                serde_json::json!({"detail":"unauthorized","reason":"invalid_token"}),
            ))
            .expect(2)
            .mount(&s)
            .await;
        let e = api.me().await.unwrap_err();
        assert!(e.to_string().contains("401"), "{e}");
    }

    #[tokio::test]
    async fn ids_are_percent_encoded() {
        let (s, api) = setup().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&s)
            .await;
        let _ = api.get_sandbox("../x/a?b#c").await;
        let _ = api.list_files("s1", "/workspace/a b&c=d").await;
        let reqs = s.received_requests().await.unwrap();
        let r = reqs
            .iter()
            .find(|r| r.url.path().contains("sandboxes/%"))
            .expect("encoded segment");
        assert_eq!(
            r.url.path(),
            format!("/api/v1/orgs/{ORG}/sandboxes/%2E%2E%2Fx%2Fa%3Fb%23c")
        );
        assert_eq!(r.url.query(), None);
        let l = reqs
            .iter()
            .find(|r| r.url.path().ends_with("/files/list"))
            .unwrap();
        let pairs: Vec<_> = l.url.query_pairs().collect();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].1, "/workspace/a b&c=d");
    }

    #[tokio::test]
    async fn download_refuses_on_content_length_without_reading_the_body() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (s, _) = setup().await;
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut c, _) = l.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = c.read(&mut buf).await;
            // Declares 100 MiB, sends 10 bytes, then stalls.
            let _ = c
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 104857600\r\n\r\n0123456789")
                .await;
            tokio::time::sleep(Duration::from_secs(10)).await;
        });
        let mut p = profile(&s.uri());
        p.api_url = format!("http://{addr}");
        let api = CloudApi::new(&p, TokenSource::new(&p, AgentKey::generate(), "v".into()));
        let started = std::time::Instant::now();
        let d = api.download("s1", "/workspace/big", 1 << 20).await.unwrap();
        assert_eq!(d, Download::TooLarge(Some(104857600)));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn slow_api_times_out() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"token_endpoint": format!("{}/oauth/token", s.uri())}),
            ))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"access_token":"AT","expires_in":3600})),
            )
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/me"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3)))
            .mount(&s)
            .await;
        let p = profile(&s.uri());
        let api = CloudApi::with_timeout(
            &p,
            TokenSource::new(&p, AgentKey::generate(), "v".into()),
            Duration::from_millis(200),
        );
        let started = std::time::Instant::now();
        let e = api.me().await.unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(
            matches!(e, QuomeError::Http(ref h) if h.is_timeout()),
            "{e}"
        );
    }

    #[tokio::test]
    async fn install_app_sends_accept_billing() {
        let (s, api) = setup().await;
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/orgs/{ORG}/app-installs")))
            .and(wiremock::matchers::body_json(
                serde_json::json!({"slug":"sales","accept_billing":true}),
            ))
            .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({
                "slug":"sales","name":"Sales","status":"installing","url":null,
                "agents_enabled":true,"error":null,"billing":"standard app billing"
            })))
            .expect(1)
            .mount(&s)
            .await;
        let i = api.install_app("sales").await.unwrap();
        assert_eq!(i.status, "installing");
        assert_eq!(i.name, "Sales");
    }

    #[tokio::test]
    async fn find_app_tools_encodes_query() {
        let (s, api) = setup().await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/orgs/{ORG}/app-installs/tools")))
            .and(query_param("q", "send & more"))
            .and(query_param("app", "sales"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "results":[{"app":"sales","tool":"send_email","summary":"s","destructive":true}],
                "truncated":false
            })))
            .expect(1)
            .mount(&s)
            .await;
        let r = api
            .find_app_tools(Some("send & more"), Some("sales"))
            .await
            .unwrap();
        assert_eq!(r.results[0].tool, "send_email");
    }

    #[tokio::test]
    async fn set_app_key_puts_value() {
        let (s, api) = setup().await;
        Mock::given(method("PUT"))
            .and(path(format!(
                "/api/v1/orgs/{ORG}/app-installs/sales/keys/sendgrid"
            )))
            .and(wiremock::matchers::body_json(
                serde_json::json!({"value":"SG.x"}),
            ))
            .respond_with(
                ResponseTemplate::new(202).set_body_json(
                    serde_json::json!({"key":"sendgrid","set":true,"delivering":false}),
                ),
            )
            .expect(1)
            .mount(&s)
            .await;
        api.set_app_key("sales", "sendgrid", "SG.x").await.unwrap();
    }

    #[tokio::test]
    async fn launch_url_uninstall_and_delete_data_shapes() {
        let (s, api) = setup().await;
        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/orgs/{ORG}/app-installs/sales/launch-url"
            )))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"url":"https://c/sales/launch"})),
            )
            .mount(&s)
            .await;
        Mock::given(method("DELETE"))
            .and(path(format!("/api/v1/orgs/{ORG}/app-installs/sales")))
            .and(wiremock::matchers::body_json(serde_json::json!({"confirm":true})))
            .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({
                "slug":"sales","name":"Sales","status":"uninstalling","url":null,"agents_enabled":true,"error":null
            })))
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/orgs/{ORG}/app-installs/sales/delete-data")))
            .and(wiremock::matchers::body_json(
                serde_json::json!({"confirm":true,"slug":"sales"}),
            ))
            .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({
                "slug":"sales","name":"Sales","status":"deleting_data","url":null,"agents_enabled":true,"error":null
            })))
            .expect(1)
            .mount(&s)
            .await;
        assert_eq!(
            api.app_launch_url("sales").await.unwrap(),
            "https://c/sales/launch"
        );
        assert_eq!(
            api.uninstall_app("sales").await.unwrap().status,
            "uninstalling"
        );
        assert_eq!(
            api.delete_app_data("sales").await.unwrap().status,
            "deleting_data"
        );
    }

    #[tokio::test]
    async fn launch_url_without_url_is_an_error() {
        let (s, api) = setup().await;
        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/orgs/{ORG}/app-installs/sales/launch-url"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&s)
            .await;
        assert!(api.app_launch_url("sales").await.is_err());
    }

    #[tokio::test]
    async fn api_error_includes_server_message() {
        let (s, api) = setup().await;
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/orgs/{ORG}/app-installs/sales/tools/t/call")))
            .respond_with(ResponseTemplate::new(502).set_body_json(
                serde_json::json!({"detail":"bad_gateway","reason":"app_error","message":"key missing"}),
            ))
            .mount(&s)
            .await;
        let e = api
            .call_app_tool("sales", "t", &serde_json::json!({}))
            .await
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            "API error: 502 bad_gateway: app_error: key missing"
        );
    }

    #[test]
    fn app_install_keeps_note() {
        let i: AppInstall = serde_json::from_value(serde_json::json!({
            "slug":"a","name":"A","status":"uninstalling","url":null,"agents_enabled":true,
            "error":null,"note":"data kept"}))
        .unwrap();
        assert_eq!(serde_json::to_value(&i).unwrap()["note"], "data kept");
    }

    const SB: &str = "22222222-2222-2222-2222-222222222222";
    const ADDR: &str = "33333333-3333-3333-3333-333333333333";

    fn addr_json(status: &str) -> serde_json::Value {
        serde_json::json!({
            "id": ADDR, "address": "jim-sendgrid-k3f9.acme@in.quome.cloud", "kind": "signup",
            "label": "sendgrid", "status": status, "forward": true, "owner_user_id": "u1",
            "sandbox_id": null, "created_by_actor": "agent", "created_by_on_behalf_of": "u1",
            "created_at": "2026-11-04T10:00:00Z", "last_message_at": null
        })
    }

    #[tokio::test]
    async fn email_create_address_validates_label_and_posts() {
        let (s, api) = setup().await;
        let too_long = "a".repeat(21);
        assert!(api
            .email_create_address("signup", "has space", None, true)
            .await
            .is_err());
        assert!(api
            .email_create_address("signup", &too_long, None, true)
            .await
            .is_err());
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/orgs/{ORG}/email/addresses")))
            .and(wiremock::matchers::body_json(
                serde_json::json!({"kind":"signup","label":"sendgrid","forward":true}),
            ))
            .respond_with(ResponseTemplate::new(201).set_body_json(addr_json("active")))
            .expect(1)
            .mount(&s)
            .await;
        let a = api
            .email_create_address("signup", "SendGrid", None, true)
            .await
            .unwrap();
        assert_eq!(a.id, ADDR);
        assert_eq!(a.address, "jim-sendgrid-k3f9.acme@in.quome.cloud");
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/orgs/{ORG}/email/addresses")))
            .and(wiremock::matchers::body_json(serde_json::json!({
                "kind":"sandbox","label":"test","forward":false,"sandbox_id":SB
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(addr_json("active")))
            .expect(1)
            .mount(&s)
            .await;
        api.email_create_address("sandbox", "test", Some(SB), false)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn email_disable_sends_status_disabled() {
        let (s, api) = setup().await;
        Mock::given(method("PATCH"))
            .and(path(format!("/api/v1/orgs/{ORG}/email/addresses/{ADDR}")))
            .and(wiremock::matchers::body_json(
                serde_json::json!({"status":"disabled"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(addr_json("disabled")))
            .expect(1)
            .mount(&s)
            .await;
        let a = api.email_disable(ADDR).await.unwrap();
        assert_eq!(a.status, "disabled");
        assert_eq!(a.sandbox_id, None);
    }

    #[tokio::test]
    async fn email_message_returns_the_body_unchanged() {
        let (s, api) = setup().await;
        let msg = "44444444-4444-4444-4444-444444444444";
        let env = serde_json::json!({
            "untrusted": true, "note": "n", "id": msg, "body": "<<<EMAIL x>>>\nhi\n<<<END EMAIL x>>>",
            "nonce": "x", "codes": [], "links": [], "attachments": [], "future_field": 1
        });
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/orgs/{ORG}/email/messages/{msg}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(env.clone()))
            .expect(1)
            .mount(&s)
            .await;
        assert_eq!(api.email_message(msg).await.unwrap(), env);
        let reqs = s.received_requests().await.unwrap();
        let r = reqs
            .iter()
            .find(|r| r.url.path().contains("/email/messages/"))
            .unwrap();
        assert_eq!(r.url.query(), None);
    }

    fn wait_path() -> String {
        format!("/api/v1/orgs/{ORG}/email/addresses/{ADDR}/wait")
    }

    #[tokio::test]
    async fn email_wait_query_omits_empty_filters() {
        let (s, api) = setup().await;
        Mock::given(method("GET"))
            .and(path(wait_path()))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"message":null,"now":"T"})),
            )
            .mount(&s)
            .await;
        api.email_wait(ADDR, None, Some(""), None, 25)
            .await
            .unwrap();
        api.email_wait(ADDR, Some("2026-11-04T10:00:00Z"), Some("acme"), None, 25)
            .await
            .unwrap();
        let reqs = s.received_requests().await.unwrap();
        let waits: Vec<_> = reqs
            .iter()
            .filter(|r| r.url.path().ends_with("/wait"))
            .collect();
        assert_eq!(waits.len(), 2);
        assert_eq!(waits[0].url.query(), Some("timeout_s=25"));
        let q: std::collections::HashMap<String, String> =
            waits[1].url.query_pairs().into_owned().collect();
        assert_eq!(q.len(), 3);
        assert_eq!(q["timeout_s"], "25");
        assert_eq!(q["since"], "2026-11-04T10:00:00Z");
        assert_eq!(q["from_contains"], "acme");
    }

    #[tokio::test]
    async fn email_wait_rejects_timeout_over_25() {
        let (s, api) = setup().await;
        for t in [26, 0] {
            assert!(matches!(
                api.email_wait(ADDR, None, None, None, t).await,
                Err(QuomeError::Usage(_))
            ));
        }
        let reqs = s.received_requests().await.unwrap();
        assert!(!reqs.iter().any(|r| r.url.path().ends_with("/wait")));
    }

    #[tokio::test]
    async fn email_wait_maps_busy_statuses() {
        let cases = [
            ResponseTemplate::new(429).set_body_json(
                serde_json::json!({"detail":"too_many_requests","reason":"too_many_waits"}),
            ),
            ResponseTemplate::new(503).set_body_json(
                serde_json::json!({"detail":"unavailable","reason":"wait_capacity"}),
            ),
            ResponseTemplate::new(503).set_body_json(
                serde_json::json!({"detail":"unavailable","reason":"wait_unavailable"}),
            ),
            ResponseTemplate::new(504),
        ];
        for r in cases {
            let (s, api) = setup().await;
            Mock::given(method("GET"))
                .and(path(wait_path()))
                .respond_with(r)
                .mount(&s)
                .await;
            let step = api.email_wait(ADDR, None, None, None, 5).await.unwrap();
            assert!(matches!(step, WaitStep::Busy));
        }
        let (s, api) = setup().await;
        Mock::given(method("GET"))
            .and(path(wait_path()))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_json(serde_json::json!({"detail":"forbidden","reason":"forbidden"})),
            )
            .mount(&s)
            .await;
        assert!(api.email_wait(ADDR, None, None, None, 5).await.is_err());
        let (s, api) = setup().await;
        Mock::given(method("GET"))
            .and(path(wait_path()))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"message":null,"now":"T"})),
            )
            .mount(&s)
            .await;
        match api.email_wait(ADDR, None, None, None, 5).await.unwrap() {
            WaitStep::NoMatch { now } => assert_eq!(now, "T"),
            _ => panic!("want NoMatch"),
        }
    }
}
