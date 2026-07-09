//! GUI 后台服务：smee 订阅 → 过滤 → 转发 cc-connect 执行 → 汇报结果
//!
//! 项目隔离模型：本应用以单一 platform（`agent-loop`）维持一条 Bridge 连接，
//! 每个订阅是其下的独立会话（session_key 互不相同，上下文隔离）；
//! cc-connect 按消息级 `project` 字段路由项目 engine（register 的 project
//! 字段被服务端忽略），因此每条 message / card_action 都盖上订阅绑定的项目。
//!
//! 在独立线程上运行 tokio runtime，GUI 通过 watch 通道下发停止信号、
//! 通过 std::sync::mpsc 接收日志。

use std::sync::mpsc::Sender as StdSender;

use chrono::Utc;
use tokio::sync::{mpsc, watch};

use std::collections::HashMap;

use crate::bridge_client::{BridgeClient, ConnStatus, SessionBinding, NEW_SESSION_ACTION};
use crate::event_log::{Log, LogEntry, LogKind};
use crate::executor::build_prompt;
use crate::filter::apply_filters;
use crate::gui_config::{AppConfig, Subscription};
use crate::reporter::get_reporter;
use crate::subscriber::run_sse_loop;
use crate::types::{ExecutionResult, SmeeEventData};

/// Bridge 协议中的平台名（组成 session_key 的第一段）
const PLATFORM: &str = "agent-loop";

/// GUI 下发给后台服务的控制指令
enum CtrlCmd {
    /// 清除指定会话的上下文（发送 `cmd:/new`，并重新下发权限模式）
    ClearSession { session_key: String },
    /// 向指定会话发送任意 card_action（如 `cmd:/model switch k2`）
    CardAction { session_key: String, action: String },
    /// 以用户身份向订阅会话推送一段话（走正常 message → reply 执行流程）
    Message { sub_name: String, content: String },
}

pub struct BridgeService {
    stop_tx: watch::Sender<bool>,
    ctrl_tx: mpsc::UnboundedSender<CtrlCmd>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl BridgeService {
    pub fn start(config: AppConfig, log_tx: StdSender<LogEntry>, conn_status: ConnStatus) -> Self {
        let (stop_tx, stop_rx) = watch::channel(false);
        let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel();
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
            rt.block_on(run_service(config, log, stop_rx, ctrl_rx, conn_status));
            // rt 在此被丢弃，所有后台任务（SSE、Bridge 连接）随之结束
        });
        Self {
            stop_tx,
            ctrl_tx,
            thread: Some(thread),
        }
    }

    /// 订阅对应的 Bridge session key（稳定值：同一订阅名总是同一会话，保留上下文）
    pub fn session_key_for(sub_name: &str) -> String {
        format!("{PLATFORM}:{0}:{0}", session_identity(sub_name))
    }

    /// 请求清除某个会话的上下文（cc-connect 侧执行 `/new` 开启新会话）
    pub fn clear_session(&self, session_key: &str) {
        let _ = self.ctrl_tx.send(CtrlCmd::ClearSession {
            session_key: session_key.to_string(),
        });
    }

    /// 向某个会话发送任意指令（card_action），回执以 📩 推送记入执行记录
    pub fn send_command(&self, session_key: &str, action: &str) {
        let _ = self.ctrl_tx.send(CtrlCmd::CardAction {
            session_key: session_key.to_string(),
            action: action.to_string(),
        });
    }

    /// 以用户身份向订阅会话推送一段话，执行结果记入执行记录
    pub fn send_message(&self, sub_name: &str, content: &str) {
        let _ = self.ctrl_tx.send(CtrlCmd::Message {
            sub_name: sub_name.to_string(),
            content: content.to_string(),
        });
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
    mut ctrl_rx: mpsc::UnboundedReceiver<CtrlCmd>,
    conn_status: ConnStatus,
) {
    if config.bridge_token.trim().is_empty() {
        log.system("⚠️ 未配置 Bridge Token，cc-connect 可能拒绝连接（HTTP 401）");
    }

    let enabled: Vec<Subscription> = config
        .subscriptions
        .into_iter()
        .filter(|s| s.enabled)
        .collect();
    if enabled.is_empty() {
        log.system("⚠️ 没有启用的订阅，仅维持与 cc-connect 的连接");
    }

    // 会话与项目的绑定：该会话所有出站消息按此路由。
    // 权限模式由 cc-connect 项目配置（mode = "bypassPermissions"）决定，无需下发
    let sessions: Vec<SessionBinding> = enabled
        .iter()
        .map(|s| SessionBinding {
            session_key: BridgeService::session_key_for(&s.name),
            project: project_of(s),
        })
        .collect();
    // GUI 控制指令按 session_key / 订阅名找回项目
    let project_by_session: HashMap<String, Option<String>> = sessions
        .iter()
        .map(|s| (s.session_key.clone(), s.project.clone()))
        .collect();
    let project_by_name: HashMap<String, Option<String>> = enabled
        .iter()
        .map(|s| (s.name.clone(), project_of(s)))
        .collect();

    let bridge = BridgeClient::spawn(
        config.ws_url.clone(),
        config.bridge_token.trim().to_string(),
        PLATFORM.to_string(),
        sessions,
        log.clone(),
        conn_status,
    );

    for sub in enabled {
        tokio::spawn(run_subscription(sub, bridge.clone(), log.clone()));
    }

    // GUI 控制指令（清除会话上下文等）
    let ctrl_bridge = bridge.clone();
    let ctrl_log = log.clone();
    tokio::spawn(async move {
        while let Some(cmd) = ctrl_rx.recv().await {
            match cmd {
                CtrlCmd::ClearSession { session_key } => {
                    let project = project_by_session.get(&session_key).cloned().flatten();
                    ctrl_log.push_for(
                        LogKind::System,
                        "系统",
                        session_key.clone(),
                        "🧹 请求清除会话上下文（/new）",
                        "",
                    );
                    // 权限模式是项目级配置，新会话自动继承，无需重新下发
                    ctrl_bridge.card_action(&session_key, project.as_deref(), NEW_SESSION_ACTION);
                }
                CtrlCmd::CardAction { session_key, action } => {
                    let project = project_by_session.get(&session_key).cloned().flatten();
                    ctrl_bridge.card_action(&session_key, project.as_deref(), &action);
                }
                CtrlCmd::Message { sub_name, content } => {
                    let project = project_by_name.get(&sub_name).cloned().flatten();
                    let uid = session_identity(&sub_name);
                    let session_key = BridgeService::session_key_for(&sub_name);
                    ctrl_log.push_for(
                        LogKind::System,
                        &sub_name,
                        &session_key,
                        "📤 已推送手动消息，等待执行结果...",
                        content.clone(),
                    );
                    // 等待回复可能很久，独立任务执行，不阻塞后续控制指令
                    let bridge = ctrl_bridge.clone();
                    let log = ctrl_log.clone();
                    tokio::spawn(async move {
                        let started_at = Utc::now();
                        let result = bridge
                            .ask(&session_key, project.as_deref(), &uid, &sub_name, &content)
                            .await;
                        let secs = Utc::now().signed_duration_since(started_at).num_seconds();
                        match result {
                            Ok(output) => log.push_for(
                                LogKind::Result,
                                &sub_name,
                                &session_key,
                                format!("手动消息执行完成（{secs}s）"),
                                output,
                            ),
                            Err(e) => log.push_for(
                                LogKind::Error,
                                &sub_name,
                                &session_key,
                                format!("手动消息执行失败（{secs}s）: {e}"),
                                "",
                            ),
                        }
                    });
                }
            }
        }
    });

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
    let sse_log = log.clone();
    tokio::spawn(async move { run_sse_loop(&smee_url, tx, sse_log).await });
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
    // 同一订阅固定使用同一 session key，cc-connect 侧保留对话上下文
    let session_key = BridgeService::session_key_for(&sub.name);
    // 订阅绑定的项目，盖在该会话所有出站消息上（None = 默认项目）
    let project = project_of(&sub);

    while let Some(event) = rx.recv().await {
        // 展示 Webhook 原始数据（headers / query / body）
        let event_json = serde_json::to_string_pretty(&event).unwrap_or_default();
        log.push_for(
            LogKind::Webhook,
            &sub.name,
            &session_key,
            "收到 Webhook 事件",
            event_json,
        );

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

        // 提交前先查询会话基础信息（工作目录 / 当前会话），
        // 回执由 cc-connect 以 📩 推送形式记入执行记录
        for cmd in ["cmd:/dir", "cmd:/current"] {
            bridge.card_action(&session_key, project.as_deref(), cmd);
        }

        let prompt = build_prompt(&sub.base_prompt, &filtered.event);
        let started_at = Utc::now();
        log.push_for(
            LogKind::System,
            &sub.name,
            &session_key,
            "🤖 已提交 cc-connect，等待执行结果...",
            prompt.clone(),
        );

        let (success, output, error) = match bridge
            .ask(&session_key, project.as_deref(), &uid, &sub.name, &prompt)
            .await
        {
            Ok(content) => (true, content, None),
            Err(e) => (false, String::new(), Some(e.to_string())),
        };
        let finished_at = Utc::now();
        let secs = finished_at.signed_duration_since(started_at).num_seconds();

        if success {
            log.push_for(
                LogKind::Result,
                &sub.name,
                &session_key,
                format!("执行完成（{secs}s）"),
                output.clone(),
            );
        } else {
            log.push_for(
                LogKind::Error,
                &sub.name,
                &session_key,
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

/// 订阅绑定的项目名（空白 = 不指定，路由到 cc-connect 默认项目）
fn project_of(sub: &Subscription) -> Option<String> {
    let p = sub.project.trim();
    if p.is_empty() {
        None
    } else {
        Some(p.to_string())
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
