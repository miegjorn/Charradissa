use axum::{extract::{Path, Query, State}, http::{HeaderMap, StatusCode}, Json};
use charradissa_core::backend::ChatBackend;
use charradissa_core::responder::should_respond;
use charradissa_core::types::{ChatEvent, ChatEventKind, RoomId, UserId};
use chrono::Utc;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppserviceState {
    pub hs_token: String,
    /// Default agent URL — receives messages for rooms without an explicit route.
    pub default_agent_url: String,
    /// Per-room overrides: room_id → agent URL.
    pub agent_routes: HashMap<String, String>,
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
) -> (StatusCode, Json<Value>) {
    // The Matrix appservice spec requires a JSON body ({}) in the transaction
    // response. Returning a bare 200 with an empty body makes synapse raise a
    // JSONDecodeError, mark the transaction failed, and enter recovery — stalling
    // all later transactions. Always answer with an (status, Json) pair.
    let ack = || Json(serde_json::json!({}));

    // Resolve token: prefer Authorization: Bearer <tok>, fall back to ?access_token=
    let resolved = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
        .or_else(|| q.get("access_token").cloned());

    if !token_ok(resolved.as_deref(), &state.hs_token) {
        return (StatusCode::FORBIDDEN, ack());
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
            let agent_url = state.agent_routes
                .get(ev.room_id.as_str())
                .cloned()
                .unwrap_or_else(|| state.default_agent_url.clone());
            let (agent_url, backend) = (agent_url, state.backend.clone());
            tokio::spawn(async move {
                let history = backend
                    .room_history(&ev.room_id, Utc::now())
                    .await
                    .unwrap_or_default();

                const MAX_ATTEMPTS: u32 = 3;
                const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

                let mut result = call_agent(&agent_url, &history, &ev).await;
                let mut attempt = 1;
                while result.is_err() && attempt < MAX_ATTEMPTS {
                    tracing::warn!(
                        "agent reply attempt {} failed: {}; retrying",
                        attempt,
                        result.as_ref().unwrap_err()
                    );
                    tokio::time::sleep(RETRY_DELAY).await;
                    result = call_agent(&agent_url, &history, &ev).await;
                    attempt += 1;
                }

                match result {
                    Ok(text) if !text.trim().is_empty() => {
                        if let Err(e) = backend.send_message(&ev.room_id, &text).await {
                            tracing::error!("agent send failed: {}", e);
                        }
                    }
                    Ok(_) => tracing::warn!("agent produced an empty reply"),
                    Err(e) => {
                        tracing::error!("agent reply failed after {} attempts: {}", MAX_ATTEMPTS, e);
                        if let Err(send_err) = backend
                            .send_message(
                                &ev.room_id,
                                "\u{26A0} Guilhem is unreachable right now — please try again in a moment.",
                            )
                            .await
                        {
                            tracing::error!("fallback message send failed: {}", send_err);
                        }
                    }
                }
            });
        }
    }
    (StatusCode::OK, ack())
}

async fn call_agent(agent_url: &str, history: &[ChatEvent], ev: &ChatEvent) -> Result<String, String> {
    #[derive(serde::Serialize)]
    struct HistoryEntry<'a> {
        sender: &'a str,
        content: &'a str,
    }
    #[derive(serde::Serialize)]
    struct Req<'a> {
        room_id: &'a str,
        sender: &'a str,
        content: &'a str,
        history: Vec<HistoryEntry<'a>>,
    }
    #[derive(serde::Deserialize)]
    struct Resp {
        text: String,
    }

    let req = Req {
        room_id: ev.room_id.as_str(),
        sender: ev.sender.as_str(),
        content: &ev.content,
        history: history
            .iter()
            .map(|e| HistoryEntry { sender: e.sender.as_str(), content: &e.content })
            .collect(),
    };

    reqwest::Client::new()
        .post(format!("{}/matrix/reply", agent_url))
        .timeout(std::time::Duration::from_secs(300))
        .json(&req)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json::<Resp>()
        .await
        .map_err(|e| e.to_string())
        .map(|r| r.text)
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
