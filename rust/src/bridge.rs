#![allow(dead_code)]

//! Bridge 平台协议消息定义（见 bridge-protocol.zh-CN.md）
//!
//! 本适配器只声明 `text` + `buttons` 能力：
//! - 不声明 `preview` / `typing` → cc-connect 不会推送执行过程（流式 / 输入指示器），
//!   只发送最终 `reply`，满足「只要执行结果」的需求。
//! - 声明 `buttons` 是为了能收到权限确认按钮并自动点「允许」（bypassPermissions 兜底）。

use serde::{Deserialize, Serialize};

/// 适配器 → cc-connect
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Outbound {
    Register {
        platform: String,
        capabilities: Vec<String>,
        metadata: serde_json::Value,
    },
    Message {
        msg_id: String,
        session_key: String,
        user_id: String,
        user_name: String,
        content: String,
        reply_ctx: String,
    },
    CardAction {
        session_key: String,
        action: String,
        reply_ctx: String,
    },
    Ping {
        ts: u64,
    },
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct BridgeButton {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub data: String,
}

/// cc-connect → 适配器
///
/// 未声明能力对应的消息类型（reply_stream、card、typing_* 等）统一落入 `Other`，
/// 按协议要求「记录错误而非崩溃」直接忽略。
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Inbound {
    RegisterAck {
        ok: bool,
        #[serde(default)]
        error: String,
    },
    Reply {
        #[serde(default)]
        session_key: String,
        #[serde(default)]
        reply_ctx: String,
        #[serde(default)]
        content: String,
    },
    Buttons {
        #[serde(default)]
        session_key: String,
        #[serde(default)]
        reply_ctx: String,
        #[serde(default)]
        content: String,
        #[serde(default)]
        buttons: Vec<Vec<BridgeButton>>,
    },
    Error {
        #[serde(default)]
        code: String,
        #[serde(default)]
        message: String,
    },
    Pong {
        #[serde(default)]
        ts: u64,
    },
    #[serde(other)]
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn outbound_uses_protocol_type_tags() {
        let register = Outbound::Register {
            platform: "agent-loop".into(),
            capabilities: vec!["text".into(), "buttons".into()],
            metadata: json!({"protocol_version": 1}),
        };
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&register).unwrap()).unwrap();
        assert_eq!(v["type"], "register");
        assert_eq!(v["platform"], "agent-loop");
        assert_eq!(v["capabilities"][1], "buttons");

        let action = Outbound::CardAction {
            session_key: "agent-loop:a:a".into(),
            action: "perm:req-1:allow".into(),
            reply_ctx: "ctx-1".into(),
        };
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&action).unwrap()).unwrap();
        assert_eq!(v["type"], "card_action");

        let ping = Outbound::Ping { ts: 123 };
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&ping).unwrap()).unwrap();
        assert_eq!(v["type"], "ping");
        assert_eq!(v["ts"], 123);
    }

    #[test]
    fn inbound_parses_protocol_messages() {
        let ack: Inbound =
            serde_json::from_str(r#"{"type":"register_ack","ok":true,"error":""}"#).unwrap();
        assert!(matches!(ack, Inbound::RegisterAck { ok: true, .. }));

        let reply: Inbound = serde_json::from_str(
            r#"{"type":"reply","session_key":"a:b:c","reply_ctx":"x","content":"done","format":"text"}"#,
        )
        .unwrap();
        match reply {
            Inbound::Reply { reply_ctx, content, .. } => {
                assert_eq!(reply_ctx, "x");
                assert_eq!(content, "done");
            }
            other => panic!("unexpected: {other:?}"),
        }

        let buttons: Inbound = serde_json::from_str(
            r#"{"type":"buttons","session_key":"a:b:c","reply_ctx":"x","content":"允许？",
                "buttons":[[{"text":"✅ 允许","data":"perm:req-123:allow"}]]}"#,
        )
        .unwrap();
        match buttons {
            Inbound::Buttons { buttons, .. } => {
                assert_eq!(buttons[0][0].data, "perm:req-123:allow");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn unknown_inbound_types_fall_into_other() {
        // 未声明 preview 能力时不应收到这些消息，即便收到也要安全忽略
        for raw in [
            r#"{"type":"reply_stream","delta":"...","done":false}"#,
            r#"{"type":"typing_start","session_key":"a:b:c"}"#,
            r#"{"type":"card","session_key":"a:b:c","card":{}}"#,
        ] {
            let msg: Inbound = serde_json::from_str(raw).unwrap();
            assert!(matches!(msg, Inbound::Other), "should ignore: {raw}");
        }
    }
}
