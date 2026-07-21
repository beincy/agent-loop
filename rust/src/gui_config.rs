use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub name: String,
    /// 触发方式：Webhook 回调地址（smee.io 通道 URL）。
    /// 旧配置字段名为 smee_url，通过 alias 兼容。
    #[serde(alias = "smee_url")]
    pub webhook_url: String,
    pub enabled: bool,
    pub base_prompt: String,
    /// 绑定的 cc-connect 项目名（register 消息的 `project` 字段）。
    /// 空字符串 = 不指定，关联到 cc-connect 的默认项目。
    #[serde(default)]
    pub project: String,
    pub reporter: Option<String>,
    pub filter_regex: Option<String>,
}

impl Default for Agent {
    fn default() -> Self {
        Self {
            name: String::new(),
            webhook_url: String::new(),
            enabled: true,
            base_prompt: String::new(),
            project: String::new(),
            reporter: Some("console".to_string()),
            filter_regex: None,
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
    /// Agent 列表。旧配置字段名为 subscriptions，通过 alias 兼容。
    #[serde(alias = "subscriptions")]
    pub agents: Vec<Agent>,
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
            agents: vec![],
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

/// 读取 cc-connect 配置（~/.cc-connect/config.toml）中 `[[projects]]` 的项目名列表，
/// 供 Agent 表单的项目下拉框使用。读取失败返回空列表（表单仍可选默认项目）。
pub fn scan_cc_connect_projects() -> Vec<String> {
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };
    let path = home.join(".cc-connect").join("config.toml");
    std::fs::read_to_string(&path)
        .map(|text| projects_from_toml(&text))
        .unwrap_or_default()
}

/// 从 cc-connect config.toml 文本中提取 `[[projects]]` 的 name 列表
fn projects_from_toml(text: &str) -> Vec<String> {
    let Ok(value) = text.parse::<toml::Value>() else {
        return vec![];
    };
    let mut names: Vec<String> = value
        .get("projects")
        .and_then(|p| p.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| p.get("name").and_then(|n| n.as_str()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_project_names_from_cc_connect_config() {
        // 与真实 ~/.cc-connect/config.toml 相同的结构：
        // 缩进的键、嵌套的 [[projects.agent.providers]]（也带 name 字段，不应混入）
        let text = r#"
[bridge]
enabled = true

[[projects]]
  name = "master"
  [projects.agent.options]
    work_dir = "/tmp/a"
  [[projects.agent.providers]]
    name = "zhipu-glm"

[[projects]]
  name = "dev"
  [[projects.agent.providers]]
    name = "aicoding"
"#;
        assert_eq!(projects_from_toml(text), vec!["master", "dev"]);
    }

    #[test]
    fn old_subscription_config_loads_as_agents() {
        // 旧版 gui-config.json 使用 subscriptions / smee_url 字段名
        let json = r#"{
            "ws_url": "ws://localhost:9810/bridge/ws",
            "subscriptions": [{
                "name": "issue处理",
                "smee_url": "https://smee.io/abc",
                "enabled": true,
                "base_prompt": "处理 issue",
                "reporter": "console",
                "filter_regex": null
            }]
        }"#;
        let config: AppConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.agents.len(), 1);
        assert_eq!(config.agents[0].name, "issue处理");
        assert_eq!(config.agents[0].webhook_url, "https://smee.io/abc");
    }

    #[test]
    fn invalid_or_empty_toml_yields_empty_list() {
        assert!(projects_from_toml("not [ valid").is_empty());
        assert!(projects_from_toml("").is_empty());
    }
}
