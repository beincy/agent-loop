pub mod console;

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use async_trait::async_trait;

use crate::types::ExecutionResult;
use crate::wasm_host::call_wasm;

// ── Reporter trait ────────────────────────────────────────────

#[async_trait]
pub trait Reporter: Send + Sync {
    async fn report(&self, result: &ExecutionResult) -> Result<()>;
    async fn initialize(&self) -> Result<()> {
        Ok(())
    }
}

// ── WASM 汇报器 ───────────────────────────────────────────────

/// 从 `~/.agent-loop/reporters/<name>.wasm` 加载的 WASM 汇报器
///
/// # WASM 汇报器接口规范
///
/// 文件位置：`~/.agent-loop/reporters/<name>.wasm`
/// 导出函数：`report(ptr: i32, len: i32) -> i32`（及 alloc/memory）
///
/// **输入 JSON**（由 host 构造）：
/// ```json
/// {
///   "subscription_name": "...",
///   "success": true,
///   "output": "...",
///   "error": null,
///   "started_at": "2026-01-01T00:00:00Z",
///   "finished_at": "2026-01-01T00:00:01Z",
///   "duration_ms": 1500,
///   "prompt": "...",
///   "log_dir": "./logs"
/// }
/// ```
///
/// **输出 JSON**（WASM 返回，host 执行）：
/// ```json
/// {
///   "stdout": "...",
///   "stderr": "...",
///   "append_file": { "path": "...", "content": "..." }
/// }
/// ```
/// 所有字段均为可选。
pub struct WasmReporter {
    wasm_path: PathBuf,
    log_dir: String,
}

#[async_trait]
impl Reporter for WasmReporter {
    async fn initialize(&self) -> Result<()> {
        fs::create_dir_all(&self.log_dir)
            .with_context(|| format!("创建日志目录失败: {}", self.log_dir))?;
        Ok(())
    }

    async fn report(&self, result: &ExecutionResult) -> Result<()> {
        let duration_ms = result
            .finished_at
            .signed_duration_since(result.started_at)
            .num_milliseconds();

        let input = serde_json::json!({
            "subscription_name": result.subscription_name,
            "success": result.success,
            "output": result.output,
            "error": result.error,
            "started_at": result.started_at.to_rfc3339(),
            "finished_at": result.finished_at.to_rfc3339(),
            "duration_ms": duration_ms,
            "prompt": result.prompt,
            "log_dir": self.log_dir,
        });

        let input_json = serde_json::to_string(&input)?;
        let wasm_path = self.wasm_path.clone();

        let output_json = tokio::task::spawn_blocking(move || {
            call_wasm(&wasm_path, "report", &input_json)
        })
        .await??;

        execute_reporter_output(&output_json)
    }
}

/// 执行 WASM 汇报器返回的指令（打印、写文件）
fn execute_reporter_output(output_json: &str) -> Result<()> {
    let output: serde_json::Value = serde_json::from_str(output_json)
        .context("WASM 汇报器返回非合法 JSON")?;

    if let Some(text) = output.get("stdout").and_then(|v| v.as_str()) {
        print!("{text}");
    }
    if let Some(text) = output.get("stderr").and_then(|v| v.as_str()) {
        eprint!("{text}");
    }
    if let Some(file) = output.get("append_file") {
        let path = file
            .get("path")
            .and_then(|v| v.as_str())
            .context("append_file 缺少 path 字段")?;
        let content = file
            .get("content")
            .and_then(|v| v.as_str())
            .context("append_file 缺少 content 字段")?;

        if let Some(parent) = std::path::Path::new(path).parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建日志目录失败: {}", parent.display()))?;
        }
        let mut f = OpenOptions::new().create(true).append(true).open(path)
            .with_context(|| format!("打开日志文件失败: {path}"))?;
        f.write_all(content.as_bytes())?;
    }

    Ok(())
}

// ── 工厂函数 ──────────────────────────────────────────────────

/// 优先查找 `~/.agent-loop/reporters/<name>.wasm`，不存在时回退到内置实现
pub fn get_reporter(name: Option<&str>) -> Box<dyn Reporter> {
    let name = name.unwrap_or("console");
    let log_dir = std::env::var("LOOP_LOG_DIR").unwrap_or_else(|_| "./logs".to_string());

    if let Some(home) = dirs::home_dir() {
        let wasm_path = home
            .join(".agent-loop")
            .join("reporters")
            .join(format!("{name}.wasm"));

        if wasm_path.exists() {
            println!("   🔌 [汇报器] 使用 WASM 插件: {}", wasm_path.display());
            return Box::new(WasmReporter { wasm_path, log_dir });
        }
    }

    // 回退到内置汇报器
    match name {
        "console" => Box::new(console::ConsoleReporter::new()),
        other => {
            eprintln!("⚠️  未找到汇报器 \"{other}\"（WASM 或内置均不存在），回退到 console");
            Box::new(console::ConsoleReporter::new())
        }
    }
}
