use uuid::Uuid;

use crate::api::models::{
    CreateStaticDeploymentRequest, PaginatedResponse, StaticDeployment, StaticDeploymentSession,
};
use crate::client::QuomeClient;
use crate::errors::Result;

impl QuomeClient {
    /// Initialize (or fetch, idempotently) the per-app static site — the
    /// GCS bucket + `static_sites` row that deployments upload into.
    pub async fn create_or_get_static_site(
        &self,
        org_id: Uuid,
        app_id: Uuid,
    ) -> Result<serde_json::Value> {
        self.post(
            &format!("/api/v1/orgs/{}/apps/{}/static/sites", org_id, app_id),
            &serde_json::json!({}),
        )
        .await
    }

    pub async fn create_static_deployment(
        &self,
        org_id: Uuid,
        app_id: Uuid,
        req: &CreateStaticDeploymentRequest,
    ) -> Result<StaticDeploymentSession> {
        self.post(
            &format!("/api/v1/orgs/{}/apps/{}/static/deployments", org_id, app_id),
            req,
        )
        .await
    }

    pub async fn finalize_static_deployment(
        &self,
        org_id: Uuid,
        app_id: Uuid,
        deployment_id: Uuid,
    ) -> Result<serde_json::Value> {
        self.post(
            &format!(
                "/api/v1/orgs/{}/apps/{}/static/deployments/{}/finalize",
                org_id, app_id, deployment_id
            ),
            &serde_json::json!({}),
        )
        .await
    }

    /// History of deploys for this app's static site, most-recent first.
    ///
    /// The backend route (`app/api/v1/apps/static_sites.py::list_static_deployments`)
    /// declares `response_model=PaginatedResponse[StaticDeploymentResponse]`,
    /// i.e. a `{"data": [...], "meta": {...}}` envelope — the same shape the
    /// CLI's own `PaginatedResponse<T>` (used by `list_apps`/`list_deployments`)
    /// already models. Unwrap it here so callers keep working with a plain
    /// `Vec`, matching how the (now-retired) Python CLI iterated rows.
    pub async fn list_static_deployments(
        &self,
        org_id: Uuid,
        app_id: Uuid,
    ) -> Result<Vec<StaticDeployment>> {
        let page: PaginatedResponse<StaticDeployment> = self
            .get(&format!(
                "/api/v1/orgs/{}/apps/{}/static/deployments",
                org_id, app_id
            ))
            .await?;
        Ok(page.data)
    }
}
