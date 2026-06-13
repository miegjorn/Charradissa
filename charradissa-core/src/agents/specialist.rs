use std::sync::Arc;
use crate::backend::ChatBackend;
use crate::types::*;

pub struct Specialist {
    pub user_id: UserId,
    pub address: CompositionAddress,
    pub room_id: RoomId,
    backend: Arc<dyn ChatBackend>,
}

impl Specialist {
    pub async fn provision(
        address: CompositionAddress,
        room_id: RoomId,
        backend: Arc<dyn ChatBackend>,
    ) -> crate::error::Result<Self> {
        let user_id = backend.register_agent(&address).await?;
        backend.invite(&room_id, &user_id).await?;
        Ok(Self { user_id, address, room_id, backend })
    }

    pub async fn deprovision(self) -> crate::error::Result<()> {
        self.backend.kick(&self.room_id, &self.user_id, "session complete").await?;
        self.backend.deregister_agent(&self.user_id).await?;
        Ok(())
    }
}
