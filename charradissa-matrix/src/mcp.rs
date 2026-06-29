//! Matrix MCP tool server (Charradissa#23).
//!
//! Exposes four tools over JSON-RPC 2.0 so agents (Guilhem and component agents) can
//! *act* in Matrix rather than only respond to inbound events:
//!
//! | Tool            | Body                              | Effect                                |
//! |-----------------|-----------------------------------|---------------------------------------|
//! | `matrix_send`   | `{room_id, content}`              | Send a message to a room or DM        |
//! | `matrix_invite` | `{room_id, user_id}`              | Invite a user to a room               |
//! | `matrix_kick`   | `{room_id, user_id, reason?}`     | Kick a user from a room               |
//! | `matrix_get_dm` | `{agent}`                         | Resolve the DM room ID for an agent   |
//! | `matrix_leave`  | `{room_id}`                       | Leave a room                          |
//! | `matrix_read`   | `{room_id, limit?}`               | Read recent messages from a room      |
//!
//! ## Transport
//!
//! This is the *protocol* layer: a [`MatrixMcp`] processes a single JSON-RPC request
//! object via [`MatrixMcp::handle`] and returns the response object. The daemon mounts it
//! over HTTP at `POST /mcp`, matching the stack convention (`dispatcher` at `:9090/mcp`,
//! read by `Responder::mcp_call`). The success payload is the standard MCP
//! `result.content[0].text` shape callers already parse.
//!
//! ## Authentication
//!
//! The server acts with the appservice's Matrix token (`MATRIX_AS_TOKEN`, read at daemon
//! startup, or resolved via Gardian). All Matrix calls go through the shared
//! [`AppserviceClient`]; Synapse enforces the actual power level, so `matrix_invite` /
//! `matrix_kick` "respect the caller's power level" by surfacing Synapse's `M_FORBIDDEN`
//! as a graceful `isError` tool result rather than a panic.

use crate::client::AppserviceClient;
use charradissa_core::approval::PersistentApprovalQueue;
use charradissa_core::dm_registry::DmRegistry;
use charradissa_core::types::{RoomId, UserId};
use serde_json::{json, Value};
use std::sync::Arc;

/// MCP protocol version advertised in `initialize`.
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// The Matrix MCP tool server.
pub struct MatrixMcp {
    client: Arc<AppserviceClient>,
    dm_registry: DmRegistry,
    approval_queue: Arc<PersistentApprovalQueue>,
    approval_room_id: String,
}

impl MatrixMcp {
    pub fn new(
        client: Arc<AppserviceClient>,
        dm_registry: DmRegistry,
        approval_queue: Arc<PersistentApprovalQueue>,
        approval_room_id: String,
    ) -> Self {
        Self { client, dm_registry, approval_queue, approval_room_id }
    }

    /// The tool definitions advertised by `tools/list`.
    pub fn tool_definitions() -> Vec<Value> {
        vec![
            json!({
                "name": "matrix_send",
                "description": "Send a message to any Matrix room or DM by room ID.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "room_id": {"type": "string", "description": "Target room ID, e.g. !abc:occitane.guilhem"},
                        "content": {"type": "string", "description": "Message body (plain text)"}
                    },
                    "required": ["room_id", "content"]
                }
            }),
            json!({
                "name": "matrix_invite",
                "description": "Invite a user to a Matrix room. Fails gracefully if the caller's power level is insufficient.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "room_id": {"type": "string"},
                        "user_id": {"type": "string", "description": "Full MXID, e.g. @farga:occitane.guilhem"}
                    },
                    "required": ["room_id", "user_id"]
                }
            }),
            json!({
                "name": "matrix_kick",
                "description": "Kick a user from a Matrix room. Fails gracefully if the caller's power level is insufficient.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "room_id": {"type": "string"},
                        "user_id": {"type": "string"},
                        "reason": {"type": "string", "description": "Optional kick reason"}
                    },
                    "required": ["room_id", "user_id"]
                }
            }),
            json!({
                "name": "matrix_get_dm",
                "description": "Resolve the DM room ID for any user — component agent (e.g. \"farga\") or human (e.g. \"@pierre-luc:occitane.guilhem\"). Checks the agent registry first; if not found, scans joined rooms for a 2-member room shared with that user.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "agent": {"type": "string", "description": "Localpart, e.g. pierre-luc, or full MXID e.g. @pierre-luc:occitane.guilhem"}
                    },
                    "required": ["agent"]
                }
            }),
            json!({
                "name": "matrix_leave",
                "description": "Leave a Matrix room. Use when invited to a room you should not be in, or to clean up after completing a task in a temporary room.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "room_id": {"type": "string", "description": "Room ID to leave, e.g. !abc:occitane.guilhem"}
                    },
                    "required": ["room_id"]
                }
            }),
            json!({
                "name": "matrix_read",
                "description": "Read recent messages from a Matrix room (newest up to `limit`, default 20, max 20). Returns messages in chronological order as 'sender: body' lines. Use to catch up on context after joining or re-joining a room.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "room_id": {"type": "string", "description": "Room ID to read from, e.g. !abc:occitane.guilhem"},
                        "limit": {"type": "integer", "description": "Number of messages to fetch (1–20, default 20)"}
                    },
                    "required": ["room_id"]
                }
            }),
            json!({
                "name": "matrix_request_approval",
                "description": "Post a pending action to the shared approval room and register it in the approval queue. Call this when you need human sign-off before continuing (e.g. a PR you opened needs review, or a destructive action needs approval). Returns an approval_id. STOP working after calling this — you will receive a Matrix message in your room when approved or rejected.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "component": {"type": "string", "description": "Your component name, e.g. amassada"},
                        "category": {"type": "string", "description": "code | db | infra"},
                        "description": {"type": "string", "description": "What needs approval — include PR URL or issue number"},
                        "params": {"type": "object", "description": "Optional structured context (pr_url, issue_number, etc.)"},
                        "source_room_id": {"type": "string", "description": "The Matrix room ID of the requesting agent's room. Used to send approval/rejection notifications back to the agent."}
                    },
                    "required": ["component", "category", "description"]
                }
            }),
        ]
    }

    /// Handle one JSON-RPC 2.0 request object, returning the response object.
    ///
    /// Notifications (requests without an `id`) return [`Value::Null`]; the HTTP layer
    /// should emit no body for them.
    pub async fn handle(&self, request: Value) -> Value {
        let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = match request.get("id") {
            Some(id) if !id.is_null() => id.clone(),
            // No id → JSON-RPC notification (e.g. notifications/initialized). No response.
            _ => return Value::Null,
        };

        match method {
            "initialize" => ok(id, json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "charradissa-matrix-mcp", "version": env!("CARGO_PKG_VERSION") }
            })),
            "ping" => ok(id, json!({})),
            "tools/list" => ok(id, json!({ "tools": Self::tool_definitions() })),
            "tools/call" => {
                let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
                let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                match self.call_tool(name, &args).await {
                    Ok(text) => ok(id, tool_content(&text, false)),
                    Err(text) => ok(id, tool_content(&text, true)),
                }
            }
            other => err(id, -32601, &format!("method not found: {other}")),
        }
    }

    /// Dispatch a tool call. `Ok` is a human-readable success string; `Err` is an error
    /// string surfaced as an `isError` tool result (graceful failure, never a panic).
    pub async fn call_tool(&self, name: &str, args: &Value) -> std::result::Result<String, String> {
        match name {
            "matrix_send" => {
                let room_id = required_str(args, "room_id")?;
                let content = required_str(args, "content")?;
                self.client
                    .send_message(&RoomId::new(room_id), content)
                    .await
                    .map(|_| format!("Sent message to {room_id}."))
                    .map_err(|e| e.to_string())
            }
            "matrix_invite" => {
                let room_id = required_str(args, "room_id")?;
                let user_id = required_str(args, "user_id")?;
                self.client
                    .invite(&RoomId::new(room_id), &UserId::new(user_id))
                    .await
                    .map(|_| format!("Invited {user_id} to {room_id}."))
                    .map_err(|e| e.to_string())
            }
            "matrix_kick" => {
                let room_id = required_str(args, "room_id")?;
                let user_id = required_str(args, "user_id")?;
                let reason = args
                    .get("reason")
                    .and_then(|r| r.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("removed by agent");
                self.client
                    .kick_user(&RoomId::new(room_id), &UserId::new(user_id), reason)
                    .await
                    .map(|_| format!("Kicked {user_id} from {room_id}."))
                    .map_err(|e| e.to_string())
            }
            "matrix_get_dm" => {
                let agent = required_str(args, "agent")?;
                // Registry covers component agents; fall back to room scan for humans.
                if let Some(room_id) = self.dm_registry.resolve(agent) {
                    return Ok(room_id.to_string());
                }
                // Normalise to full MXID for the scan.
                let mxid = if agent.starts_with('@') {
                    agent.to_string()
                } else {
                    format!("@{}:{}", agent, self.client.server_name())
                };
                match self.client.find_dm_room(&mxid).await {
                    Ok(Some(room_id)) => Ok(room_id.to_string()),
                    Ok(None) => Err(format!(
                        "no DM room found with '{mxid}' — not in agent registry and no \
                         2-member joined room shared with that user exists"
                    )),
                    Err(e) => Err(format!("DM room scan failed: {e}")),
                }
            }
            "matrix_leave" => {
                let room_id = required_str(args, "room_id")?;
                self.client
                    .leave_room(&RoomId::new(room_id))
                    .await
                    .map(|_| format!("Left room {room_id}."))
                    .map_err(|e| e.to_string())
            }
            "matrix_read" => {
                let room_id = required_str(args, "room_id")?;
                let limit = args.get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(20)
                    .clamp(1, 20) as u32;
                self.client
                    .get_messages(&RoomId::new(room_id), limit)
                    .await
                    .map(|msgs| {
                        if msgs.is_empty() {
                            "No messages found.".to_string()
                        } else {
                            msgs.iter()
                                .map(|(sender, body)| {
                                    // Strip server suffix: @user:server → @user
                                    let short = sender.split(':').next().unwrap_or(sender);
                                    format!("{short}: {body}")
                                })
                                .collect::<Vec<_>>()
                                .join("\n")
                        }
                    })
                    .map_err(|e| e.to_string())
            }
            "matrix_request_approval" => {
                let component = required_str(args, "component")?;
                let category = args.get("category").and_then(|v| v.as_str()).unwrap_or("code");
                let description = required_str(args, "description")?;
                // Merge component into params so appservice can surface it in notifications.
                let mut merged: serde_json::Map<String, Value> = args
                    .get("params")
                    .and_then(|v| v.as_object())
                    .cloned()
                    .unwrap_or_default();
                merged.insert("component".to_string(), Value::String(component.to_string()));
                let params = Value::Object(merged);

                let bot_uid = self.client.bot_user_id().to_string();
                let source_room_id = args
                    .get("source_room_id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&bot_uid);

                let id = self.approval_queue
                    .register(source_room_id, category, description, params)
                    .map_err(|e| e.to_string())?;

                let msg = format!(
                    "⏳ **[{component}/{category}]** {description}\n   ID: `{id}`\n   Reply: `/approve {id}` or `/reject {id} <reason>`"
                );
                self.client
                    .send_message(&RoomId::new(&self.approval_room_id), &msg)
                    .await
                    .map_err(|e| e.to_string())?;

                Ok(format!("approval_id: {id} — posted to approval room. Stop working and wait for approval notification in your Matrix room."))
            }
            other => Err(format!("unknown tool: {other}")),
        }
    }
}

/// Extract a required, non-empty string argument.
fn required_str<'a>(args: &'a Value, key: &str) -> std::result::Result<&'a str, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("missing required string argument: {key}"))
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn err(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Standard MCP tool-call result envelope: `{ content: [{type:"text", text}], isError }`.
fn tool_content(text: &str, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn registry() -> DmRegistry {
        let mut m = HashMap::new();
        m.insert("farga".to_string(), "!dmroom:occitane.guilhem".to_string());
        DmRegistry::from_map(m)
    }

    fn mcp_for(server: &MockServer, reg: DmRegistry) -> MatrixMcp {
        let client = Arc::new(AppserviceClient::new(
            server.uri(),
            "test-as-token".to_string(),
            "@charradissa:occitane.guilhem".to_string(),
            "occitane.guilhem".to_string(),
        ));
        let approval_queue = Arc::new(charradissa_core::approval::PersistentApprovalQueue::new(
            std::path::PathBuf::from("/tmp/test-charradissa-approval-queue.json"),
        ));
        MatrixMcp::new(client, reg, approval_queue, String::new())
    }

    // ---- tools/list & initialize ------------------------------------------------------

    #[tokio::test]
    async fn tools_list_advertises_seven_tools() {
        let server = MockServer::start().await;
        let mcp = mcp_for(&server, registry());
        let resp = mcp.handle(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).await;
        let tools = resp["result"]["tools"].as_array().expect("tools array");
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names.len(), 7);
        for expected in ["matrix_send", "matrix_invite", "matrix_kick", "matrix_get_dm", "matrix_leave", "matrix_read", "matrix_request_approval"] {
            assert!(names.contains(&expected), "missing tool {expected}");
        }
    }

    #[tokio::test]
    async fn initialize_returns_server_info() {
        let server = MockServer::start().await;
        let mcp = mcp_for(&server, registry());
        let resp = mcp.handle(json!({"jsonrpc":"2.0","id":1,"method":"initialize"})).await;
        assert_eq!(resp["result"]["serverInfo"]["name"], "charradissa-matrix-mcp");
        assert_eq!(resp["result"]["protocolVersion"], MCP_PROTOCOL_VERSION);
    }

    #[tokio::test]
    async fn notification_without_id_returns_null() {
        let server = MockServer::start().await;
        let mcp = mcp_for(&server, registry());
        let resp = mcp.handle(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await;
        assert!(resp.is_null());
    }

    #[tokio::test]
    async fn unknown_method_returns_jsonrpc_error() {
        let server = MockServer::start().await;
        let mcp = mcp_for(&server, registry());
        let resp = mcp.handle(json!({"jsonrpc":"2.0","id":7,"method":"bogus"})).await;
        assert_eq!(resp["error"]["code"], -32601);
    }

    // ---- matrix_send ------------------------------------------------------------------

    #[tokio::test]
    async fn matrix_send_puts_message_and_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path_regex(r"^/_matrix/client/v3/rooms/.*/send/m\.room\.message/.*$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"event_id": "$e:occitane.guilhem"})))
            .mount(&server)
            .await;
        let mcp = mcp_for(&server, registry());
        let out = mcp
            .call_tool("matrix_send", &json!({"room_id": "!r:occitane.guilhem", "content": "hello"}))
            .await;
        assert!(out.is_ok(), "expected ok, got {out:?}");
        assert!(out.unwrap().contains("Sent message"));
    }

    #[tokio::test]
    async fn matrix_send_missing_content_is_error() {
        let server = MockServer::start().await;
        let mcp = mcp_for(&server, registry());
        let out = mcp.call_tool("matrix_send", &json!({"room_id": "!r:occitane.guilhem"})).await;
        assert!(out.is_err());
        assert!(out.unwrap_err().contains("content"));
    }

    #[tokio::test]
    async fn matrix_send_surfaces_via_handle_as_tool_content() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path_regex(r"^/_matrix/client/v3/rooms/.*/send/m\.room\.message/.*$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"event_id": "$e"})))
            .mount(&server)
            .await;
        let mcp = mcp_for(&server, registry());
        let resp = mcp
            .handle(json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": {"name": "matrix_send", "arguments": {"room_id": "!r:occitane.guilhem", "content": "hi"}}
            }))
            .await;
        assert_eq!(resp["result"]["isError"], false);
        assert!(resp["result"]["content"][0]["text"].as_str().unwrap().contains("Sent message"));
    }

    // ---- matrix_invite ----------------------------------------------------------------

    #[tokio::test]
    async fn matrix_invite_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"^/_matrix/client/v3/rooms/.*/invite$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        let mcp = mcp_for(&server, registry());
        let out = mcp
            .call_tool("matrix_invite", &json!({"room_id": "!r:occitane.guilhem", "user_id": "@farga:occitane.guilhem"}))
            .await;
        assert!(out.is_ok(), "expected ok, got {out:?}");
    }

    #[tokio::test]
    async fn matrix_invite_forbidden_fails_gracefully() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"^/_matrix/client/v3/rooms/.*/invite$"))
            .respond_with(ResponseTemplate::new(403).set_body_json(json!({"errcode": "M_FORBIDDEN"})))
            .mount(&server)
            .await;
        let mcp = mcp_for(&server, registry());
        // Drive through handle() to confirm a power-level rejection becomes isError, not a panic.
        let resp = mcp
            .handle(json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": {"name": "matrix_invite", "arguments": {"room_id": "!r:occitane.guilhem", "user_id": "@x:occitane.guilhem"}}
            }))
            .await;
        assert_eq!(resp["result"]["isError"], true);
    }

    // ---- matrix_kick ------------------------------------------------------------------

    #[tokio::test]
    async fn matrix_kick_with_reason_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"^/_matrix/client/v3/rooms/.*/kick$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        let mcp = mcp_for(&server, registry());
        let out = mcp
            .call_tool("matrix_kick", &json!({"room_id": "!r:occitane.guilhem", "user_id": "@x:occitane.guilhem", "reason": "spam"}))
            .await;
        assert!(out.is_ok(), "expected ok, got {out:?}");
        assert!(out.unwrap().contains("Kicked"));
    }

    #[tokio::test]
    async fn matrix_kick_without_reason_uses_default() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"^/_matrix/client/v3/rooms/.*/kick$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        let mcp = mcp_for(&server, registry());
        let out = mcp
            .call_tool("matrix_kick", &json!({"room_id": "!r:occitane.guilhem", "user_id": "@x:occitane.guilhem"}))
            .await;
        assert!(out.is_ok(), "expected ok, got {out:?}");
    }

    // ---- matrix_get_dm ----------------------------------------------------------------

    #[tokio::test]
    async fn matrix_get_dm_resolves_room_id() {
        let server = MockServer::start().await;
        let mcp = mcp_for(&server, registry());
        let out = mcp.call_tool("matrix_get_dm", &json!({"agent": "farga"})).await;
        assert_eq!(out.unwrap(), "!dmroom:occitane.guilhem");
    }

    #[tokio::test]
    async fn matrix_get_dm_resolves_by_mxid() {
        let server = MockServer::start().await;
        let mcp = mcp_for(&server, registry());
        let out = mcp.call_tool("matrix_get_dm", &json!({"agent": "@farga:occitane.guilhem"})).await;
        assert_eq!(out.unwrap(), "!dmroom:occitane.guilhem");
    }

    #[tokio::test]
    async fn matrix_get_dm_unknown_agent_is_error() {
        let server = MockServer::start().await;
        // Mock joined_rooms returning empty list so the scan returns Ok(None),
        // which hits the "no DM room found" branch (not the "scan failed" branch).
        Mock::given(method("GET"))
            .and(path_regex(r"^/_matrix/client/v3/joined_rooms$"))
            .respond_with(ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"joined_rooms": []})))
            .mount(&server)
            .await;
        let mcp = mcp_for(&server, registry());
        let out = mcp.call_tool("matrix_get_dm", &json!({"agent": "nope"})).await;
        assert!(out.is_err());
        assert!(out.unwrap_err().contains("no DM room"));
    }

    // ---- matrix_leave ----------------------------------------------------------------

    #[tokio::test]
    async fn matrix_leave_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"^/_matrix/client/v3/rooms/.*/leave$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        let mcp = mcp_for(&server, registry());
        let out = mcp
            .call_tool("matrix_leave", &json!({"room_id": "!r:occitane.guilhem"}))
            .await;
        assert!(out.is_ok(), "expected ok, got {out:?}");
        assert!(out.unwrap().contains("Left room"));
    }

    #[tokio::test]
    async fn matrix_leave_forbidden_fails_gracefully() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"^/_matrix/client/v3/rooms/.*/leave$"))
            .respond_with(ResponseTemplate::new(403).set_body_json(json!({"errcode": "M_FORBIDDEN"})))
            .mount(&server)
            .await;
        let mcp = mcp_for(&server, registry());
        let resp = mcp
            .handle(json!({
                "jsonrpc": "2.0", "id": 9, "method": "tools/call",
                "params": {"name": "matrix_leave", "arguments": {"room_id": "!r:occitane.guilhem"}}
            }))
            .await;
        assert_eq!(resp["result"]["isError"], true);
    }

    // ---- matrix_read ----------------------------------------------------------------

    #[tokio::test]
    async fn matrix_read_returns_chronological_messages() {
        let server = MockServer::start().await;
        // dir=b returns newest-first; the tool must reverse to chronological.
        Mock::given(method("GET"))
            .and(path_regex(r"^/_matrix/client/v3/rooms/.*/messages$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "chunk": [
                    {"type":"m.room.message","sender":"@b:s","content":{"body":"second","msgtype":"m.text"}},
                    {"type":"m.room.message","sender":"@a:s","content":{"body":"first","msgtype":"m.text"}}
                ],
                "start": "t1", "end": "t2"
            })))
            .mount(&server)
            .await;
        let mcp = mcp_for(&server, registry());
        let out = mcp.call_tool("matrix_read", &json!({"room_id": "!r:occitane.guilhem"})).await;
        assert!(out.is_ok(), "expected ok, got {out:?}");
        let text = out.unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("@a") && lines[0].contains("first"), "got: {}", lines[0]);
        assert!(lines[1].contains("@b") && lines[1].contains("second"), "got: {}", lines[1]);
    }

    #[tokio::test]
    async fn matrix_read_empty_room_returns_no_messages_string() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex(r"^/_matrix/client/v3/rooms/.*/messages$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"chunk":[],"start":"t1","end":"t2"})))
            .mount(&server)
            .await;
        let mcp = mcp_for(&server, registry());
        let out = mcp.call_tool("matrix_read", &json!({"room_id": "!r:occitane.guilhem"})).await;
        assert_eq!(out.unwrap(), "No messages found.");
    }

    #[tokio::test]
    async fn unknown_tool_is_error() {
        let server = MockServer::start().await;
        let mcp = mcp_for(&server, registry());
        let out = mcp.call_tool("matrix_teleport", &json!({})).await;
        assert!(out.is_err());
    }
}
