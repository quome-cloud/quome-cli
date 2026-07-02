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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
