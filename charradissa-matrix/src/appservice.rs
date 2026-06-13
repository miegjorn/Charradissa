use axum::{extract::{Path, State}, http::StatusCode, Json};
use charradissa_core::types::{ChatEvent, ChatEventKind, RoomId, UserId};
use chrono::Utc;
use serde_json::Value;

#[derive(Clone)]
pub struct AppserviceState {
    pub hs_token: String,
}

/// Matrix Appservice PUT /_matrix/app/v1/transactions/{txnId}
pub async fn handle_transaction(
    State(state): State<AppserviceState>,
    Path(txn_id): Path<String>,
    Json(body): Json<Value>,
) -> StatusCode {
    let events = body["events"].as_array().cloned().unwrap_or_default();
    for event in events {
        if let Some(event) = parse_matrix_event(&event) {
            tracing::info!("appservice event from {}: {}", event.sender, event.content);
        }
    }
    StatusCode::OK
}

pub fn parse_matrix_event(event: &Value) -> Option<ChatEvent> {
    let event_id = event["event_id"].as_str()?.to_string();
    let room_id = RoomId::new(event["room_id"].as_str()?);
    let sender = UserId::new(event["sender"].as_str()?);
    let content_body = event["content"]["body"].as_str().unwrap_or("").to_string();

    let kind = if content_body.starts_with('/') {
        let parts: Vec<&str> = content_body.splitn(2, ' ').collect();
        ChatEventKind::SlashCommand {
            command: parts[0][1..].to_string(),
            args: parts.get(1).copied().unwrap_or("").to_string(),
        }
    } else {
        ChatEventKind::Message
    };

    Some(ChatEvent {
        event_id,
        room_id,
        sender,
        content: content_body,
        timestamp: Utc::now(),
        kind,
    })
}
