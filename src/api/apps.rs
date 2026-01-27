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
