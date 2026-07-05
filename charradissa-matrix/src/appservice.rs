use axum::{extract::{Path, Query, State}, http::{HeaderMap, StatusCode}, Json};
use charradissa_core::backend::ChatBackend;
use charradissa_core::blocks::strip_block_markers;
use charradissa_core::config::ProjectAgentConfig;
use charradissa_core::responder::{should_respond, Responder};
use charradissa_core::types::{ChatEvent, ChatEventKind, RoomId, UserId};
use chrono::Utc;
use crate::client::AGENT_LOCAL_PARTS;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Room IDs for the 9 agents that now run independent Matrix sessions
/// (guilhem + 8 component agents). Messages in these rooms are never
/// relayed by Charradissa — each agent's own pod handles its own room via
/// its own /sync loop (see Caissa's run_matrix_client_loop). Without this
/// guard, once component_agents/agents.routes are emptied (Task 7), these
/// rooms would silently fall through to the default_agent_url branch below
/// and get double-relayed.
const MIGRATED_AGENT_ROOM_IDS: &[&str] = &[
    "!iNQRqUAMckCUQrSFHk:occitane.guilhem", // guilhem (#occitan, the project room/Space)
    "!bwuKXFvUXnVZfXcKuz:occitane.guilhem", // gardian
    "!KuWBSmYyvyiyTMFKqJ:occitane.guilhem", // fondament
    "!CtktMiOTNtSIkdwOxq:occitane.guilhem", // farga
    "!vLjgiURMSlkqTXgaDG:occitane.guilhem", // amassada
    "!FgfTbZMpLLVGiISZTj:occitane.guilhem", // cor
    "!ZqGBDioAYnOATihiEU:occitane.guilhem", // caissa
    "!qZGQFrjAcKjPinhQnp:occitane.guilhem", // charradissa
    "!QQeweqsLsOTZYdonXi:occitane.guilhem", // nervi
];

#[derive(Clone)]
pub struct AppserviceState {
    pub hs_token: String,
    /// Default agent URL — receives messages for rooms without an explicit route.
    pub default_agent_url: String,
    /// Per-room overrides: room_id → agent URL.
    pub agent_routes: HashMap<String, String>,
    pub backend: Arc<dyn ChatBackend>,
    pub self_user_id: String,
    /// Component agents keyed by Matrix room ID. Each entry carries the agent's
    /// Matrix localpart (e.g. "nervi") so responses are sent as that virtual user
    /// rather than as the appservice bot.
    pub component_agents: HashMap<String, (String, Arc<Responder>)>,
    /// Project agent configs keyed by Matrix room ID. Messages in these rooms are
    /// forwarded to Amassada with `project_id` injected into the turn body.
    pub project_routes: HashMap<String, ProjectAgentConfig>,
    /// Kroki server URL for server-side diagram rendering. When `None`, Mermaid
    /// blocks are not rendered (they still reach the agent as plain text).
    pub kroki_url: Option<String>,
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
            // Also ignore messages sent by appservice-managed virtual users
            // (e.g. @farga, @gardian) — they are our own echoes.
            if is_appservice_sender(ev.sender.as_str(), &state.self_user_id) {
                continue;
            }

            // These 9 rooms are handled entirely by their own agent's independent
            // Matrix session now — never relay, regardless of what project_routes/
            // component_agents/default_agent_url would otherwise say.
            if MIGRATED_AGENT_ROOM_IDS.contains(&ev.room_id.as_str()) {
                continue;
            }

            // Diagram render hook: detect ```mermaid blocks and post rendered SVG.
            // Posts as the room's virtual user (component agent) if one owns the room,
            // otherwise as the appservice bot. Fire-and-forget — does not block routing.
            if let Some(kroki_url) = &state.kroki_url {
                let blocks = charradissa_core::mermaid::extract_mermaid_blocks(&ev.content);
                if !blocks.is_empty() {
                    let backend = state.backend.clone();
                    let room = ev.room_id.clone();
                    let kroki = kroki_url.clone();
                    let blocks = blocks.clone();
                    let sender_lp = state.component_agents.get(ev.room_id.as_str())
                        .map(|(lp, _)| lp.clone());
                    tokio::spawn(async move {
                        for (i, diagram) in blocks.iter().enumerate() {
                            match charradissa_core::mermaid::render_svg(&kroki, diagram).await {
                                Ok(svg) => {
                                    match backend.upload_media("image/svg+xml", svg).await {
                                        Ok(mxc) => {
                                            let name = if blocks.len() == 1 {
                                                "diagram.svg".to_string()
                                            } else {
                                                format!("diagram-{}.svg", i + 1)
                                            };
                                            if let Err(e) = backend.send_image(&room, &mxc, &name, sender_lp.as_deref()).await {
                                                tracing::warn!("diagram send_image: {}", e);
                                            }
                                        }
                                        Err(e) => tracing::warn!("diagram upload failed: {}", e),
                                    }
                                }
                                Err(e) => tracing::warn!("diagram render failed: {}", e),
                            }
                        }
                    });
                }
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

            if let Some((agent_lp, component_responder)) = state.component_agents.get(ev.room_id.as_str()).cloned() {
                let backend = state.backend.clone();
                tokio::spawn(async move {
                    let history = backend.room_history(&ev.room_id, Utc::now()).await.unwrap_or_default();
                    match component_responder.reply(&history, &ev).await {
                        Ok(text) if !text.trim().is_empty() => {
                            let text = strip_block_markers(&text);
                            if let Err(e) = backend.send_message_as(&ev.room_id, &text, Some(&agent_lp)).await {
                                tracing::error!("component agent send failed: {}", e);
                            }
                        }
                        Ok(_) => tracing::warn!("component agent produced an empty reply"),
                        Err(e) => {
                            tracing::error!("component agent reply failed: {}", e);
                            if let Err(send_err) = backend
                                .send_message_as(
                                    &ev.room_id,
                                    "\u{26A0} This component agent is unreachable right now — please try again in a moment.",
                                    Some(&agent_lp),
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
            // Every agent (including Guilhem) sends as its own virtual user identity.
            // sender_from_agent_url extracts the localpart from the agent's k8s service URL.
            let component_sender = sender_from_agent_url(&agent_url);
            let (agent_url, backend, self_user_id) = (agent_url, state.backend.clone(), state.self_user_id.clone());
            tokio::spawn(async move {
                let history = backend
                    .room_history(&ev.room_id, Utc::now())
                    .await
                    .unwrap_or_default();

                // Typing indicator sent as the AS bot (@charradissa) only when the room's agent
                // IS @charradissa (i.e. the charradissa component room itself). All other agents
                // have their own identity and are members of their own rooms — but set_typing
                // would need the agent's own access token, which we don't hold. Suppress for now.
                if component_sender.as_deref() == Some("charradissa") {
                    let _ = backend.set_typing(&ev.room_id, &self_user_id, true, 120_000).await;
                }

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

                if component_sender.as_deref() == Some("charradissa") {
                    let _ = backend.set_typing(&ev.room_id, &self_user_id, false, 0).await;
                }

                let sender = component_sender.as_deref();
                match result {
                    Ok(text) if !text.trim().is_empty() => {
                        let text = strip_block_markers(&text);
                        if let Err(e) = backend.send_message_as(&ev.room_id, &text, sender).await {
                            tracing::error!("agent send failed: {}", e);
                        }
                    }
                    Ok(_) => tracing::warn!("agent produced an empty reply"),
                    Err(e) => {
                        tracing::error!("agent reply failed after {} attempts: {}", MAX_ATTEMPTS, e);
                        let msg = if sender.is_some() {
                            "\u{26A0} This agent is unreachable right now — please try again in a moment."
                        } else {
                            "\u{26A0} Guilhem is unreachable right now — please try again in a moment."
                        };
                        if let Err(send_err) = backend.send_message_as(&ev.room_id, msg, sender).await {
                            tracing::error!("fallback message send failed: {}", send_err);
                        }
                    }
                }
            });
        }
    }
    (StatusCode::OK, ack())
}

/// Derive the Matrix localpart of the component agent from its service URL.
/// `http://farga-agent.agents.svc.cluster.local:8080` → `Some("farga")`
/// Returns None for the default Guilhem URL (no `-agent` suffix pattern).
fn sender_from_agent_url(url: &str) -> Option<String> {
    let host = url.split("://").nth(1)?.split('/').next()?.split(':').next()?;
    let first = host.split('.').next()?;
    Some(first.strip_suffix("-agent").unwrap_or(first).to_string())
}

/// Returns true when the event sender is an appservice-managed virtual user
/// (i.e. any @*:server user in the same server namespace as self_user_id).
/// These are our own echoes and must not trigger a new agent call.
fn is_appservice_sender(sender: &str, self_user_id: &str) -> bool {
    // self_user_id is e.g. "@charradissa:occitane.guilhem"
    // Extract the server part: "occitane.guilhem"
    let Some(self_server) = self_user_id.split(':').nth(1) else { return false };
    // Virtual users managed by this appservice are AGENT_LOCAL_PARTS@server
    sender.ends_with(&format!(":{}", self_server))
        && AGENT_LOCAL_PARTS.iter().any(|lp| sender == format!("@{}:{}", lp, self_server))
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
/// The endpoint template's `{session_id}` placeholder is substituted with a stable,
/// URL-safe session handle derived from the room (see
/// [`charradissa_core::routing::project_session_id`]): the same room reuses the same
/// Amassada session across turns and restarts. `{room_id}` is also substituted (with
/// the raw room id) for backward compatibility with older endpoint templates.
///
/// The body carries both `room_id` — which Amassada uses to resolve the project from
/// its registry (Amassada#11) — and `project_id`, the project Charradissa resolved
/// from its own routing table. The response format is `{ "text": "..." }`.
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

    let session_id = charradissa_core::routing::project_session_id(ev.room_id.as_str());
    let url = endpoint_template
        .replace("{session_id}", &session_id)
        .replace("{room_id}", ev.room_id.as_str());
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
    fn migrated_room_ids_include_all_nine_agents() {
        assert_eq!(MIGRATED_AGENT_ROOM_IDS.len(), 9);
        assert!(MIGRATED_AGENT_ROOM_IDS.contains(&"!iNQRqUAMckCUQrSFHk:occitane.guilhem")); // guilhem (#occitan)
        assert!(MIGRATED_AGENT_ROOM_IDS.contains(&"!qZGQFrjAcKjPinhQnp:occitane.guilhem")); // charradissa
    }

    #[test]
    fn component_agents_map_keyed_by_room_id() {
        // Verify the HashMap key type is room_id string — routing contract test.
        let room_id = "!amassada:occitane.guilhem";
        let mut map: HashMap<String, Arc<Responder>> = HashMap::new();
        let r = Arc::new(Responder::with_config(
            "k".into(), None, "m".into(), "s".into(),
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

    #[tokio::test]
    async fn call_project_agent_uses_stable_session_id_in_path() {
        use axum::{extract::{Path, State}, Json, Router, routing::post};
        use charradissa_core::types::{ChatEventKind, RoomId, UserId};
        use chrono::Utc;
        use std::sync::{Arc, Mutex};

        // Capture the `:session_id` path segment the request actually hits.
        type Captured = Arc<Mutex<Option<String>>>;
        let captured: Captured = Arc::new(Mutex::new(None));
        let captured_clone = captured.clone();

        let app = Router::new()
            .route(
                "/sessions/:session_id/message",
                post(|Path(session_id): Path<String>, State(cap): State<Captured>, Json(_): Json<serde_json::Value>| async move {
                    *cap.lock().unwrap() = Some(session_id);
                    Json(serde_json::json!({"text": "pong"}))
                }),
            )
            .with_state(captured_clone);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let room = "!proj-a:occitane.guilhem";
        let ev = ChatEvent {
            event_id: "$e1".into(),
            room_id: RoomId::new(room),
            sender: UserId::new("@user:server"),
            content: "hello".into(),
            timestamp: Utc::now(),
            kind: ChatEventKind::Message,
        };

        // Endpoint template uses the new {session_id} placeholder.
        let endpoint = format!("http://{}/sessions/{{session_id}}/message", addr);
        let result = call_project_agent(&endpoint, "alpha", &[], &ev).await;
        assert_eq!(result.as_deref().ok(), Some("pong"), "expected pong from mock server");

        let path_session = captured.lock().unwrap().clone().unwrap();
        assert_eq!(
            path_session,
            charradissa_core::routing::project_session_id(room),
            "URL path must carry the derived stable session id"
        );
        assert!(path_session.starts_with("project-"));
        assert!(
            !path_session.contains('!') && !path_session.contains(':'),
            "the raw Matrix room id must not leak into the session path"
        );
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
