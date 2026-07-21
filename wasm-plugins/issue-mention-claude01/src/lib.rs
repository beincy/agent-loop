//! issue-mention-claude01 过滤插件
//!
//! 与 issue-mention 逻辑完全一致，仅提及账号改为 `@claude01`。
//!
//! 放行条件（按事件类型区分）：
//! - 新 issue：`action == "opened"` 且含 `issue` 对象 → 检查 `issue.body` 含 `@claude01`
//! - 新评论：`action == "created"` 且含 `issue` + `comment` 对象 → 只检查 `comment.body`
//!   （不看 issue 正文，避免旧提及让每条新评论都误触发）
//!
//! 任何解析失败、字段缺失、类型不符 → 一律过滤（allow=false），不做处理。
//! 放行时输出的 json 只保留 body（headers/query 置空），去掉转发头等噪音。
//!
//! 内存协议（与 agent-loop 约定）：
//!   host 调用 alloc(len) 分配输入缓冲区，写入后调用 process(ptr, len)
//!   process 返回 [4字节 LE u32 长度][JSON 字节] 的指针
//!   host 可选调用 dealloc(ptr, size) 释放

use serde_json::{json, Value};

const MENTION: &str = "@claude01";

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

    // 必须是 issue 类事件：body.issue 为对象
    if !body.get("issue")?.is_object() {
        return None;
    }

    // 按事件类型区分提及检测位置
    let action = body.get("action")?.as_str()?;
    let mentioned = match action {
        // 新 issue：看 issue 正文
        "opened" => body
            .get("issue")
            .and_then(|i| i.get("body"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .contains(MENTION),
        // 新评论：必须有 comment 对象，且只看评论正文
        // （不看 issue 正文，避免旧提及让每条新评论都误触发）
        "created" => body
            .get("comment")?
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or("")
            .contains(MENTION),
        _ => false,
    };
    if !mentioned {
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

    /// 新 issue payload（参考 新issue.json 的关键结构）
    fn new_issue_body(issue_text: &str) -> Value {
        json!({
            "action": "opened",
            "issue": { "number": 1, "title": "测试", "body": issue_text },
            "repository": { "full_name": "Instaon/insta-develop-skills" }
        })
    }

    /// issue 评论 payload（参考 添加内容回复.json 的关键结构）
    fn comment_body(issue_text: &str, comment_text: &str) -> Value {
        json!({
            "action": "created",
            "issue": { "number": 1, "title": "测试", "body": issue_text },
            "comment": { "id": 4900436262u64, "body": comment_text },
            "repository": { "full_name": "Instaon/insta-develop-skills" }
        })
    }

    #[test]
    fn allows_new_issue_with_mention() {
        let out = evaluate(&event(new_issue_body("请处理 @claude01 谢谢"))).unwrap();
        assert_eq!(out["allow"], true);
        // 输出只保留 body
        assert_eq!(out["json"]["body"]["action"], "opened");
        assert!(out["json"]["headers"].as_object().unwrap().is_empty());
    }

    #[test]
    fn allows_comment_with_mention_in_comment_body() {
        let out = evaluate(&event(comment_body("普通正文", "@claude01 看一下"))).unwrap();
        assert_eq!(out["allow"], true);
        assert_eq!(out["json"]["body"]["comment"]["id"], 4900436262u64);
    }

    #[test]
    fn comment_event_ignores_issue_body_mention() {
        // 评论事件只看 comment.body：issue 正文里的旧提及不应让新评论误触发
        assert!(evaluate(&event(comment_body("正文有 @claude01", "无关回复"))).is_none());
    }

    #[test]
    fn does_not_match_other_account() {
        // @claude0805 是另一个账号，不应触发本插件
        assert!(evaluate(&event(new_issue_body("请处理 @claude0805"))).is_none());
        assert!(evaluate(&event(comment_body("正文", "@claude0805 看一下"))).is_none());
    }

    #[test]
    fn opened_event_ignores_comment_field() {
        // opened 事件只看 issue.body（正常不会带 comment，防御性验证）
        let mut b = new_issue_body("普通正文");
        b["comment"] = json!({ "body": "@claude01" });
        assert!(evaluate(&event(b)).is_none());
    }

    #[test]
    fn created_without_comment_object_is_filtered() {
        // action=created 但缺 comment 对象（非评论事件）→ 过滤
        let mut b = new_issue_body("@claude01");
        b["action"] = json!("created");
        assert!(evaluate(&event(b)).is_none());
    }

    #[test]
    fn filters_without_mention() {
        assert!(evaluate(&event(new_issue_body("测试123jjk"))).is_none());
        assert!(evaluate(&event(comment_body("测试123jjk", "什么情况啊"))).is_none());
    }

    #[test]
    fn filters_wrong_action() {
        let mut b = new_issue_body("@claude01");
        b["action"] = json!("closed");
        assert!(evaluate(&event(b)).is_none());
        let mut b = comment_body("x", "@claude01");
        b["action"] = json!("edited");
        assert!(evaluate(&event(b)).is_none());
    }

    #[test]
    fn filters_non_issue_events_and_bad_shapes() {
        // push 事件：无 issue 对象
        assert!(evaluate(&event(
            json!({ "action": "created", "ref": "refs/heads/main" })
        ))
        .is_none());
        // body 不是对象 / 字段类型不符 / 解析失败 → 一律过滤，不 panic
        assert!(evaluate(&event(json!("plain string"))).is_none());
        assert!(evaluate(&event(json!({ "action": 123, "issue": {} }))).is_none());
        assert!(evaluate(&event(json!(null))).is_none());
        assert!(evaluate("not json at all").is_none());
        assert!(evaluate(r#"{"no_body": true}"#).is_none());
        // issue 为非对象
        assert!(evaluate(&event(json!({ "action": "opened", "issue": "oops" }))).is_none());
        // issue.body 为 null（GitHub 允许空正文）且无 comment → 过滤且不 panic
        assert!(evaluate(&event(
            json!({ "action": "opened", "issue": { "body": null } })
        ))
        .is_none());
    }
}
