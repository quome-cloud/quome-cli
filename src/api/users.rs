use crate::api::models::*;
use crate::client::QuomeClient;
use crate::errors::Result;

impl QuomeClient {
    /// The signed-in *user* — only answers for a browser/session identity.
    /// An API key gets 401 here; use `get_api_key_self` for keys.
    #[allow(dead_code)]
    pub async fn get_current_user(&self) -> Result<User> {
        self.get("/api/v1/users").await
    }

    /// Resolve the org / service account / scopes behind the presented key.
    pub async fn get_api_key_self(&self) -> Result<ApiKeySelf> {
        self.get("/api/v1/api-keys/self").await
    }

    /// `quome login --browser`: redeem the browser-approved one-time code.
    /// Unauthenticated — the code + PKCE verifier are the credential.
    pub async fn exchange_cli_key(&self, req: &CliKeyExchangeRequest) -> Result<CliApiKeyResponse> {
        self.post("/api/v1/auth/cli/key", req).await
    }
}
