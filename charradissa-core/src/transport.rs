use async_trait::async_trait;
use std::sync::Arc;
use amassada_core::channels::consult::{ConsultRequest, ConsultResponse};
use amassada_core::error::Result as AmassadaResult;
use amassada_core::error::AmassadaError;
use amassada_core::transport::Transport;
use amassada_core::types::{AgentId, HumanInput, SessionEvent, SessionOutput, WhisperMsg};
use amassada_core::dispatch;
use crate::backend::ChatBackend;
use crate::types::RoomId;

/// Implements the amassada_core Transport trait for Matrix rooms.
/// Translates Amassada session events into Matrix room operations.
pub struct CharradissaTransport {
    room_id: RoomId,
    backend: Arc<dyn ChatBackend>,
}

impl CharradissaTransport {
    pub fn new(room_id: RoomId, backend: Arc<dyn ChatBackend>) -> Self {
        Self { room_id, backend }
    }

    fn format_event(event: &SessionEvent) -> String {
        match event {
            SessionEvent::TurnCompleted { record } => {
                format!("**[{} / {}]**\n{}", record.agent_id, record.persona, record.content)
            }
            SessionEvent::RoundStarted { round } => {
                format!("--- Round {} ---", round)
            }
            SessionEvent::BtwEmitted { from, to, content } => {
                format!("*[btw from {} to {}]* {}", from, to, content)
            }
            SessionEvent::ApprovalRequested { reason } => {
                format!("Approval requested: {}", reason)
            }
            SessionEvent::ArtifactCompleted { title, .. } => {
                format!("Artifact ready: **{}**", title)
            }
            SessionEvent::SessionCompleted => "Session complete.".into(),
            SessionEvent::ModeratorAction { action } => {
                format!("*[moderator: {}]*", action)
            }
            _ => format!("{:?}", event),
        }
    }
}

#[async_trait]
impl Transport for CharradissaTransport {
    async fn broadcast(&self, event: &SessionEvent) -> AmassadaResult<()> {
        let text = Self::format_event(event);
        self.backend.send_message(&self.room_id, &text).await
            .map_err(|e| AmassadaError::Transport(e.to_string()))
    }

    async fn consult(&self, req: &ConsultRequest) -> AmassadaResult<ConsultResponse> {
        // Parse persona from agent_id (e.g. "builder-1" → "builder")
        let persona = req.target.to_string();
        let persona_hint = persona.split('-').next().unwrap_or("specialist");

        let system = format!(
            "You are {} — a specialist agent. \
             Answer the following question concisely and with practical, expert-level insight. \
             Respond only with the answer, no preamble.",
            persona_hint
        );

        let resp = dispatch::dispatch(dispatch::TurnRequest {
            system_prompt: system,
            context: req.question.clone(),
            model: "claude-haiku-4-5-20251001".to_string(),
            max_tokens: 1024,
            thinking_budget: None,
            api_key: None,
            shared_context: None,
            mcp_scopes: vec![],
        }).await.map_err(|e| AmassadaError::Transport(e.to_string()))?;

        Ok(ConsultResponse {
            from: req.target.clone(),
            content: resp.text,
        })
    }

    async fn whisper(&self, agent: &AgentId, msg: &WhisperMsg) -> AmassadaResult<()> {
        tracing::debug!("[whisper in room {} -> {}] {}", self.room_id, agent, msg.content);
        Ok(())
    }

    async fn recv_human(&self) -> Option<HumanInput> {
        None
    }

    async fn emit_output(&self, output: &SessionOutput) -> AmassadaResult<()> {
        let summary = format!("**Session Output** ({})\n\n{}",
            output.canvas_id,
            output.artifacts.iter()
                .map(|a| format!("### {}\n{}", a.title, a.content))
                .collect::<Vec<_>>()
                .join("\n\n")
        );
        self.backend.send_message(&self.room_id, &summary).await
            .map_err(|e| AmassadaError::Transport(e.to_string()))
    }
}
