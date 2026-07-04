use eframe::egui;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub name: String,
    pub smee_url: String,
    pub enabled: bool,
    pub base_prompt: String,
    pub workspace: Option<String>,
    pub reporter: Option<String>,
    pub filter_regex: Option<String>,
}

impl Default for Subscription {
    fn default() -> Self {
        Self {
            name: String::new(),
            smee_url: String::new(),
            enabled: true,
            base_prompt: String::new(),
            workspace: None,
            reporter: Some("console".to_string()),
            filter_regex: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub ws_url: String,
    pub default_workspace: String,
    pub subscriptions: Vec<Subscription>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            ws_url: "ws://localhost:8765".to_string(),
            default_workspace: dirs::home_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            subscriptions: vec![],
        }
    }
}

impl AppConfig {
    pub fn load() -> Self {
        let config_path = Self::config_path();
        if config_path.exists() {
            std::fs::read_to_string(&config_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            Self::default()
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let config_path = Self::config_path();
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&config_path, json)?;
        Ok(())
    }

    fn config_path() -> std::path::PathBuf {
        dirs::home_dir()
            .expect("无法获取 home 目录")
            .join(".agent-loop")
            .join("gui-config.json")
    }
}
