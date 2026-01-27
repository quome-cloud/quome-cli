use uuid::Uuid;

use crate::api::models::*;
use crate::client::QuomeClient;
use crate::errors::Result;

impl QuomeClient {
    pub async fn get_current_user(&self) -> Result<User> {
        self.get("/api/v1/users").await
    }

    pub async fn create_user(&self, req: &CreateUserRequest) -> Result<User> {
        self.post("/api/v1/users", req).await
    }

    pub async fn get_user(&self, id: Uuid) -> Result<User> {
        self.get(&format!("/api/v1/users/{}", id)).await
    }
}
