use futures_util::StreamExt;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;

use crate::executor::execute_with_claude;
use crate::reporter::Reporter;
use crate::types::{SmeeEventData, SubscriptionConfig};

/// SSE 事件（解析后）
struct SseEvent {
    data: String,
}

/// 解析 SSE 字节流
struct SseParser {
    current_data: Vec<String>,
    buffer: String,
}

impl SseParser {
    fn new() -> Self {
        Self {
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
                // 空行 → 分发事件
                if !self.current_data.is_empty() {
                    let data = self.current_data.join("\n");
                    self.current_data.clear();
                    events.push(SseEvent { data });
                }
            } else if let Some(value) = line.strip_prefix("data: ") {
                self.current_data.push(value.to_string());
            }
            // 忽略 event:, id:, retry:, 注释行
        }

        events
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

/// 连接 smee.io SSE 并持续推送事件，断线后自动重连
async fn run_sse_loop(smee_url: &str, tx: mpsc::UnboundedSender<SmeeEventData>) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(0)) // SSE 长连接，不设超时
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
            Ok(response) => {
                let mut stream = response.bytes_stream();
                let mut parser = SseParser::new();

                while let Some(chunk_result) = stream.next().await {
                    match chunk_result {
                        Ok(bytes) => {
                            if let Ok(text) = std::str::from_utf8(&bytes) {
                                for event in parser.feed(text) {
                                    if let Some(data) = parse_smee_payload(&event.data) {
                                        let _ = tx.send(data);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("   ❌ SSE 流读取错误: {e}");
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("   ❌ 连接 smee.io 失败: {e}");
            }
        }

        eprintln!("   ⚠️  连接断开，5 秒后重连...");
        tokio::time::sleep(Duration::from_secs(5)).await;
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
    tokio::spawn(async move {
        run_sse_loop(&smee_url, tx).await;
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
                println!("\n📨 [{}] 收到事件，准备调用 Claude...", subscription.name);
                match execute_with_claude(&subscription, &default_workspace, &event).await {
                    Ok(result) => {
                        if let Err(e) = reporter.report(&result).await {
                            eprintln!("[{}] 汇报失败: {e}", subscription.name);
                        }
                    }
                    Err(e) => {
                        eprintln!("[{}] 执行异常: {e}", subscription.name);
                    }
                }
            });
        }
    } else {
        // 串行模式：顺序处理队列中的每个事件
        while let Some(event) = rx.recv().await {
            println!("\n📨 [{}] 收到事件，准备调用 Claude...", subscription.name);
            match execute_with_claude(&subscription, &default_workspace, &event).await {
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
