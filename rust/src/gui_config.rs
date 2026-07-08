use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub name: String,
    pub smee_url: String,
    pub enabled: bool,
    pub base_prompt: String,
    pub workspace: Option<String>,
    pub reporter: Option<String>,
    pub filter_regex: Option<String>,
    /// 可选：服务启动后对该会话执行 `/provider switch <名称>`
    #[serde(default)]
    pub provider: Option<String>,
    /// 可选：服务启动后对该会话执行 `/model switch <别名>`
    #[serde(default)]
    pub model: Option<String>,
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
            provider: None,
            model: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// cc-connect Bridge WebSocket 端点（bridge-protocol.zh-CN.md）
    pub ws_url: String,
    /// Bridge 认证 token（config.toml [bridge].token）
    #[serde(default)]
    pub bridge_token: String,
    /// 启动 cc-connect 的命令行
    #[serde(default = "default_cc_connect_command")]
    pub cc_connect_command: String,
    pub default_workspace: String,
    pub subscriptions: Vec<Subscription>,
}

fn default_cc_connect_command() -> String {
    "cc-connect".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            ws_url: "ws://localhost:9810/bridge/ws".to_string(),
            bridge_token: String::new(),
            cc_connect_command: default_cc_connect_command(),
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
