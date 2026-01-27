use crate::api::models::*;
use crate::client::QuomeClient;
use crate::errors::Result;

impl QuomeClient {
    pub async fn create_session(&self, req: &CreateSessionRequest) -> Result<CreatedSession> {
        self.post("/api/v1/auth/sessions", req).await
    }

    pub async fn list_sessions(&self) -> Result<ListSessionsResponse> {
        self.get("/api/v1/auth/sessions").await
    }

    pub async fn renew_session(&self) -> Result<RenewedSession> {
        self.post("/api/v1/auth/sessions/renew", &()).await
    }

    pub async fn delete_session(&self, session_id: &str) -> Result<()> {
        self.delete(&format!("/api/v1/auth/sessions/{}", session_id))
            .await
    }
}
