use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::PathBuf;

use crate::types::LoopConfig;

/// ~/.agent-loop 目录路径
pub fn agent_loop_dir() -> PathBuf {
    dirs::home_dir()
        .expect("无法获取 home 目录")
        .join(".agent-loop")
}

fn default_config_path() -> PathBuf {
    agent_loop_dir().join("config.json")
}

fn default_config_content(home: &str) -> String {
    format!(
        r#"{{
  "defaultWorkspace": "{home}",
  "subscriptions": []
}}
"#,
        home = home
    )
}

fn ensure_config_dir() -> Result<()> {
    let dir = agent_loop_dir();
    if !dir.exists() {
        fs::create_dir_all(&dir)?;
        println!("📁 已创建配置目录: {}", dir.display());
    }

    let config_path = default_config_path();
    if !config_path.exists() {
        let home = dirs::home_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        fs::write(&config_path, default_config_content(&home))?;
        println!("📝 已生成默认配置: {}", config_path.display());
    }

    Ok(())
}

/// 加载并验证配置文件。
///
/// 优先级：
///   1. 环境变量 LOOP_CONFIG 指定的路径
///   2. ~/.agent-loop/config.json（不存在时自动创建）
pub fn load_config() -> Result<LoopConfig> {
    let config_path = if let Ok(env_path) = std::env::var("LOOP_CONFIG") {
        PathBuf::from(env_path)
    } else {
        ensure_config_dir()?;
        default_config_path()
    };

    if !config_path.exists() {
        return Err(anyhow!(
            "配置文件不存在: {}\n请检查 LOOP_CONFIG 环境变量或 {}",
            config_path.display(),
            default_config_path().display()
        ));
    }

    let raw = fs::read_to_string(&config_path)
        .with_context(|| format!("读取配置文件失败: {}", config_path.display()))?;

    let config: LoopConfig = serde_json::from_str(&raw)
        .with_context(|| format!("解析配置文件失败: {}", config_path.display()))?;

    validate_config(&config)?;
    Ok(config)
}

fn validate_config(config: &LoopConfig) -> Result<()> {
    if config.default_workspace.is_empty() {
        return Err(anyhow!("配置文件格式错误：defaultWorkspace 不能为空"));
    }
    for (i, sub) in config.subscriptions.iter().enumerate() {
        if sub.name.is_empty() {
            return Err(anyhow!("subscriptions[{i}].name 必须是非空字符串"));
        }
        if sub.smee_url.is_empty() {
            return Err(anyhow!("subscriptions[{i}].smeeUrl 必须是非空字符串"));
        }
    }
    Ok(())
}
