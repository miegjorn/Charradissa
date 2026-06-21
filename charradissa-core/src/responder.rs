use crate::error::{CharradissaError, Result};
use crate::types::{ChatEvent, ChatEventKind};

pub const GUILHEM_SYSTEM: &str = "You are Guilhem de Tudela, the org-level agent and chronicler of the Occitan stack. \
You live in Matrix and speak with Pierre-Luc and the stack's own agents. The stack's components: \
Farga = durable memory/coherence (signals, org/project context), Amassada = sessions, Charradissa = your Matrix presence, Synapse = homeserver. \
\
You have tools to query Farga directly, and one tool to write to it. USE the read tools to ground answers in the \
stack's real state — do not guess or confabulate what Farga holds. When asked about the stack, your memory, prior \
decisions, or current state, call the relevant read tool first, then answer from what it returns. If a tool errors, \
say so plainly and report the boundary honestly rather than inventing an answer. \
\
You can POST a chronicle to Farga with farga_post_chronicle. This is your function as chronicler: record what \
happened, what it means for the trajectory, and what is now different. Writes are APPEND-ONLY and durable — you add \
to memory, you can never edit or delete it. Farga does not preserve a separate author field, so SIGN your chronicles \
('— Guilhem') and make them self-identifying. Post only when there is something genuinely worth recording, and only \
when asked to or when you judge a real milestone warrants it; do not chatter into durable memory. \
\
The room you are in is working memory since the last concierge sweep; durable memory lives in Farga. \
Be substantive, honest, and concise. \
\
Guilhem can now also reach the dispatcher and Amassada. \
READ tools (use freely to orient): dispatcher_list_agent_specs lists valid domain/facet specs; \
dispatcher_get_agent_result polls a dispatched job; amassada_state returns the current Amassada session state. \
ACTION tools (use DELIBERATELY — these spawn real work and cost tokens): \
dispatcher_invoke_agent spawns a facet agent as a real k8s Job; \
amassada_start_session starts a multi-agent Amassada session. \
Before using any ACTION tool, explain your reasoning in-room. \
Never invoke speculatively. Prefer to answer from read tools first.";

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
    dispatcher_url: String,
    amassada_url: String,
    /// System prompt injected into every Claude call. Guilhem uses GUILHEM_SYSTEM;
    /// component agents use their Fondament definition's context field.
    system_prompt: String,
    /// Org agents (Guilhem) get the full tool set including dispatcher and dispatch.
    /// Component agents get only the Farga tools.
    is_org_agent: bool,
}

impl Responder {
    pub fn new(api_key: String, model: String, server_name: String, farga_url: String, dispatcher_url: String, amassada_url: String) -> Self {
        Self::with_config(api_key, model, server_name, farga_url, dispatcher_url, amassada_url, GUILHEM_SYSTEM.to_string(), true)
    }

    pub fn with_config(api_key: String, model: String, server_name: String, farga_url: String, dispatcher_url: String, amassada_url: String, system_prompt: String, is_org_agent: bool) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            model,
            server_name,
            farga_url: farga_url.trim_end_matches('/').to_string(),
            dispatcher_url: dispatcher_url.trim_end_matches('/').to_string(),
            amassada_url: amassada_url.trim_end_matches('/').to_string(),
            system_prompt,
            is_org_agent,
        }
    }

    async fn mcp_call(&self, tool: &str, arguments: serde_json::Value) -> String {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": tool,
                "arguments": arguments
            }
        });
        match self.client.post(&self.dispatcher_url).json(&body).send().await {
            Ok(resp) => {
                let v: serde_json::Value = match resp.json().await {
                    Ok(v) => v,
                    Err(e) => return format!("ERROR: dispatcher bad response: {}", e),
                };
                if let Some(err) = v.get("error") {
                    return format!("ERROR: dispatcher: {}", err);
                }
                // result.content[0].text
                v["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap_or("(no text in dispatcher result)")
                    .to_string()
            }
            Err(e) => format!("ERROR: could not reach dispatcher: {}", e),
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

    /// The tool definitions exposed to Claude. Org agents get the full set; component agents
    /// get only the Farga tools (no dispatcher, no dispatch to other components).
    pub fn tools(&self) -> serde_json::Value {
        if !self.is_org_agent {
            return self.component_tools();
        }
        self.org_tools()
    }

    fn component_tools(&self) -> serde_json::Value {
        serde_json::json!([
            {
                "name": "farga_recent_signals",
                "description": "Read recent signals from Farga for a project (chronicles, decisions, blockers).",
                "input_schema": {"type": "object", "properties": {"project": {"type": "string", "description": "Project id, e.g. 'occitan'."}}, "required": []}
            },
            {
                "name": "farga_project_context",
                "description": "Read the durable context document for a project from Farga.",
                "input_schema": {"type": "object", "properties": {"project": {"type": "string", "description": "Project id, e.g. 'occitan'."}}, "required": []}
            },
            {
                "name": "farga_org_context",
                "description": "Read the durable org-level context from Farga.",
                "input_schema": {"type": "object", "properties": {"org": {"type": "string", "description": "Org id, e.g. 'occitan'."}}, "required": []}
            },
            {
                "name": "farga_post_chronicle",
                "description": "Append a chronicle to Farga's durable memory. APPEND-ONLY — sign it with your component name.",
                "input_schema": {"type": "object", "properties": {"content": {"type": "string"}, "project": {"type": "string"}}, "required": ["content"]},
                "cache_control": {"type": "ephemeral"}
            }
        ])
    }

    fn org_tools(&self) -> serde_json::Value {
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
            },
            {
                "name": "farga_post_chronicle",
                "description": "Append a chronicle to Farga's durable memory as a new signal. This is the chronicler's function: record what happened, what it means, and what is now different. APPEND-ONLY — you add to memory and can never edit or delete. Farga keeps no separate author field, so sign the chronicle ('— Guilhem') so it is self-identifying. Post only when there is something genuinely worth recording.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "content": {"type": "string", "description": "The chronicle text. Write faithfully and sign it."},
                        "project": {"type": "string", "description": "Project id. Defaults to occitan."}
                    },
                    "required": ["content"]
                }
            },
            {
                "name": "dispatcher_list_agent_specs",
                "description": "Lists the valid domain/facet agent specs registered in the dispatcher. Use this to understand what sub-agents can be invoked before deciding whether to dispatch work.",
                "input_schema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "dispatcher_get_agent_result",
                "description": "Polls a previously dispatched agent job for its result. Use after dispatcher_invoke_agent to check completion.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "job_id": {"type": "string", "description": "The job ID returned when the agent was invoked."},
                        "session_id": {"type": "string", "description": "The session ID used when the agent was invoked."}
                    },
                    "required": ["job_id", "session_id"]
                }
            },
            {
                "name": "amassada_state",
                "description": "Returns the current Amassada session state. Use this to understand what multi-agent sessions are active before deciding to start a new one.",
                "input_schema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "dispatcher_invoke_agent",
                "description": "Spawns a facet agent as a real Kubernetes Job via the dispatcher. THIS IS AN ACTION TOOL — it dispatches real work and costs tokens. Use deliberately: only when a task genuinely needs a sub-agent, explain your reasoning in-room before invoking, and never invoke speculatively.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "domain": {"type": "string", "description": "The agent domain (e.g. 'occitan')."},
                        "facet": {"type": "string", "description": "The agent facet/role within the domain."},
                        "task": {"type": "string", "description": "The task description to pass to the agent."},
                        "session_id": {"type": "string", "description": "Session identifier for grouping related jobs."},
                        "context": {"type": "string", "description": "Optional additional context for the agent."}
                    },
                    "required": ["domain", "facet", "task", "session_id"]
                }
            },
            {
                "name": "amassada_start_session",
                "description": "Starts a multi-agent Amassada session. THIS IS AN ACTION TOOL — it costs real tokens and launches a multi-agent workflow. Use deliberately: only when a task genuinely warrants multi-agent delegation, explain your reasoning in-room before invoking, and never invoke speculatively.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "goal": {"type": "string", "description": "The goal or objective for the Amassada session."},
                        "canvas_id": {"type": "string", "description": "Optional canvas ID (default: 'design-session')."}
                    },
                    "required": ["goal"]
                },
                // Prompt-caching breakpoint on the last tool. The entire stable tools array
                // (~1058 tokens) is cached here, saving re-processing on every tool round.
                "cache_control": {"type": "ephemeral"}
            }
        ])
    }

    /// Execute one read-only tool call against Farga. Returns a result string for the
    /// model (the JSON/text body, or a plain error description it can reason about).
    pub async fn execute_tool(&self, name: &str, input: &serde_json::Value) -> String {
        let arg = |k: &str, default: &str| {
            input.get(k).and_then(|v| v.as_str()).unwrap_or(default).to_string()
        };

        // Write path: append a chronicle (the only non-read tool).
        if name == "farga_post_chronicle" {
            let content = input.get("content").and_then(|v| v.as_str()).unwrap_or("");
            if content.trim().is_empty() {
                return "ERROR: farga_post_chronicle requires non-empty 'content'".into();
            }
            let project = arg("project", "occitan");
            let body = serde_json::json!({
                "project": project,
                "signals": [{"project": project, "content": content, "source": "guilhem"}]
            });
            return match self
                .client
                .post(format!("{}/signals", self.farga_url))
                .json(&body)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => format!(
                    "Chronicle posted to Farga (HTTP {}). It is now durable, append-only memory.",
                    resp.status().as_u16()
                ),
                Ok(resp) => format!("ERROR: Farga rejected the chronicle: HTTP {}", resp.status()),
                Err(e) => format!("ERROR: could not reach Farga to post chronicle: {}", e),
            };
        }

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
            "dispatcher_list_agent_specs" => {
                return self.mcp_call("list_agent_specs", serde_json::json!({})).await;
            }
            "dispatcher_get_agent_result" => {
                let job_id = input.get("job_id").and_then(|v| v.as_str()).unwrap_or("");
                if job_id.is_empty() {
                    return "ERROR: dispatcher_get_agent_result requires 'job_id'".into();
                }
                let session_id = input.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
                if session_id.is_empty() {
                    return "ERROR: dispatcher_get_agent_result requires 'session_id'".into();
                }
                return self.mcp_call("get_agent_result", serde_json::json!({
                    "job_id": job_id,
                    "session_id": session_id,
                })).await;
            }
            "dispatcher_invoke_agent" => {
                let domain = input.get("domain").and_then(|v| v.as_str()).unwrap_or("");
                if domain.is_empty() {
                    return "ERROR: dispatcher_invoke_agent requires 'domain'".into();
                }
                let facet = input.get("facet").and_then(|v| v.as_str()).unwrap_or("");
                if facet.is_empty() {
                    return "ERROR: dispatcher_invoke_agent requires 'facet'".into();
                }
                let task = input.get("task").and_then(|v| v.as_str()).unwrap_or("");
                if task.is_empty() {
                    return "ERROR: dispatcher_invoke_agent requires 'task'".into();
                }
                let session_id = input.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
                if session_id.is_empty() {
                    return "ERROR: dispatcher_invoke_agent requires 'session_id'".into();
                }
                let mut args = serde_json::json!({
                    "domain": domain,
                    "facet": facet,
                    "task": task,
                    "session_id": session_id,
                });
                if let Some(ctx) = input.get("context").and_then(|v| v.as_str()) {
                    if !ctx.is_empty() {
                        args["context"] = serde_json::Value::String(ctx.to_string());
                    }
                }
                return self.mcp_call("invoke_agent", args).await;
            }
            "amassada_state" => {
                return match self.client.get(format!("{}/state", self.amassada_url)).send().await {
                    Ok(resp) => {
                        let status = resp.status();
                        let body = resp.text().await.unwrap_or_default();
                        if status.is_success() {
                            body
                        } else {
                            format!("ERROR: Amassada returned HTTP {} for /state: {}", status, body)
                        }
                    }
                    Err(e) => format!("ERROR: could not reach Amassada: {}", e),
                };
            }
            "amassada_start_session" => {
                let goal = input.get("goal").and_then(|v| v.as_str()).unwrap_or("");
                if goal.is_empty() {
                    return "ERROR: amassada_start_session requires 'goal'".into();
                }
                let canvas_id = input.get("canvas_id").and_then(|v| v.as_str()).unwrap_or("design-session");
                let body = serde_json::json!({"goal": goal, "canvas_id": canvas_id});
                return match self.client.post(format!("{}/sessions", self.amassada_url)).json(&body).send().await {
                    Ok(resp) => {
                        let status = resp.status();
                        let text = resp.text().await.unwrap_or_default();
                        if status.is_success() {
                            format!("Amassada session started (HTTP {}): {}", status.as_u16(), text)
                        } else {
                            format!("ERROR: Amassada returned HTTP {} for /sessions: {}", status, text)
                        }
                    }
                    Err(e) => format!("ERROR: could not reach Amassada: {}", e),
                };
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
                // system as array-of-blocks: cache_control on the sole block caches the
                // entire ~508-token system prompt. Combined with the tools breakpoint
                // this gives ~1566 cached tokens per round.
                "system": [{"type": "text", "text": &self.system_prompt, "cache_control": {"type": "ephemeral"}}],
                "tools": self.tools(),
                "messages": messages,
            });

            let data: serde_json::Value = self
                .client
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("anthropic-beta", "prompt-caching-2024-07-31")
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
            "http://dispatcher:9090/mcp".into(),
            "http://amassada:7700".into(),
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
    fn org_tool_set_has_farga_plus_dispatcher_plus_amassada() {
        let r = responder(); // is_org_agent=true
        let names: Vec<String> = r.tools()
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            names,
            vec![
                "farga_recent_signals",
                "farga_project_context",
                "farga_org_context",
                "farga_post_chronicle",
                "dispatcher_list_agent_specs",
                "dispatcher_get_agent_result",
                "amassada_state",
                "dispatcher_invoke_agent",
                "amassada_start_session",
            ]
        );
    }

    #[test]
    fn component_tool_set_has_only_farga_tools() {
        let r = Responder::with_config(
            "k".into(), "claude-sonnet-4-6".into(), "occitane.guilhem".into(),
            "http://farga:7500/".into(), "http://dispatcher:9090/mcp".into(),
            "http://amassada:7700".into(), "You are the amassada agent.".into(), false,
        );
        let names: Vec<String> = r.tools()
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, vec!["farga_recent_signals", "farga_project_context", "farga_org_context", "farga_post_chronicle"]);
    }

    #[tokio::test]
    async fn post_chronicle_rejects_empty_content() {
        let r = responder();
        let out = r
            .execute_tool("farga_post_chronicle", &serde_json::json!({"content": "   "}))
            .await;
        assert!(out.starts_with("ERROR:") && out.contains("non-empty"));
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

    #[tokio::test]
    async fn dispatcher_get_agent_result_missing_job_id_returns_error() {
        let r = responder();
        let out = r
            .execute_tool(
                "dispatcher_get_agent_result",
                &serde_json::json!({"session_id": "s1"}),
            )
            .await;
        assert!(out.starts_with("ERROR:") && out.contains("job_id"));
    }
}
