use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

// ============ Common ============

/// Standard list envelope: `{"data": [...], "meta": {...}}`
#[derive(Debug, Deserialize)]
pub struct PaginatedResponse<T> {
    pub data: Vec<T>,
    #[serde(default)]
    pub meta: Option<PaginationMeta>,
}

#[derive(Debug, Deserialize, Default)]
pub struct PaginationMeta {
    #[serde(default)]
    #[allow(dead_code)]
    pub total: Option<i64>,
    #[serde(default)]
    #[allow(dead_code)]
    pub limit: Option<i64>,
    #[serde(default)]
    #[allow(dead_code)]
    pub offset: Option<i64>,
    #[serde(default)]
    pub has_more: Option<bool>,
}

// ============ Users ============

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub email_verified: bool,
    #[serde(default)]
    pub default_org_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// `GET /api/v1/api-keys/self` — what the presented API key resolves to.
/// This is the only identity endpoint an API key can call: keys authenticate
/// as an org-scoped service account, never as a user (the platform removed
/// the "key acts as the org owner" path in 2026-08). `org_name` / `org_slug`
/// are served by newer control planes only.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ApiKeySelf {
    pub org_id: Uuid,
    pub service_account_id: Uuid,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub org_name: Option<String>,
    #[serde(default)]
    pub org_slug: Option<String>,
}

// ============ Organizations ============

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Organization {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub description: Option<String>,
    pub owner_id: Uuid,
    #[serde(default)]
    pub gcp_project_id: Option<String>,
    #[serde(default)]
    pub gcp_connected: bool,
    #[serde(default)]
    pub cloud_provider: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct CreateOrgRequest {
    pub name: String,
    pub slug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

// ============ Org Members & Invites ============

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OrgMember {
    pub id: Uuid,
    pub user_id: Uuid,
    pub user_name: String,
    pub user_email: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct CreateOrgInviteRequest {
    pub email: String,
    pub role: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OrgInvite {
    pub id: Uuid,
    pub email: String,
    pub role: String,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub redeemed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

// ============ API Keys ============

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ApiKey {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub key_prefix: String,
    #[serde(default)]
    pub scopes: Option<String>,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct CreateApiKeyRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub scopes: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in_days: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CreatedApiKey {
    pub id: Uuid,
    pub name: String,
    pub key_prefix: String,
    /// Plaintext key — only returned at creation time.
    pub key: String,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

// ============ Apps ============

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct App {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub organization_id: Uuid,
    pub status: String,
    #[serde(default)]
    pub source_type: Option<String>,
    #[serde(default)]
    pub github_repo_owner: Option<String>,
    #[serde(default)]
    pub github_repo_name: Option<String>,
    #[serde(default)]
    pub github_branch: Option<String>,
    #[serde(default)]
    pub container_image_url: Option<String>,
    #[serde(default)]
    pub cloud_run_url: Option<String>,
    #[serde(default)]
    pub primary_url: Option<String>,
    #[serde(default)]
    pub dns_hostname: Option<String>,
    #[serde(default)]
    pub custom_domain: Option<String>,
    #[serde(default)]
    pub resource_tier: Option<String>,
    #[serde(default)]
    pub spec: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// `source` discriminated union — only the variants the CLI can construct.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum AppSource {
    #[serde(rename = "image")]
    Image { image_url: String },
    #[serde(rename = "git")]
    Git {
        repo_owner: String,
        repo_name: String,
        branch: String,
    },
    /// No container, no build step — files live in a per-app GCS bucket.
    /// `framework` is a hint only (defaults server-side to "plain").
    #[serde(rename = "static")]
    Static { framework: String },
}

#[derive(Debug, Serialize, Default)]
pub struct AppSpecCreate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub env_vars: HashMap<String, String>,
}

#[derive(Debug, Serialize)]
pub struct CreateAppRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub source: AppSource,
    pub spec: AppSpecCreate,
}

#[derive(Debug, Serialize)]
pub struct UpdateAppRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_branch: Option<String>,
}

// ============ Static sites ============

#[derive(Debug, Serialize)]
pub struct StaticManifestFile {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Serialize)]
pub struct CreateStaticDeploymentRequest {
    pub source_type: &'static str, // always "api"
    pub files: Vec<StaticManifestFile>,
}

#[derive(Debug, Deserialize)]
pub struct StaticDeploymentSession {
    pub deployment_id: Uuid,
    pub upload_urls: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct StaticDeployment {
    pub id: Uuid,
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
}

// ============ Deployments ============

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Deployment {
    pub id: Uuid,
    pub app_id: Uuid,
    pub status: DeploymentStatus,
    #[serde(default)]
    pub failure_reason: Option<String>,
    #[serde(default)]
    pub git_commit_sha: Option<String>,
    #[serde(default)]
    pub git_commit_message: Option<String>,
    #[serde(default)]
    pub image_uri: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub trigger_type: Option<String>,
    #[serde(default)]
    pub events: Vec<DeploymentEvent>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentStatus {
    Created,
    InProgress,
    Success,
    Failed,
    Cancelled,
}

impl std::fmt::Display for DeploymentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeploymentStatus::Created => write!(f, "created"),
            DeploymentStatus::InProgress => write!(f, "in_progress"),
            DeploymentStatus::Success => write!(f, "success"),
            DeploymentStatus::Failed => write!(f, "failed"),
            DeploymentStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DeploymentEvent {
    #[serde(default)]
    pub id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub message: String,
    #[serde(default)]
    pub details: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Serialize, Default)]
pub struct CreateDeploymentRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_commit_sha: Option<String>,
}

// ============ Secrets ============

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Secret {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub secret_type: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct SecretValue {
    pub value: String,
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
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

// ============ Audit events ============

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AuditLog {
    pub id: String,
    #[serde(default)]
    pub user_id: Option<Uuid>,
    #[serde(default)]
    pub organization_id: Option<Uuid>,
    pub action: String,
    #[serde(default)]
    pub resource_type: Option<String>,
    #[serde(default)]
    pub resource_id: Option<String>,
    #[serde(default)]
    pub details: Option<serde_json::Value>,
    #[serde(default)]
    pub ip_address: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct AuditLogList {
    pub items: Vec<AuditLog>,
    #[serde(default)]
    #[allow(dead_code)]
    pub total: Option<i64>,
}

// ============ Logs ============

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AppLogs {
    #[serde(default)]
    pub revisions: Vec<RevisionLogs>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RevisionLogs {
    pub revision_name: String,
    #[serde(default)]
    pub logs: Vec<LogEntry>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub severity: Option<String>,
    pub message: String,
}

// ============ Databases (DBaaS) ============

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Database {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub db_type: Option<String>,
    pub status: String,
    pub version: String,
    pub tier: String,
    pub storage_gb: i32,
    #[serde(default)]
    pub ha_enabled: bool,
    #[serde(default)]
    pub private_ip: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct CreateDatabaseRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub version: String,
    pub tier: String,
    pub storage_gb: i32,
    pub ha_enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct UpdateDatabaseRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_gb: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ha_enabled: Option<bool>,
}

// ── App resource bindings ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingResourceType {
    Secret,
    Database,
    Bucket,
    Cache,
    // Created by event flows, never by `quome apps bind` — present so list
    // rows deserialize.
    EventSubscription,
}

impl BindingResourceType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Secret => "secret",
            Self::Database => "database",
            Self::Bucket => "bucket",
            Self::Cache => "cache",
            Self::EventSubscription => "event_subscription",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppBinding {
    pub id: Uuid,
    pub app_id: Uuid,
    pub resource_type: BindingResourceType,
    pub resource_id: Uuid,
    pub env_var_name: String,
    #[serde(default)]
    pub container_name: Option<String>,
    #[serde(default)]
    pub environment_id: Option<String>,
    #[serde(default)]
    pub allow_in_preview: bool,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct CreateBindingRequest {
    pub resource_type: BindingResourceType,
    pub resource_id: Uuid,
    pub env_var_name: String,
    pub container_name: Option<String>,
    pub environment_id: Option<String>,
    pub allow_in_preview: bool,
}

// ============ Storage buckets ============

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct StorageBucket {
    pub id: Uuid,
    pub name: String,
    pub region: String,
    pub status: String,
    pub storage_class: String,
    #[serde(default)]
    pub size_bytes: i64,
    #[serde(default)]
    pub object_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ============ Caches ============

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Cache {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub engine: String,
    pub engine_version: String,
    pub status: String,
    pub tier: String,
    pub memory_size_gb: i32,
    #[serde(default)]
    pub private_ip: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ── App environments ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppEnvironment {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub is_default: bool,
    #[serde(default)]
    pub deploy_branch: Option<String>,
    pub auto_deploy: bool,
    pub status: String,
    #[serde(default)]
    pub sort_order: i64,
    #[serde(default = "default_promotion_gate")]
    pub promotion_gate: String,
    #[serde(default)]
    pub config_overrides: serde_json::Value,
}

fn default_promotion_gate() -> String {
    "none".to_string()
}

#[derive(Debug, Serialize)]
pub struct CreateEnvironmentRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deploy_branch: Option<String>,
    pub auto_deploy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy_vars_from_environment_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct PromoteEnvironmentRequest {
    pub from_environment_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate_ack: Option<String>,
}

#[cfg(test)]
mod static_site_tests {
    use super::*;

    #[test]
    fn static_session_deserializes() {
        let body = r#"{"deployment_id":"9b2f0a34-1111-2222-3333-444455556666",
                       "upload_urls":{"index.html":"https://signed"},
                       "expires_at":"2026-09-04T00:00:00Z"}"#;
        let s: StaticDeploymentSession = serde_json::from_str(body).unwrap();
        assert_eq!(s.upload_urls["index.html"], "https://signed");
    }

    #[test]
    fn static_deployment_list_page_deserializes() {
        // The list endpoint's real envelope (confirmed against
        // `app/api/v1/apps/static_sites.py::list_static_deployments`,
        // `response_model=PaginatedResponse[StaticDeploymentResponse]`):
        // `{"data": [...], "meta": {...}}`, not a bare array. Extra
        // response-only fields (site_id, source_type, ...) are ignored.
        let body = r#"{
            "data": [
                {"id": "9b2f0a34-1111-2222-3333-444455556666", "status": "active", "error": null,
                 "site_id": "aaaa0a34-1111-2222-3333-444455556666", "source_type": "api"}
            ],
            "meta": {"total": 1, "limit": 50, "offset": 0, "has_more": false}
        }"#;
        let page: PaginatedResponse<StaticDeployment> = serde_json::from_str(body).unwrap();
        assert_eq!(page.data.len(), 1);
        assert_eq!(page.data[0].status, "active");
    }
}

#[cfg(test)]
mod binding_tests {
    use super::*;

    #[test]
    fn app_binding_deserializes_api_row() {
        let row = r#"{
            "id": "9b2f0a34-1111-2222-3333-444455556666",
            "app_id": "aaaa0a34-1111-2222-3333-444455556666",
            "resource_type": "secret",
            "resource_id": "bbbb0a34-1111-2222-3333-444455556666",
            "env_var_name": "DATABASE_PASSWORD",
            "container_name": null,
            "environment_id": null,
            "allow_in_preview": false,
            "created_at": "2026-09-01T00:00:00Z"
        }"#;
        let b: AppBinding = serde_json::from_str(row).unwrap();
        assert_eq!(b.resource_type, BindingResourceType::Secret);
        assert_eq!(b.env_var_name, "DATABASE_PASSWORD");
        assert!(b.environment_id.is_none());
    }

    #[test]
    fn create_binding_request_serializes_snake_case() {
        let req = CreateBindingRequest {
            resource_type: BindingResourceType::Bucket,
            resource_id: uuid::Uuid::nil(),
            env_var_name: "ASSETS".into(),
            container_name: None,
            environment_id: None,
            allow_in_preview: true,
        };
        let v: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert_eq!(v["resource_type"], "bucket");
        assert_eq!(v["allow_in_preview"], true);
        // None fields serialize as null — the API treats null and absent alike.
        assert!(v.get("container_name").is_some());
    }
}

#[cfg(test)]
mod environment_tests {
    use super::*;

    #[test]
    fn app_environment_deserializes_api_row() {
        let row = r#"{
            "id": "9b2f0a34-1111-2222-3333-444455556666",
            "app_id": "aaaa0a34-1111-2222-3333-444455556666",
            "organization_id": "bbbb0a34-1111-2222-3333-444455556666",
            "name": "staging", "slug": "staging", "is_default": false,
            "kind": "persistent", "deploy_branch": "staging",
            "auto_deploy": true, "status": "active", "sort_order": 2,
            "promotion_gate": "confirm", "color": "blue",
            "config_overrides": {"env_vars": {"A": "1"}, "memory": "1Gi"},
            "primary_url": null, "current_deployment_id": null,
            "last_deployed_at": null, "created_at": "2026-09-01T00:00:00Z"
        }"#;
        let e: AppEnvironment = serde_json::from_str(row).unwrap();
        assert_eq!(e.name, "staging");
        assert_eq!(e.promotion_gate, "confirm");
        assert_eq!(e.config_overrides["env_vars"]["A"], "1");
    }

    #[test]
    fn create_environment_request_serializes() {
        let req = CreateEnvironmentRequest {
            name: "staging".into(),
            deploy_branch: Some("staging".into()),
            auto_deploy: true,
            copy_vars_from_environment_id: None,
        };
        let v: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert_eq!(v["name"], "staging");
        assert_eq!(v["auto_deploy"], true);
    }
}
