use uuid::Uuid;

use crate::api::models::*;
use crate::client::QuomeClient;
use crate::errors::Result;

impl QuomeClient {
    pub async fn list_audit_logs(&self, org_id: Uuid, limit: Option<u32>) -> Result<AuditLogList> {
        let page_size = limit.unwrap_or(50).min(100);
        self.get(&format!(
            "/api/v1/audit/logs?org_id={}&page_size={}",
            org_id, page_size
        ))
        .await
    }
}
