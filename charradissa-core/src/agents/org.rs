use std::sync::Arc;
use crate::backend::ChatBackend;
use crate::types::*;

pub struct OrgAgent {
    pub org: String,
    pub user_id: UserId,
    pub general_room: RoomId,
    backend: Arc<dyn ChatBackend>,
}

impl OrgAgent {
    pub fn new(org: String, user_id: UserId, general_room: RoomId, backend: Arc<dyn ChatBackend>) -> Self {
        Self { org, user_id, general_room, backend }
    }

    pub async fn handle_event(&self, event: &ChatEvent) -> crate::error::Result<()> {
        tracing::debug!("org agent received event from {}", event.sender);
        Ok(())
    }

    pub async fn broadcast_org(&self, message: &str) -> crate::error::Result<()> {
        self.backend.send_message(&self.general_room, message).await
    }
}
