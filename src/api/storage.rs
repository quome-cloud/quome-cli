use uuid::Uuid;

use crate::api::models::*;
use crate::client::QuomeClient;
use crate::errors::Result;

impl QuomeClient {
    pub async fn list_buckets(&self, org_id: Uuid) -> Result<PaginatedResponse<StorageBucket>> {
        self.get(&format!("/api/v1/orgs/{}/storage?limit=100", org_id))
            .await
    }
}
