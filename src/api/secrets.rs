use uuid::Uuid;

use crate::api::models::*;
use crate::client::QuomeClient;
use crate::errors::Result;

impl QuomeClient {
    pub async fn list_secrets(&self, org_id: Uuid) -> Result<ListSecretsResponse> {
        self.get(&format!("/api/v1/orgs/{}/secrets", org_id)).await
    }

    pub async fn create_secret(&self, org_id: Uuid, req: &CreateSecretRequest) -> Result<Secret> {
        self.post(&format!("/api/v1/orgs/{}/secrets", org_id), req)
            .await
    }

    pub async fn get_secret(&self, org_id: Uuid, secret_id: Uuid) -> Result<Secret> {
        self.get(&format!(
            "/api/v1/orgs/{}/secrets/{}?reveal=true",
            org_id, secret_id
        ))
        .await
    }

    pub async fn update_secret(
        &self,
        org_id: Uuid,
        secret_id: Uuid,
        req: &UpdateSecretRequest,
    ) -> Result<Secret> {
        self.put(
            &format!("/api/v1/orgs/{}/secrets/{}", org_id, secret_id),
            req,
        )
        .await
    }

    pub async fn delete_secret(&self, org_id: Uuid, secret_id: Uuid) -> Result<()> {
        self.delete(&format!("/api/v1/orgs/{}/secrets/{}", org_id, secret_id))
            .await
    }
}
