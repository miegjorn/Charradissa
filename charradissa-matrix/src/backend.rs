use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use charradissa_core::backend::ChatBackend;
use charradissa_core::error::Result;
use charradissa_core::types::*;
use crate::client::AppserviceClient;

pub struct MatrixBackend {
    client: Arc<AppserviceClient>,
}

impl MatrixBackend {
    pub fn new(homeserver: String, as_token: String, bot_user_id: String) -> Self {
        Self { client: Arc::new(AppserviceClient::new(homeserver, as_token, bot_user_id)) }
    }
}

#[async_trait]
impl ChatBackend for MatrixBackend {
    async fn send_message(&self, room: &RoomId, content: &str) -> Result<()> {
        self.client.send_message(room, content).await
    }

    async fn send_dm(&self, user: &UserId, content: &str) -> Result<()> {
        tracing::debug!("DM to {}: {}", user, content);
        Ok(())
    }

    async fn create_room(&self, opts: &RoomOptions) -> Result<RoomId> {
        self.client.create_room(&opts.alias, &opts.name).await
    }

    async fn create_space(&self, name: &str) -> Result<SpaceId> {
        Ok(SpaceId::new(&format!("!space-{}:homeserver", name)))
    }

    async fn add_to_space(&self, space: &SpaceId, room: &RoomId) -> Result<()> {
        tracing::debug!("add {} to space {}", room, space.as_str());
        Ok(())
    }

    async fn invite(&self, room: &RoomId, user: &UserId) -> Result<()> {
        self.client.invite(room, user).await
    }

    async fn kick(&self, room: &RoomId, user: &UserId, reason: &str) -> Result<()> {
        tracing::info!("kick {} from {} ({})", user, room, reason);
        Ok(())
    }

    async fn register_agent(&self, address: &CompositionAddress) -> Result<UserId> {
        let local_part = format!("charradissa-{}", uuid::Uuid::new_v4());
        self.client.register_agent(&local_part).await
    }

    async fn deregister_agent(&self, user: &UserId) -> Result<()> {
        tracing::info!("deregister agent: {}", user);
        Ok(())
    }

    async fn room_history(&self, _room: &RoomId, _since: DateTime<Utc>) -> Result<Vec<ChatEvent>> {
        Ok(vec![])
    }

    async fn delete_room(&self, room: &RoomId) -> Result<()> {
        tracing::info!("delete room: {}", room);
        Ok(())
    }
}
