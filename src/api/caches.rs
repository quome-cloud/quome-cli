use uuid::Uuid;

use crate::api::models::*;
use crate::client::QuomeClient;
use crate::errors::Result;

impl QuomeClient {
    pub async fn list_caches(&self, org_id: Uuid) -> Result<PaginatedResponse<Cache>> {
        self.get(&format!("/api/v1/orgs/{}/caches?limit=100", org_id))
            .await
    }
}
