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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UserConfig {
    pub token: String,
    pub id: Uuid,
    pub email: String,
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

    pub fn set_user(&mut self, token: String, id: Uuid, email: String) {
        self.user = Some(UserConfig { token, id, email });
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

        Ok(self.get_linked()?.map(|l| l.org_id))
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
