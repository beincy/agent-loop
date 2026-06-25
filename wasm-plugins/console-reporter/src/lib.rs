/// Console 汇报器 WASM 插件
///
/// 复刻内置 ConsoleReporter 的行为：
///   - 格式化输出打印到 stdout
///   - 追加写入 <log_dir>/<subscription_name>.log
use serde::{Deserialize, Serialize};

// ── 内存协议（与 agent-loop host 约定）─────────────────────────

#[no_mangle]
pub extern "C" fn alloc(size: i32) -> i32 {
    let mut buf: Vec<u8> = vec![0u8; size as usize];
    let ptr = buf.as_mut_ptr() as i32;
    std::mem::forget(buf);
    ptr
}

#[no_mangle]
pub extern "C" fn dealloc(ptr: i32, size: i32) {
    if ptr == 0 || size == 0 {
        return;
    }
    unsafe {
        let _ = Vec::from_raw_parts(ptr as *mut u8, size as usize, size as usize);
    }
}

// ── 入口函数 ──────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn report(ptr: i32, len: i32) -> i32 {
    let input_bytes =
        unsafe { std::slice::from_raw_parts(ptr as *const u8, len as usize) };

    let output = match serde_json::from_slice::<Input>(input_bytes) {
        Ok(input) => build_output(input),
        Err(e) => Output {
            stdout: None,
            stderr: Some(format!("[console-reporter] 解析输入失败: {e}\n")),
            append_file: None,
        },
    };

    write_output(serde_json::to_vec(&output).unwrap_or_default())
}

fn write_output(json: Vec<u8>) -> i32 {
    let mut out: Vec<u8> = Vec::with_capacity(4 + json.len());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&json);
    let ptr = out.as_ptr() as i32;
    std::mem::forget(out);
    ptr
}

// ── 数据类型 ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct Input {
    subscription_name: String,
    success: bool,
    output: String,
    error: Option<String>,
    started_at: String,
    finished_at: String,
    duration_ms: i64,
    prompt: String,
    log_dir: String,
}

#[derive(Serialize)]
struct Output {
    #[serde(skip_serializing_if = "Option::is_none")]
    stdout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stderr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    append_file: Option<AppendFile>,
}

#[derive(Serialize)]
struct AppendFile {
    path: String,
    content: String,
}

// ── 格式化逻辑 ────────────────────────────────────────────────

fn build_output(input: Input) -> Output {
    Output {
        stdout: Some(format_console(&input)),
        stderr: None,
        append_file: Some(AppendFile {
            path: format!("{}/{}.log", input.log_dir, input.subscription_name),
            content: format_log_entry(&input),
        }),
    }
}

fn format_console(input: &Input) -> String {
    let sep = "─".repeat(60);
    let icon = if input.success { "✅" } else { "❌" };
    let mut out = String::new();

    out.push('\n');
    out.push_str(&sep); out.push('\n');
    out.push_str(&format!("{icon} [{}] 执行完成\n", input.subscription_name));
    out.push_str(&format!("   开始: {}\n", input.started_at));
    out.push_str(&format!("   耗时: {}ms\n", input.duration_ms));
    out.push_str(&sep); out.push('\n');

    if input.success {
        out.push_str(&input.output);
        out.push('\n');
    } else {
        out.push_str(&format!(
            "❌ 错误: {}\n",
            input.error.as_deref().unwrap_or("未知错误")
        ));
        if !input.output.is_empty() {
            out.push_str(&format!("输出:\n{}\n", input.output));
        }
    }

    out.push_str(&sep); out.push('\n');
    out
}

fn format_log_entry(input: &Input) -> String {
    let divider = "=".repeat(80);
    let status = if input.success { "SUCCESS" } else { "FAILURE" };
    let mut log = String::new();

    log.push_str(&divider); log.push('\n');
    log.push_str(&format!(
        "[{}] 订阅: {}\n",
        input.started_at, input.subscription_name
    ));
    log.push_str(&format!(
        "状态: {status} | 耗时: {}ms\n",
        input.duration_ms
    ));
    log.push_str(&format!("完成时间: {}\n", input.finished_at));
    log.push('\n');
    log.push_str("── 提示词 ──\n");
    log.push_str(&input.prompt); log.push('\n');
    log.push('\n');
    log.push_str("── 输出 ──\n");

    if input.success {
        log.push_str(&input.output);
        log.push('\n');
    } else {
        log.push_str(&format!(
            "错误: {}\n",
            input.error.as_deref().unwrap_or("未知错误")
        ));
        if !input.output.is_empty() {
            log.push_str(&format!("\n── 部分输出 ──\n{}\n", input.output));
        }
    }

    log.push_str(&divider); log.push('\n');
    log
}

// ── 单元测试 ──────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_input(success: bool) -> Input {
        Input {
            subscription_name: "test".to_string(),
            success,
            output: "some output".to_string(),
            error: if success { None } else { Some("oops".to_string()) },
            started_at: "2026-01-01T00:00:00Z".to_string(),
            finished_at: "2026-01-01T00:00:01Z".to_string(),
            duration_ms: 1000,
            prompt: "do something".to_string(),
            log_dir: "/tmp/logs".to_string(),
        }
    }

    #[test]
    fn success_output_contains_icon() {
        let out = format_console(&make_input(true));
        assert!(out.contains("✅"));
        assert!(out.contains("some output"));
    }

    #[test]
    fn failure_output_contains_error() {
        let out = format_console(&make_input(false));
        assert!(out.contains("❌"));
        assert!(out.contains("oops"));
    }

    #[test]
    fn log_entry_contains_prompt() {
        let log = format_log_entry(&make_input(true));
        assert!(log.contains("do something"));
        assert!(log.contains("SUCCESS"));
    }

    #[test]
    fn log_path_uses_log_dir() {
        let output = build_output(make_input(true));
        assert_eq!(
            output.append_file.unwrap().path,
            "/tmp/logs/test.log"
        );
    }
}
