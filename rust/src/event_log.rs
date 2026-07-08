//! 结构化运行日志：GUI「执行记录」面板的数据模型
//!
//! 后台服务/Bridge 客户端通过 [`Log`] 写入条目，GUI 侧按类型筛选渲染。

use chrono::{DateTime, Local};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    /// 连接、服务状态等系统消息
    System,
    /// 收到的 Webhook 事件数据
    Webhook,
    /// cc-connect 返回的执行结果
    Result,
    /// 错误（始终展示，不受筛选影响）
    Error,
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub time: DateTime<Local>,
    pub kind: LogKind,
    /// 来源：订阅名、"cc-connect" 或 "系统"
    pub source: String,
    /// 关联的 Bridge 会话（`{platform}:{scope}:{user}`），无关联时为空
    pub session_key: String,
    /// 单行摘要
    pub summary: String,
    /// 可折叠详情（Webhook 原始数据 / 完整执行结果），可为空
    pub detail: String,
}

/// 同时写入终端和 GUI 执行记录面板的日志器
#[derive(Clone)]
pub struct Log(std::sync::mpsc::Sender<LogEntry>);

impl Log {
    pub fn new(tx: std::sync::mpsc::Sender<LogEntry>) -> Self {
        Self(tx)
    }

    pub fn push(
        &self,
        kind: LogKind,
        source: impl Into<String>,
        summary: impl Into<String>,
        detail: impl Into<String>,
    ) {
        self.push_for(kind, source, "", summary, detail);
    }

    /// 携带会话标识的日志条目（GUI 中展示 session key）
    pub fn push_for(
        &self,
        kind: LogKind,
        source: impl Into<String>,
        session_key: impl Into<String>,
        summary: impl Into<String>,
        detail: impl Into<String>,
    ) {
        let entry = LogEntry {
            time: Local::now(),
            kind,
            source: source.into(),
            session_key: session_key.into(),
            summary: summary.into(),
            detail: detail.into(),
        };
        println!(
            "[{}] [{}] {}",
            entry.time.format("%H:%M:%S"),
            entry.source,
            entry.summary
        );
        let _ = self.0.send(entry);
    }

    /// 系统消息的快捷方式
    pub fn system(&self, summary: impl Into<String>) {
        self.push(LogKind::System, "系统", summary, "");
    }
}
