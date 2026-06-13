use std::sync::Arc;
use crate::approval::{ApprovalOutcome, ApprovalQueue};
use crate::backend::ChatBackend;
use crate::task::TaskManager;
use crate::tool_loop::{parse_slash_command, SlashCommand};
use crate::types::*;

pub struct ProjectAgent {
    pub project: ProjectId,
    pub user_id: UserId,
    pub main_room: RoomId,
    pub space: SpaceId,
    backend: Arc<dyn ChatBackend>,
    task_manager: Arc<dyn TaskManager>,
    approval_queue: Arc<tokio::sync::Mutex<ApprovalQueue>>,
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
    ) -> Self {
        Self { project, user_id, main_room, space, backend, task_manager, approval_queue }
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
                tracing::info!("project {}: session requested — canvas={} goal={}",
                    self.project, canvas_id, goal);
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
