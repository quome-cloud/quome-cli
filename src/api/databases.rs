use uuid::Uuid;

use crate::api::models::*;
use crate::client::QuomeClient;
use crate::errors::Result;

impl QuomeClient {
    pub async fn list_databases(&self, org_id: Uuid) -> Result<ListDatabasesResponse> {
        self.get(&format!("/api/v1/orgs/{}/dbaas", org_id)).await
    }

    pub async fn create_database(
        &self,
        org_id: Uuid,
        req: &CreateDatabaseRequest,
    ) -> Result<Database> {
        self.post(&format!("/api/v1/orgs/{}/dbaas", org_id), req)
            .await
    }

    pub async fn get_database(&self, org_id: Uuid, db_id: Uuid) -> Result<Database> {
        self.get(&format!("/api/v1/orgs/{}/dbaas/{}", org_id, db_id))
            .await
    }

    pub async fn update_database(
        &self,
        org_id: Uuid,
        db_id: Uuid,
        req: &UpdateDatabaseRequest,
    ) -> Result<Database> {
        self.put(&format!("/api/v1/orgs/{}/dbaas/{}", org_id, db_id), req)
            .await
    }

    pub async fn delete_database(&self, org_id: Uuid, db_id: Uuid) -> Result<()> {
        self.delete(&format!("/api/v1/orgs/{}/dbaas/{}", org_id, db_id))
            .await
    }
}
