use uuid::Uuid;

use crate::api::models::AppEnvironment;
use crate::client::QuomeClient;
use crate::errors::{QuomeError, Result};

/// Resolve an environment reference: exact name match first, then UUID.
/// Env names are slug-validated and unique per app, so no ambiguity.
#[allow(dead_code)]
pub async fn resolve_environment(
    client: &QuomeClient,
    org_id: Uuid,
    app_id: Uuid,
    env_ref: &str,
) -> Result<AppEnvironment> {
    let envs = client.list_environments(org_id, app_id).await?;
    if let Some(env) = envs.iter().find(|e| e.name == env_ref) {
        return Ok(env.clone());
    }
    if let Ok(id) = Uuid::parse_str(env_ref) {
        if let Some(env) = envs.iter().find(|e| e.id == id) {
            return Ok(env.clone());
        }
    }
    Err(QuomeError::NotFound(format!(
        "no environment '{}' — see `quome apps envs`",
        env_ref
    )))
}
