#![allow(dead_code)]

use futures_util::StreamExt;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;

use crate::event_log::{Log, LogKind};
use crate::executor::execute_with_claude;
use crate::filter::apply_filters;
use crate::reporter::Reporter;
use crate::types::{SmeeEventData, SubscriptionConfig};

/// SSE 事件（解析后）
struct SseEvent {
    /// `event:` 字段（未命名事件为空）。smee 的控制事件为 `ready` / `ping`
    name: String,
    data: String,
}

/// 解析 SSE 字节流
struct SseParser {
    current_name: String,
    current_data: Vec<String>,
    buffer: String,
}

impl SseParser {
    fn new() -> Self {
        Self {
            current_name: String::new(),
            current_data: Vec::new(),
            buffer: String::new(),
        }
    }

    /// 喂入原始文本块，返回所有完整的 SSE 事件
    fn feed(&mut self, chunk: &str) -> Vec<SseEvent> {
        self.buffer.push_str(chunk);
        let mut events = Vec::new();

        while let Some(pos) = self.buffer.find('\n') {
            let line = self.buffer[..pos].trim_end_matches('\r').to_string();
            self.buffer = self.buffer[pos + 1..].to_string();

            if line.is_empty() {
                // 空行 → 分发事件（数据为空时仅重置状态，SSE 规范不分发）
                if !self.current_data.is_empty() {
                    events.push(SseEvent {
                        name: std::mem::take(&mut self.current_name),
                        data: self.current_data.join("\n"),
                    });
                    self.current_data.clear();
                } else {
                    self.current_name.clear();
                }
            } else if let Some(value) = line.strip_prefix("data:") {
                // 规范允许 "data:value"（无空格），仅去掉一个前导空格
                self.current_data
                    .push(value.strip_prefix(' ').unwrap_or(value).to_string());
            } else if let Some(value) = line.strip_prefix("event:") {
                self.current_name = value.trim().to_string();
            }
            // 忽略 id:, retry:, 注释行
        }

        events
    }
}

/// 按 UTF-8 字符边界切分字节流：多字节字符被 chunk 截断时，
/// 保留不完整的尾部字节，等下一个 chunk 拼上再解码
struct Utf8Chunker {
    pending: Vec<u8>,
}

impl Utf8Chunker {
    fn new() -> Self {
        Self { pending: Vec::new() }
    }

    fn feed(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        match std::str::from_utf8(&self.pending) {
            Ok(s) => {
                let s = s.to_string();
                self.pending.clear();
                s
            }
            Err(e) => {
                let valid_up_to = e.valid_up_to();
                let s = String::from_utf8_lossy(&self.pending[..valid_up_to]).into_owned();
                // 尾部不完整字节留待下一个 chunk；中间出现非法字节则整段丢弃避免卡死
                if e.error_len().is_none() {
                    self.pending.drain(..valid_up_to);
                } else {
                    self.pending.clear();
                }
                s
            }
        }
    }
}

/// 将 smee 转发的 JSON payload 解析为 SmeeEventData
fn parse_smee_payload(raw: &str) -> Option<SmeeEventData> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let obj = value.as_object()?;

    // 提取 headers
    let mut headers: HashMap<String, String> = HashMap::new();
    if let Some(h) = obj.get("headers").and_then(|v| v.as_object()) {
        for (k, v) in h {
            if let Some(s) = v.as_str() {
                headers.insert(k.clone(), s.to_string());
            }
        }
    }

    // 提取 query
    let mut query: HashMap<String, String> = HashMap::new();
    if let Some(q) = obj.get("query").and_then(|v| v.as_object()) {
        for (k, v) in q {
            if let Some(s) = v.as_str() {
                query.insert(k.clone(), s.to_string());
            }
        }
    }

    // 提取 body
    let body = obj.get("body").cloned().unwrap_or(serde_json::Value::Null);

    Some(SmeeEventData {
        headers,
        query,
        body,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
    })
}

/// 连接 smee.io SSE 并持续推送事件，断线后自动重连。
/// 连接状态与错误通过 `log` 上报（GUI 执行记录可见）。
pub async fn run_sse_loop(smee_url: &str, tx: mpsc::UnboundedSender<SmeeEventData>, log: Log) {
    let client = reqwest::Client::builder()
        // SSE 长连接不能设总超时（reqwest 的 timeout 是整个请求的上限，
        // 设 0 会立即超时）；只限制建连阶段
        .connect_timeout(Duration::from_secs(15))
        .build()
        .expect("构建 HTTP 客户端失败");

    loop {
        match client
            .get(smee_url)
            .header("Accept", "text/event-stream")
            .header("Cache-Control", "no-cache")
            .send()
            .await
        {
            Ok(response) if !response.status().is_success() => {
                log.push(
                    LogKind::Error,
                    "smee",
                    format!("订阅失败（HTTP {}）: {smee_url}", response.status()),
                    "",
                );
            }
            Ok(response) => {
                log.push(LogKind::System, "smee", format!("🔗 SSE 已连接: {smee_url}"), "");
                let mut stream = response.bytes_stream();
                let mut parser = SseParser::new();
                let mut chunker = Utf8Chunker::new();

                while let Some(chunk_result) = stream.next().await {
                    match chunk_result {
                        Ok(bytes) => {
                            let text = chunker.feed(&bytes);
                            for event in parser.feed(&text) {
                                // 只处理未命名（message）事件；
                                // smee 的 ready / ping 控制事件也带 data，须跳过
                                if !event.name.is_empty() && event.name != "message" {
                                    continue;
                                }
                                if let Some(data) = parse_smee_payload(&event.data) {
                                    let _ = tx.send(data);
                                }
                            }
                        }
                        Err(e) => {
                            log.push(LogKind::Error, "smee", format!("SSE 流读取错误: {e}"), "");
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                log.push(LogKind::Error, "smee", format!("连接 smee.io 失败: {e}"), "");
            }
        }

        log.push(LogKind::System, "smee", "⚠️ SSE 连接断开，5 秒后重连...", "");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_tracks_event_names_for_smee_control_events() {
        let mut parser = SseParser::new();
        let events = parser.feed(
            "event: ready\ndata: {}\n\nevent: ping\ndata: {}\n\ndata: {\"body\":{\"a\":1}}\n\n",
        );
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].name, "ready");
        assert_eq!(events[1].name, "ping");
        // webhook 消息是未命名事件，且事件名不会从上一条泄漏过来
        assert_eq!(events[2].name, "");
        assert_eq!(events[2].data, r#"{"body":{"a":1}}"#);
    }

    #[test]
    fn parser_handles_chunk_split_and_no_space_data() {
        let mut parser = SseParser::new();
        assert!(parser.feed("data:hel").is_empty());
        assert!(parser.feed("lo\n").is_empty());
        let events = parser.feed("\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "hello");
    }

    #[test]
    fn utf8_chunker_reassembles_split_multibyte_chars() {
        let bytes = "数据中文".as_bytes();
        let mut chunker = Utf8Chunker::new();
        // 在多字节字符中间切开
        let first = chunker.feed(&bytes[..4]);
        let second = chunker.feed(&bytes[4..]);
        assert_eq!(format!("{first}{second}"), "数据中文");
    }
}

/// 启动单个订阅：连接 smee.io 并调度 Claude 执行
pub async fn start_subscriber(
    subscription: SubscriptionConfig,
    default_workspace: String,
    reporter: Box<dyn Reporter>,
    concurrent: bool,
) {
    println!(
        "📡 [{}] 已订阅 {}",
        subscription.name, subscription.smee_url
    );

    let (tx, mut rx) = mpsc::unbounded_channel::<SmeeEventData>();

    let smee_url = subscription.smee_url.clone();
    // CLI 模式没有 GUI 日志面板：接收端直接丢弃，Log 仍会打印到终端
    let (log_tx, _log_rx) = std::sync::mpsc::channel();
    let log = Log::new(log_tx);
    tokio::spawn(async move {
        run_sse_loop(&smee_url, tx, log).await;
    });

    // 使用 Arc 让 reporter 可以跨 task 共享
    let reporter = std::sync::Arc::new(reporter);
    let subscription = std::sync::Arc::new(subscription);
    let default_workspace = std::sync::Arc::new(default_workspace);

    if concurrent {
        // 并发模式：每个事件独立 spawn
        while let Some(event) = rx.recv().await {
            let reporter = reporter.clone();
            let subscription = subscription.clone();
            let default_workspace = default_workspace.clone();

            tokio::spawn(async move {
                println!("\n📨 [{}] 收到事件，执行预处理...", subscription.name);
                match apply_filters(
                    event,
                    subscription.filter_regex.as_deref(),
                    subscription.wasm_policy.as_deref(),
                )
                .await
                {
                    Err(e) => {
                        eprintln!("[{}] 过滤器错误: {e}", subscription.name);
                    }
                    Ok(fr) if !fr.allow => {
                        println!("[{}] 事件已被过滤，跳过", subscription.name);
                    }
                    Ok(fr) => {
                        println!("   ➡️  准备调用 Claude...");
                        match execute_with_claude(&subscription, &default_workspace, &fr.event)
                            .await
                        {
                            Ok(result) => {
                                if let Err(e) = reporter.report(&result).await {
                                    eprintln!("[{}] 汇报失败: {e}", subscription.name);
                                }
                            }
                            Err(e) => {
                                eprintln!("[{}] 执行异常: {e}", subscription.name);
                            }
                        }
                    }
                }
            });
        }
    } else {
        // 串行模式：顺序处理队列中的每个事件
        while let Some(event) = rx.recv().await {
            println!("\n📨 [{}] 收到事件，执行预处理...", subscription.name);
            match apply_filters(
                event,
                subscription.filter_regex.as_deref(),
                subscription.wasm_policy.as_deref(),
            )
            .await
            {
                Err(e) => {
                    eprintln!("[{}] 过滤器错误: {e}", subscription.name);
                }
                Ok(fr) if !fr.allow => {
                    println!("[{}] 事件已被过滤，跳过", subscription.name);
                }
                Ok(fr) => {
                    println!("   ➡️  准备调用 Claude...");
                    match execute_with_claude(&subscription, &default_workspace, &fr.event).await {
                        Ok(result) => {
                            if let Err(e) = reporter.report(&result).await {
                                eprintln!("[{}] 汇报失败: {e}", subscription.name);
                            }
                        }
                        Err(e) => {
                            eprintln!("[{}] 执行异常: {e}", subscription.name);
                        }
                    }
                }
            }
        }
    }
}
