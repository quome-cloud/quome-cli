//! The `quome mcp` server: Quome Cloud tools for Claude Code over MCP stdio.
use super::api::{CloudApi, Download, Sandbox, MAX_EXEC_TIMEOUT};
use super::pack::pack_folder;
use super::state::Profile;
use crate::errors::QuomeError;
use base64::Engine;
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    tool, tool_handler, tool_router, ServiceExt,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const DISCONNECTED: &str =
    "This agent was disconnected from Quome Cloud — run `quome connect` again";
/// `read_file` returns text up to this size.
pub const MAX_READ: usize = 1 << 20;

/// Polling cadence and limits; shortened in tests.
#[derive(Clone, Copy)]
pub struct Timing {
    pub poll: Duration,
    pub sandbox_wait: Duration,
    pub build_wait: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            poll: Duration::from_secs(2),
            sandbox_wait: Duration::from_secs(180),
            build_wait: Duration::from_secs(600),
        }
    }
}

/// A user-facing message for an error; session errors get the reconnect sentence.
pub fn describe(e: &QuomeError) -> String {
    match e {
        QuomeError::CloudDisconnected => DISCONNECTED.to_string(),
        QuomeError::CloudInvalidGrant => format!(
            "{DISCONNECTED}. If you just reconnected, check this computer's clock is correct."
        ),
        QuomeError::CloudNotYetConnected => {
            "Finish approving in the browser, then try again".into()
        }
        other => other.to_string(),
    }
}

fn fail(e: QuomeError) -> String {
    describe(&e)
}

fn json<T: Serialize>(v: &T) -> Result<String, String> {
    serde_json::to_string_pretty(v).map_err(|e| e.to_string())
}

/// `/workspace/...`, absolute, no `..` segment.
pub fn check_workspace_path(path: &str) -> Result<(), String> {
    let rule = "path must be absolute, under /workspace/, and contain no `..` segment";
    if !path.starts_with("/workspace/") || path.len() == "/workspace/".len() {
        return Err(format!("Refusing to write {path:?}: {rule}."));
    }
    if path.split('/').any(|s| s == "..") {
        return Err(format!("Refusing to write {path:?}: {rule}."));
    }
    Ok(())
}

#[derive(Clone)]
pub struct QuomeMcp {
    api: Arc<CloudApi>,
    profile: Arc<Profile>,
    timing: Timing,
    tool_router: ToolRouter<Self>,
}

#[derive(Deserialize, JsonSchema)]
pub struct CreateSandboxArgs {
    /// Sandbox name (2-31 chars: lowercase letter, then lowercase letters, digits or hyphens). Default: agent-<6 hex>.
    name: Option<String>,
    /// Memory in GB: 0.5, 1, 2, 4, 8, 16, 32 or 64. Default 4.
    memory_gb: Option<f64>,
    /// Agent runtime: claude, codex, opencode or antigravity. Default claude.
    runtime: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SandboxArgs {
    /// Sandbox name or id.
    sandbox: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ExecArgs {
    /// Sandbox name or id.
    sandbox: String,
    /// Shell command to run in /workspace.
    command: String,
    /// Timeout in seconds, clamped to 1-25. Default 25.
    timeout_s: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
pub struct WriteFileArgs {
    /// Sandbox name or id.
    sandbox: String,
    /// Absolute path under /workspace/ with no `..` segment.
    path: String,
    /// Text content. Give exactly one of `content` and `content_base64`.
    content: Option<String>,
    /// Base64 content for binary files.
    content_base64: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct PathArgs {
    /// Sandbox name or id.
    sandbox: String,
    /// Absolute path in the sandbox.
    path: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListFilesArgs {
    /// Sandbox name or id.
    sandbox: String,
    /// Directory to list. Default /workspace.
    path: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct PreviewArgs {
    /// Sandbox name or id.
    sandbox: String,
    /// TCP port the app listens on inside the sandbox.
    port: u16,
}

#[derive(Deserialize, JsonSchema)]
pub struct DeployArgs {
    /// App name (2-31 chars: lowercase letter, then lowercase letters, digits or hyphens).
    name: String,
    /// Local folder containing the app and its Dockerfile.
    folder: String,
    /// Memory in GB: 0.5, 1, 2, 4, 8, 16, 32 or 64. Default 1.
    memory_gb: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct AppArgs {
    /// App name.
    name: String,
}

impl QuomeMcp {
    pub fn new(api: CloudApi, profile: Profile) -> QuomeMcp {
        Self::with_timing(api, profile, Timing::default())
    }

    pub fn with_timing(api: CloudApi, profile: Profile, timing: Timing) -> QuomeMcp {
        QuomeMcp {
            api: Arc::new(api),
            profile: Arc::new(profile),
            timing,
            tool_router: Self::tool_router(),
        }
    }

    /// Serve MCP over any reader/writer pair until the client disconnects.
    pub async fn serve_io<R, W>(self, r: R, w: W) -> Result<(), String>
    where
        R: tokio::io::AsyncRead + Send + Unpin + 'static,
        W: tokio::io::AsyncWrite + Send + Unpin + 'static,
    {
        let svc = self.serve((r, w)).await.map_err(|e| e.to_string())?;
        svc.waiting().await.map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn serve_stdio(self) -> Result<(), String> {
        self.serve_io(tokio::io::stdin(), tokio::io::stdout()).await
    }

    /// A sandbox id from a name or an id. A UUID is taken as an id without a lookup.
    async fn sandbox_id(&self, sandbox: &str) -> Result<String, String> {
        if uuid::Uuid::parse_str(sandbox).is_ok() {
            return Ok(sandbox.to_string());
        }
        let all = self.api.list_sandboxes().await.map_err(fail)?;
        all.into_iter()
            .find(|s| s.name == sandbox)
            .map(|s| s.id)
            .ok_or_else(|| format!("No sandbox named {sandbox:?}. Use list_sandboxes to see them."))
    }

    fn app_url(&self, name: &str) -> String {
        format!(
            "https://{name}-{}.{}",
            self.profile.org_slug, self.profile.apps_domain
        )
    }

    async fn find_app(&self, name: &str) -> Result<Option<super::api::Service>, String> {
        // The services list keeps deleted rows (names can be reused), so skip
        // them and prefer the newest of what is left.
        let ts = |s: &super::api::Service| {
            s.created_at
                .as_deref()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        };
        Ok(self
            .api
            .list_services()
            .await
            .map_err(fail)?
            .into_iter()
            .filter(|s| {
                s.kind == "app" && s.name == name && s.status != "deleted" && s.status != "deleting"
            })
            .max_by_key(|s| ts(s)))
    }
}

fn sandbox_json(s: &Sandbox) -> serde_json::Value {
    serde_json::json!({"id": s.id, "name": s.name, "status": s.status})
}

#[tool_router]
impl QuomeMcp {
    #[tool(
        description = "Read the Quome Cloud agent guide: how sandboxes, previews and app deploys work. Read it first."
    )]
    async fn guide(&self) -> Result<String, String> {
        self.api.agent_guide().await.map_err(fail)
    }

    #[tool(
        description = "Create a sandbox (a Linux workspace with a coding-agent runtime) and wait until it is running."
    )]
    async fn create_sandbox(
        &self,
        Parameters(a): Parameters<CreateSandboxArgs>,
    ) -> Result<String, String> {
        let name = a.name.unwrap_or_else(|| {
            format!("agent-{}", &uuid::Uuid::new_v4().simple().to_string()[..6])
        });
        let sb = self
            .api
            .create_sandbox(
                &name,
                a.memory_gb.unwrap_or(4.0),
                a.runtime.as_deref().unwrap_or("claude"),
            )
            .await
            .map_err(fail)?;
        let deadline = Instant::now() + self.timing.sandbox_wait;
        let mut cur = sb;
        loop {
            match cur.status.as_str() {
                "running" => return json(&sandbox_json(&cur)),
                "failed" | "stopped" | "stopping" | "deleting" => {
                    return Err(format!(
                        "Sandbox {} is {}{}",
                        cur.name,
                        cur.status,
                        cur.failure_reason
                            .as_deref()
                            .map(|r| format!(": {r}"))
                            .unwrap_or_default()
                    ))
                }
                _ => {}
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "Sandbox {} is still {} after {} seconds; check list_sandboxes later.",
                    cur.name,
                    cur.status,
                    self.timing.sandbox_wait.as_secs()
                ));
            }
            tokio::time::sleep(self.timing.poll).await;
            cur = self.api.get_sandbox(&cur.id).await.map_err(fail)?;
        }
    }

    #[tool(description = "List this organization's sandboxes.")]
    async fn list_sandboxes(&self) -> Result<String, String> {
        json(&self.api.list_sandboxes().await.map_err(fail)?)
    }

    #[tool(description = "Stop a sandbox (by name or id).")]
    async fn stop_sandbox(&self, Parameters(a): Parameters<SandboxArgs>) -> Result<String, String> {
        let id = self.sandbox_id(&a.sandbox).await?;
        let sb = self.api.stop_sandbox(&id).await.map_err(fail)?;
        json(&sandbox_json(&sb))
    }

    #[tool(
        description = "Run a shell command in a sandbox's /workspace. Returns exit_code, stdout, stderr, truncated. timeout_s is clamped to 1-25 seconds; run long jobs in the background and poll."
    )]
    async fn exec(&self, Parameters(a): Parameters<ExecArgs>) -> Result<String, String> {
        let id = self.sandbox_id(&a.sandbox).await?;
        let t = a
            .timeout_s
            .unwrap_or(MAX_EXEC_TIMEOUT)
            .clamp(1, MAX_EXEC_TIMEOUT);
        let r = self.api.exec(&id, &a.command, t).await.map_err(fail)?;
        json(&r)
    }

    #[tool(
        description = "Write a file in a sandbox. path must be absolute, under /workspace/, with no `..`. Give exactly one of content (text) or content_base64."
    )]
    async fn write_file(&self, Parameters(a): Parameters<WriteFileArgs>) -> Result<String, String> {
        check_workspace_path(&a.path)?;
        let bytes = match (&a.content, &a.content_base64) {
            (Some(c), None) => c.as_bytes().to_vec(),
            (None, Some(b)) => base64::engine::general_purpose::STANDARD
                .decode(b.trim())
                .map_err(|e| format!("content_base64 is not valid base64: {e}"))?,
            _ => return Err("Give exactly one of `content` and `content_base64`.".into()),
        };
        let id = self.sandbox_id(&a.sandbox).await?;
        self.api.upload(&id, &a.path, &bytes).await.map_err(fail)?;
        json(&serde_json::json!({"path": a.path, "bytes": bytes.len()}))
    }

    #[tool(
        description = "Read a text file (up to 1 MiB) from a sandbox. For larger or binary files use exec (for example `base64` or `head`)."
    )]
    async fn read_file(&self, Parameters(a): Parameters<PathArgs>) -> Result<String, String> {
        let id = self.sandbox_id(&a.sandbox).await?;
        let bytes = match self
            .api
            .download(&id, &a.path, MAX_READ)
            .await
            .map_err(fail)?
        {
            Download::Body(b) => b,
            Download::TooLarge(size) => {
                let size = size
                    .map(|n| format!("{n} bytes"))
                    .unwrap_or_else(|| "more than 1 MiB".into());
                return Err(format!(
                    "{} is {size}, over the 1 MiB read limit. Use exec (for example `head -c`, `tail` or `base64`) to read part of it.",
                    a.path
                ));
            }
        };
        String::from_utf8(bytes).map_err(|_| {
            format!(
                "{} is not valid UTF-8 text. Use exec (for example `base64 {}`) to read it.",
                a.path, a.path
            )
        })
    }

    #[tool(description = "List a directory in a sandbox (default /workspace).")]
    async fn list_files(&self, Parameters(a): Parameters<ListFilesArgs>) -> Result<String, String> {
        let id = self.sandbox_id(&a.sandbox).await?;
        let l = self
            .api
            .list_files(&id, a.path.as_deref().unwrap_or("/workspace"))
            .await
            .map_err(fail)?;
        json(&l)
    }

    #[tool(
        description = "Get a public preview URL for a port a sandbox is serving. Reuses the URL if the port is already exposed."
    )]
    async fn preview(&self, Parameters(a): Parameters<PreviewArgs>) -> Result<String, String> {
        let id = self.sandbox_id(&a.sandbox).await?;
        let ports = self.api.ports(&id).await.map_err(fail)?;
        if let Some(p) = ports.exposed.iter().find(|p| p.port == a.port) {
            return json(&serde_json::json!({"url": p.url}));
        }
        let p = self.api.expose_port(&id, a.port).await.map_err(fail)?;
        json(&serde_json::json!({"url": p.url}))
    }

    #[tool(
        description = "Deploy a local folder with a Dockerfile as a Quome Cloud app: create the app if needed, upload the source, build, and wait up to 10 minutes. Returns the app URL when the build succeeds."
    )]
    async fn deploy_app(&self, Parameters(a): Parameters<DeployArgs>) -> Result<String, String> {
        let folder = std::path::PathBuf::from(&a.folder);
        let packed = tokio::task::spawn_blocking(move || pack_folder(&folder))
            .await
            .map_err(|e| e.to_string())?
            .map_err(fail)?;
        let app = match self.find_app(&a.name).await? {
            Some(app) => app,
            None => self
                .api
                .create_app(&a.name, a.memory_gb.unwrap_or(1.0))
                .await
                .map_err(fail)?,
        };
        let up = self
            .api
            .upload_source(&app.id, packed.path())
            .await
            .map_err(fail)?;
        let dep = self
            .api
            .deploy(&app.id, &up.source_key)
            .await
            .map_err(fail)?;
        let deadline = Instant::now() + self.timing.build_wait;
        loop {
            let b = self.api.build(&app.id, &dep.build_id).await.map_err(fail)?;
            match b.status.as_str() {
                "succeeded" => {
                    return json(&serde_json::json!({
                        "status": "succeeded", "url": self.app_url(&a.name), "build_id": b.id}))
                }
                "failed" => {
                    return json(&serde_json::json!({
                        "status": "failed", "build_id": b.id, "error": b.error}))
                }
                _ => {}
            }
            if Instant::now() >= deadline {
                return json(&serde_json::json!({
                    "status": "building", "build_id": dep.build_id,
                    "url": null,
                    "note": "Still building after 10 minutes; check app_status later."}));
            }
            tokio::time::sleep(self.timing.poll).await;
        }
    }

    #[tool(description = "Show an app's state and URL by name.")]
    async fn app_status(&self, Parameters(a): Parameters<AppArgs>) -> Result<String, String> {
        match self.find_app(&a.name).await? {
            Some(app) => json(&serde_json::json!({
                "name": app.name, "id": app.id, "status": app.status,
                "memory_gb": app.memory_gb, "url": self.app_url(&app.name)})),
            None => Err(format!("No app named {:?}.", a.name)),
        }
    }
}

#[tool_handler(
    name = "quome",
    instructions = "Quome Cloud tools: sandboxes you can run code in, previews, and app deploys. Call guide first."
)]
impl rmcp::ServerHandler for QuomeMcp {}

#[cfg(test)]
mod tests {
    use super::super::auth::tests::profile;
    use super::super::auth::TokenSource;
    use super::super::keys::AgentKey;
    use super::*;
    use serde_json::{json, Value};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ORG: &str = "11111111-1111-1111-1111-111111111111";
    const SB: &str = "22222222-2222-2222-2222-222222222222";

    struct Client {
        w: tokio::io::DuplexStream,
        r: BufReader<tokio::io::DuplexStream>,
        id: u64,
    }

    impl Client {
        async fn rpc(&mut self, method: &str, params: Value) -> Value {
            self.id += 1;
            let msg = json!({"jsonrpc":"2.0","id":self.id,"method":method,"params":params});
            self.w
                .write_all(format!("{msg}\n").as_bytes())
                .await
                .unwrap();
            loop {
                let mut line = String::new();
                self.r.read_line(&mut line).await.unwrap();
                let v: Value = serde_json::from_str(&line).unwrap();
                if v["id"] == json!(self.id) {
                    return v;
                }
            }
        }
        async fn call(&mut self, tool: &str, args: Value) -> (bool, String) {
            let v = self
                .rpc("tools/call", json!({"name": tool, "arguments": args}))
                .await;
            let res = &v["result"];
            (
                res["isError"].as_bool().unwrap_or(false),
                res["content"][0]["text"].as_str().unwrap_or("").to_string(),
            )
        }
    }

    async fn start() -> (MockServer, Client) {
        start_with(Timing {
            poll: Duration::from_millis(5),
            sandbox_wait: Duration::from_secs(5),
            build_wait: Duration::from_secs(5),
        })
        .await
    }

    async fn start_with(timing: Timing) -> (MockServer, Client) {
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
        let api = CloudApi::new(&p, TokenSource::new(&p, AgentKey::generate(), "vrc".into()));
        let server = QuomeMcp::with_timing(api, p, timing);
        let (cw, sr) = tokio::io::duplex(1 << 20);
        let (sw, cr) = tokio::io::duplex(1 << 20);
        tokio::spawn(server.serve_io(sr, sw));
        let mut c = Client {
            w: cw,
            r: BufReader::new(cr),
            id: 0,
        };
        c.rpc(
            "initialize",
            json!({"protocolVersion":"2025-06-18","capabilities":{},
                   "clientInfo":{"name":"t","version":"0"}}),
        )
        .await;
        c.w.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
            .await
            .unwrap();
        (s, c)
    }

    fn api_path(rest: &str) -> String {
        format!("/api/v1/orgs/{ORG}{rest}")
    }

    #[tokio::test]
    async fn lists_exactly_the_brief_tools() {
        let (_s, mut c) = start().await;
        let v = c.rpc("tools/list", json!({})).await;
        let mut names: Vec<String> = v["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        names.sort();
        let mut want = vec![
            "guide",
            "create_sandbox",
            "list_sandboxes",
            "stop_sandbox",
            "exec",
            "write_file",
            "read_file",
            "list_files",
            "preview",
            "deploy_app",
            "app_status",
        ];
        want.sort();
        assert_eq!(names, want);
    }

    #[tokio::test]
    async fn exec_clamps_timeout_to_25() {
        let (s, mut c) = start().await;
        Mock::given(method("POST"))
            .and(path(api_path(&format!("/sandboxes/{SB}/exec"))))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({"stdout":"hi","stderr":"","exit_code":0,"truncated":false}),
                ),
            )
            .expect(2)
            .mount(&s)
            .await;
        let (err, text) = c
            .call(
                "exec",
                json!({"sandbox": SB, "command":"echo hi","timeout_s":999}),
            )
            .await;
        assert!(!err, "{text}");
        assert!(text.contains("\"exit_code\": 0"));
        let (err, _) = c
            .call("exec", json!({"sandbox": SB, "command":"x","timeout_s":0}))
            .await;
        assert!(!err);
        let timeouts: Vec<i64> = s
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path().ends_with("/exec"))
            .map(|r| {
                serde_json::from_slice::<Value>(&r.body).unwrap()["timeout"]
                    .as_i64()
                    .unwrap()
            })
            .collect();
        assert_eq!(timeouts, vec![25, 1]);
    }

    #[tokio::test]
    async fn preview_reuses_exposed_port_without_post() {
        let (s, mut c) = start().await;
        Mock::given(method("GET"))
            .and(path(api_path(&format!("/sandboxes/{SB}/ports"))))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "detected":[3000],"exposed":[{"port":3000,"url":"https://p3000.example"}]})))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path(api_path(&format!("/sandboxes/{SB}/ports"))))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&s)
            .await;
        let (err, text) = c
            .call("preview", json!({"sandbox": SB, "port": 3000}))
            .await;
        assert!(!err, "{text}");
        assert!(text.contains("https://p3000.example"));
    }

    #[tokio::test]
    async fn preview_exposes_once_when_missing() {
        let (s, mut c) = start().await;
        Mock::given(method("GET"))
            .and(path(api_path(&format!("/sandboxes/{SB}/ports"))))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "detected":[8080],"exposed":[]})))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path(api_path(&format!("/sandboxes/{SB}/ports"))))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"port":8080,"url":"https://p8080.example"})),
            )
            .expect(1)
            .mount(&s)
            .await;
        let (err, text) = c
            .call("preview", json!({"sandbox": SB, "port": 8080}))
            .await;
        assert!(!err, "{text}");
        assert!(text.contains("https://p8080.example"));
    }

    #[tokio::test]
    async fn sandbox_name_resolves_via_list() {
        let (s, mut c) = start().await;
        Mock::given(method("GET"))
            .and(path(api_path("/sandboxes")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"id":SB,"name":"mine","status":"running","runtime":"claude","memory_gb":4.0}])))
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path(api_path(&format!("/sandboxes/{SB}/ports"))))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "detected":[],"exposed":[{"port":1,"url":"https://u"}]})))
            .mount(&s)
            .await;
        let (err, text) = c.call("preview", json!({"sandbox":"mine","port":1})).await;
        assert!(!err, "{text}");
        let (err, text) = c.call("preview", json!({"sandbox":"nope","port":1})).await;
        assert!(err);
        assert!(text.contains("No sandbox named"));
    }

    #[tokio::test]
    async fn write_file_outside_workspace_makes_no_request() {
        let (s, mut c) = start().await;
        for p in [
            "/etc/passwd",
            "workspace/a",
            "/workspace/../etc/x",
            "/workspace/a/../../x",
            "/workspacex/a",
            "/workspace/",
        ] {
            let (err, text) = c
                .call(
                    "write_file",
                    json!({"sandbox": SB, "path": p, "content":"x"}),
                )
                .await;
            assert!(err, "{p}");
            assert!(text.contains("/workspace/"), "{text}");
        }
        let (err, text) = c
            .call(
                "write_file",
                json!({"sandbox": SB, "path":"/workspace/a",
                                       "content":"x","content_base64":"eA=="}),
            )
            .await;
        assert!(err);
        assert!(text.contains("exactly one"));
        assert!(s.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn write_file_uploads_content() {
        let (s, mut c) = start().await;
        Mock::given(method("PUT"))
            .and(path(api_path(&format!("/sandboxes/{SB}/workspace-files"))))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&s)
            .await;
        let (err, text) = c
            .call(
                "write_file",
                json!({"sandbox": SB, "path":"/workspace/a.txt",
                                       "content_base64":"aGk="}),
            )
            .await;
        assert!(!err, "{text}");
        let put = s
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.method.as_str() == "PUT")
            .unwrap();
        assert_eq!(put.body, b"hi");
    }

    #[tokio::test]
    async fn revoked_delegation_gives_the_reconnect_sentence() {
        let (s, mut c) = start().await;
        Mock::given(method("GET"))
            .and(path(api_path("/sandboxes")))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_json(json!({"detail":"unauthorized","reason":"delegation_revoked"})),
            )
            .mount(&s)
            .await;
        let (err, text) = c.call("list_sandboxes", json!({})).await;
        assert!(err);
        assert_eq!(text, DISCONNECTED);
    }

    #[test]
    fn invalid_grant_keeps_clock_hint() {
        let m = describe(&QuomeError::CloudInvalidGrant);
        assert!(m.starts_with(DISCONNECTED));
        assert!(m.contains("clock"));
        assert_eq!(
            describe(&QuomeError::CloudNotYetConnected),
            "Finish approving in the browser, then try again"
        );
    }

    #[tokio::test]
    async fn deploy_app_happy_path_returns_url() {
        let (s, mut c) = start().await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Dockerfile"), "FROM scratch\n").unwrap();
        Mock::given(method("GET"))
            .and(path(api_path("/services")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path(api_path("/services")))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id":"app1","kind":"app","name":"web","memory_gb":1.0,"status":"created"})))
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(method("PUT"))
            .and(path(format!("/api/v1/uploads/orgs/{ORG}/apps/app1/source")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"source_key":"k1","size":10})),
            )
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path(api_path("/apps/app1/deploy")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"deploy_id":"d1","build_id":"b1","workflow_id":"w1"})),
            )
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path(api_path("/apps/app1/builds/b1")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"id":"b1","status":"succeeded"})),
            )
            .mount(&s)
            .await;
        let (err, text) = c
            .call(
                "deploy_app",
                json!({"name":"web","folder": dir.path().to_str().unwrap()}),
            )
            .await;
        assert!(!err, "{text}");
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["status"], "succeeded");
        assert_eq!(v["url"], "https://web-acme.apps.example");
        assert_eq!(v["build_id"], "b1");
    }

    #[tokio::test]
    async fn deploy_app_without_dockerfile_uploads_nothing() {
        let (s, mut c) = start().await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        let (err, text) = c
            .call(
                "deploy_app",
                json!({"name":"web","folder": dir.path().to_str().unwrap()}),
            )
            .await;
        assert!(err);
        assert!(text.contains("Dockerfile"), "{text}");
        assert!(s.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn deploy_app_still_building_reports_build_id() {
        let (s, c) = start().await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Dockerfile"), "FROM scratch\n").unwrap();
        Mock::given(method("GET"))
            .and(path(api_path("/services")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"id":"app1","kind":"app","name":"web","memory_gb":1.0,"status":"running"}])))
            .mount(&s)
            .await;
        Mock::given(method("PUT"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"source_key":"k1","size":10})),
            )
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path(api_path("/apps/app1/deploy")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"deploy_id":"d1","build_id":"b1","workflow_id":"w1"})),
            )
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path(api_path("/apps/app1/builds/b1")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"id":"b1","status":"running"})),
            )
            .mount(&s)
            .await;
        // Shorten the wait by calling through a server with a tiny build_wait.
        drop(c);
        let p = profile(&s.uri());
        let api = CloudApi::new(&p, TokenSource::new(&p, AgentKey::generate(), "v".into()));
        let m = QuomeMcp::with_timing(
            api,
            p,
            Timing {
                poll: Duration::from_millis(5),
                sandbox_wait: Duration::from_secs(1),
                build_wait: Duration::from_millis(60),
            },
        );
        let out = m
            .deploy_app(Parameters(DeployArgs {
                name: "web".into(),
                folder: dir.path().to_str().unwrap().into(),
                memory_gb: None,
            }))
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["status"], "building");
        assert_eq!(v["build_id"], "b1");
    }

    fn sb_json(status: &str) -> Value {
        json!({"id":SB,"name":"box","status":status,"runtime":"claude","memory_gb":4.0,
               "failure_reason": if status == "failed" { json!("no capacity") } else { Value::Null }})
    }

    async fn mount_create(s: &MockServer, first: &str) {
        Mock::given(method("POST"))
            .and(path(api_path("/sandboxes")))
            .respond_with(ResponseTemplate::new(201).set_body_json(sb_json(first)))
            .mount(s)
            .await;
    }

    #[tokio::test]
    async fn create_sandbox_waits_until_running() {
        let (s, mut c) = start().await;
        mount_create(&s, "created").await;
        Mock::given(method("GET"))
            .and(path(api_path(&format!("/sandboxes/{SB}"))))
            .respond_with(ResponseTemplate::new(200).set_body_json(sb_json("starting")))
            .up_to_n_times(2)
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path(api_path(&format!("/sandboxes/{SB}"))))
            .respond_with(ResponseTemplate::new(200).set_body_json(sb_json("running")))
            .mount(&s)
            .await;
        let (err, text) = c.call("create_sandbox", json!({"name":"box"})).await;
        assert!(!err, "{text}");
        assert!(text.contains("\"running\""), "{text}");
        let gets = s
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.method.as_str() == "GET" && r.url.path().ends_with(SB))
            .count();
        assert_eq!(gets, 3, "polled until running");
    }

    #[tokio::test]
    async fn create_sandbox_reports_failure_reason() {
        let (s, mut c) = start().await;
        mount_create(&s, "created").await;
        Mock::given(method("GET"))
            .and(path(api_path(&format!("/sandboxes/{SB}"))))
            .respond_with(ResponseTemplate::new(200).set_body_json(sb_json("failed")))
            .mount(&s)
            .await;
        let (err, text) = c.call("create_sandbox", json!({"name":"box"})).await;
        assert!(err);
        assert!(text.contains("is failed: no capacity"), "{text}");
    }

    #[tokio::test]
    async fn create_sandbox_stops_waiting_when_stopping_or_deleting() {
        for st in ["stopping", "deleting", "stopped"] {
            let (s, mut c) = start().await;
            mount_create(&s, "created").await;
            Mock::given(method("GET"))
                .and(path(api_path(&format!("/sandboxes/{SB}"))))
                .respond_with(ResponseTemplate::new(200).set_body_json(sb_json(st)))
                .mount(&s)
                .await;
            let (err, text) = c.call("create_sandbox", json!({"name":"box"})).await;
            assert!(err);
            assert!(text.contains(&format!("is {st}")), "{st}: {text}");
        }
    }

    #[tokio::test]
    async fn create_sandbox_times_out_with_a_pointer() {
        let (s, mut c) = start_with(Timing {
            poll: Duration::from_millis(5),
            sandbox_wait: Duration::from_millis(60),
            build_wait: Duration::from_secs(1),
        })
        .await;
        mount_create(&s, "created").await;
        Mock::given(method("GET"))
            .and(path(api_path(&format!("/sandboxes/{SB}"))))
            .respond_with(ResponseTemplate::new(200).set_body_json(sb_json("starting")))
            .mount(&s)
            .await;
        let (err, text) = c.call("create_sandbox", json!({"name":"box"})).await;
        assert!(err);
        assert!(text.contains("still starting after"), "{text}");
        assert!(text.contains("list_sandboxes"), "{text}");
    }

    async fn serve_file(s: &MockServer, body: Vec<u8>) {
        Mock::given(method("GET"))
            .and(path(api_path(&format!("/sandboxes/{SB}/files/download"))))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(s)
            .await;
    }

    #[tokio::test]
    async fn read_file_limits() {
        let (s, mut c) = start().await;
        serve_file(&s, vec![b'a'; MAX_READ]).await;
        let (err, text) = c
            .call("read_file", json!({"sandbox": SB, "path":"/workspace/x"}))
            .await;
        assert!(!err, "exactly 1 MiB is readable");
        assert_eq!(text.len(), MAX_READ);
        let (s, mut c) = start().await;
        serve_file(&s, vec![b'a'; MAX_READ + 1]).await;
        let (err, text) = c
            .call("read_file", json!({"sandbox": SB, "path":"/workspace/x"}))
            .await;
        assert!(err);
        assert!(text.contains("1 MiB read limit"), "{text}");
        assert!(text.contains(&(MAX_READ + 1).to_string()), "{text}");
        let (s, mut c) = start().await;
        serve_file(&s, vec![0xff, 0xfe, 0x00]).await;
        let (err, text) = c
            .call("read_file", json!({"sandbox": SB, "path":"/workspace/x"}))
            .await;
        assert!(err);
        assert!(text.contains("not valid UTF-8"), "{text}");
    }

    async fn deploy_mocks(s: &MockServer, app_id: &str) {
        Mock::given(method("PUT"))
            .and(path(format!(
                "/api/v1/uploads/orgs/{ORG}/apps/{app_id}/source"
            )))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"source_key":"k1","size":10})),
            )
            .mount(s)
            .await;
        Mock::given(method("POST"))
            .and(path(api_path(&format!("/apps/{app_id}/deploy"))))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"deploy_id":"d1","build_id":"b1","workflow_id":"w1"})),
            )
            .mount(s)
            .await;
        Mock::given(method("GET"))
            .and(path(api_path(&format!("/apps/{app_id}/builds/b1"))))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"id":"b1","status":"failed","error":"x"})),
            )
            .mount(s)
            .await;
    }

    fn docker_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Dockerfile"), "FROM scratch\n").unwrap();
        dir
    }

    #[tokio::test]
    async fn deploy_skips_deleted_apps_and_prefers_the_newest_live_one() {
        let (s, mut c) = start().await;
        let dir = docker_dir();
        Mock::given(method("GET"))
            .and(path(api_path("/services")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"id":"dead","kind":"app","name":"web","memory_gb":1.0,"status":"deleted","created_at":"2026-03-01T00:00:00Z"},
                {"id":"gone","kind":"app","name":"web","memory_gb":1.0,"status":"deleting","created_at":"2026-04-01T00:00:00Z"},
                {"id":"old","kind":"app","name":"web","memory_gb":1.0,"status":"running","created_at":"2026-01-01T00:00:00Z"},
                {"id":"live","kind":"app","name":"web","memory_gb":1.0,"status":"running","created_at":"2026-02-01T00:00:00Z"}])))
            .mount(&s)
            .await;
        deploy_mocks(&s, "live").await;
        let (err, text) = c
            .call(
                "deploy_app",
                json!({"name":"web","folder": dir.path().to_str().unwrap()}),
            )
            .await;
        assert!(!err, "{text}");
        let posted = s
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/services"));
        assert!(!posted, "must not create a second app");
        assert!(s
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.url.path().contains("/apps/live/deploy")));
    }

    #[tokio::test]
    async fn deploy_with_only_deleted_rows_creates_a_new_app() {
        let (s, mut c) = start().await;
        let dir = docker_dir();
        Mock::given(method("GET"))
            .and(path(api_path("/services")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"id":"dead","kind":"app","name":"web","memory_gb":1.0,"status":"deleted"}])))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path(api_path("/services")))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id":"fresh","kind":"app","name":"web","memory_gb":1.0,"status":"created"})))
            .expect(1)
            .mount(&s)
            .await;
        deploy_mocks(&s, "fresh").await;
        let (err, text) = c
            .call(
                "deploy_app",
                json!({"name":"web","folder": dir.path().to_str().unwrap()}),
            )
            .await;
        assert!(!err, "{text}");
    }

    #[tokio::test]
    async fn app_status_ignores_deleted_apps() {
        let (s, mut c) = start().await;
        Mock::given(method("GET"))
            .and(path(api_path("/services")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"id":"dead","kind":"app","name":"web","memory_gb":1.0,"status":"deleted"}])))
            .mount(&s)
            .await;
        let (err, text) = c.call("app_status", json!({"name":"web"})).await;
        assert!(err);
        assert!(text.contains("No app named"), "{text}");
    }
}
