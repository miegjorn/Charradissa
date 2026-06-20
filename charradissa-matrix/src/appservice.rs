use axum::{extract::{Path, Query, State}, http::{HeaderMap, StatusCode}, Json};
use charradissa_core::backend::ChatBackend;
use charradissa_core::responder::{should_respond, Responder};
use charradissa_core::types::{ChatEvent, ChatEventKind, RoomId, UserId};
use chrono::Utc;
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppserviceState {
    pub hs_token: String,
    pub responder: Arc<Responder>,
    pub backend: Arc<dyn ChatBackend>,
    pub self_user_id: String,
}

pub fn token_ok(provided: Option<&str>, expected: &str) -> bool {
    provided == Some(expected)
}

/// Matrix Appservice PUT /_matrix/app/v1/transactions/{txnId}
pub async fn handle_transaction(
    State(state): State<AppserviceState>,
    headers: HeaderMap,
    Query(q): Query<std::collections::HashMap<String, String>>,
    Path(_txn): Path<String>,
    Json(body): Json<Value>,
) -> StatusCode {
    // Resolve token: prefer Authorization: Bearer <tok>, fall back to ?access_token=
    let resolved = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
        .or_else(|| q.get("access_token").cloned());

    if !token_ok(resolved.as_deref(), &state.hs_token) {
        return StatusCode::FORBIDDEN;
    }

    let events = body["events"].as_array().cloned().unwrap_or_default();
    for raw in events {
        // Auto-join on invite before attempting message parsing.
        if raw["type"] == "m.room.member"
            && raw["content"]["membership"] == "invite"
            && raw["state_key"].as_str() == Some(state.self_user_id.as_str())
        {
            if let Some(room) = raw["room_id"].as_str() {
                let (backend, room) = (state.backend.clone(), RoomId::new(room));
                tokio::spawn(async move {
                    if let Err(e) = backend.join_room(&room).await {
                        tracing::warn!("auto-join failed: {}", e);
                    }
                });
            }
            continue; // membership events are not messages
        }

        if let Some(ev) = parse_matrix_event(&raw) {
            if !should_respond(&ev, &state.self_user_id) {
                continue;
            }
            let (responder, backend) = (state.responder.clone(), state.backend.clone());
            tokio::spawn(async move {
                let history = backend
                    .room_history(&ev.room_id, Utc::now())
                    .await
                    .unwrap_or_default();
                match responder.reply(&history, &ev).await {
                    Ok(text) if !text.trim().is_empty() => {
                        let _ = backend.send_message(&ev.room_id, &text).await;
                    }
                    Ok(_) => {}
                    Err(e) => tracing::error!("guilhem reply failed: {}", e),
                }
            });
        }
    }
    StatusCode::OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_wrong_hs_token() {
        assert!(token_ok(Some("good"), "good")); // correct token accepted
        assert!(!token_ok(Some("bad"), "good")); // wrong token rejected
        assert!(!token_ok(None, "good")); // missing token rejected
    }
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
