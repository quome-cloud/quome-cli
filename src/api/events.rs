use uuid::Uuid;

use crate::api::models::*;
use crate::client::QuomeClient;
use crate::errors::Result;

impl QuomeClient {
    pub async fn list_events(
        &self,
        org_id: Uuid,
        limit: Option<u32>,
    ) -> Result<ListEventsResponse> {
        let mut path = format!("/api/v1/orgs/{}/events", org_id);
        if let Some(l) = limit {
            path = format!("{}?limit={}", path, l);
        }
        self.get(&path).await
    }
}
