use uuid::Uuid;

use crate::api::models::*;
use crate::client::QuomeClient;
use crate::errors::Result;

impl QuomeClient {
    pub async fn list_orgs(&self) -> Result<ListOrgsResponse> {
        self.get("/api/v1/orgs").await
    }

    pub async fn create_org(&self, req: &CreateOrgRequest) -> Result<Organization> {
        self.post("/api/v1/orgs", req).await
    }

    pub async fn get_org(&self, id: Uuid) -> Result<Organization> {
        self.get(&format!("/api/v1/orgs/{}", id)).await
    }

    pub async fn list_org_members(&self, org_id: Uuid) -> Result<Vec<OrgMember>> {
        self.get(&format!("/api/v1/orgs/{}/members", org_id)).await
    }

    pub async fn add_org_member(
        &self,
        org_id: Uuid,
        req: &AddOrgMemberRequest,
    ) -> Result<OrgMember> {
        self.post(&format!("/api/v1/orgs/{}/members", org_id), req)
            .await
    }

    pub async fn list_org_keys(&self, org_id: Uuid) -> Result<ListOrgKeysResponse> {
        self.get(&format!("/api/v1/orgs/{}/keys", org_id)).await
    }

    pub async fn create_org_key(
        &self,
        org_id: Uuid,
        req: &CreateOrgKeyRequest,
    ) -> Result<CreatedOrgKey> {
        self.post(&format!("/api/v1/orgs/{}/keys", org_id), req)
            .await
    }

    pub async fn delete_org_key(&self, org_id: Uuid, key_id: Uuid) -> Result<()> {
        self.delete(&format!("/api/v1/orgs/{}/apikeys/{}", org_id, key_id))
            .await
    }
}
