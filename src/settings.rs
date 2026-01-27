use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::errors::Result;

const SETTINGS_FILE: &str = "settings.json";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Settings {
    /// API base URL (e.g., "https://demo.quome.cloud")
    #[serde(default = "default_api_url")]
    pub api_url: String,

    /// Documentation URL (e.g., "https://docs.quome.com")
    #[serde(default = "default_docs_url")]
    pub docs_url: String,

    /// Main website URL (e.g., "https://quome.com")
    #[serde(default = "default_website_url")]
    pub website_url: String,
}

fn default_api_url() -> String {
    "https://demo.quome.cloud".to_string()
}

fn default_docs_url() -> String {
    "https://docs.quome.com".to_string()
}

fn default_website_url() -> String {
    "https://quome.com".to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            api_url: default_api_url(),
            docs_url: default_docs_url(),
            website_url: default_website_url(),
        }
    }
}

impl Settings {
    /// Get the path to the settings file in the config directory
    fn global_settings_path() -> Result<PathBuf> {
        let home = dirs::home_dir().ok_or_else(|| {
            crate::errors::QuomeError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Could not find home directory",
            ))
        })?;
        Ok(home.join(".quome").join(SETTINGS_FILE))
    }

    /// Get the path to the local settings file in the current directory
    fn local_settings_path() -> PathBuf {
        PathBuf::from(SETTINGS_FILE)
    }

    /// Load settings with precedence: local file > global file > defaults
    pub fn load() -> Result<Self> {
        // Try local settings first
        let local_path = Self::local_settings_path();
        if local_path.exists() {
            let content = fs::read_to_string(&local_path)?;
            let settings: Settings = serde_json::from_str(&content)?;
            return Ok(settings);
        }

        // Try global settings
        if let Ok(global_path) = Self::global_settings_path() {
            if global_path.exists() {
                let content = fs::read_to_string(&global_path)?;
                let settings: Settings = serde_json::from_str(&content)?;
                return Ok(settings);
            }
        }

        // Return defaults
        Ok(Self::default())
    }

    /// Get the API URL, with environment variable override
    pub fn get_api_url(&self) -> String {
        std::env::var("QUOME_API_URL").unwrap_or_else(|_| self.api_url.clone())
    }
}
