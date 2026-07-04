use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClaudeRequest {
    #[serde(rename = "type")]
    pub message_type: String,
    pub conversation_id: String,
    pub message: ChatMessage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClaudeResponse {
    #[serde(rename = "type")]
    pub message_type: String,
    pub conversation_id: String,
    #[serde(default)]
    pub delta: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub error: Option<String>,
}

pub struct WsClient {
    ws_stream: WsStream,
    conversation_id: String,
}

impl WsClient {
    pub async fn connect(ws_url: &str) -> Result<Self> {
        let (ws_stream, _) = connect_async(ws_url)
            .await
            .map_err(|e| anyhow!("WebSocket 连接失败: {}", e))?;

        let conversation_id = Uuid::new_v4().to_string();

        Ok(Self {
            ws_stream,
            conversation_id,
        })
    }

    pub async fn send_message(
        &mut self,
        content: &str,
        model: Option<String>,
        temperature: Option<f32>,
    ) -> Result<String> {
        let request = ClaudeRequest {
            message_type: "message".to_string(),
            conversation_id: self.conversation_id.clone(),
            message: ChatMessage {
                role: "user".to_string(),
                content: content.to_string(),
            },
            model,
            temperature,
        };

        let json = serde_json::to_string(&request)?;
        self.ws_stream.send(Message::Text(json)).await?;

        let mut full_response = String::new();

        while let Some(msg) = self.ws_stream.next().await {
            match msg? {
                Message::Text(text) => {
                    let response: ClaudeResponse = serde_json::from_str(&text)?;

                    if let Some(err) = response.error {
                        return Err(anyhow!("Claude API 错误: {}", err));
                    }

                    match response.message_type.as_str() {
                        "delta" => {
                            full_response.push_str(&response.delta);
                        }
                        "message_complete" => {
                            if !response.content.is_empty() {
                                full_response = response.content;
                            }
                            break;
                        }
                        "error" => {
                            return Err(anyhow!("服务端错误: {}", response.delta));
                        }
                        _ => {}
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }

        Ok(full_response)
    }

    pub async fn close(mut self) -> Result<()> {
        self.ws_stream.close(None).await?;
        Ok(())
    }
}
