use uuid::Uuid;

use crate::api::models::{AppEnvironment, CreateEnvironmentRequest, PromoteEnvironmentRequest};
use crate::client::QuomeClient;
use crate::errors::Result;

#[allow(dead_code)]
impl QuomeClient {
    pub async fn list_environments(
        &self,
        org_id: Uuid,
        app_id: Uuid,
    ) -> Result<Vec<AppEnvironment>> {
        self.list_all_pages(&format!(
            "/api/v1/orgs/{}/apps/{}/environments",
            org_id, app_id
        ))
        .await
    }

    pub async fn create_environment(
        &self,
        org_id: Uuid,
        app_id: Uuid,
        req: &CreateEnvironmentRequest,
    ) -> Result<AppEnvironment> {
        self.post(
            &format!("/api/v1/orgs/{}/apps/{}/environments", org_id, app_id),
            req,
        )
        .await
    }

    pub async fn delete_environment(&self, org_id: Uuid, app_id: Uuid, env_id: Uuid) -> Result<()> {
        self.delete(&format!(
            "/api/v1/orgs/{}/apps/{}/environments/{}",
            org_id, app_id, env_id
        ))
        .await
    }

    /// PATCH the environment. Callers send ONLY the top-level keys they
    /// touched — the server merge-patches config_overrides at the top level
    /// (null deletes a key), so untouched keys must never ride along.
    pub async fn update_environment_overrides(
        &self,
        org_id: Uuid,
        app_id: Uuid,
        env_id: Uuid,
        config_overrides: &serde_json::Value,
    ) -> Result<AppEnvironment> {
        self.patch(
            &format!(
                "/api/v1/orgs/{}/apps/{}/environments/{}",
                org_id, app_id, env_id
            ),
            &serde_json::json!({ "config_overrides": config_overrides }),
        )
        .await
    }

    pub async fn promote_environment(
        &self,
        org_id: Uuid,
        app_id: Uuid,
        target_env_id: Uuid,
        req: &PromoteEnvironmentRequest,
    ) -> Result<AppEnvironment> {
        self.post(
            &format!(
                "/api/v1/orgs/{}/apps/{}/environments/{}/promote",
                org_id, app_id, target_env_id
            ),
            req,
        )
        .await
    }
}
