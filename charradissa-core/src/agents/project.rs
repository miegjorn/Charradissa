use std::sync::Arc;
use amassada_core::canvas::Canvas;
use amassada_core::canvas_library;
use amassada_core::error::AmassadaError;
use amassada_core::mission::engine::MissionEngine;
use amassada_core::mission::evaluator::ClaudeEvaluator;
use amassada_core::mission::meta::ClaudeMetaModerator;
use amassada_core::mission::session_runner::DefaultSessionRunner;
use amassada_core::mission::types::{FargaVerdict, SubObjective, SubObjectiveStatus};
use amassada_core::session::SessionEngine;
use amassada_core::transport::Transport;
use crate::approval::{ApprovalOutcome, ApprovalQueue};
use crate::backend::ChatBackend;
use crate::error::CharradissaError;
use crate::farga::{FargaWriter, MissionContribution};
use crate::task::TaskManager;
use crate::tool_loop::{parse_slash_command, SlashCommand};
use crate::transport::CharradissaTransport;
use crate::types::*;

pub struct ProjectAgent {
    pub project: ProjectId,
    pub user_id: UserId,
    pub main_room: RoomId,
    pub space: SpaceId,
    backend: Arc<dyn ChatBackend>,
    task_manager: Arc<dyn TaskManager>,
    approval_queue: Arc<tokio::sync::Mutex<ApprovalQueue>>,
    farga_writer: Option<Arc<dyn FargaWriter>>,
}

impl ProjectAgent {
    pub fn new(
        project: ProjectId,
        user_id: UserId,
        main_room: RoomId,
        space: SpaceId,
        backend: Arc<dyn ChatBackend>,
        task_manager: Arc<dyn TaskManager>,
        approval_queue: Arc<tokio::sync::Mutex<ApprovalQueue>>,
        farga_writer: Option<Arc<dyn FargaWriter>>,
    ) -> Self {
        Self { project, user_id, main_room, space, backend, task_manager, approval_queue, farga_writer }
    }

    pub async fn handle_event(&self, event: &ChatEvent) -> crate::error::Result<()> {
        if let ChatEventKind::SlashCommand { command, args } = &event.kind {
            let full = format!("/{} {}", command, args);
            if let Some(cmd) = parse_slash_command(&full) {
                return self.handle_slash_command(cmd, &event.sender).await;
            }
        }

        if let Some(cmd) = parse_slash_command(&event.content) {
            match cmd {
                SlashCommand::Approve { id } => {
                    let mut q = self.approval_queue.lock().await;
                    let _ = q.resolve(&id, ApprovalOutcome::Approved);
                }
                SlashCommand::Reject { id, reason } => {
                    let mut q = self.approval_queue.lock().await;
                    let _ = q.resolve(&id, ApprovalOutcome::Rejected(reason));
                }
                _ => {}
            }
        }

        Ok(())
    }

    async fn handle_slash_command(&self, cmd: SlashCommand, _from: &UserId) -> crate::error::Result<()> {
        match cmd {
            SlashCommand::Session { canvas_id, goal } => {
                self.run_session(canvas_id, goal).await?;
            }
            SlashCommand::Mission { goal, budget_tokens } => {
                self.run_mission(goal, budget_tokens).await?;
            }
            SlashCommand::Invite { address } => {
                let addr = CompositionAddress::Role { role: address, stance_override: None };
                let specialist_id = self.backend.register_agent(&addr).await?;
                self.backend.invite(&self.main_room, &specialist_id).await?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Runs a single `SessionEngine` with the given canvas and goal.
    async fn run_session(&self, canvas_id: String, goal: String) -> crate::error::Result<()> {
        tracing::info!("project {}: starting session — canvas={} goal={}", self.project, canvas_id, goal);

        let canvases = canvas_library::stdlib();
        let canvas = canvases.get(&canvas_id)
            .cloned()
            .unwrap_or_else(|| {
                tracing::warn!("canvas '{}' not found in stdlib, falling back to design-session", canvas_id);
                canvases["design-session"].clone()
            });

        let transport = Arc::new(CharradissaTransport::new(
            self.main_room.clone(),
            Arc::clone(&self.backend),
        ));

        let output = SessionEngine::new(canvas, goal, transport as Arc<dyn Transport>)
            .run()
            .await
            .map_err(|e| CharradissaError::Dispatch(e.to_string()))?;

        tracing::info!("project {}: session complete — {} artifacts, {} tokens",
            self.project, output.artifacts.len(), output.total_tokens);

        Ok(())
    }

    /// Runs a `MissionEngine` with the given goal and budget, submitting to Farga if warranted.
    async fn run_mission(&self, goal: String, budget_tokens: u64) -> crate::error::Result<()> {
        tracing::info!("project {}: starting mission — goal={} budget={}", self.project, goal, budget_tokens);

        let transport = Arc::new(CharradissaTransport::new(
            self.main_room.clone(),
            Arc::clone(&self.backend),
        ));

        let runner = Box::new(DefaultSessionRunner::new(transport as Arc<dyn Transport>));
        let evaluator = Box::new(ClaudeEvaluator::default());
        let meta = Box::new(ClaudeMetaModerator::default());

        let sub_objectives = vec![SubObjective {
            id: "main".into(),
            description: goal.clone(),
            completion_condition: format!("The goal is accomplished: {}", goal),
            status: SubObjectiveStatus::Pending,
            output: None,
            last_eval_reason: None,
        }];

        let mut engine = MissionEngine::new(
            goal.clone(),
            format!("The overall mission goal is accomplished: {}", goal),
            sub_objectives,
            budget_tokens,
            runner,
            evaluator,
            meta,
        );

        let canvases = canvas_library::stdlib();
        let outcome = engine.run(move |id: &str| canvases.get(id).cloned())
            .await
            .map_err(|e| CharradissaError::Dispatch(e.to_string()))?;

        tracing::info!("project {}: mission complete — exhausted={} completed={:?}",
            self.project, outcome.exhausted, outcome.completed_sub_objective_ids);

        if let FargaVerdict::Submit { ref contribution } = outcome.verdict {
            if let Some(ref writer) = self.farga_writer {
                let mc = MissionContribution {
                    mission_id: outcome.mission_id.clone(),
                    title: contribution.title.clone(),
                    narrative: contribution.narrative.clone(),
                    goal: goal.clone(),
                    sessions_run: outcome.metadata.sessions_run,
                    total_tokens_spent: outcome.metadata.total_tokens_spent,
                    duration_secs: outcome.metadata.duration_secs,
                };
                match writer.submit_mission_contribution(mc).await {
                    Ok(id) => tracing::info!("mission contribution submitted to Farga — id={}", id),
                    Err(e) => tracing::warn!("Farga submission failed: {}", e),
                }
            } else {
                tracing::info!("no Farga writer configured — skipping submission for mission {}",
                    outcome.mission_id);
            }
        }

        Ok(())
    }

    pub async fn create_task(&self, opts: &TaskOptions) -> crate::error::Result<TaskId> {
        self.task_manager.create_task(&self.project, opts).await
    }

    pub async fn dispatch_implementation(&self, ticket_id: &TaskId) -> crate::error::Result<()> {
        self.task_manager.update_status(ticket_id, TaskStatus::InProgress).await?;
        let room_alias = format!("#{}-impl-{}", self.project.as_str(), ticket_id.as_str());
        let impl_room = self.backend.create_room(&RoomOptions {
            alias: room_alias.clone(),
            name: format!("impl: {}", ticket_id.as_str()),
            topic: None,
            invite: vec![self.user_id.clone()],
        }).await?;
        self.backend.add_to_space(&self.space, &impl_room).await?;
        self.task_manager.update_status(ticket_id, TaskStatus::InReview).await?;
        Ok(())
    }
}
