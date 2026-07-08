//! pr-mention 过滤插件
//!
//! 放行条件（全部满足）：
//! - 事件 body 含 `pull_request` 对象（PR 事件）
//! - `action == "opened"`（新建 PR）
//! - `pull_request.body` 或 `pull_request.title` 中包含 `@claude0805`
//!
//! 任何解析失败、字段缺失、类型不符 → 一律过滤（allow=false），不做处理。
//! 放行时输出的 json 只保留 body（headers/query 置空），去掉转发头等噪音。
//!
//! 注：PR 里的评论是 `issue_comment` 事件（带 issue 对象），由 issue-mention 插件覆盖。
//!
//! 内存协议（与 agent-loop 约定）：
//!   host 调用 alloc(len) 分配输入缓冲区，写入后调用 process(ptr, len)
//!   process 返回 [4字节 LE u32 长度][JSON 字节] 的指针
//!   host 可选调用 dealloc(ptr, size) 释放

use serde_json::{json, Value};

const MENTION: &str = "@claude0805";

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

#[no_mangle]
pub extern "C" fn process(ptr: i32, len: i32) -> i32 {
    let input = unsafe { std::slice::from_raw_parts(ptr as *const u8, len as usize) };
    let output = std::str::from_utf8(input)
        .ok()
        .and_then(evaluate)
        .map(|v| v.to_string())
        .unwrap_or_else(|| r#"{"allow":false}"#.to_string());

    // 输出格式：[4字节 LE u32 长度][JSON 字节]
    let bytes = output.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(4 + bytes.len());
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);

    let out_ptr = out.as_ptr() as i32;
    std::mem::forget(out);
    out_ptr
}

/// 判定并构造输出。返回 None 表示过滤（allow=false）。
fn evaluate(input: &str) -> Option<Value> {
    let event: Value = serde_json::from_str(input).ok()?;
    let body = event.get("body")?;

    // 必须是 PR 事件：body.pull_request 为对象
    let pr = body.get("pull_request")?;
    if !pr.is_object() {
        return None;
    }

    // 只响应新建 PR
    let action = body.get("action")?.as_str()?;
    if action != "opened" {
        return None;
    }

    // 提及检测：PR 正文或标题任一包含即可
    let pr_body = pr.get("body").and_then(Value::as_str).unwrap_or("");
    let pr_title = pr.get("title").and_then(Value::as_str).unwrap_or("");
    if !pr_body.contains(MENTION) && !pr_title.contains(MENTION) {
        return None;
    }

    // 放行：json 只保留 body，headers/query 清空（符合 SmeeEventData 结构）
    let timestamp = event.get("timestamp").cloned().unwrap_or(json!(0));
    Some(json!({
        "allow": true,
        "json": {
            "headers": {},
            "query": {},
            "body": body,
            "timestamp": timestamp,
        }
    }))
}

// ── 单元测试 ──────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// 模拟 SmeeEventData 输入（host 传入的完整事件 JSON）
    fn event(body: Value) -> String {
        json!({ "headers": {}, "query": {}, "body": body, "timestamp": 1783401631255u64 })
            .to_string()
    }

    /// 新建 PR payload（参考 提交pr.json 的关键结构）
    fn pr_body(action: &str, title: &str, text: &str) -> Value {
        json!({
            "action": action,
            "number": 2,
            "pull_request": { "number": 2, "title": title, "body": text },
            "repository": { "full_name": "Instaon/insta-develop-skills" }
        })
    }

    #[test]
    fn allows_pr_with_mention_in_body() {
        let out = evaluate(&event(pr_body("opened", "docs: 补充", "@claude0805 请审查"))).unwrap();
        assert_eq!(out["allow"], true);
        // 输出只保留 body
        assert_eq!(out["json"]["body"]["pull_request"]["number"], 2);
        assert!(out["json"]["headers"].as_object().unwrap().is_empty());
    }

    #[test]
    fn allows_pr_with_mention_in_title() {
        let out = evaluate(&event(pr_body("opened", "@claude0805 帮忙看这个 PR", "Fixes #42")));
        assert!(out.is_some());
    }

    #[test]
    fn filters_pr_without_mention() {
        // 对应样例文件原始内容（title/body 都无提及）→ 过滤
        assert!(evaluate(&event(pr_body("opened", "docs: README 补充", "Fixes #42"))).is_none());
    }

    #[test]
    fn filters_wrong_action() {
        for action in ["closed", "synchronize", "edited", "reopened"] {
            assert!(evaluate(&event(pr_body(action, "t", "@claude0805"))).is_none());
        }
    }

    #[test]
    fn filters_non_pr_events_and_bad_shapes() {
        // issue 事件：无 pull_request 对象 → 交给 issue-mention，这里过滤
        assert!(evaluate(&event(json!({
            "action": "opened",
            "issue": { "body": "@claude0805" }
        })))
        .is_none());
        // 结构不符 / 类型不符 / 解析失败 → 一律过滤，不 panic
        assert!(evaluate(&event(json!("plain string"))).is_none());
        assert!(evaluate(&event(json!({ "action": "opened", "pull_request": "oops" }))).is_none());
        assert!(evaluate(&event(json!({ "action": 123, "pull_request": {} }))).is_none());
        assert!(evaluate(&event(json!(null))).is_none());
        assert!(evaluate("not json at all").is_none());
        assert!(evaluate(r#"{"no_body": true}"#).is_none());
        // PR 正文为 null（GitHub 允许空正文）→ 过滤且不 panic
        assert!(evaluate(&event(json!({
            "action": "opened",
            "pull_request": { "title": "t", "body": null }
        })))
        .is_none());
    }
}
