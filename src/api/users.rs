use crate::api::models::*;
use crate::client::QuomeClient;
use crate::errors::Result;

impl QuomeClient {
    pub async fn get_current_user(&self) -> Result<User> {
        self.get("/api/v1/users").await
    }
}
