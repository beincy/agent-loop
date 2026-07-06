//! GUI 后台服务：smee 订阅 → 过滤 → 转发 cc-connect 执行 → 汇报结果
//!
//! 在独立线程上运行 tokio runtime，GUI 通过 watch 通道下发停止信号、
//! 通过 std::sync::mpsc 接收日志。

use std::sync::mpsc::Sender as StdSender;

use chrono::Utc;
use tokio::sync::{mpsc, watch};

use crate::bridge_client::{BridgeClient, ConnStatus};
use crate::event_log::{Log, LogEntry, LogKind};
use crate::executor::build_prompt;
use crate::filter::apply_filters;
use crate::gui_config::{AppConfig, Subscription};
use crate::reporter::get_reporter;
use crate::subscriber::run_sse_loop;
use crate::types::{ExecutionResult, SmeeEventData};

/// Bridge 协议中的平台名（组成 session_key 的第一段）
const PLATFORM: &str = "agent-loop";

pub struct BridgeService {
    stop_tx: watch::Sender<bool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl BridgeService {
    pub fn start(config: AppConfig, log_tx: StdSender<LogEntry>, conn_status: ConnStatus) -> Self {
        let (stop_tx, stop_rx) = watch::channel(false);
        let thread = std::thread::spawn(move || {
            let log = Log::new(log_tx);
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    log.push(LogKind::Error, "系统", format!("创建异步运行时失败: {e}"), "");
                    return;
                }
            };
            rt.block_on(run_service(config, log, stop_rx, conn_status));
            // rt 在此被丢弃，所有后台任务（SSE、Bridge 连接）随之结束
        });
        Self {
            stop_tx,
            thread: Some(thread),
        }
    }

    pub fn stop(&mut self) {
        let _ = self.stop_tx.send(true);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for BridgeService {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn run_service(
    config: AppConfig,
    log: Log,
    mut stop_rx: watch::Receiver<bool>,
    conn_status: ConnStatus,
) {
    if config.bridge_token.trim().is_empty() {
        log.system("⚠️ 未配置 Bridge Token，cc-connect 可能拒绝连接（HTTP 401）");
    }

    let bridge = BridgeClient::spawn(
        config.ws_url.clone(),
        config.bridge_token.trim().to_string(),
        PLATFORM.to_string(),
        log.clone(),
        conn_status,
    );

    let enabled: Vec<Subscription> = config
        .subscriptions
        .into_iter()
        .filter(|s| s.enabled)
        .collect();
    if enabled.is_empty() {
        log.system("⚠️ 没有启用的订阅，仅维持与 cc-connect 的连接");
    }
    for sub in enabled {
        tokio::spawn(run_subscription(sub, bridge.clone(), log.clone()));
    }

    // 等待停止信号
    while !*stop_rx.borrow() {
        if stop_rx.changed().await.is_err() {
            break;
        }
    }
    log.system("🛑 服务已停止");
}

/// 单个订阅的事件循环：SSE 接收 → 过滤 → 提交 cc-connect → 汇报结果（串行处理）
async fn run_subscription(sub: Subscription, bridge: BridgeClient, log: Log) {
    let (tx, mut rx) = mpsc::unbounded_channel::<SmeeEventData>();
    let smee_url = sub.smee_url.clone();
    tokio::spawn(async move { run_sse_loop(&smee_url, tx).await });
    log.push(
        LogKind::System,
        &sub.name,
        format!("📡 已订阅 {}", sub.smee_url),
        "",
    );

    let reporter = get_reporter(sub.reporter.as_deref());
    if let Err(e) = reporter.initialize().await {
        log.push(LogKind::Error, &sub.name, format!("汇报器初始化失败: {e}"), "");
    }

    let (filter_regex, wasm_policy) = parse_filter(sub.filter_regex.as_deref());
    let uid = session_identity(&sub.name);
    let session_key = format!("{PLATFORM}:{uid}:{uid}");

    while let Some(event) = rx.recv().await {
        // 展示 Webhook 原始数据（headers / query / body）
        let event_json = serde_json::to_string_pretty(&event).unwrap_or_default();
        log.push(LogKind::Webhook, &sub.name, "收到 Webhook 事件", event_json);

        let filtered =
            match apply_filters(event, filter_regex.as_deref(), wasm_policy.as_deref()).await {
                Ok(fr) => fr,
                Err(e) => {
                    log.push(LogKind::Error, &sub.name, format!("过滤器错误: {e}"), "");
                    continue;
                }
            };
        if !filtered.allow {
            log.push(LogKind::System, &sub.name, "🚫 事件已被过滤，跳过", "");
            continue;
        }

        let prompt = build_prompt(&sub.base_prompt, &filtered.event);
        let started_at = Utc::now();
        log.push(
            LogKind::System,
            &sub.name,
            "🤖 已提交 cc-connect，等待执行结果...",
            "",
        );

        let (success, output, error) = match bridge
            .ask(&session_key, &uid, &sub.name, &prompt)
            .await
        {
            Ok(content) => (true, content, None),
            Err(e) => (false, String::new(), Some(e.to_string())),
        };
        let finished_at = Utc::now();
        let secs = finished_at.signed_duration_since(started_at).num_seconds();

        if success {
            log.push(
                LogKind::Result,
                &sub.name,
                format!("执行完成（{secs}s）"),
                output.clone(),
            );
        } else {
            log.push(
                LogKind::Error,
                &sub.name,
                format!(
                    "执行失败（{secs}s）: {}",
                    error.as_deref().unwrap_or("未知错误")
                ),
                "",
            );
        }

        let result = ExecutionResult {
            subscription_name: sub.name.clone(),
            success,
            output,
            error,
            started_at,
            finished_at,
            prompt,
        };
        if let Err(e) = reporter.report(&result).await {
            log.push(LogKind::Error, &sub.name, format!("汇报失败: {e}"), "");
        }
    }
}

/// 解析 GUI 中的 Filter 字段：
/// - "none" / 空 → 无过滤
/// - "regex:<pattern>" → 正则过滤
/// - 其他 → WASM 策略插件名
fn parse_filter(raw: Option<&str>) -> (Option<String>, Option<String>) {
    match raw {
        None | Some("none") | Some("") => (None, None),
        Some(s) if s.starts_with("regex:") => {
            let pattern = &s["regex:".len()..];
            if pattern.is_empty() {
                (None, None)
            } else {
                (Some(pattern.to_string()), None)
            }
        }
        Some(s) => (None, Some(s.to_string())),
    }
}

/// 由订阅名生成 session_key 中的 scope/user 段。
/// 协议要求小写字母、数字、连字符；附加名称哈希避免中文名等清洗后冲突。
fn session_identity(name: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let base: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let base = base.trim_matches('-');

    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    let hash = hasher.finish() & 0xff_ffff;

    if base.is_empty() {
        format!("sub-{hash:06x}")
    } else {
        format!("{base}-{hash:06x}")
    }
}
