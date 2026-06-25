/// 通用 WASM 函数调用协议
///
/// 所有 WASM 插件遵循同一内存约定：
///   导出 `alloc(len: i32) -> i32`     — host 分配输入缓冲区
///   导出 `<fn>(ptr: i32, len: i32) -> i32` — 处理，返回输出指针
///   导出 `dealloc(ptr: i32, len: i32)` — 可选，释放内存
///   导出 `memory`                      — 线性内存
///
/// 输出格式（输出指针处）：[4 字节 LE u32 长度][JSON 字节]
use anyhow::{Context, Result};
use std::path::Path;

pub fn call_wasm(wasm_path: &Path, fn_name: &str, input_json: &str) -> Result<String> {
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

    let func = instance
        .get_typed_func::<(i32, i32), i32>(&mut store, fn_name)
        .with_context(|| format!("WASM 模块必须导出 '{fn_name}(i32, i32) -> i32'"))?;

    let input_bytes = input_json.as_bytes();
    let input_len = input_bytes.len() as i32;

    // 分配并写入输入
    let input_ptr = alloc.call(&mut store, input_len).context("WASM alloc 失败")? as usize;
    memory
        .data_mut(&mut store)
        .get_mut(input_ptr..input_ptr + input_bytes.len())
        .context("WASM 内存越界（写入输入）")?
        .copy_from_slice(input_bytes);

    // 调用目标函数，返回 [4字节LE长度][JSON字节] 的指针
    let output_ptr = func
        .call(&mut store, (input_ptr as i32, input_len))
        .with_context(|| format!("WASM {fn_name} 调用失败"))? as usize;

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

    Ok(output_str)
}
