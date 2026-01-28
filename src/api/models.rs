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

// ============ Quome Coder V2 Agent ============

#[derive(Debug, Serialize)]
pub struct StartAgentRequest {
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_github: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_mode: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accessibility_target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tech_stack: Option<TechStack>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_preferences: Option<ColorPreferences>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TechStack {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<StackConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frontend: Option<StackConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StackConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ColorPreferences {
    #[serde(rename = "type")]
    pub color_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary_color: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartAgentResponse {
    pub thread_id: Uuid,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct SendPromptRequest {
    pub prompt: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SendPromptResponse {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StopWorkflowResponse {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PullLatestResponse {
    pub success: bool,
    pub message: String,
    #[serde(default)]
    pub state: Option<AgentState>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AgentState {
    pub thread_id: Uuid,
    #[serde(default)]
    pub is_working: bool,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub phase: Option<String>,
    #[serde(default)]
    pub app_uuid: Option<Uuid>,
    #[serde(default)]
    pub app_domain_name: Option<String>,
    #[serde(default)]
    pub app_context: Option<AppContext>,
    #[serde(default)]
    pub messages: Vec<AgentMessage>,
    #[serde(default)]
    pub files: HashMap<String, String>,
    #[serde(default)]
    pub container_info: Option<ContainerInfo>,
    #[serde(default)]
    pub deployment: Option<AgentDeploymentInfo>,
    #[serde(default)]
    pub progress: Option<ProgressInfo>,
    #[serde(default)]
    pub plan: Option<AgentPlan>,
    #[serde(default)]
    pub brand_kit: Option<BrandKit>,
    #[serde(default)]
    pub github_repo_url: Option<String>,
    #[serde(default)]
    pub github_repo_name: Option<String>,
    #[serde(default)]
    pub github_repo_created: Option<bool>,
    #[serde(default)]
    pub tests_passed: Option<i32>,
    #[serde(default)]
    pub tests_failed: Option<i32>,
    #[serde(default)]
    pub tests_ran: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AppContext {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub goal: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AgentMessage {
    #[serde(rename = "type")]
    pub message_type: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub timestamp: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ContainerInfo {
    #[serde(default)]
    pub container_id: Option<String>,
    #[serde(default)]
    pub sandbox_id: Option<String>,
    #[serde(default)]
    pub app_relative_dir: Option<String>,
    #[serde(default)]
    pub frontend_port: Option<i32>,
    #[serde(default)]
    pub backend_port: Option<i32>,
    #[serde(default)]
    pub testing_port: Option<i32>,
    #[serde(default)]
    pub frontend_url: Option<String>,
    #[serde(default)]
    pub backend_url: Option<String>,
    #[serde(default)]
    pub testing_url: Option<String>,
    #[serde(default)]
    pub is_healthy: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AgentDeploymentInfo {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub files_path: Option<String>,
    #[serde(default)]
    pub port: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ProgressInfo {
    #[serde(default)]
    pub percentage: Option<f64>,
    #[serde(default)]
    pub current_stage: Option<i32>,
    #[serde(default)]
    pub total_stages: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AgentPlan {
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub stages: Vec<AgentPlanStage>,
    #[serde(default)]
    pub current_stage: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AgentPlanStage {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub lanes: Vec<AgentPlanWorkLane>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AgentPlanWorkLane {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub parts: Vec<String>,
    #[serde(default)]
    pub target_files: Vec<String>,
    #[serde(default)]
    pub is_complete: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct BrandKit {
    #[serde(default)]
    pub primary_color: Option<String>,
    #[serde(default)]
    pub secondary_color: Option<String>,
    #[serde(default)]
    pub accent_color: Option<String>,
    #[serde(default)]
    pub background_color: Option<String>,
    #[serde(default)]
    pub text_color: Option<String>,
    #[serde(default)]
    pub font_family: Option<String>,
    #[serde(default)]
    pub company_name: Option<String>,
    #[serde(default)]
    pub logo_public_urls: Vec<String>,
    #[serde(default)]
    pub hero_public_urls: Vec<String>,
    #[serde(default)]
    pub primary_logo_index: Option<i32>,
    #[serde(default)]
    pub primary_logo_url: Option<String>,
}

// ============ Shared Error Types ============

#[derive(Debug, Deserialize)]
pub struct ApiErrorResponse {
    pub message: String,
}
