use crate::error::{CharradissaError, Result};
use crate::types::{ChatEvent, ChatEventKind};

pub const GUILHEM_SYSTEM: &str = "You are Guilhem de Tudela, the org-level agent and chronicler of the Occitan stack. \
You live in Matrix and speak with Pierre-Luc and the stack's own agents. The stack's components: \
Farga = durable memory/coherence (signals, org/project context), Amassada = sessions, Charradissa = your Matrix presence, Synapse = homeserver. \
\
You have READ-ONLY tools to query Farga directly. USE THEM to ground answers in the stack's real state — \
do not guess or confabulate what Farga holds. When asked about the stack, your memory, prior decisions, or current \
state, call the relevant tool first, then answer from what it returns. If a tool errors, say so plainly and report the \
boundary honestly rather than inventing an answer. The room you are in is working memory since the last concierge sweep; \
durable memory lives in Farga. Be substantive, honest, and concise.";

/// Max Claude<->tool round-trips per reply, to bound cost/latency.
const MAX_TOOL_ROUNDS: u32 = 5;
const MAX_TOKENS: u32 = 1536;
/// Cap a single tool result so a large Farga payload can't blow the context.
const MAX_TOOL_RESULT_CHARS: usize = 6000;

pub struct Responder {
    client: reqwest::Client,
    api_key: String,
    model: String,
    pub server_name: String,
    /// Base URL of Farga's HTTP API (e.g. http://farga:7500), used by the read-only tools.
    farga_url: String,
}

impl Responder {
    pub fn new(api_key: String, model: String, server_name: String, farga_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            model,
            server_name,
            farga_url: farga_url.trim_end_matches('/').to_string(),
        }
    }

    pub fn build_user_prompt(&self, history: &[ChatEvent], latest: &ChatEvent) -> String {
        let mut s = String::from("Recent conversation (oldest first):\n");
        for e in history {
            s.push_str(&format!("{}: {}\n", e.sender, e.content));
        }
        s.push_str(&format!(
            "\nLatest message from {}:\n{}\n\nReply as Guilhem.",
            latest.sender, latest.content
        ));
        s
    }

    /// The read-only tool definitions exposed to Claude. All map to Farga GET endpoints.
    pub fn tools() -> serde_json::Value {
        serde_json::json!([
            {
                "name": "farga_recent_signals",
                "description": "Read recent signals from Farga (the stack's durable memory) for a project. Signals include prior chronicles, decisions, blockers, and observations. This is how you recall your own and the stack's history.",
                "input_schema": {
                    "type": "object",
                    "properties": {"project": {"type": "string", "description": "Project id, e.g. 'occitan'. Defaults to occitan."}},
                    "required": []
                }
            },
            {
                "name": "farga_project_context",
                "description": "Read the durable context document for a project from Farga (goals, state, structure).",
                "input_schema": {
                    "type": "object",
                    "properties": {"project": {"type": "string", "description": "Project id, e.g. 'occitan'. Defaults to occitan."}},
                    "required": []
                }
            },
            {
                "name": "farga_org_context",
                "description": "Read the durable org-level context from Farga (the organization's overall state and initiatives).",
                "input_schema": {
                    "type": "object",
                    "properties": {"org": {"type": "string", "description": "Org id, e.g. 'occitan'. Defaults to occitan."}},
                    "required": []
                }
            }
        ])
    }

    /// Execute one read-only tool call against Farga. Returns a result string for the
    /// model (the JSON/text body, or a plain error description it can reason about).
    pub async fn execute_tool(&self, name: &str, input: &serde_json::Value) -> String {
        let arg = |k: &str, default: &str| {
            input.get(k).and_then(|v| v.as_str()).unwrap_or(default).to_string()
        };
        let url = match name {
            "farga_recent_signals" => {
                format!("{}/signals/recent?project={}", self.farga_url, arg("project", "occitan"))
            }
            "farga_project_context" => {
                format!("{}/context/project/{}", self.farga_url, arg("project", "occitan"))
            }
            "farga_org_context" => {
                format!("{}/context/org/{}", self.farga_url, arg("org", "occitan"))
            }
            other => return format!("ERROR: unknown tool '{}'", other),
        };

        match self.client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                let mut out = if status.is_success() {
                    body
                } else {
                    format!("ERROR: Farga returned HTTP {} for {}: {}", status, url, body)
                };
                if out.len() > MAX_TOOL_RESULT_CHARS {
                    out.truncate(MAX_TOOL_RESULT_CHARS);
                    out.push_str("\n…(truncated)");
                }
                out
            }
            Err(e) => format!("ERROR: could not reach Farga at {}: {}", url, e),
        }
    }

    /// Generate a reply, letting Guilhem call read-only Farga tools to ground its answer.
    pub async fn reply(&self, history: &[ChatEvent], latest: &ChatEvent) -> Result<String> {
        let mut messages: Vec<serde_json::Value> = vec![serde_json::json!({
            "role": "user",
            "content": self.build_user_prompt(history, latest),
        })];

        for _round in 0..MAX_TOOL_ROUNDS {
            let body = serde_json::json!({
                "model": self.model,
                "max_tokens": MAX_TOKENS,
                "system": GUILHEM_SYSTEM,
                "tools": Self::tools(),
                "messages": messages,
            });

            let data: serde_json::Value = self
                .client
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| CharradissaError::Dispatch(e.to_string()))?
                .json()
                .await
                .map_err(|e| CharradissaError::Dispatch(e.to_string()))?;

            if let Some(err) = data.get("error") {
                return Err(CharradissaError::Dispatch(err.to_string()));
            }

            let content = data["content"].as_array().cloned().unwrap_or_default();
            let stop = data["stop_reason"].as_str().unwrap_or("");

            if stop == "tool_use" {
                // Record the assistant's turn (text + tool_use blocks) verbatim.
                messages.push(serde_json::json!({"role": "assistant", "content": content}));
                // Execute each tool_use block and collect tool_result blocks.
                let mut results = Vec::new();
                for block in &content {
                    if block["type"] == "tool_use" {
                        let name = block["name"].as_str().unwrap_or("");
                        let id = block["id"].as_str().unwrap_or("");
                        let result = self.execute_tool(name, &block["input"]).await;
                        results.push(serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": id,
                            "content": result,
                        }));
                    }
                }
                messages.push(serde_json::json!({"role": "user", "content": results}));
                continue;
            }

            // Terminal turn: concatenate any text blocks into the reply.
            let text: String = content
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("");
            return Ok(text);
        }

        // Exhausted the tool budget without a terminal answer.
        Ok(String::from(
            "(I reached my tool-call limit before composing a full answer — ask me to continue.)",
        ))
    }
}

pub fn should_respond(event: &ChatEvent, self_user_id: &str) -> bool {
    event.sender.as_str() != self_user_id
        && matches!(event.kind, ChatEventKind::Message | ChatEventKind::Mention)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatEvent, ChatEventKind, RoomId, UserId};
    use chrono::Utc;

    fn ev(sender: &str, body: &str) -> ChatEvent {
        ChatEvent {
            event_id: "$x".into(),
            room_id: RoomId::new("!r:occitane.guilhem"),
            sender: UserId::new(sender),
            content: body.into(),
            timestamp: Utc::now(),
            kind: ChatEventKind::Message,
        }
    }

    fn responder() -> Responder {
        Responder::new(
            "k".into(),
            "claude-sonnet-4-6".into(),
            "occitane.guilhem".into(),
            "http://farga:7500/".into(),
        )
    }

    #[test]
    fn prompt_includes_history_and_latest() {
        let r = responder();
        let hist = vec![
            ev("@p:occitane.guilhem", "hello"),
            ev("@guilhem:occitane.guilhem", "hi"),
        ];
        let latest = ev("@p:occitane.guilhem", "what is farga?");
        let p = r.build_user_prompt(&hist, &latest);
        assert!(p.contains("hello") && p.contains("what is farga?"));
        assert!(p.contains("@p:occitane.guilhem"));
    }

    #[test]
    fn ignores_self_and_nonmessages() {
        let me = "@guilhem:occitane.guilhem";
        assert!(!should_respond(&ev(me, "my own message"), me)); // no self-loop
        assert!(should_respond(&ev("@p:occitane.guilhem", "hi"), me)); // human → respond
        let mut join = ev("@p:occitane.guilhem", "");
        join.kind = ChatEventKind::MemberJoin;
        assert!(!should_respond(&join, me)); // only real messages
    }

    #[test]
    fn tools_are_read_only_farga_set() {
        let names: Vec<String> = Responder::tools()
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            names,
            vec!["farga_recent_signals", "farga_project_context", "farga_org_context"]
        );
    }

    #[tokio::test]
    async fn unknown_tool_returns_error_not_panic() {
        let r = responder();
        let out = r.execute_tool("not_a_tool", &serde_json::json!({})).await;
        assert!(out.starts_with("ERROR: unknown tool"));
    }

    #[test]
    fn farga_url_trailing_slash_trimmed() {
        let r = responder();
        // constructed with a trailing slash above; the tool URL must not double up.
        // (white-box: build a URL the way execute_tool does)
        assert!(!r.farga_url.ends_with('/'));
    }
}
