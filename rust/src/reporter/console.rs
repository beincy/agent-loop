use anyhow::Result;
use async_trait::async_trait;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use crate::types::ExecutionResult;
use super::Reporter;

pub struct ConsoleReporter {
    log_dir: PathBuf,
}

impl ConsoleReporter {
    pub fn new() -> Self {
        let log_dir = std::env::var("LOOP_LOG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("./logs"));
        Self { log_dir }
    }
}

#[async_trait]
impl Reporter for ConsoleReporter {
    async fn initialize(&self) -> Result<()> {
        fs::create_dir_all(&self.log_dir)?;
        Ok(())
    }

    async fn report(&self, result: &ExecutionResult) -> Result<()> {
        let separator = "─".repeat(60);
        let status_icon = if result.success { "✅" } else { "❌" };
        let duration_ms = result
            .finished_at
            .signed_duration_since(result.started_at)
            .num_milliseconds();

        println!("\n{separator}");
        println!("{status_icon} [{}] 执行完成", result.subscription_name);
        println!("   开始: {}", result.started_at.to_rfc3339());
        println!("   耗时: {duration_ms}ms");
        println!("{separator}");

        if result.success {
            println!("{}", result.output);
        } else {
            eprintln!("❌ 错误: {}", result.error.as_deref().unwrap_or("未知错误"));
            if !result.output.is_empty() {
                println!("输出:\n{}", result.output);
            }
        }
        println!("{separator}\n");

        // 写入日志文件
        let log_file = self.log_dir.join(format!("{}.log", result.subscription_name));
        let log_entry = format_log_entry(result, duration_ms);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)?;
        file.write_all(log_entry.as_bytes())?;

        Ok(())
    }
}

fn format_log_entry(result: &ExecutionResult, duration_ms: i64) -> String {
    let divider = "=".repeat(80);
    let status = if result.success { "SUCCESS" } else { "FAILURE" };

    let mut lines = vec![
        divider.clone(),
        format!(
            "[{}] 订阅: {}",
            result.started_at.to_rfc3339(),
            result.subscription_name
        ),
        format!("状态: {status} | 耗时: {duration_ms}ms"),
        format!("完成时间: {}", result.finished_at.to_rfc3339()),
        String::new(),
        "── 提示词 ──".to_string(),
        result.prompt.clone(),
        String::new(),
        "── 输出 ──".to_string(),
    ];

    if result.success {
        lines.push(result.output.clone());
    } else {
        lines.push(format!(
            "错误: {}",
            result.error.as_deref().unwrap_or("未知错误")
        ));
        if !result.output.is_empty() {
            lines.extend(["".to_string(), "── 部分输出 ──".to_string(), result.output.clone()]);
        }
    }

    lines.push(divider);
    lines.push(String::new());
    lines.join("\n")
}
