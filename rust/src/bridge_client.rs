#![allow(dead_code)]

//! cc-connect Bridge WebSocket 客户端
//!
//! 按 bridge-protocol.zh-CN.md 实现：
//! - token 认证（URL 查询参数 `?token=...`）
//! - `register` / `register_ack` 握手
//! - 每 30 秒发送 `ping` 心跳
//! - 断线指数退避重连（1s 起步，最大 60s）
//! - 只声明 `text` / `buttons` 能力：不接收执行过程，只等待最终 `reply`
//! - 收到权限确认按钮时自动回复「允许」（权限模式由 cc-connect 项目配置
//!   `mode = "bypassPermissions"` 决定，这里只是兜底）

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

use crate::bridge::{BridgeButton, Inbound, Outbound};
use crate::event_log::{Log, LogKind};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;
type PendingMap = HashMap<String, oneshot::Sender<Result<String>>>;

/// 等待 cc-connect 最终回复的上限（agent 任务可能耗时很长）
const REPLY_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// 协议建议的心跳间隔
const PING_INTERVAL: Duration = Duration::from_secs(30);
const REGISTER_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BACKOFF_SECS: u64 = 60;

/// 清空会话上下文、开启新会话的引擎命令
pub const NEW_SESSION_ACTION: &str = "cmd:/new";

/// 会话与 cc-connect 项目的绑定关系
#[derive(Debug, Clone)]
pub struct SessionBinding {
    pub session_key: String,
    /// 会话绑定的项目（盖在该会话所有出站消息上；None = 默认项目）
    pub project: Option<String>,
}

/// 与 cc-connect 的连接状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Disconnected = 0,
    Connecting = 1,
    Connected = 2,
}

/// GUI 与后台连接任务共享的状态标记
#[derive(Clone, Default)]
pub struct ConnStatus(Arc<AtomicU8>);

impl ConnStatus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, state: ConnState) {
        self.0.store(state as u8, Ordering::Relaxed);
    }

    pub fn get(&self) -> ConnState {
        match self.0.load(Ordering::Relaxed) {
            2 => ConnState::Connected,
            1 => ConnState::Connecting,
            _ => ConnState::Disconnected,
        }
    }
}

struct AskCmd {
    session_key: String,
    project: Option<String>,
    user_id: String,
    user_name: String,
    content: String,
    respond: oneshot::Sender<Result<String>>,
}

/// GUI/服务下发给连接 actor 的指令
enum Cmd {
    /// 发送 `message` 并等待最终 `reply`
    Ask(AskCmd),
    /// 发送 `card_action`（如 `cmd:/new`、`cmd:/mode bypassPermissions`），不等待回复
    CardAction {
        session_key: String,
        project: Option<String>,
        action: String,
    },
}

/// Bridge 客户端句柄。可跨任务 Clone，内部由单个 actor 管理连接。
#[derive(Clone)]
pub struct BridgeClient {
    cmd_tx: mpsc::UnboundedSender<Cmd>,
}

impl BridgeClient {
    /// 在当前 tokio runtime 上启动客户端 actor（自动重连，直到 runtime 结束）。
    ///
    /// `sessions`：会话与项目的绑定关系，用于服务端主动下行（如权限按钮）
    /// 需要回 card_action 时补上正确的 project。
    pub fn spawn(
        ws_url: String,
        token: String,
        platform: String,
        sessions: Vec<SessionBinding>,
        log: Log,
        status: ConnStatus,
    ) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        tokio::spawn(run_actor(
            ws_url,
            token,
            platform,
            sessions,
            cmd_rx,
            log,
            status,
        ));
        Self { cmd_tx }
    }

    /// 发送一条 `message` 给 cc-connect 并等待最终 `reply`。
    /// `project`：目标项目（cc-connect 按消息级 project 路由 engine）。
    pub async fn ask(
        &self,
        session_key: &str,
        project: Option<&str>,
        user_id: &str,
        user_name: &str,
        content: &str,
    ) -> Result<String> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(Cmd::Ask(AskCmd {
                session_key: session_key.to_string(),
                project: project.map(str::to_string),
                user_id: user_id.to_string(),
                user_name: user_name.to_string(),
                content: content.to_string(),
                respond: tx,
            }))
            .map_err(|_| anyhow!("Bridge 客户端已停止"))?;

        match tokio::time::timeout(REPLY_TIMEOUT, rx).await {
            Err(_) => Err(anyhow!("等待 cc-connect 回复超时（30 分钟）")),
            Ok(Err(_)) => Err(anyhow!("连接中断，未收到回复")),
            Ok(Ok(result)) => result,
        }
    }

    /// 发送 `card_action`（fire-and-forget）。连接断开时指令会排队，重连后发出。
    pub fn card_action(&self, session_key: &str, project: Option<&str>, action: &str) {
        let _ = self.cmd_tx.send(Cmd::CardAction {
            session_key: session_key.to_string(),
            project: project.map(str::to_string),
            action: action.to_string(),
        });
    }
}

/// 首次连接前等待 WS 端口就绪：cc-connect 可能刚被本应用拉起，
/// 直接连必然吃一次 Connection refused。端口已在监听则立即返回。
async fn wait_for_port(ws_url: &str, log: &Log) {
    let Some((host, port)) = url::Url::parse(ws_url)
        .ok()
        .and_then(|u| Some((u.host_str()?.to_string(), u.port_or_known_default()?)))
    else {
        return; // 地址解析失败交由正常连接流程报错
    };

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut logged = false;
    while tokio::net::TcpStream::connect((host.as_str(), port)).await.is_err() {
        if !logged {
            log.system(format!("⏳ 等待 cc-connect 端口就绪（{host}:{port}）..."));
            logged = true;
        }
        if tokio::time::Instant::now() >= deadline {
            log.system("⚠️ 等待端口就绪超时，转入正常重连流程");
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// 连接管理主循环：连接 → 注册 → 服务，断开后指数退避重连
async fn run_actor(
    ws_url: String,
    token: String,
    platform: String,
    sessions: Vec<SessionBinding>,
    mut cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    log: Log,
    status: ConnStatus,
) {
    wait_for_port(&ws_url, &log).await;
    let mut backoff = 1u64;
    loop {
        status.set(ConnState::Connecting);
        log.system(format!("🔌 正在连接 cc-connect: {ws_url}"));
        match connect_and_register(&ws_url, &token, &platform).await {
            Ok(ws) => {
                status.set(ConnState::Connected);
                log.system(format!("✅ 已注册到 cc-connect（platform: {platform}）"));
                backoff = 1;
                let served = serve_connection(ws, &sessions, &mut cmd_rx, &log).await;
                status.set(ConnState::Disconnected);
                match served {
                    Ok(()) => {
                        log.system("🔌 Bridge 客户端已停止");
                        return;
                    }
                    Err(e) => log.push(
                        LogKind::Error,
                        "cc-connect",
                        format!("连接中断: {e}"),
                        "",
                    ),
                }
            }
            Err(e) => {
                status.set(ConnState::Disconnected);
                log.push(LogKind::Error, "cc-connect", format!("连接失败: {e}"), "");
            }
        }

        log.system(format!("⏳ {backoff} 秒后重连..."));
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(MAX_BACKOFF_SECS);
    }
}

/// 建立 WebSocket 连接（携带 token）并完成 register 握手
async fn connect_and_register(ws_url: &str, token: &str, platform: &str) -> Result<WsStream> {
    let mut url =
        url::Url::parse(ws_url).with_context(|| format!("WS 地址无效: {ws_url}"))?;
    if !token.is_empty() {
        url.query_pairs_mut().append_pair("token", token);
    }

    let (mut ws, _) = connect_async(url.as_str())
        .await
        .map_err(|e| anyhow!("WebSocket 连接失败: {e}"))?;

    let register = Outbound::Register {
        platform: platform.to_string(),
        // 只声明 text/buttons：不接收流式执行过程，只要最终结果；
        // buttons 用于自动允许权限确认。
        capabilities: vec!["text".to_string(), "buttons".to_string()],
        // metadata 仅用于日志/调试，权限模式在注册后通过 card_action `/mode` 设置
        metadata: json!({
            "protocol_version": 1,
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Agent Loop Bridge 适配器",
        }),
    };
    ws.send(Message::Text(serde_json::to_string(&register)?))
        .await
        .map_err(|e| anyhow!("发送 register 失败: {e}"))?;

    let deadline = tokio::time::Instant::now() + REGISTER_TIMEOUT;
    loop {
        let msg = tokio::time::timeout_at(deadline, ws.next())
            .await
            .map_err(|_| anyhow!("等待 register_ack 超时"))?
            .ok_or_else(|| anyhow!("连接在注册期间被关闭"))?
            .map_err(|e| anyhow!("读取 register_ack 失败: {e}"))?;

        let Message::Text(text) = msg else { continue };
        match serde_json::from_str::<Inbound>(&text) {
            Ok(Inbound::RegisterAck { ok: true, .. }) => return Ok(ws),
            Ok(Inbound::RegisterAck { ok: false, error }) => bail!("注册被拒绝: {error}"),
            _ => continue,
        }
    }
}

/// 发送一条 card_action（reply_ctx 随机生成，回复按未匹配推送记入日志）
async fn send_card_action(
    ws: &mut WsStream,
    project: Option<&str>,
    session_key: &str,
    action: &str,
) -> Result<()> {
    let msg = Outbound::CardAction {
        session_key: session_key.to_string(),
        action: action.to_string(),
        reply_ctx: Uuid::new_v4().to_string(),
        project: project.map(str::to_string),
    };
    ws.send(Message::Text(serde_json::to_string(&msg)?))
        .await
        .map_err(|e| anyhow!("发送 card_action 失败: {e}"))
}

/// 单条连接的消息收发循环。返回 Ok(()) 表示客户端被要求停止，Err 表示连接断开需重连。
async fn serve_connection(
    mut ws: WsStream,
    sessions: &[SessionBinding],
    cmd_rx: &mut mpsc::UnboundedReceiver<Cmd>,
    log: &Log,
) -> Result<()> {
    // 会话 → 项目映射：服务端主动下行（如权限按钮）需要回 card_action 时，
    // 按会话补上 project，保证路由到正确的 engine
    let session_projects: HashMap<String, Option<String>> = sessions
        .iter()
        .map(|s| (s.session_key.clone(), s.project.clone()))
        .collect();

    let mut pending: PendingMap = HashMap::new();
    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping.tick().await; // interval 首次立即触发，跳过

    let result: Result<()> = loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                match cmd {
                    None => break Ok(()),
                    Some(Cmd::CardAction { session_key, project, action }) => {
                        if let Err(e) =
                            send_card_action(&mut ws, project.as_deref(), &session_key, &action)
                                .await
                        {
                            break Err(e);
                        }
                        log.push_for(
                            LogKind::System,
                            "cc-connect",
                            session_key,
                            format!("📤 已发送 card_action: {action}"),
                            "",
                        );
                    }
                    Some(Cmd::Ask(cmd)) => {
                        let reply_ctx = Uuid::new_v4().to_string();
                        let msg = Outbound::Message {
                            msg_id: Uuid::new_v4().to_string(),
                            session_key: cmd.session_key,
                            user_id: cmd.user_id,
                            user_name: cmd.user_name,
                            content: cmd.content,
                            reply_ctx: reply_ctx.clone(),
                            project: cmd.project,
                        };
                        match serde_json::to_string(&msg) {
                            Ok(json) => {
                                if let Err(e) = ws.send(Message::Text(json)).await {
                                    let _ = cmd.respond.send(Err(anyhow!("消息发送失败: {e}")));
                                    break Err(anyhow!("WebSocket 发送失败: {e}"));
                                }
                                pending.insert(reply_ctx, cmd.respond);
                            }
                            Err(e) => {
                                let _ = cmd.respond.send(Err(anyhow!("消息序列化失败: {e}")));
                            }
                        }
                    }
                }
            }
            msg = ws.next() => {
                match msg {
                    None | Some(Ok(Message::Close(_))) => break Err(anyhow!("连接被服务端关闭")),
                    Some(Err(e)) => break Err(anyhow!("WebSocket 读取错误: {e}")),
                    Some(Ok(Message::Text(text))) => {
                        if let Err(e) =
                            handle_inbound(&text, &mut ws, &session_projects, &mut pending, log)
                                .await
                        {
                            break Err(e);
                        }
                    }
                    Some(Ok(_)) => {} // 二进制帧 / ping / pong 由底层处理
                }
            }
            _ = ping.tick() => {
                let ping_msg = Outbound::Ping {
                    ts: chrono::Utc::now().timestamp_millis() as u64,
                };
                let json = serde_json::to_string(&ping_msg).unwrap_or_default();
                if let Err(e) = ws.send(Message::Text(json)).await {
                    break Err(anyhow!("心跳发送失败: {e}"));
                }
            }
        }
    };

    // 连接结束：所有等待中的请求立即返回错误（协议规定回复无法跨连接路由）
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(anyhow!("与 cc-connect 的连接已断开")));
    }
    result
}

/// 分发 cc-connect 下行消息
async fn handle_inbound(
    text: &str,
    ws: &mut WsStream,
    session_projects: &HashMap<String, Option<String>>,
    pending: &mut PendingMap,
    log: &Log,
) -> Result<()> {
    let inbound: Inbound = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => {
            log.push(
                LogKind::Error,
                "cc-connect",
                "收到无法解析的消息",
                truncate(text, 2000),
            );
            return Ok(());
        }
    };

    match inbound {
        Inbound::Reply { session_key, reply_ctx, content } => {
            // cc-connect 会把 Claude 的思考过程以 💭 前缀的 reply 提前推送，
            // 且与最终回复共用 reply_ctx——只记录，不当作执行结果，继续等待
            if content.trim_start().starts_with("💭") {
                let summary = format!("💭 思考过程: {}", truncate(content.trim_start(), 120));
                log.push_for(LogKind::System, "cc-connect", session_key, summary, content);
            } else if let Some(tx) = pending.remove(&reply_ctx) {
                let _ = tx.send(Ok(content));
            } else {
                // 主动推送（命令回执、心跳/定时任务消息等），仅记录
                let summary = format!("📩 {}", truncate(&content, 120));
                log.push_for(LogKind::System, "cc-connect", session_key, summary, content);
            }
        }
        Inbound::Buttons { session_key, reply_ctx, content, buttons } => {
            // bypassPermissions 兜底：自动点击「允许」类按钮
            if let Some(action) = pick_allow_action(&buttons) {
                log.push_for(
                    LogKind::System,
                    "cc-connect",
                    session_key.clone(),
                    format!("🔓 自动允许权限请求: {}", truncate(&content, 120)),
                    "",
                );
                let project = session_projects.get(&session_key).cloned().flatten();
                let msg = Outbound::CardAction {
                    session_key,
                    action,
                    reply_ctx,
                    project,
                };
                ws.send(Message::Text(serde_json::to_string(&msg)?))
                    .await
                    .map_err(|e| anyhow!("发送 card_action 失败: {e}"))?;
            } else {
                log.push_for(
                    LogKind::System,
                    "cc-connect",
                    session_key,
                    format!(
                        "⚠️ 收到无法自动处理的按钮消息，已忽略: {}",
                        truncate(&content, 120)
                    ),
                    "",
                );
            }
        }
        Inbound::Error { code, message } => {
            log.push(LogKind::Error, "cc-connect", format!("[{code}] {message}"), "");
        }
        Inbound::RegisterAck { .. } | Inbound::Pong { .. } | Inbound::Other => {}
    }
    Ok(())
}

/// 从按钮中挑出「允许」按钮的回调值
fn pick_allow_action(buttons: &[Vec<BridgeButton>]) -> Option<String> {
    for row in buttons {
        for btn in row {
            let data_says_allow = btn
                .data
                .to_lowercase()
                .split(':')
                .any(|seg| matches!(seg, "allow" | "approve" | "yes" | "accept"));
            let text_says_allow = btn.text.contains("允许")
                || btn.text.contains("同意")
                || btn.text.to_lowercase().contains("allow");
            if data_says_allow || text_says_allow {
                return Some(btn.data.clone());
            }
        }
    }
    None
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::handshake::server::{
        Request, Response as HsResponse,
    };

    /// 端到端：token 认证 → register → message（带 project）→
    /// 权限按钮自动允许 → 最终 reply → card_action /new 清除上下文
    #[tokio::test]
    async fn ask_round_trip_with_mock_cc_connect() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen_uri = Arc::new(Mutex::new(String::new()));

        let uri_capture = seen_uri.clone();
        let server = tokio::spawn(async move {
            // 客户端会先用 TCP 探测端口就绪（wait_for_port），该连接不做 WS 握手
            // 即断开——循环 accept 直到收到真正的 WebSocket 握手
            let mut ws = loop {
                let (stream, _) = listener.accept().await.unwrap();
                let capture = uri_capture.clone();
                match tokio_tungstenite::accept_hdr_async(
                    stream,
                    move |req: &Request, resp: HsResponse| {
                        *capture.lock().unwrap() = req.uri().to_string();
                        Ok(resp)
                    },
                )
                .await
                {
                    Ok(ws) => break ws,
                    Err(_) => continue,
                }
            };

            // 1. register：校验能力（register 不携带 project，路由靠消息级 project）
            let raw = ws.next().await.unwrap().unwrap().into_text().unwrap();
            let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(v["type"], "register");
            assert_eq!(v["platform"], "agent-loop");
            assert!(v.get("project").is_none());
            assert_eq!(v["metadata"]["protocol_version"], 1);
            let caps: Vec<&str> = v["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c.as_str().unwrap())
                .collect();
            assert!(caps.contains(&"text") && caps.contains(&"buttons"));
            // 不声明 preview/typing → 不会收到执行过程
            assert!(!caps.contains(&"preview") && !caps.contains(&"typing"));
            ws.send(Message::Text(
                r#"{"type":"register_ack","ok":true,"error":""}"#.to_string(),
            ))
            .await
            .unwrap();

            // 2. message：携带消息级 project（cc-connect 按此路由 engine）；
            //    权限模式由项目配置决定，注册后不应有任何初始化 card_action
            let raw = ws.next().await.unwrap().unwrap().into_text().unwrap();
            let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(v["type"], "message");
            assert_eq!(v["content"], "hello");
            assert_eq!(v["project"], "demo-project");
            let session_key = v["session_key"].as_str().unwrap().to_string();
            let reply_ctx = v["reply_ctx"].as_str().unwrap().to_string();

            // 3. 权限确认按钮 → 客户端应自动回 card_action 允许
            ws.send(Message::Text(
                serde_json::json!({
                    "type": "buttons",
                    "session_key": session_key,
                    "reply_ctx": reply_ctx,
                    "content": "允许执行工具：bash(ls)？",
                    "buttons": [[
                        {"text": "✅ 允许", "data": "perm:req-1:allow"},
                        {"text": "❌ 拒绝", "data": "perm:req-1:deny"}
                    ]]
                })
                .to_string(),
            ))
            .await
            .unwrap();
            let raw = ws.next().await.unwrap().unwrap().into_text().unwrap();
            let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(v["type"], "card_action");
            assert_eq!(v["action"], "perm:req-1:allow");

            // 4. 思考过程（💭 前缀、共用 reply_ctx）→ 客户端应忽略并继续等待
            ws.send(Message::Text(
                serde_json::json!({
                    "type": "reply",
                    "session_key": session_key,
                    "reply_ctx": reply_ctx,
                    "content": "💭 用户想要……我直接回复即可。",
                    "format": "text"
                })
                .to_string(),
            ))
            .await
            .unwrap();

            // 5. 最终结果
            ws.send(Message::Text(
                serde_json::json!({
                    "type": "reply",
                    "session_key": session_key,
                    "reply_ctx": reply_ctx,
                    "content": "任务完成",
                    "format": "text"
                })
                .to_string(),
            ))
            .await
            .unwrap();

            // 6. 清除上下文 → 应收到 card_action cmd:/new（同样带 project）
            let raw = ws.next().await.unwrap().unwrap().into_text().unwrap();
            let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(v["type"], "card_action");
            assert_eq!(v["action"], "cmd:/new");
            assert_eq!(v["project"], "demo-project");
        });

        let (log_tx, _log_rx) = std::sync::mpsc::channel();
        let status = ConnStatus::new();
        let client = BridgeClient::spawn(
            format!("ws://{addr}/bridge/ws"),
            "secret-token".to_string(),
            "agent-loop".to_string(),
            vec![SessionBinding {
                session_key: "agent-loop:t-abc:t-abc".to_string(),
                project: Some("demo-project".to_string()),
            }],
            Log::new(log_tx),
            status.clone(),
        );

        let result = client
            .ask(
                "agent-loop:t-abc:t-abc",
                Some("demo-project"),
                "t-abc",
                "test",
                "hello",
            )
            .await
            .unwrap();
        assert_eq!(result, "任务完成");
        client.card_action(
            "agent-loop:t-abc:t-abc",
            Some("demo-project"),
            NEW_SESSION_ACTION,
        );
        // 收到回复时连接必然已建立过
        assert_ne!(status.get(), ConnState::Connecting);

        server.await.unwrap();
        assert!(
            seen_uri.lock().unwrap().contains("token=secret-token"),
            "连接 URL 应携带 token 查询参数"
        );
    }
}
