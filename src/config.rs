use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

use crate::errors::{QuomeError, Result};

const CONFIG_DIR: &str = ".quome";
const CONFIG_FILE: &str = "config.json";

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub user: Option<UserConfig>,
    #[serde(default)]
    pub linked: HashMap<String, LinkedContext>,
}

/// The stored login. Since 0.2.6 a login is an org-scoped API key resolved
/// to its service account (`org_id` / `service_account_id` / `scopes`);
/// `id` / `email` are what pre-0.2.6 versions wrote and are kept optional so
/// an old config file still loads (and `quome login` upgrades it in place).
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UserConfig {
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_account_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
}

impl UserConfig {
    /// `qk_AbC123…` — the first 12 characters, the same prefix the dashboard
    /// lists so a stored login can be matched to a key row.
    pub fn key_prefix(&self) -> String {
        self.token.chars().take(12).collect()
    }

    /// Human label for the org the key belongs to.
    pub fn org_label(&self) -> Option<String> {
        match (&self.org_name, self.org_id) {
            (Some(name), Some(id)) => Some(format!("{} ({})", name, id)),
            (None, Some(id)) => Some(id.to_string()),
            _ => None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LinkedContext {
    pub org_id: Uuid,
    pub org_name: String,
    #[serde(default)]
    pub app_id: Option<Uuid>,
    #[serde(default)]
    pub app_name: Option<String>,
}

impl Config {
    fn config_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().ok_or_else(|| {
            QuomeError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Could not find home directory",
            ))
        })?;
        Ok(home.join(CONFIG_DIR))
    }

    fn config_path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join(CONFIG_FILE))
    }

    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;

        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(&path)?;
        let config: Config = serde_json::from_str(&content)?;
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let dir = Self::config_dir()?;
        fs::create_dir_all(&dir)?;

        let path = Self::config_path()?;
        let tmp_path = path.with_extension("tmp");

        let content = serde_json::to_string_pretty(self)?;
        fs::write(&tmp_path, content)?;
        fs::rename(&tmp_path, &path)?;

        Ok(())
    }

    #[allow(dead_code)]
    pub fn get_token(&self) -> Option<&str> {
        // Environment variable takes precedence
        if std::env::var("QUOME_TOKEN").is_ok() {
            // Return None here since we can't return a reference to a local
            // The caller should check QUOME_TOKEN separately
            return None;
        }
        self.user.as_ref().map(|u| u.token.as_str())
    }

    pub fn get_token_string(&self) -> Option<String> {
        // Environment variable takes precedence
        if let Ok(token) = std::env::var("QUOME_TOKEN") {
            return Some(token);
        }
        self.user.as_ref().map(|u| u.token.clone())
    }

    pub fn require_token(&self) -> Result<String> {
        self.get_token_string().ok_or(QuomeError::NotLoggedIn)
    }

    /// Store a validated API-key login.
    pub fn set_key_login(&mut self, token: String, identity: &crate::api::models::ApiKeySelf) {
        self.user = Some(UserConfig {
            token,
            id: None,
            email: None,
            org_id: Some(identity.org_id),
            org_name: identity.org_name.clone(),
            service_account_id: Some(identity.service_account_id),
            scopes: identity.scopes.clone(),
        });
    }

    /// The org the stored key belongs to, if the login was made with 0.2.6+
    /// (older logins carry no org; `quome login` again to upgrade). Ignored
    /// when `QUOME_TOKEN` overrides the stored login — that key's org is
    /// unknown until the caller resolves it.
    pub fn key_org_id(&self) -> Option<Uuid> {
        if std::env::var("QUOME_TOKEN").is_ok() {
            return None;
        }
        self.user.as_ref().and_then(|u| u.org_id)
    }

    pub fn clear_user(&mut self) {
        self.user = None;
    }

    pub fn current_dir_key() -> Result<String> {
        let cwd = std::env::current_dir()?;
        Ok(cwd.to_string_lossy().to_string())
    }

    pub fn get_linked(&self) -> Result<Option<&LinkedContext>> {
        // Environment variables take precedence
        if std::env::var("QUOME_ORG").is_ok() {
            return Ok(None); // Caller should check env vars
        }

        let key = Self::current_dir_key()?;
        Ok(self.linked.get(&key))
    }

    pub fn get_linked_org_id(&self) -> Result<Option<Uuid>> {
        // Environment variable takes precedence
        if let Ok(org) = std::env::var("QUOME_ORG") {
            return org
                .parse::<Uuid>()
                .map(Some)
                .map_err(|_| QuomeError::ApiError("Invalid QUOME_ORG UUID".into()));
        }

        // No link, no env: the key itself names the org (0.2.6+ logins), so
        // `quome apps list` works right after `quome login` without a link.
        Ok(self.get_linked()?.map(|l| l.org_id).or(self.key_org_id()))
    }

    pub fn require_linked_org(&self) -> Result<Uuid> {
        self.get_linked_org_id()?.ok_or(QuomeError::NoLinkedOrg)
    }

    pub fn get_linked_app_id(&self) -> Result<Option<Uuid>> {
        // Environment variable takes precedence
        if let Ok(app) = std::env::var("QUOME_APP") {
            return app
                .parse::<Uuid>()
                .map(Some)
                .map_err(|_| QuomeError::ApiError("Invalid QUOME_APP UUID".into()));
        }

        Ok(self.get_linked()?.and_then(|l| l.app_id))
    }

    pub fn require_linked_app(&self) -> Result<Uuid> {
        self.get_linked_app_id()?.ok_or(QuomeError::NoLinkedApp)
    }

    pub fn set_linked(&mut self, context: LinkedContext) -> Result<()> {
        let key = Self::current_dir_key()?;
        self.linked.insert(key, context);
        Ok(())
    }

    pub fn clear_linked(&mut self) -> Result<()> {
        let key = Self::current_dir_key()?;
        self.linked.remove(&key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_0_2_6_config_still_loads_and_reports_no_org() {
        let raw = r#"{"user":{"token":"qk_abc","id":"a1b2c3d4-0000-4000-8000-000000000000","email":"x@y.z"},"linked":{}}"#;
        let cfg: Config = serde_json::from_str(raw).unwrap();
        let user = cfg.user.as_ref().unwrap();
        assert_eq!(user.email.as_deref(), Some("x@y.z"));
        assert!(user.org_id.is_none());
        assert!(user.scopes.is_empty());
    }

    #[test]
    fn unlinked_directory_falls_back_to_the_keys_org() {
        let identity = crate::api::models::ApiKeySelf {
            org_id: Uuid::from_u128(7),
            service_account_id: Uuid::nil(),
            scopes: vec![],
            org_name: None,
            org_slug: None,
        };
        let mut cfg = Config::default();
        cfg.set_key_login("qk_x".into(), &identity);
        // Only meaningful when neither QUOME_ORG nor QUOME_TOKEN is set in
        // the test environment; both are unset in CI.
        if std::env::var("QUOME_ORG").is_err() && std::env::var("QUOME_TOKEN").is_err() {
            assert_eq!(cfg.get_linked_org_id().unwrap(), Some(Uuid::from_u128(7)));
        }
    }

    #[test]
    fn key_login_round_trips_org_and_scopes_and_drops_user_fields() {
        let identity = crate::api::models::ApiKeySelf {
            org_id: Uuid::nil(),
            service_account_id: Uuid::nil(),
            scopes: vec!["*".into()],
            org_name: Some("acme".into()),
            org_slug: None,
        };
        let mut cfg = Config::default();
        cfg.set_key_login("qk_0123456789abcdef".into(), &identity);
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(!json.contains("\"email\""));
        let back: Config = serde_json::from_str(&json).unwrap();
        let user = back.user.unwrap();
        assert_eq!(user.org_id, Some(Uuid::nil()));
        assert_eq!(user.scopes, vec!["*".to_string()]);
        assert_eq!(user.key_prefix(), "qk_012345678");
        assert_eq!(
            user.org_label().as_deref(),
            Some("acme (00000000-0000-0000-0000-000000000000)")
        );
    }
}
