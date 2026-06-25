use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 单条订阅配置
#[derive(Debug, Clone, Deserialize)]
pub struct SubscriptionConfig {
    /// 订阅名称（唯一标识）
    pub name: String,
    /// Smee.io 频道 URL
    #[serde(rename = "smeeUrl")]
    pub smee_url: String,
    /// 是否启用
    pub enabled: bool,
    /// 基础提示词
    #[serde(rename = "basePrompt")]
    pub base_prompt: String,
    /// 工作区目录，不填则使用 defaultWorkspace
    pub workspace: Option<String>,
    /// 汇报器名称，不填则使用 "console"
    pub reporter: Option<String>,
}

/// 根配置
#[derive(Debug, Deserialize)]
pub struct LoopConfig {
    /// 默认工作区目录
    #[serde(rename = "defaultWorkspace")]
    pub default_workspace: String,
    /// 订阅列表
    pub subscriptions: Vec<SubscriptionConfig>,
}

/// Smee 转发过来的原始事件数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmeeEventData {
    pub headers: HashMap<String, String>,
    pub query: HashMap<String, String>,
    pub body: serde_json::Value,
    pub timestamp: u64,
}

/// Claude 执行结果
#[derive(Debug)]
pub struct ExecutionResult {
    pub subscription_name: String,
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub prompt: String,
}
