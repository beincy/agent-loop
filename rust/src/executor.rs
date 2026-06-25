use anyhow::Result;
use chrono::Utc;
use tokio::process::Command;

use crate::types::{ExecutionResult, SmeeEventData, SubscriptionConfig};

fn build_prompt(base_prompt: &str, event: &SmeeEventData) -> String {
    let body_str = match &event.body {
        serde_json::Value::String(s) => s.clone(),
        v => serde_json::to_string_pretty(v).unwrap_or_default(),
    };

    let headers_str = serde_json::to_string_pretty(&event.headers).unwrap_or_default();

    let query_str = if event.query.is_empty() {
        "（无）".to_string()
    } else {
        serde_json::to_string_pretty(&event.query).unwrap_or_default()
    };

    let timestamp = chrono::DateTime::from_timestamp_millis(event.timestamp as i64)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default();

    format!(
        "{base_prompt}\n\n---\n以下是通过 Webhook 接收到的事件信息，请结合上述指令处理：\n\n## 请求头（Headers）\n```json\n{headers_str}\n```\n\n## 查询参数（Query Parameters）\n```json\n{query_str}\n```\n\n## 请求体（Body）\n```\n{body_str}\n```\n\n> 事件接收时间：{timestamp}",
        base_prompt = base_prompt.trim(),
    )
}

async fn run_claude(prompt: &str, workspace: &str) -> (String, bool, Option<String>) {
    let result = Command::new("claude")
        .args(["-p", prompt, "--dangerously-skip-permissions"])
        .current_dir(workspace)
        .output()
        .await;

    match result {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            let combined = if stderr.is_empty() {
                stdout
            } else {
                format!("{stdout}\n[stderr]\n{stderr}")
            };

            if output.status.success() {
                (combined, true, None)
            } else {
                let err = format!(
                    "进程退出码: {}",
                    output.status.code().unwrap_or(-1)
                );
                (combined, false, Some(err))
            }
        }
        Err(e) => (String::new(), false, Some(e.to_string())),
    }
}

pub async fn execute_with_claude(
    subscription: &SubscriptionConfig,
    default_workspace: &str,
    event: &SmeeEventData,
) -> Result<ExecutionResult> {
    let workspace = subscription
        .workspace
        .as_deref()
        .unwrap_or(default_workspace);
    let prompt = build_prompt(&subscription.base_prompt, event);
    let started_at = Utc::now();

    println!(
        "\n🤖 [{}] 开始调用 Claude（工作区: {}）...",
        subscription.name, workspace
    );

    let (output, success, error) = run_claude(&prompt, workspace).await;
    let finished_at = Utc::now();

    Ok(ExecutionResult {
        subscription_name: subscription.name.clone(),
        success,
        output,
        error,
        started_at,
        finished_at,
        prompt,
    })
}
