/// 两阶段预处理：正则过滤 → WASM 策略插件
///
/// # WASM 插件接口规范
///
/// 插件文件放在 `~/.agent-loop/policy/<name>.wasm`，必须导出：
///
/// ```
/// alloc(len: i32) -> i32          // 分配 len 字节，返回指针（host 写入输入）
/// process(ptr: i32, len: i32) -> i32  // 处理后返回指向输出的指针
/// dealloc(ptr: i32, len: i32)     // 可选，释放内存
/// memory                          // 线性内存
/// ```
///
/// 输出格式（output_ptr 处）：
///   [4 字节 LE u32 长度][JSON 字节]
///
/// 输出 JSON：
/// ```json
/// { "allow": true, "json": { ...可选的修改后事件... } }
/// ```
use anyhow::{Context, Result};
use regex::Regex;
use std::path::PathBuf;

use crate::types::SmeeEventData;

pub struct FilterResult {
    pub allow: bool,
    /// 可能被 WASM 插件修改过的事件
    pub event: SmeeEventData,
}

/// 依次执行正则过滤和 WASM 策略，任一拒绝则返回 allow=false
pub async fn apply_filters(
    event: SmeeEventData,
    filter_regex: Option<&str>,
    wasm_policy: Option<&str>,
) -> Result<FilterResult> {
    // ── 阶段 1：正则过滤 ───────────────────────────────────────
    if let Some(pattern) = filter_regex {
        let re = Regex::new(pattern)
            .with_context(|| format!("正则表达式无效: {pattern}"))?;
        let event_json = serde_json::to_string(&event)?;
        if !re.is_match(&event_json) {
            println!("   🚫 [正则过滤] 未匹配，跳过此事件");
            return Ok(FilterResult { allow: false, event });
        }
        println!("   ✅ [正则过滤] 通过");
    }

    // ── 阶段 2：WASM 策略插件 ──────────────────────────────────
    if let Some(plugin_name) = wasm_policy {
        let wasm_path = dirs::home_dir()
            .context("无法获取 Home 目录")?
            .join(".agent-loop")
            .join("policy")
            .join(format!("{plugin_name}.wasm"));

        if !wasm_path.exists() {
            return Err(anyhow::anyhow!(
                "WASM 插件不存在: {}",
                wasm_path.display()
            ));
        }

        let event_json = serde_json::to_string(&event)?;
        let plugin_name_owned = plugin_name.to_string();

        // wasmtime 是同步 API，放到 blocking 线程池避免阻塞 tokio
        let wasm_result = tokio::task::spawn_blocking(move || {
            run_wasm_policy(&wasm_path, &event_json)
        })
        .await??;

        let icon = if wasm_result.allow { "✅" } else { "🚫" };
        let verdict = if wasm_result.allow { "允许" } else { "拒绝" };
        println!("   {icon} [WASM 策略 | {plugin_name_owned}] {verdict}");

        if !wasm_result.allow {
            return Ok(FilterResult { allow: false, event });
        }

        // WASM 可选返回修改后的事件 JSON
        let final_event = match wasm_result.modified {
            Some(v) => serde_json::from_value(v).unwrap_or(event),
            None => event,
        };

        return Ok(FilterResult {
            allow: true,
            event: final_event,
        });
    }

    Ok(FilterResult { allow: true, event })
}

// ── WASM 执行 ──────────────────────────────────────────────────

struct WasmResult {
    allow: bool,
    /// WASM 返回的可选修改事件（`json` 字段）
    modified: Option<serde_json::Value>,
}

fn run_wasm_policy(wasm_path: &PathBuf, input_json: &str) -> Result<WasmResult> {
    use wasmtime::{Engine, Instance, Module, Store};

    let engine = Engine::default();
    let module = Module::from_file(&engine, wasm_path)
        .with_context(|| format!("加载 WASM 模块失败: {}", wasm_path.display()))?;
    let mut store = Store::new(&engine, ());
    let instance = Instance::new(&mut store, &module, &[])
        .context("实例化 WASM 模块失败")?;

    let memory = instance
        .get_memory(&mut store, "memory")
        .ok_or_else(|| anyhow::anyhow!("WASM 模块必须导出 'memory'"))?;

    let alloc = instance
        .get_typed_func::<i32, i32>(&mut store, "alloc")
        .context("WASM 模块必须导出 'alloc(i32) -> i32'")?;

    let process = instance
        .get_typed_func::<(i32, i32), i32>(&mut store, "process")
        .context("WASM 模块必须导出 'process(i32, i32) -> i32'")?;

    let input_bytes = input_json.as_bytes();
    let input_len = input_bytes.len() as i32;

    // 分配输入缓冲区并写入数据
    let input_ptr = alloc
        .call(&mut store, input_len)
        .context("WASM alloc 调用失败")? as usize;

    memory
        .data_mut(&mut store)
        .get_mut(input_ptr..input_ptr + input_bytes.len())
        .context("WASM 内存越界（写入输入）")?
        .copy_from_slice(input_bytes);

    // 调用 process，返回指向 [u32 LE 长度 | JSON 字节] 的指针
    let output_ptr = process
        .call(&mut store, (input_ptr as i32, input_len))
        .context("WASM process 调用失败")? as usize;

    // 读取输出长度（前 4 字节）
    let output_len = {
        let data = memory.data(&store);
        let buf = data
            .get(output_ptr..output_ptr + 4)
            .context("WASM 内存越界（读取输出长度）")?;
        u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize
    };

    // 读取输出 JSON
    let output_str = {
        let data = memory.data(&store);
        let start = output_ptr + 4;
        let bytes = data
            .get(start..start + output_len)
            .context("WASM 内存越界（读取输出数据）")?;
        String::from_utf8(bytes.to_vec()).context("WASM 输出非 UTF-8")?
    };

    // 可选：释放输入内存
    if let Ok(dealloc) =
        instance.get_typed_func::<(i32, i32), ()>(&mut store, "dealloc")
    {
        let _ = dealloc.call(&mut store, (input_ptr as i32, input_len));
    }

    // 解析输出
    let output: serde_json::Value =
        serde_json::from_str(&output_str).context("WASM 输出非合法 JSON")?;

    let allow = output
        .get("allow")
        .and_then(|v| v.as_bool())
        .context("WASM 输出必须包含布尔类型的 'allow' 字段")?;

    let modified = output.get("json").cloned();

    Ok(WasmResult { allow, modified })
}
