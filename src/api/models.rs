use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

// ============ Users ============

#[allow(dead_code)]
#[derive(Debug, Serialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub default_org: Option<Uuid>,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub last_login_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub two_factor: Option<bool>,
}

// ============ Auth/Sessions ============

#[allow(dead_code)]
#[derive(Debug, Serialize)]
pub struct CreateSessionRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<Uuid>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct CreatedSession {
    pub session: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct RenewedSession {
    pub session: String,
    pub revoked_id: Uuid,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct Session {
    pub id: Uuid,
    pub user_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub source_ip: String,
    #[serde(default)]
    pub revoked_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub org_scope: Option<Uuid>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ListSessionsResponse {
    pub sessions: Vec<Session>,
}

// ============ Organizations ============

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
    #[serde(default)]
    pub id: Option<Uuid>,
    pub user_id: Uuid,
    pub org_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[allow(dead_code)]
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

#[derive(Debug, Deserialize, Serialize)]
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
    Success,
    Failed,
}

impl std::fmt::Display for DeploymentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeploymentStatus::Created => write!(f, "created"),
            DeploymentStatus::InProgress => write!(f, "in_progress"),
            DeploymentStatus::Deployed => write!(f, "deployed"),
            DeploymentStatus::Success => write!(f, "success"),
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
    /// Value is only returned when fetching a single secret, not when listing
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub organization_id: Option<Uuid>,
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
    #[allow(dead_code)]
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

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ListLogsResponse {
    pub logs: Vec<LogEntry>,
    #[serde(default)]
    pub next_before: Option<DateTime<Utc>>,
}

// ============ Databases ============

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Database {
    pub id: Uuid,
    pub name: String,
    pub organization_id: Uuid,
    pub compute: DatabaseCompute,
    pub storage: DatabaseStorage,
    pub replicas: DatabaseReplicas,
    pub postgres: DatabasePostgres,
    #[serde(default)]
    pub status: Option<DatabaseStatus>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DatabaseCompute {
    pub requested: ComputeRequested,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ComputeRequested {
    pub vcpu: String,
    pub memory: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DatabaseStorage {
    pub requested: StorageRequested,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct StorageRequested {
    pub disk_space: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DatabaseReplicas {
    pub requested: i32,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DatabasePostgres {
    pub major_version: i32,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DatabaseStatus {
    pub state: DatabaseState,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub enum DatabaseState {
    Initializing,
    Ready,
    Paused,
    Stopping,
    Error,
}

impl std::fmt::Display for DatabaseState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DatabaseState::Initializing => write!(f, "Initializing"),
            DatabaseState::Ready => write!(f, "Ready"),
            DatabaseState::Paused => write!(f, "Paused"),
            DatabaseState::Stopping => write!(f, "Stopping"),
            DatabaseState::Error => write!(f, "Error"),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ListDatabasesResponse {
    #[serde(default)]
    pub databases: Vec<Database>,
}

#[derive(Debug, Serialize)]
pub struct CreateDatabaseRequest {
    pub name: String,
    pub compute: DatabaseCompute,
    pub storage: DatabaseStorage,
    pub replicas: DatabaseReplicas,
    pub postgres: DatabasePostgres,
}

#[derive(Debug, Serialize)]
pub struct UpdateDatabaseRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compute: Option<DatabaseCompute>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage: Option<DatabaseStorage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replicas: Option<DatabaseReplicas>,
}

// ============ Shared Error Types ============

#[derive(Debug, Deserialize)]
pub struct ApiErrorResponse {
    pub message: String,
}
