# Quome CLI Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a production-ready Rust CLI for the Quome platform with full API feature parity.

**Architecture:** Layered design with commands (CLI interface), API client (typed REST calls), and config (state/auth management). Commands use clap derive macros, API client wraps reqwest with typed request/response structs.

**Tech Stack:** Rust, clap 4.5, tokio, reqwest, serde, thiserror, inquire, colored

---

## Task 1: Initialize Cargo Project

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`

**Step 1: Create Cargo.toml**

```toml
[package]
name = "quome-cli"
version = "0.1.0"
edition = "2021"
description = "CLI for the Quome platform"
license = "MIT"

[[bin]]
name = "quome"
path = "src/main.rs"

[dependencies]
# CLI framework
clap = { version = "4.5", features = ["derive"] }

# Async runtime
tokio = { version = "1", features = ["full"] }

# HTTP client
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }

# Serialization
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

# Error handling
anyhow = "1.0"
thiserror = "2.0"

# Interactive prompts
inquire = "0.7"

# Terminal output
colored = "2.0"

# Utilities
dirs = "5.0"
open = "5.0"
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1.0", features = ["serde"] }

[profile.release]
lto = "fat"
opt-level = "z"
panic = "abort"
```

**Step 2: Create minimal main.rs**

```rust
use clap::Parser;

#[derive(Parser)]
#[command(name = "quome")]
#[command(about = "CLI for the Quome platform")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Show version information
    Version,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Version => {
            println!("quome {}", env!("CARGO_PKG_VERSION"));
        }
    }

    Ok(())
}
```

**Step 3: Verify it compiles and runs**

Run: `cargo build`
Expected: Compiles successfully

Run: `cargo run -- version`
Expected: `quome 0.1.0`

**Step 4: Commit**

```bash
git add Cargo.toml src/main.rs
git commit -m "feat: initialize quome-cli cargo project"
```

---

## Task 2: Add Error Types

**Files:**
- Create: `src/errors.rs`
- Modify: `src/main.rs`

**Step 1: Create src/errors.rs**

```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum QuomeError {
    #[error("Not logged in. Run `quome login` first.")]
    NotLoggedIn,

    #[error("No linked organization. Run `quome link` to connect.")]
    NoLinkedOrg,

    #[error("No linked application. Run `quome link` to connect.")]
    NoLinkedApp,

    #[error("Unauthorized. Your session may have expired. Run `quome login`.")]
    Unauthorized,

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("API error: {0}")]
    ApiError(String),

    #[error("Rate limited. Please wait and try again.")]
    RateLimited,

    #[error("Invalid response from server")]
    InvalidResponse,

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, QuomeError>;
```

**Step 2: Update main.rs to declare module**

Add at top of `src/main.rs`:
```rust
mod errors;
```

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: Compiles successfully

**Step 4: Commit**

```bash
git add src/errors.rs src/main.rs
git commit -m "feat: add custom error types"
```

---

## Task 3: Add API Models

**Files:**
- Create: `src/api/mod.rs`
- Create: `src/api/models.rs`

**Step 1: Create src/api/mod.rs**

```rust
pub mod models;
```

**Step 2: Create src/api/models.rs**

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

// ============ Users ============

#[derive(Debug, Serialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct User {
    pub id: Uuid,
    pub username: String,
    pub email: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ============ Organizations ============
// Note: Authentication uses API keys from the Quome dashboard.
// No session-based auth models needed.

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Organization {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct ListOrgsResponse {
    pub organizations: Vec<Organization>,
}

#[derive(Debug, Serialize)]
pub struct CreateOrgRequest {
    pub name: String,
}

// ============ Org Members ============

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OrgMember {
    pub id: Uuid,
    pub user_id: Uuid,
    pub org_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct ListOrgMembersResponse {
    pub members: Vec<OrgMember>,
}

#[derive(Debug, Serialize)]
pub struct AddOrgMemberRequest {
    pub user_id: Uuid,
}

// ============ API Keys ============

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OrgKey {
    pub id: Uuid,
    pub org_id: Uuid,
    pub key_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct ListOrgKeysResponse {
    pub keys: Vec<OrgKey>,
}

#[derive(Debug, Serialize)]
pub struct CreateOrgKeyRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiration: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct CreatedOrgKey {
    pub id: Uuid,
    pub key: String,
    pub created_at: DateTime<Utc>,
}

// ============ Apps ============

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct App {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub organization_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub spec: Option<AppSpec>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AppSpec {
    #[serde(default)]
    pub containers: Vec<ContainerSpec>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ContainerSpec {
    pub name: String,
    pub image: String,
    pub port: u16,
}

#[derive(Debug, Deserialize)]
pub struct AppList {
    pub apps: Vec<App>,
}

#[derive(Debug, Serialize)]
pub struct CreateAppRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub spec: AppSpec,
}

#[derive(Debug, Serialize)]
pub struct UpdateAppRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<AppSpec>,
}

// ============ Deployments ============

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Deployment {
    pub id: Uuid,
    pub app_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub status: DeploymentStatus,
    #[serde(default)]
    pub failure_message: Option<String>,
    #[serde(default)]
    pub events: Vec<DeploymentEvent>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentStatus {
    Created,
    InProgress,
    Deployed,
    Failed,
}

impl std::fmt::Display for DeploymentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeploymentStatus::Created => write!(f, "created"),
            DeploymentStatus::InProgress => write!(f, "in_progress"),
            DeploymentStatus::Deployed => write!(f, "deployed"),
            DeploymentStatus::Failed => write!(f, "failed"),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DeploymentEvent {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub message: String,
    #[serde(default)]
    pub details: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
pub struct DeploymentList {
    pub deployments: Vec<Deployment>,
}

// ============ Secrets ============

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Secret {
    pub id: Uuid,
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub description: Option<String>,
    pub organization_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct ListSecretsResponse {
    pub secrets: Vec<Secret>,
}

#[derive(Debug, Serialize)]
pub struct CreateSecretRequest {
    pub name: String,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UpdateSecretRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

// ============ Events ============

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Event {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub event_type: String,
    pub actor: EventActor,
    pub resource: EventResource,
    #[serde(default)]
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    pub organization_id: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct EventActor {
    pub id: Uuid,
    pub email: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct EventResource {
    #[serde(rename = "type")]
    pub resource_type: String,
    pub id: Uuid,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListEventsResponse {
    pub events: Vec<Event>,
    #[serde(default)]
    pub next_before: Option<DateTime<Utc>>,
}

// ============ Logs ============

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub level: LogLevel,
    pub message: String,
    #[serde(default)]
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogLevel::Debug => write!(f, "DEBUG"),
            LogLevel::Info => write!(f, "INFO"),
            LogLevel::Warn => write!(f, "WARN"),
            LogLevel::Error => write!(f, "ERROR"),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ListLogsResponse {
    pub logs: Vec<LogEntry>,
    #[serde(default)]
    pub next_before: Option<DateTime<Utc>>,
}

// ============ Shared Error Types ============

#[derive(Debug, Deserialize)]
pub struct ApiErrorResponse {
    pub message: String,
}
```

**Step 3: Update main.rs to declare api module**

Add at top of `src/main.rs`:
```rust
mod api;
```

**Step 4: Verify it compiles**

Run: `cargo build`
Expected: Compiles successfully

**Step 5: Commit**

```bash
git add src/api/mod.rs src/api/models.rs src/main.rs
git commit -m "feat: add API models from OpenAPI spec"
```

---

## Task 4: Add HTTP Client

**Files:**
- Create: `src/client.rs`
- Modify: `src/main.rs`

**Step 1: Create src/client.rs**

```rust
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::time::Duration;

use crate::api::models::ApiErrorResponse;
use crate::errors::{QuomeError, Result};

const DEFAULT_BASE_URL: &str = "https://api.quome.io";
const USER_AGENT: &str = concat!("quome-cli/", env!("CARGO_PKG_VERSION"));

pub struct QuomeClient {
    http: reqwest::Client,
    base_url: String,
}

impl QuomeClient {
    pub fn new(token: Option<&str>, base_url: Option<&str>) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        if let Some(t) = token {
            let auth_value = format!("Bearer {}", t);
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&auth_value).map_err(|_| QuomeError::InvalidResponse)?,
            );
        }

        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .build()?;

        let base_url = base_url
            .map(String::from)
            .or_else(|| std::env::var("QUOME_API_URL").ok())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());

        Ok(Self { http, base_url })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    async fn handle_response<T: DeserializeOwned>(&self, response: reqwest::Response) -> Result<T> {
        let status = response.status();

        if status.is_success() {
            let body = response.json::<T>().await?;
            Ok(body)
        } else {
            match status {
                StatusCode::UNAUTHORIZED => Err(QuomeError::Unauthorized),
                StatusCode::NOT_FOUND => {
                    let err = response.json::<ApiErrorResponse>().await.ok();
                    Err(QuomeError::NotFound(
                        err.map(|e| e.message).unwrap_or_else(|| "Resource not found".into()),
                    ))
                }
                StatusCode::TOO_MANY_REQUESTS => Err(QuomeError::RateLimited),
                _ => {
                    let err = response.json::<ApiErrorResponse>().await.ok();
                    Err(QuomeError::ApiError(
                        err.map(|e| e.message)
                            .unwrap_or_else(|| format!("Request failed with status {}", status)),
                    ))
                }
            }
        }
    }

    async fn handle_empty_response(&self, response: reqwest::Response) -> Result<()> {
        let status = response.status();

        if status.is_success() {
            Ok(())
        } else {
            match status {
                StatusCode::UNAUTHORIZED => Err(QuomeError::Unauthorized),
                StatusCode::NOT_FOUND => {
                    let err = response.json::<ApiErrorResponse>().await.ok();
                    Err(QuomeError::NotFound(
                        err.map(|e| e.message).unwrap_or_else(|| "Resource not found".into()),
                    ))
                }
                StatusCode::TOO_MANY_REQUESTS => Err(QuomeError::RateLimited),
                _ => {
                    let err = response.json::<ApiErrorResponse>().await.ok();
                    Err(QuomeError::ApiError(
                        err.map(|e| e.message)
                            .unwrap_or_else(|| format!("Request failed with status {}", status)),
                    ))
                }
            }
        }
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self.http.get(self.url(path)).send().await?;
        self.handle_response(response).await
    }

    pub async fn post<T: DeserializeOwned, B: Serialize>(&self, path: &str, body: &B) -> Result<T> {
        let response = self.http.post(self.url(path)).json(body).send().await?;
        self.handle_response(response).await
    }

    pub async fn put<T: DeserializeOwned, B: Serialize>(&self, path: &str, body: &B) -> Result<T> {
        let response = self.http.put(self.url(path)).json(body).send().await?;
        self.handle_response(response).await
    }

    pub async fn delete(&self, path: &str) -> Result<()> {
        let response = self.http.delete(self.url(path)).send().await?;
        self.handle_empty_response(response).await
    }
}
```

**Step 2: Update main.rs to declare client module**

Add at top of `src/main.rs`:
```rust
mod client;
```

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: Compiles successfully

**Step 4: Commit**

```bash
git add src/client.rs src/main.rs
git commit -m "feat: add HTTP client with error handling"
```

---

## Task 5: Add Configuration Management

**Files:**
- Create: `src/config.rs`
- Modify: `src/main.rs`

**Step 1: Create src/config.rs**

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

use crate::errors::{QuomeError, Result};

const CONFIG_DIR: &str = ".quome";
const CONFIG_FILE: &str = "config.json";

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub user: Option<UserConfig>,
    #[serde(default)]
    pub linked: HashMap<String, LinkedContext>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UserConfig {
    pub token: String,
    pub id: Uuid,
    pub email: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LinkedContext {
    pub org_id: Uuid,
    pub org_name: String,
    #[serde(default)]
    pub app_id: Option<Uuid>,
    #[serde(default)]
    pub app_name: Option<String>,
}

impl Config {
    fn config_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().ok_or_else(|| {
            QuomeError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Could not find home directory",
            ))
        })?;
        Ok(home.join(CONFIG_DIR))
    }

    fn config_path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join(CONFIG_FILE))
    }

    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;

        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(&path)?;
        let config: Config = serde_json::from_str(&content)?;
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let dir = Self::config_dir()?;
        fs::create_dir_all(&dir)?;

        let path = Self::config_path()?;
        let tmp_path = path.with_extension("tmp");

        let content = serde_json::to_string_pretty(self)?;
        fs::write(&tmp_path, content)?;
        fs::rename(&tmp_path, &path)?;

        Ok(())
    }

    pub fn get_token(&self) -> Option<&str> {
        // Environment variable takes precedence
        if let Ok(token) = std::env::var("QUOME_TOKEN") {
            // Return None here since we can't return a reference to a local
            // The caller should check QUOME_TOKEN separately
            return None;
        }
        self.user.as_ref().map(|u| u.token.as_str())
    }

    pub fn get_token_string(&self) -> Option<String> {
        // Environment variable takes precedence
        if let Ok(token) = std::env::var("QUOME_TOKEN") {
            return Some(token);
        }
        self.user.as_ref().map(|u| u.token.clone())
    }

    pub fn require_token(&self) -> Result<String> {
        self.get_token_string().ok_or(QuomeError::NotLoggedIn)
    }

    pub fn set_user(&mut self, token: String, id: Uuid, email: String) {
        self.user = Some(UserConfig { token, id, email });
    }

    pub fn clear_user(&mut self) {
        self.user = None;
    }

    pub fn current_dir_key() -> Result<String> {
        let cwd = std::env::current_dir()?;
        Ok(cwd.to_string_lossy().to_string())
    }

    pub fn get_linked(&self) -> Result<Option<&LinkedContext>> {
        // Environment variables take precedence
        if std::env::var("QUOME_ORG").is_ok() {
            return Ok(None); // Caller should check env vars
        }

        let key = Self::current_dir_key()?;
        Ok(self.linked.get(&key))
    }

    pub fn get_linked_org_id(&self) -> Result<Option<Uuid>> {
        // Environment variable takes precedence
        if let Ok(org) = std::env::var("QUOME_ORG") {
            return org
                .parse::<Uuid>()
                .map(Some)
                .map_err(|_| QuomeError::ApiError("Invalid QUOME_ORG UUID".into()));
        }

        Ok(self.get_linked()?.map(|l| l.org_id))
    }

    pub fn require_linked_org(&self) -> Result<Uuid> {
        self.get_linked_org_id()?.ok_or(QuomeError::NoLinkedOrg)
    }

    pub fn get_linked_app_id(&self) -> Result<Option<Uuid>> {
        // Environment variable takes precedence
        if let Ok(app) = std::env::var("QUOME_APP") {
            return app
                .parse::<Uuid>()
                .map(Some)
                .map_err(|_| QuomeError::ApiError("Invalid QUOME_APP UUID".into()));
        }

        Ok(self.get_linked()?.and_then(|l| l.app_id))
    }

    pub fn require_linked_app(&self) -> Result<Uuid> {
        self.get_linked_app_id()?.ok_or(QuomeError::NoLinkedApp)
    }

    pub fn set_linked(&mut self, context: LinkedContext) -> Result<()> {
        let key = Self::current_dir_key()?;
        self.linked.insert(key, context);
        Ok(())
    }

    pub fn clear_linked(&mut self) -> Result<()> {
        let key = Self::current_dir_key()?;
        self.linked.remove(&key);
        Ok(())
    }
}
```

**Step 2: Update main.rs to declare config module**

Add at top of `src/main.rs`:
```rust
mod config;
```

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: Compiles successfully

**Step 4: Commit**

```bash
git add src/config.rs src/main.rs
git commit -m "feat: add configuration management with linking support"
```

---

## Task 6: Add API Client Methods

**Files:**
- Create: `src/api/users.rs`
- Create: `src/api/orgs.rs`
- Create: `src/api/apps.rs`
- Create: `src/api/secrets.rs`
- Create: `src/api/events.rs`
- Modify: `src/api/mod.rs`

Note: Auth uses API keys from the Quome dashboard - no session endpoints needed.

**Step 1: Create src/api/users.rs**

```rust
use uuid::Uuid;

use crate::api::models::*;
use crate::client::QuomeClient;
use crate::errors::Result;

impl QuomeClient {
    pub async fn get_current_user(&self) -> Result<User> {
        self.get("/api/v1/users").await
    }

    pub async fn create_user(&self, req: &CreateUserRequest) -> Result<User> {
        self.post("/api/v1/users", req).await
    }

    pub async fn get_user(&self, id: Uuid) -> Result<User> {
        self.get(&format!("/api/v1/users/{}", id)).await
    }
}
```

**Step 3: Create src/api/orgs.rs**

```rust
use uuid::Uuid;

use crate::api::models::*;
use crate::client::QuomeClient;
use crate::errors::Result;

impl QuomeClient {
    pub async fn list_orgs(&self) -> Result<ListOrgsResponse> {
        self.get("/api/v1/orgs").await
    }

    pub async fn create_org(&self, req: &CreateOrgRequest) -> Result<Organization> {
        self.post("/api/v1/orgs", req).await
    }

    pub async fn get_org(&self, id: Uuid) -> Result<Organization> {
        self.get(&format!("/api/v1/orgs/{}", id)).await
    }

    pub async fn list_org_members(&self, org_id: Uuid) -> Result<ListOrgMembersResponse> {
        self.get(&format!("/api/v1/orgs/{}/members", org_id)).await
    }

    pub async fn add_org_member(&self, org_id: Uuid, req: &AddOrgMemberRequest) -> Result<OrgMember> {
        self.post(&format!("/api/v1/orgs/{}/members", org_id), req).await
    }

    pub async fn list_org_keys(&self, org_id: Uuid) -> Result<ListOrgKeysResponse> {
        self.get(&format!("/api/v1/orgs/{}/keys", org_id)).await
    }

    pub async fn create_org_key(&self, org_id: Uuid, req: &CreateOrgKeyRequest) -> Result<CreatedOrgKey> {
        self.post(&format!("/api/v1/orgs/{}/keys", org_id), req).await
    }

    pub async fn delete_org_key(&self, org_id: Uuid, key_id: Uuid) -> Result<()> {
        self.delete(&format!("/api/v1/orgs/{}/apikeys/{}", org_id, key_id)).await
    }
}
```

**Step 4: Create src/api/apps.rs**

```rust
use uuid::Uuid;

use crate::api::models::*;
use crate::client::QuomeClient;
use crate::errors::Result;

impl QuomeClient {
    pub async fn list_apps(&self, org_id: Uuid) -> Result<AppList> {
        self.get(&format!("/api/v1/orgs/{}/apps", org_id)).await
    }

    pub async fn create_app(&self, org_id: Uuid, req: &CreateAppRequest) -> Result<App> {
        self.post(&format!("/api/v1/orgs/{}/apps", org_id), req).await
    }

    pub async fn get_app(&self, org_id: Uuid, app_id: Uuid) -> Result<App> {
        self.get(&format!("/api/v1/orgs/{}/apps/{}", org_id, app_id)).await
    }

    pub async fn update_app(&self, org_id: Uuid, app_id: Uuid, req: &UpdateAppRequest) -> Result<App> {
        self.put(&format!("/api/v1/orgs/{}/apps/{}", org_id, app_id), req).await
    }

    pub async fn delete_app(&self, org_id: Uuid, app_id: Uuid) -> Result<()> {
        self.delete(&format!("/api/v1/orgs/{}/apps/{}", org_id, app_id)).await
    }

    pub async fn list_deployments(&self, org_id: Uuid, app_id: Uuid) -> Result<DeploymentList> {
        self.get(&format!("/api/v1/orgs/{}/apps/{}/deployments", org_id, app_id)).await
    }

    pub async fn get_deployment(&self, org_id: Uuid, app_id: Uuid, deployment_id: Uuid) -> Result<Deployment> {
        self.get(&format!("/api/v1/orgs/{}/apps/{}/deployments/{}", org_id, app_id, deployment_id)).await
    }

    pub async fn get_logs(&self, org_id: Uuid, app_id: Uuid, limit: Option<u32>) -> Result<ListLogsResponse> {
        let mut path = format!("/api/v1/orgs/{}/apps/{}/logs", org_id, app_id);
        if let Some(l) = limit {
            path = format!("{}?limit={}", path, l);
        }
        self.get(&path).await
    }
}
```

**Step 5: Create src/api/secrets.rs**

```rust
use uuid::Uuid;

use crate::api::models::*;
use crate::client::QuomeClient;
use crate::errors::Result;

impl QuomeClient {
    pub async fn list_secrets(&self, org_id: Uuid) -> Result<ListSecretsResponse> {
        self.get(&format!("/api/v1/orgs/{}/secrets", org_id)).await
    }

    pub async fn create_secret(&self, org_id: Uuid, req: &CreateSecretRequest) -> Result<Secret> {
        self.post(&format!("/api/v1/orgs/{}/secrets", org_id), req).await
    }

    pub async fn get_secret(&self, org_id: Uuid, secret_id: Uuid) -> Result<Secret> {
        self.get(&format!("/api/v1/orgs/{}/secrets/{}", org_id, secret_id)).await
    }

    pub async fn update_secret(&self, org_id: Uuid, secret_id: Uuid, req: &UpdateSecretRequest) -> Result<Secret> {
        self.put(&format!("/api/v1/orgs/{}/secrets/{}", org_id, secret_id), req).await
    }

    pub async fn delete_secret(&self, org_id: Uuid, secret_id: Uuid) -> Result<()> {
        self.delete(&format!("/api/v1/orgs/{}/secrets/{}", org_id, secret_id)).await
    }
}
```

**Step 6: Create src/api/events.rs**

```rust
use uuid::Uuid;

use crate::api::models::*;
use crate::client::QuomeClient;
use crate::errors::Result;

impl QuomeClient {
    pub async fn list_events(&self, org_id: Uuid, limit: Option<u32>) -> Result<ListEventsResponse> {
        let mut path = format!("/api/v1/orgs/{}/events", org_id);
        if let Some(l) = limit {
            path = format!("{}?limit={}", path, l);
        }
        self.get(&path).await
    }
}
```

**Step 7: Update src/api/mod.rs**

```rust
pub mod models;
mod users;
mod orgs;
mod apps;
mod secrets;
mod events;
```

**Step 8: Verify it compiles**

Run: `cargo build`
Expected: Compiles successfully

**Step 9: Commit**

```bash
git add src/api/
git commit -m "feat: add typed API client methods for all endpoints"
```

---

## Task 7: Add Command Infrastructure

**Files:**
- Create: `src/commands/mod.rs`
- Modify: `src/main.rs`

**Step 1: Create src/commands/mod.rs**

```rust
pub mod login;
pub mod logout;
pub mod whoami;
pub mod link;
pub mod unlink;
pub mod orgs;
pub mod members;
pub mod apps;
pub mod deployments;
pub mod logs;
pub mod secrets;
pub mod keys;
pub mod events;
```

**Step 2: Rewrite src/main.rs with full command structure**

```rust
mod api;
mod client;
mod config;
mod errors;
mod commands;

use clap::Parser;
use colored::Colorize;

#[derive(Parser)]
#[command(name = "quome")]
#[command(about = "CLI for the Quome platform")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Login to Quome
    Login(commands::login::Args),
    /// Logout from Quome
    Logout(commands::logout::Args),
    /// Show current user info
    Whoami(commands::whoami::Args),
    /// Link current directory to an org and app
    Link(commands::link::Args),
    /// Unlink current directory
    Unlink(commands::unlink::Args),
    /// Manage organizations
    Orgs {
        #[command(subcommand)]
        command: commands::orgs::OrgsCommands,
    },
    /// Manage organization members
    Members {
        #[command(subcommand)]
        command: commands::members::MembersCommands,
    },
    /// Manage applications
    Apps {
        #[command(subcommand)]
        command: commands::apps::AppsCommands,
    },
    /// Manage deployments
    Deployments {
        #[command(subcommand)]
        command: commands::deployments::DeploymentsCommands,
    },
    /// View application logs
    Logs(commands::logs::Args),
    /// Manage secrets
    Secrets {
        #[command(subcommand)]
        command: commands::secrets::SecretsCommands,
    },
    /// Manage API keys
    Keys {
        #[command(subcommand)]
        command: commands::keys::KeysCommands,
    },
    /// View organization events
    Events(commands::events::Args),
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Login(args) => commands::login::execute(args).await,
        Commands::Logout(args) => commands::logout::execute(args).await,
        Commands::Whoami(args) => commands::whoami::execute(args).await,
        Commands::Link(args) => commands::link::execute(args).await,
        Commands::Unlink(args) => commands::unlink::execute(args).await,
        Commands::Orgs { command } => commands::orgs::execute(command).await,
        Commands::Members { command } => commands::members::execute(command).await,
        Commands::Apps { command } => commands::apps::execute(command).await,
        Commands::Deployments { command } => commands::deployments::execute(command).await,
        Commands::Logs(args) => commands::logs::execute(args).await,
        Commands::Secrets { command } => commands::secrets::execute(command).await,
        Commands::Keys { command } => commands::keys::execute(command).await,
        Commands::Events(args) => commands::events::execute(args).await,
    };

    if let Err(e) = result {
        eprintln!("{} {}", "error:".red().bold(), e);
        std::process::exit(1);
    }
}
```

**Step 3: This will not compile yet - we need to create command modules**

Continue to next tasks to create each command module.

---

## Task 8: Add Login Command

**Files:**
- Create: `src/commands/login.rs`

**Step 1: Create src/commands/login.rs**

```rust
use clap::Parser;
use colored::Colorize;

use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;

#[derive(Parser)]
pub struct Args {
    /// API token (will prompt if not provided)
    #[arg(short, long)]
    token: Option<String>,
}

pub async fn execute(args: Args) -> Result<()> {
    let token = match args.token {
        Some(t) => t,
        None => inquire::Password::new("API Token:")
            .without_confirmation()
            .with_help_message("Get your token from the Quome dashboard")
            .prompt()
            .map_err(|e| {
                crate::errors::QuomeError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
            })?,
    };

    println!("Validating token...");

    // Validate the token by fetching user info
    let client = QuomeClient::new(Some(&token), None)?;
    let user = client.get_current_user().await?;

    // Save to config
    let mut config = Config::load()?;
    config.set_user(token, user.id, user.email.clone());
    config.save()?;

    println!(
        "{} Logged in as {}",
        "Success!".green().bold(),
        user.email.cyan()
    );

    Ok(())
}
```

---

## Task 9: Add Logout Command

**Files:**
- Create: `src/commands/logout.rs`

**Step 1: Create src/commands/logout.rs**

```rust
use clap::Parser;
use colored::Colorize;

use crate::config::Config;
use crate::errors::Result;

#[derive(Parser)]
pub struct Args {}

pub async fn execute(_args: Args) -> Result<()> {
    let mut config = Config::load()?;

    if config.user.is_none() {
        println!("Not logged in.");
        return Ok(());
    }

    config.clear_user();
    config.save()?;

    println!("{} Logged out successfully.", "Success!".green().bold());

    Ok(())
}
```

---

## Task 10: Add Whoami Command

**Files:**
- Create: `src/commands/whoami.rs`

**Step 1: Create src/commands/whoami.rs**

```rust
use clap::Parser;
use colored::Colorize;

use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;

#[derive(Parser)]
pub struct Args {
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

pub async fn execute(args: Args) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let client = QuomeClient::new(Some(&token), None)?;
    let user = client.get_current_user().await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&user)?);
    } else {
        println!("{}", "Current User".bold());
        println!("  {} {}", "ID:".dimmed(), user.id);
        println!("  {} {}", "Username:".dimmed(), user.username);
        println!("  {} {}", "Email:".dimmed(), user.email);

        // Show linked context if any
        if let Some(linked) = config.get_linked()? {
            println!();
            println!("{}", "Linked Context".bold());
            println!("  {} {}", "Organization:".dimmed(), linked.org_name);
            if let Some(ref app_name) = linked.app_name {
                println!("  {} {}", "Application:".dimmed(), app_name);
            }
        }
    }

    Ok(())
}
```

---

## Task 11: Add Link Command

**Files:**
- Create: `src/commands/link.rs`

**Step 1: Create src/commands/link.rs**

```rust
use clap::Parser;
use colored::Colorize;
use inquire::Select;

use crate::client::QuomeClient;
use crate::config::{Config, LinkedContext};
use crate::errors::Result;

#[derive(Parser)]
pub struct Args {
    /// Organization ID (skips interactive selection)
    #[arg(long)]
    org: Option<String>,

    /// Application ID (skips interactive selection)
    #[arg(long)]
    app: Option<String>,
}

pub async fn execute(args: Args) -> Result<()> {
    let mut config = Config::load()?;
    let token = config.require_token()?;

    let client = QuomeClient::new(Some(&token), None)?;

    // Get or select organization
    let (org_id, org_name) = if let Some(ref org_str) = args.org {
        let org_id = org_str.parse().map_err(|_| {
            crate::errors::QuomeError::ApiError("Invalid organization ID".into())
        })?;
        let org = client.get_org(org_id).await?;
        (org.id, org.name)
    } else {
        let orgs_resp = client.list_orgs().await?;

        if orgs_resp.organizations.is_empty() {
            println!("No organizations found. Create one with `quome orgs create <name>`");
            return Ok(());
        }

        let options: Vec<String> = orgs_resp
            .organizations
            .iter()
            .map(|o| format!("{} ({})", o.name, o.id))
            .collect();

        let selection = Select::new("Select organization:", options)
            .prompt()
            .map_err(|e| {
                crate::errors::QuomeError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                ))
            })?;

        let idx = orgs_resp
            .organizations
            .iter()
            .position(|o| format!("{} ({})", o.name, o.id) == selection)
            .unwrap();

        let org = &orgs_resp.organizations[idx];
        (org.id, org.name.clone())
    };

    // Get or select application (optional)
    let (app_id, app_name) = if let Some(ref app_str) = args.app {
        let app_id = app_str.parse().map_err(|_| {
            crate::errors::QuomeError::ApiError("Invalid application ID".into())
        })?;
        let app = client.get_app(org_id, app_id).await?;
        (Some(app.id), Some(app.name))
    } else {
        let apps_resp = client.list_apps(org_id).await?;

        if apps_resp.apps.is_empty() {
            println!("No applications found in this organization.");
            (None, None)
        } else {
            let mut options: Vec<String> = apps_resp
                .apps
                .iter()
                .map(|a| format!("{} ({})", a.name, a.id))
                .collect();
            options.push("(Skip - don't link an app)".to_string());

            let selection = Select::new("Select application:", options)
                .prompt()
                .map_err(|e| {
                    crate::errors::QuomeError::Io(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    ))
                })?;

            if selection == "(Skip - don't link an app)" {
                (None, None)
            } else {
                let idx = apps_resp
                    .apps
                    .iter()
                    .position(|a| format!("{} ({})", a.name, a.id) == selection)
                    .unwrap();

                let app = &apps_resp.apps[idx];
                (Some(app.id), Some(app.name.clone()))
            }
        }
    };

    // Save linked context
    config.set_linked(LinkedContext {
        org_id,
        org_name: org_name.clone(),
        app_id,
        app_name: app_name.clone(),
    })?;
    config.save()?;

    println!("{} Linked to:", "Success!".green().bold());
    println!("  {} {}", "Organization:".dimmed(), org_name.cyan());
    if let Some(name) = app_name {
        println!("  {} {}", "Application:".dimmed(), name.cyan());
    }

    Ok(())
}
```

---

## Task 12: Add Unlink Command

**Files:**
- Create: `src/commands/unlink.rs`

**Step 1: Create src/commands/unlink.rs**

```rust
use clap::Parser;
use colored::Colorize;

use crate::config::Config;
use crate::errors::Result;

#[derive(Parser)]
pub struct Args {}

pub async fn execute(_args: Args) -> Result<()> {
    let mut config = Config::load()?;

    if config.get_linked()?.is_none() {
        println!("Not linked to any organization or application.");
        return Ok(());
    }

    config.clear_linked()?;
    config.save()?;

    println!(
        "{} Unlinked current directory.",
        "Success!".green().bold()
    );

    Ok(())
}
```

---

## Task 13: Add Orgs Commands

**Files:**
- Create: `src/commands/orgs.rs`

**Step 1: Create src/commands/orgs.rs**

```rust
use clap::{Parser, Subcommand};
use colored::Colorize;
use uuid::Uuid;

use crate::api::models::CreateOrgRequest;
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;

#[derive(Subcommand)]
pub enum OrgsCommands {
    /// List all organizations
    List(ListArgs),
    /// Create a new organization
    Create(CreateArgs),
    /// Get organization details
    Get(GetArgs),
}

#[derive(Parser)]
pub struct ListArgs {
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct CreateArgs {
    /// Organization name
    name: String,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct GetArgs {
    /// Organization ID (uses linked org if not provided)
    #[arg(short, long)]
    id: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

pub async fn execute(command: OrgsCommands) -> Result<()> {
    match command {
        OrgsCommands::List(args) => list(args).await,
        OrgsCommands::Create(args) => create(args).await,
        OrgsCommands::Get(args) => get(args).await,
    }
}

async fn list(args: ListArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let client = QuomeClient::new(Some(&token), None)?;
    let response = client.list_orgs().await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.organizations)?);
    } else {
        if response.organizations.is_empty() {
            println!("No organizations found.");
            return Ok(());
        }

        println!(
            "{:<36}  {:<20}  {:<20}",
            "ID".bold(),
            "NAME".bold(),
            "CREATED".bold()
        );
        println!("{}", "-".repeat(78));

        for org in response.organizations {
            println!(
                "{:<36}  {:<20}  {:<20}",
                org.id,
                org.name,
                org.created_at.format("%Y-%m-%d %H:%M")
            );
        }
    }

    Ok(())
}

async fn create(args: CreateArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let client = QuomeClient::new(Some(&token), None)?;
    let org = client
        .create_org(&CreateOrgRequest { name: args.name })
        .await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&org)?);
    } else {
        println!("{} Created organization:", "Success!".green().bold());
        println!("  {} {}", "ID:".dimmed(), org.id);
        println!("  {} {}", "Name:".dimmed(), org.name);
    }

    Ok(())
}

async fn get(args: GetArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.id {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;
    let org = client.get_org(org_id).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&org)?);
    } else {
        println!("{}", "Organization".bold());
        println!("  {} {}", "ID:".dimmed(), org.id);
        println!("  {} {}", "Name:".dimmed(), org.name);
        println!(
            "  {} {}",
            "Created:".dimmed(),
            org.created_at.format("%Y-%m-%d %H:%M:%S")
        );
        println!(
            "  {} {}",
            "Updated:".dimmed(),
            org.updated_at.format("%Y-%m-%d %H:%M:%S")
        );
    }

    Ok(())
}
```

---

## Task 14: Add Members Commands

**Files:**
- Create: `src/commands/members.rs`

**Step 1: Create src/commands/members.rs**

```rust
use clap::{Parser, Subcommand};
use colored::Colorize;
use uuid::Uuid;

use crate::api::models::AddOrgMemberRequest;
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;

#[derive(Subcommand)]
pub enum MembersCommands {
    /// List organization members
    List(ListArgs),
    /// Add a member to the organization
    Add(AddArgs),
}

#[derive(Parser)]
pub struct ListArgs {
    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct AddArgs {
    /// User ID to add
    user_id: Uuid,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

pub async fn execute(command: MembersCommands) -> Result<()> {
    match command {
        MembersCommands::List(args) => list(args).await,
        MembersCommands::Add(args) => add(args).await,
    }
}

async fn list(args: ListArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;
    let response = client.list_org_members(org_id).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.members)?);
    } else {
        if response.members.is_empty() {
            println!("No members found.");
            return Ok(());
        }

        println!(
            "{:<36}  {:<36}  {:<20}",
            "ID".bold(),
            "USER ID".bold(),
            "JOINED".bold()
        );
        println!("{}", "-".repeat(94));

        for member in response.members {
            println!(
                "{:<36}  {:<36}  {:<20}",
                member.id,
                member.user_id,
                member.created_at.format("%Y-%m-%d %H:%M")
            );
        }
    }

    Ok(())
}

async fn add(args: AddArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;
    let member = client
        .add_org_member(org_id, &AddOrgMemberRequest { user_id: args.user_id })
        .await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&member)?);
    } else {
        println!("{} Added member:", "Success!".green().bold());
        println!("  {} {}", "Member ID:".dimmed(), member.id);
        println!("  {} {}", "User ID:".dimmed(), member.user_id);
    }

    Ok(())
}
```

---

## Task 15: Add Apps Commands

**Files:**
- Create: `src/commands/apps.rs`

**Step 1: Create src/commands/apps.rs**

```rust
use clap::{Parser, Subcommand};
use colored::Colorize;
use uuid::Uuid;

use crate::api::models::{AppSpec, ContainerSpec, CreateAppRequest, UpdateAppRequest};
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;

#[derive(Subcommand)]
pub enum AppsCommands {
    /// List all applications
    List(ListArgs),
    /// Create a new application
    Create(CreateArgs),
    /// Get application details
    Get(GetArgs),
    /// Update an application
    Update(UpdateArgs),
    /// Delete an application
    Delete(DeleteArgs),
}

#[derive(Parser)]
pub struct ListArgs {
    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct CreateArgs {
    /// Application name
    name: String,

    /// Application description
    #[arg(short, long)]
    description: Option<String>,

    /// Container image (e.g., nginx:latest)
    #[arg(long)]
    image: String,

    /// Container port
    #[arg(long, default_value = "80")]
    port: u16,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct GetArgs {
    /// Application ID (uses linked app if not provided)
    #[arg(short, long)]
    id: Option<Uuid>,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct UpdateArgs {
    /// Application ID (uses linked app if not provided)
    #[arg(short, long)]
    id: Option<Uuid>,

    /// New name
    #[arg(long)]
    name: Option<String>,

    /// New description
    #[arg(long)]
    description: Option<String>,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct DeleteArgs {
    /// Application ID
    id: Uuid,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Skip confirmation prompt
    #[arg(short, long)]
    force: bool,
}

pub async fn execute(command: AppsCommands) -> Result<()> {
    match command {
        AppsCommands::List(args) => list(args).await,
        AppsCommands::Create(args) => create(args).await,
        AppsCommands::Get(args) => get(args).await,
        AppsCommands::Update(args) => update(args).await,
        AppsCommands::Delete(args) => delete(args).await,
    }
}

async fn list(args: ListArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;
    let response = client.list_apps(org_id).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.apps)?);
    } else {
        if response.apps.is_empty() {
            println!("No applications found.");
            return Ok(());
        }

        println!(
            "{:<36}  {:<20}  {:<20}",
            "ID".bold(),
            "NAME".bold(),
            "CREATED".bold()
        );
        println!("{}", "-".repeat(78));

        for app in response.apps {
            println!(
                "{:<36}  {:<20}  {:<20}",
                app.id,
                app.name,
                app.created_at.format("%Y-%m-%d %H:%M")
            );
        }
    }

    Ok(())
}

async fn create(args: CreateArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;

    let spec = AppSpec {
        containers: vec![ContainerSpec {
            name: args.name.clone(),
            image: args.image,
            port: args.port,
        }],
    };

    let app = client
        .create_app(
            org_id,
            &CreateAppRequest {
                name: args.name,
                description: args.description,
                spec,
            },
        )
        .await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&app)?);
    } else {
        println!("{} Created application:", "Success!".green().bold());
        println!("  {} {}", "ID:".dimmed(), app.id);
        println!("  {} {}", "Name:".dimmed(), app.name);
    }

    Ok(())
}

async fn get(args: GetArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let app_id = match args.id {
        Some(id) => id,
        None => config.require_linked_app()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;
    let app = client.get_app(org_id, app_id).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&app)?);
    } else {
        println!("{}", "Application".bold());
        println!("  {} {}", "ID:".dimmed(), app.id);
        println!("  {} {}", "Name:".dimmed(), app.name);
        if let Some(ref desc) = app.description {
            println!("  {} {}", "Description:".dimmed(), desc);
        }
        println!(
            "  {} {}",
            "Created:".dimmed(),
            app.created_at.format("%Y-%m-%d %H:%M:%S")
        );

        if let Some(ref spec) = app.spec {
            if !spec.containers.is_empty() {
                println!();
                println!("  {}", "Containers:".bold());
                for container in &spec.containers {
                    println!("    {} {}", "-".dimmed(), container.name);
                    println!("      {} {}", "Image:".dimmed(), container.image);
                    println!("      {} {}", "Port:".dimmed(), container.port);
                }
            }
        }
    }

    Ok(())
}

async fn update(args: UpdateArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let app_id = match args.id {
        Some(id) => id,
        None => config.require_linked_app()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;
    let app = client
        .update_app(
            org_id,
            app_id,
            &UpdateAppRequest {
                name: args.name,
                description: args.description,
                spec: None,
            },
        )
        .await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&app)?);
    } else {
        println!("{} Updated application:", "Success!".green().bold());
        println!("  {} {}", "ID:".dimmed(), app.id);
        println!("  {} {}", "Name:".dimmed(), app.name);
    }

    Ok(())
}

async fn delete(args: DeleteArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    if !args.force {
        let confirm = inquire::Confirm::new(&format!(
            "Are you sure you want to delete application {}?",
            args.id
        ))
        .with_default(false)
        .prompt()
        .map_err(|e| {
            crate::errors::QuomeError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                e.to_string(),
            ))
        })?;

        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let client = QuomeClient::new(Some(&token), None)?;
    client.delete_app(org_id, args.id).await?;

    println!(
        "{} Deleted application {}",
        "Success!".green().bold(),
        args.id
    );

    Ok(())
}
```

---

## Task 16: Add Deployments Commands

**Files:**
- Create: `src/commands/deployments.rs`

**Step 1: Create src/commands/deployments.rs**

```rust
use clap::{Parser, Subcommand};
use colored::Colorize;
use uuid::Uuid;

use crate::api::models::DeploymentStatus;
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;

#[derive(Subcommand)]
pub enum DeploymentsCommands {
    /// List deployments
    List(ListArgs),
    /// Get deployment details
    Get(GetArgs),
}

#[derive(Parser)]
pub struct ListArgs {
    /// Application ID (uses linked app if not provided)
    #[arg(long)]
    app: Option<Uuid>,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct GetArgs {
    /// Deployment ID
    id: Uuid,

    /// Application ID (uses linked app if not provided)
    #[arg(long)]
    app: Option<Uuid>,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

pub async fn execute(command: DeploymentsCommands) -> Result<()> {
    match command {
        DeploymentsCommands::List(args) => list(args).await,
        DeploymentsCommands::Get(args) => get(args).await,
    }
}

fn status_color(status: &DeploymentStatus) -> colored::ColoredString {
    match status {
        DeploymentStatus::Created => "created".yellow(),
        DeploymentStatus::InProgress => "in_progress".blue(),
        DeploymentStatus::Deployed => "deployed".green(),
        DeploymentStatus::Failed => "failed".red(),
    }
}

async fn list(args: ListArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let app_id = match args.app {
        Some(id) => id,
        None => config.require_linked_app()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;
    let response = client.list_deployments(org_id, app_id).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.deployments)?);
    } else {
        if response.deployments.is_empty() {
            println!("No deployments found.");
            return Ok(());
        }

        println!(
            "{:<36}  {:<12}  {:<20}",
            "ID".bold(),
            "STATUS".bold(),
            "CREATED".bold()
        );
        println!("{}", "-".repeat(70));

        for deployment in response.deployments {
            println!(
                "{:<36}  {:<12}  {:<20}",
                deployment.id,
                status_color(&deployment.status),
                deployment.created_at.format("%Y-%m-%d %H:%M")
            );
        }
    }

    Ok(())
}

async fn get(args: GetArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let app_id = match args.app {
        Some(id) => id,
        None => config.require_linked_app()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;
    let deployment = client.get_deployment(org_id, app_id, args.id).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&deployment)?);
    } else {
        println!("{}", "Deployment".bold());
        println!("  {} {}", "ID:".dimmed(), deployment.id);
        println!("  {} {}", "Status:".dimmed(), status_color(&deployment.status));
        println!(
            "  {} {}",
            "Created:".dimmed(),
            deployment.created_at.format("%Y-%m-%d %H:%M:%S")
        );

        if let Some(ref msg) = deployment.failure_message {
            println!("  {} {}", "Failure:".red(), msg);
        }

        if !deployment.events.is_empty() {
            println!();
            println!("  {}", "Events:".bold());
            for event in &deployment.events {
                println!(
                    "    {} {} - {}",
                    event.created_at.format("%H:%M:%S").to_string().dimmed(),
                    "-".dimmed(),
                    event.message
                );
            }
        }
    }

    Ok(())
}
```

---

## Task 17: Add Logs Command

**Files:**
- Create: `src/commands/logs.rs`

**Step 1: Create src/commands/logs.rs**

```rust
use clap::Parser;
use colored::Colorize;
use uuid::Uuid;

use crate::api::models::LogLevel;
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;

#[derive(Parser)]
pub struct Args {
    /// Application ID (uses linked app if not provided)
    #[arg(long)]
    app: Option<Uuid>,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Number of log entries to fetch
    #[arg(short = 'n', long, default_value = "100")]
    limit: u32,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

fn level_color(level: &LogLevel) -> colored::ColoredString {
    match level {
        LogLevel::Debug => "DEBUG".dimmed(),
        LogLevel::Info => "INFO ".blue(),
        LogLevel::Warn => "WARN ".yellow(),
        LogLevel::Error => "ERROR".red(),
    }
}

pub async fn execute(args: Args) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let app_id = match args.app {
        Some(id) => id,
        None => config.require_linked_app()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;
    let response = client.get_logs(org_id, app_id, Some(args.limit)).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.logs)?);
    } else {
        if response.logs.is_empty() {
            println!("No logs found.");
            return Ok(());
        }

        for entry in response.logs {
            println!(
                "{} {} {}",
                entry.timestamp.format("%Y-%m-%d %H:%M:%S").to_string().dimmed(),
                level_color(&entry.level),
                entry.message
            );
        }
    }

    Ok(())
}
```

---

## Task 18: Add Secrets Commands

**Files:**
- Create: `src/commands/secrets.rs`

**Step 1: Create src/commands/secrets.rs**

```rust
use clap::{Parser, Subcommand};
use colored::Colorize;
use uuid::Uuid;

use crate::api::models::{CreateSecretRequest, UpdateSecretRequest};
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;

#[derive(Subcommand)]
pub enum SecretsCommands {
    /// List all secrets
    List(ListArgs),
    /// Set (create or update) a secret
    Set(SetArgs),
    /// Get a secret value
    Get(GetArgs),
    /// Delete a secret
    Delete(DeleteArgs),
}

#[derive(Parser)]
pub struct ListArgs {
    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct SetArgs {
    /// Secret name
    name: String,

    /// Secret value
    value: String,

    /// Secret description
    #[arg(short, long)]
    description: Option<String>,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct GetArgs {
    /// Secret name
    name: String,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct DeleteArgs {
    /// Secret name
    name: String,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Skip confirmation prompt
    #[arg(short, long)]
    force: bool,
}

pub async fn execute(command: SecretsCommands) -> Result<()> {
    match command {
        SecretsCommands::List(args) => list(args).await,
        SecretsCommands::Set(args) => set(args).await,
        SecretsCommands::Get(args) => get(args).await,
        SecretsCommands::Delete(args) => delete(args).await,
    }
}

async fn list(args: ListArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;
    let response = client.list_secrets(org_id).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.secrets)?);
    } else {
        if response.secrets.is_empty() {
            println!("No secrets found.");
            return Ok(());
        }

        println!(
            "{:<20}  {:<36}  {:<20}",
            "NAME".bold(),
            "ID".bold(),
            "UPDATED".bold()
        );
        println!("{}", "-".repeat(78));

        for secret in response.secrets {
            println!(
                "{:<20}  {:<36}  {:<20}",
                secret.name,
                secret.id,
                secret.updated_at.format("%Y-%m-%d %H:%M")
            );
        }
    }

    Ok(())
}

async fn set(args: SetArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;

    // Check if secret exists
    let response = client.list_secrets(org_id).await?;
    let existing = response.secrets.iter().find(|s| s.name == args.name);

    let secret = if let Some(existing_secret) = existing {
        // Update existing secret
        client
            .update_secret(
                org_id,
                existing_secret.id,
                &UpdateSecretRequest {
                    name: None,
                    value: Some(args.value),
                    description: args.description,
                },
            )
            .await?
    } else {
        // Create new secret
        client
            .create_secret(
                org_id,
                &CreateSecretRequest {
                    name: args.name,
                    value: args.value,
                    description: args.description,
                },
            )
            .await?
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&secret)?);
    } else {
        let action = if existing.is_some() { "Updated" } else { "Created" };
        println!("{} {} secret:", "Success!".green().bold(), action);
        println!("  {} {}", "Name:".dimmed(), secret.name);
        println!("  {} {}", "ID:".dimmed(), secret.id);
    }

    Ok(())
}

async fn get(args: GetArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;

    // Find secret by name
    let response = client.list_secrets(org_id).await?;
    let secret_meta = response
        .secrets
        .iter()
        .find(|s| s.name == args.name)
        .ok_or_else(|| crate::errors::QuomeError::NotFound(format!("Secret '{}'", args.name)))?;

    let secret = client.get_secret(org_id, secret_meta.id).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&secret)?);
    } else {
        println!("{}", secret.value);
    }

    Ok(())
}

async fn delete(args: DeleteArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    if !args.force {
        let confirm = inquire::Confirm::new(&format!(
            "Are you sure you want to delete secret '{}'?",
            args.name
        ))
        .with_default(false)
        .prompt()
        .map_err(|e| {
            crate::errors::QuomeError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                e.to_string(),
            ))
        })?;

        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let client = QuomeClient::new(Some(&token), None)?;

    // Find secret by name
    let response = client.list_secrets(org_id).await?;
    let secret = response
        .secrets
        .iter()
        .find(|s| s.name == args.name)
        .ok_or_else(|| crate::errors::QuomeError::NotFound(format!("Secret '{}'", args.name)))?;

    client.delete_secret(org_id, secret.id).await?;

    println!(
        "{} Deleted secret '{}'",
        "Success!".green().bold(),
        args.name
    );

    Ok(())
}
```

---

## Task 19: Add Keys Commands

**Files:**
- Create: `src/commands/keys.rs`

**Step 1: Create src/commands/keys.rs**

```rust
use chrono::{Duration, Utc};
use clap::{Parser, Subcommand};
use colored::Colorize;
use uuid::Uuid;

use crate::api::models::CreateOrgKeyRequest;
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;

#[derive(Subcommand)]
pub enum KeysCommands {
    /// List API keys
    List(ListArgs),
    /// Create a new API key
    Create(CreateArgs),
    /// Delete an API key
    Delete(DeleteArgs),
}

#[derive(Parser)]
pub struct ListArgs {
    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct CreateArgs {
    /// Days until expiration (0 = never expires)
    #[arg(long, default_value = "0")]
    expires_days: u32,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct DeleteArgs {
    /// API key ID
    id: Uuid,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Skip confirmation prompt
    #[arg(short, long)]
    force: bool,
}

pub async fn execute(command: KeysCommands) -> Result<()> {
    match command {
        KeysCommands::List(args) => list(args).await,
        KeysCommands::Create(args) => create(args).await,
        KeysCommands::Delete(args) => delete(args).await,
    }
}

async fn list(args: ListArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;
    let response = client.list_org_keys(org_id).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.keys)?);
    } else {
        if response.keys.is_empty() {
            println!("No API keys found.");
            return Ok(());
        }

        println!(
            "{:<36}  {:<20}",
            "ID".bold(),
            "CREATED".bold()
        );
        println!("{}", "-".repeat(58));

        for key in response.keys {
            println!(
                "{:<36}  {:<20}",
                key.id,
                key.created_at.format("%Y-%m-%d %H:%M")
            );
        }
    }

    Ok(())
}

async fn create(args: CreateArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let expiration = if args.expires_days > 0 {
        Some(Utc::now() + Duration::days(args.expires_days as i64))
    } else {
        None
    };

    let client = QuomeClient::new(Some(&token), None)?;
    let key = client
        .create_org_key(org_id, &CreateOrgKeyRequest { expiration })
        .await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&key)?);
    } else {
        println!("{} Created API key:", "Success!".green().bold());
        println!("  {} {}", "ID:".dimmed(), key.id);
        println!();
        println!(
            "  {} {}",
            "Key:".yellow().bold(),
            key.key.cyan()
        );
        println!();
        println!(
            "  {}",
            "Save this key - it won't be shown again!".yellow()
        );
    }

    Ok(())
}

async fn delete(args: DeleteArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    if !args.force {
        let confirm = inquire::Confirm::new(&format!(
            "Are you sure you want to delete API key {}?",
            args.id
        ))
        .with_default(false)
        .prompt()
        .map_err(|e| {
            crate::errors::QuomeError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                e.to_string(),
            ))
        })?;

        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let client = QuomeClient::new(Some(&token), None)?;
    client.delete_org_key(org_id, args.id).await?;

    println!(
        "{} Deleted API key {}",
        "Success!".green().bold(),
        args.id
    );

    Ok(())
}
```

---

## Task 20: Add Events Command

**Files:**
- Create: `src/commands/events.rs`

**Step 1: Create src/commands/events.rs**

```rust
use clap::Parser;
use colored::Colorize;
use uuid::Uuid;

use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;

#[derive(Parser)]
pub struct Args {
    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Number of events to fetch
    #[arg(short = 'n', long, default_value = "50")]
    limit: u32,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

pub async fn execute(args: Args) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;
    let response = client.list_events(org_id, Some(args.limit)).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.events)?);
    } else {
        if response.events.is_empty() {
            println!("No events found.");
            return Ok(());
        }

        for event in response.events {
            let resource_name = event
                .resource
                .name
                .as_deref()
                .unwrap_or(&event.resource.id.to_string());

            println!(
                "{} {} {} {} on {} {}",
                event.created_at.format("%Y-%m-%d %H:%M:%S").to_string().dimmed(),
                event.actor.email.cyan(),
                event.event_type.yellow(),
                event.resource.resource_type.dimmed(),
                resource_name.bold(),
                format!("({})", event.resource.id).dimmed()
            );
        }
    }

    Ok(())
}
```

---

## Task 21: Build and Test

**Step 1: Verify everything compiles**

Run: `cargo build`
Expected: Compiles successfully with possible warnings

**Step 2: Run help to verify CLI structure**

Run: `cargo run -- --help`
Expected: Shows all commands

Run: `cargo run -- apps --help`
Expected: Shows apps subcommands

**Step 3: Build release binary**

Run: `cargo build --release`
Expected: Creates optimized binary at `target/release/quome`

**Step 4: Commit all command modules**

```bash
git add src/commands/ src/main.rs
git commit -m "feat: add all CLI commands"
```

---

## Task 22: Final Cleanup and Documentation

**Step 1: Run clippy for lints**

Run: `cargo clippy`
Fix any warnings

**Step 2: Format code**

Run: `cargo fmt`

**Step 3: Final commit**

```bash
git add -A
git commit -m "chore: format and lint cleanup"
```

---

## Summary

This plan implements a full-featured Quome CLI with:

- **13 command modules**: login, logout, whoami, link, unlink, orgs, members, apps, deployments, logs, secrets, keys, events
- **Typed API client**: All endpoints covered with proper request/response types
- **Configuration management**: Token storage, directory linking, environment variable overrides
- **Error handling**: Custom error types with user-friendly messages
- **Output formatting**: Human-readable tables and JSON output modes
- **Interactive prompts**: For login, linking, and confirmations

The CLI follows railway-cli patterns adapted for REST API:
- Clap derive macros for CLI structure
- Layered architecture (commands → client → config)
- Atomic config file writes
- Colored terminal output
