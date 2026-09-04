use uuid::Uuid;

use crate::api::models::*;
use crate::client::QuomeClient;
use crate::errors::Result;

impl QuomeClient {
    pub async fn list_apps(&self, org_id: Uuid) -> Result<PaginatedResponse<App>> {
        self.get(&format!("/api/v1/orgs/{}/apps?limit=100", org_id))
            .await
    }

    pub async fn create_app(&self, org_id: Uuid, req: &CreateAppRequest) -> Result<App> {
        self.post(&format!("/api/v1/orgs/{}/apps", org_id), req)
            .await
    }

    /// Create a plain static app (`quome deploy --create`'s app-not-found path).
    /// Reuses `create_app`/`CreateAppRequest` — `AppSource` already models the
    /// backend's `static` source variant (`app/schemas/source.py::StaticSource`),
    /// so there's no need for a separate ad hoc request shape.
    pub async fn create_static_app(&self, org_id: Uuid, name: &str) -> Result<App> {
        self.create_app(
            org_id,
            &CreateAppRequest {
                name: name.to_string(),
                description: None,
                source: AppSource::Static {
                    framework: "plain".to_string(),
                },
                spec: AppSpecCreate::default(),
            },
        )
        .await
    }

    pub async fn get_app(&self, org_id: Uuid, app_id: Uuid) -> Result<App> {
        self.get(&format!("/api/v1/orgs/{}/apps/{}", org_id, app_id))
            .await
    }

    pub async fn update_app(
        &self,
        org_id: Uuid,
        app_id: Uuid,
        req: &UpdateAppRequest,
    ) -> Result<App> {
        self.put(&format!("/api/v1/orgs/{}/apps/{}", org_id, app_id), req)
            .await
    }

    pub async fn delete_app(&self, org_id: Uuid, app_id: Uuid) -> Result<()> {
        self.delete(&format!("/api/v1/orgs/{}/apps/{}", org_id, app_id))
            .await
    }

    pub async fn list_deployments(
        &self,
        org_id: Uuid,
        app_id: Uuid,
    ) -> Result<PaginatedResponse<Deployment>> {
        self.get(&format!(
            "/api/v1/orgs/{}/apps/{}/deployments?limit=50",
            org_id, app_id
        ))
        .await
    }

    pub async fn get_deployment(
        &self,
        org_id: Uuid,
        app_id: Uuid,
        deployment_id: Uuid,
    ) -> Result<Deployment> {
        self.get(&format!(
            "/api/v1/orgs/{}/apps/{}/deployments/{}",
            org_id, app_id, deployment_id
        ))
        .await
    }

    pub async fn create_deployment(
        &self,
        org_id: Uuid,
        app_id: Uuid,
        req: &CreateDeploymentRequest,
    ) -> Result<Deployment> {
        self.post(
            &format!("/api/v1/orgs/{}/apps/{}/deployments", org_id, app_id),
            req,
        )
        .await
    }

    pub async fn get_logs(
        &self,
        org_id: Uuid,
        app_id: Uuid,
        limit: Option<u32>,
    ) -> Result<AppLogs> {
        let mut path = format!("/api/v1/orgs/{}/apps/{}/logs", org_id, app_id);
        if let Some(l) = limit {
            path = format!("{}?limit={}", path, l);
        }
        self.get(&path).await
    }

    pub async fn list_bindings(&self, org_id: Uuid, app_id: Uuid) -> Result<Vec<AppBinding>> {
        self.get(&format!("/api/v1/orgs/{}/apps/{}/bindings", org_id, app_id))
            .await
    }

    pub async fn create_binding(
        &self,
        org_id: Uuid,
        app_id: Uuid,
        req: &CreateBindingRequest,
    ) -> Result<AppBinding> {
        self.post(
            &format!("/api/v1/orgs/{}/apps/{}/bindings", org_id, app_id),
            req,
        )
        .await
    }

    pub async fn delete_binding(&self, org_id: Uuid, app_id: Uuid, binding_id: Uuid) -> Result<()> {
        self.delete(&format!(
            "/api/v1/orgs/{}/apps/{}/bindings/{}",
            org_id, app_id, binding_id
        ))
        .await
    }
}
