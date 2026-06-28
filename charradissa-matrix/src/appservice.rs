use axum::{extract::{Path, Query, State}, http::{HeaderMap, StatusCode}, Json};
use charradissa_core::backend::ChatBackend;
use charradissa_core::blocks::strip_block_markers;
use charradissa_core::config::ProjectAgentConfig;
use charradissa_core::responder::{should_respond, Responder};
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
    /// Component agents keyed by Matrix room ID. When a message arrives in one of
    /// these rooms, the corresponding Responder handles it inline (in-process)
    /// rather than going through the agent_routes/default_agent_url HTTP path.
    pub component_agents: HashMap<String, Arc<Responder>>,
    /// Project agent configs keyed by Matrix room ID. Messages in these rooms are
    /// forwarded to Amassada with `project_id` injected into the turn body.
    pub project_routes: HashMap<String, ProjectAgentConfig>,
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

            if let Some(project_cfg) = state.project_routes.get(ev.room_id.as_str()).cloned() {
                let backend = state.backend.clone();
                tokio::spawn(async move {
                    let history = backend.room_history(&ev.room_id, Utc::now()).await.unwrap_or_default();
                    // Typing indicators are not sent on the project-agent path. Amassada
                    // manages its own session lifecycle and response timing; a typing
                    // indicator fired here could linger if the session takes longer than
                    // the Matrix timeout.
                    match call_project_agent(&project_cfg.endpoint, &project_cfg.project_id, &history, &ev).await {
                        Ok(raw) if !raw.trim().is_empty() => {
                            // Strip must happen before the empty check — a response that is
                            // entirely block-protocol markers (e.g. [CONSULT]…[LEAVE] with no
                            // [MAIN]) strips to empty and should not be forwarded to the room.
                            let text = strip_block_markers(&raw);
                            if text.trim().is_empty() {
                                tracing::warn!("project agent reply stripped to empty (no [MAIN] block in response)");
                            } else if let Err(e) = backend.send_message(&ev.room_id, &text).await {
                                tracing::error!("project agent send failed: {}", e);
                            }
                        }
                        Ok(_) => tracing::warn!("project agent produced an empty reply"),
                        Err(e) => {
                            tracing::error!("project agent reply failed: {}", e);
                            if let Err(send_err) = backend
                                .send_message(
                                    &ev.room_id,
                                    "\u{26A0} This project agent is unreachable — please try again in a moment.",
                                )
                                .await
                            {
                                tracing::error!("fallback message send failed: {}", send_err);
                            }
                        }
                    }
                });
                continue;
            }

            if let Some(component_responder) = state.component_agents.get(ev.room_id.as_str()).cloned() {
                let backend = state.backend.clone();
                tokio::spawn(async move {
                    let history = backend.room_history(&ev.room_id, Utc::now()).await.unwrap_or_default();
                    // Typing indicators are not sent on the component-agent path. Component
                    // agents run in-process via the Responder and typically reply quickly;
                    // adding a typing indicator here would require pairing it with a clear
                    // call, which is unnecessary overhead for synchronous in-process dispatch.
                    match component_responder.reply(&history, &ev).await {
                        Ok(text) if !text.trim().is_empty() => {
                            let text = strip_block_markers(&text);
                            if let Err(e) = backend.send_message(&ev.room_id, &text).await {
                                tracing::error!("component agent send failed: {}", e);
                            }
                        }
                        Ok(_) => tracing::warn!("component agent produced an empty reply"),
                        Err(e) => {
                            tracing::error!("component agent reply failed: {}", e);
                            if let Err(send_err) = backend
                                .send_message(
                                    &ev.room_id,
                                    "\u{26A0} This component agent is unreachable right now — please try again in a moment.",
                                )
                                .await
                            {
                                tracing::error!("fallback message send failed: {}", send_err);
                            }
                        }
                    }
                });
                continue;
            }

            let agent_url = state.agent_routes
                .get(ev.room_id.as_str())
                .cloned()
                .unwrap_or_else(|| state.default_agent_url.clone());
            let (agent_url, backend, self_user_id) = (agent_url, state.backend.clone(), state.self_user_id.clone());
            tokio::spawn(async move {
                let history = backend
                    .room_history(&ev.room_id, Utc::now())
                    .await
                    .unwrap_or_default();

                // Signal to the client that Guilhem is thinking. This keeps the long-poll
                // sync connection alive so the response arrives without requiring a second
                // message from the user (Charradissa#27).
                let _ = backend.set_typing(&ev.room_id, &self_user_id, true, 120_000).await;

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

                // Clear typing indicator before posting the reply (or error message).
                let _ = backend.set_typing(&ev.room_id, &self_user_id, false, 0).await;

                match result {
                    Ok(text) if !text.trim().is_empty() => {
                        let text = strip_block_markers(&text);
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

/// POST a turn to an Amassada project endpoint.
///
/// The endpoint template has `{room_id}` substituted with the actual room ID.
/// The body carries `project_id` so Amassada can resolve the right persona via
/// its project registry (A-1).  The response format is `{ "text": "..." }`.
async fn call_project_agent(
    endpoint_template: &str,
    project_id: &str,
    history: &[ChatEvent],
    ev: &ChatEvent,
) -> Result<String, String> {
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
        project_id: &'a str,
        history: Vec<HistoryEntry<'a>>,
    }
    #[derive(serde::Deserialize)]
    struct Resp {
        text: String,
    }

    let url = endpoint_template.replace("{room_id}", ev.room_id.as_str());
    let req = Req {
        room_id: ev.room_id.as_str(),
        sender: ev.sender.as_str(),
        content: &ev.content,
        project_id,
        history: history
            .iter()
            .map(|e| HistoryEntry { sender: e.sender.as_str(), content: &e.content })
            .collect(),
    };

    // Retry on connection errors (pod restart, rolling-deploy gap).
    // Backoff: 1s → 2s → 4s before giving up and surfacing the "unreachable" message.
    let client = reqwest::Client::new();
    let mut last_err = String::new();
    for attempt in 0u32..3 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(1u64 << (attempt - 1))).await;
        }
        match client.post(&url).timeout(std::time::Duration::from_secs(300)).json(&req).send().await {
            Ok(resp) => {
                return resp.json::<Resp>().await.map_err(|e| e.to_string()).map(|r| r.text);
            }
            Err(e) if e.is_connect() || e.is_request() => {
                tracing::warn!("project agent connection error (attempt {}/3): {}", attempt + 1, e);
                last_err = e.to_string();
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Err(last_err)
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

    #[test]
    fn component_agents_map_keyed_by_room_id() {
        // Verify the HashMap key type is room_id string — routing contract test.
        let room_id = "!amassada:occitane.guilhem";
        let mut map: HashMap<String, Arc<Responder>> = HashMap::new();
        let r = Arc::new(Responder::with_config(
            "k".into(), "m".into(), "s".into(),
            "http://f".into(), "http://d".into(), "http://a".into(),
            "amassada context".into(), false,
        ));
        map.insert(room_id.to_string(), r.clone());
        assert!(map.contains_key(room_id));
        assert!(!map.contains_key("!other:occitane.guilhem"));
    }

    // ── project routing ───────────────────────────────────────────────────────

    use charradissa_core::config::ProjectAgentConfig;

    fn make_project_cfg(project_id: &str, rooms: &[&str], endpoint: &str) -> HashMap<String, ProjectAgentConfig> {
        let cfg = ProjectAgentConfig {
            agent_type: "amassada_backed".into(),
            rooms: rooms.iter().map(|r| r.to_string()).collect(),
            project_id: project_id.into(),
            endpoint: endpoint.into(),
        };
        rooms.iter().map(|r| (r.to_string(), cfg.clone())).collect()
    }

    #[test]
    fn project_routes_map_room_to_project_config() {
        let routes = make_project_cfg(
            "alpha",
            &["!proj-a:occitane.guilhem", "!proj-b:occitane.guilhem"],
            "http://amassada:7700/sessions/{room_id}/message",
        );
        let cfg_a = routes.get("!proj-a:occitane.guilhem").unwrap();
        assert_eq!(cfg_a.project_id, "alpha");
        let cfg_b = routes.get("!proj-b:occitane.guilhem").unwrap();
        assert_eq!(cfg_b.project_id, "alpha");
        // unregistered room → not in map
        assert!(routes.get("!org-general:occitane.guilhem").is_none());
    }

    #[test]
    fn endpoint_room_id_substitution() {
        let template = "http://amassada:7700/sessions/{room_id}/message";
        let room_id = "!proj-a:occitane.guilhem";
        let url = template.replace("{room_id}", room_id);
        assert_eq!(url, "http://amassada:7700/sessions/!proj-a:occitane.guilhem/message");
        assert!(!url.contains("{room_id}"), "substitution must replace the placeholder");
    }

    #[tokio::test]
    async fn call_project_agent_sends_project_id_in_body() {
        use axum::{extract::State, Json, Router, routing::post};
        use charradissa_core::types::{ChatEventKind, RoomId, UserId};
        use chrono::Utc;
        use std::sync::{Arc, Mutex};

        type Captured = Arc<Mutex<Option<serde_json::Value>>>;
        let captured: Captured = Arc::new(Mutex::new(None));
        let captured_clone = captured.clone();

        let app = Router::new()
            .route(
                "/sessions/:room_id/message",
                post(|State(cap): State<Captured>, Json(body): Json<serde_json::Value>| async move {
                    *cap.lock().unwrap() = Some(body);
                    Json(serde_json::json!({"text": "pong"}))
                }),
            )
            .with_state(captured_clone);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let ev = ChatEvent {
            event_id: "$e1".into(),
            room_id: RoomId::new("!proj-a:occitane.guilhem"),
            sender: UserId::new("@user:server"),
            content: "hello project".into(),
            timestamp: Utc::now(),
            kind: ChatEventKind::Message,
        };

        let endpoint = format!("http://{}/sessions/{{room_id}}/message", addr);
        let result = call_project_agent(&endpoint, "alpha", &[], &ev).await;
        assert_eq!(result.as_deref().ok(), Some("pong"), "expected pong from mock server");

        let json = captured.lock().unwrap().clone().unwrap();
        assert_eq!(json["project_id"].as_str(), Some("alpha"),
            "project_id must be present in the turn body");
        assert_eq!(json["room_id"].as_str(), Some("!proj-a:occitane.guilhem"),
            "room_id must be present in the turn body");
        assert_eq!(json["content"].as_str(), Some("hello project"),
            "content must be present in the turn body");
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
