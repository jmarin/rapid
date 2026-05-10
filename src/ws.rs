use axum::{
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use std::time::Duration;

use crate::AppState;

const WS_IDLE_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes
const WS_PING_INTERVAL: Duration = Duration::from_secs(30);

/// Messages the client sends over the WebSocket.
#[derive(Deserialize)]
#[serde(tag = "type")]
pub enum WsCommand {
    #[serde(rename = "subscribe")]
    Subscribe { upload_id: String },
}

/// Progress events the server pushes to the client over the WebSocket.
#[derive(Clone, Serialize)]
#[serde(tag = "type")]
pub enum UploadEvent {
    #[serde(rename = "subscribed")]
    Subscribed { upload_id: String },

    #[serde(rename = "upload_started")]
    UploadStarted { upload_id: String, total_parts: u64 },

    #[serde(rename = "part_completed")]
    PartCompleted {
        upload_id: String,
        part_number: i32,
        total_parts: u64,
    },

    #[serde(rename = "upload_completed")]
    UploadCompleted { upload_id: String },

    #[serde(rename = "upload_failed")]
    UploadFailed { upload_id: String, error: String },
}

pub async fn ws_upload_progress(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Single mpsc channel for this WS connection.
    // All subscribed upload_ids funnel events into this one channel.
    let (event_tx, mut event_rx) = mpsc::channel::<UploadEvent>(256);

    // Track which upload_ids this connection subscribed to, for cleanup.
    let subscribed_ids = tokio::sync::Mutex::new(Vec::<String>::new());

    // Forward events from mpsc channel -> WebSocket, with periodic pings
    let send_task = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(WS_PING_INTERVAL);
        ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            // Executes futures concurrently and returns the first one, cancelling the others
            tokio::select! {
                event = event_rx.recv() => {
                    match event {
                        Some(event) => {
                            if let Ok(json) = serde_json::to_string(&event) {
                                if ws_tx.send(Message::Text(json.into())).await.is_err() {
                                    break;
                                }
                            }
                        }
                        None => break,
                    }
                }
                _ = ping_interval.tick() => {
                    if ws_tx.send(Message::Ping(vec![].into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Read client messages with idle timeout
    let progress_map = state.upload_progress.clone();
    let recv_task = async {
        loop {
            match tokio::time::timeout(WS_IDLE_TIMEOUT, ws_rx.next()).await {
                Ok(Some(Ok(msg))) => match msg {
                    Message::Text(text) => {
                        if let Ok(cmd) = serde_json::from_str::<WsCommand>(&text) {
                            match cmd {
                                WsCommand::Subscribe { upload_id } => {
                                    progress_map.insert(upload_id.clone(), event_tx.clone());
                                    subscribed_ids.lock().await.push(upload_id.clone());
                                    let _ =
                                        event_tx.send(UploadEvent::Subscribed { upload_id }).await;
                                }
                            }
                        }
                    }
                    Message::Pong(_) => {} // keep-alive response, resets idle timer
                    Message::Close(_) => break,
                    _ => {}
                },
                Ok(Some(Err(_))) => break, // WebSocket error
                Ok(None) => break,         // Stream ended
                Err(_) => {
                    tracing::debug!("WebSocket idle timeout, closing connection");
                    break;
                }
            }
        }
    };

    tokio::select! {
        _ = send_task => {},
        _ = recv_task => {},
    }

    // Cleanup: remove all this connection's upload_ids from the shared map
    let ids = subscribed_ids.lock().await.clone();
    for id in &ids {
        state.upload_progress.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_subscribe_command() {
        let json = r#"{"type":"subscribe","upload_id":"abc-123"}"#;
        let cmd: WsCommand = serde_json::from_str(json).unwrap();
        match cmd {
            WsCommand::Subscribe { upload_id } => assert_eq!(upload_id, "abc-123"),
        }
    }

    #[test]
    fn unknown_command_fails() {
        let json = r#"{"type":"unsubscribe","upload_id":"abc"}"#;
        assert!(serde_json::from_str::<WsCommand>(json).is_err());
    }

    #[test]
    fn serialize_subscribed_event() {
        let event = UploadEvent::Subscribed {
            upload_id: "x".into(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "subscribed");
        assert_eq!(json["upload_id"], "x");
    }

    #[test]
    fn serialize_upload_started_event() {
        let event = UploadEvent::UploadStarted {
            upload_id: "u1".into(),
            total_parts: 5,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "upload_started");
        assert_eq!(json["total_parts"], 5);
    }

    #[test]
    fn serialize_part_completed_event() {
        let event = UploadEvent::PartCompleted {
            upload_id: "u1".into(),
            part_number: 3,
            total_parts: 10,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "part_completed");
        assert_eq!(json["part_number"], 3);
        assert_eq!(json["total_parts"], 10);
    }

    #[test]
    fn serialize_upload_completed_event() {
        let event = UploadEvent::UploadCompleted {
            upload_id: "done".into(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "upload_completed");
    }

    #[test]
    fn serialize_upload_failed_event() {
        let event = UploadEvent::UploadFailed {
            upload_id: "f".into(),
            error: "boom".into(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "upload_failed");
        assert_eq!(json["error"], "boom");
    }
}
