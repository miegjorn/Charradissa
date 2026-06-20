use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use charradissa_core::backend::ChatBackend;
use charradissa_core::error::Result;
use charradissa_core::types::*;
use crate::client::AppserviceClient;
use serde_json;

pub struct MatrixBackend {
    client: Arc<AppserviceClient>,
}

impl MatrixBackend {
    pub fn new(homeserver: String, as_token: String, bot_user_id: String, server_name: String) -> Self {
        Self { client: Arc::new(AppserviceClient::new(homeserver, as_token, bot_user_id, server_name)) }
    }

    /// Materialize the appservice sender user (so profile writes work).
    pub async fn ensure_registered(&self) -> Result<()> {
        self.client.register_self().await
    }

    /// Set the display name of the appservice sender (guilhem).
    pub async fn set_self_display_name(&self, name: &str) -> Result<()> {
        let user_id = self.client.bot_user_id().to_string();
        self.client.set_display_name(&user_id, name).await
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

    async fn room_history(&self, room: &RoomId, _since: DateTime<Utc>) -> Result<Vec<ChatEvent>> {
        let body = self.client.room_messages(room, HISTORY_LIMIT).await?;
        Ok(parse_messages_chunk(&body, room))
    }

    async fn delete_room(&self, room: &RoomId) -> Result<()> {
        tracing::info!("delete room: {}", room);
        Ok(())
    }

    async fn join_room(&self, room: &RoomId) -> Result<()> {
        self.client.join_room(room.as_str()).await.map(|_| ())
    }

    async fn joined_rooms(&self) -> Result<Vec<RoomId>> {
        self.client.joined_rooms().await
    }
}

/// Number of recent messages fed to guilhem as conversational context each turn.
pub const HISTORY_LIMIT: u32 = 20;

pub fn parse_messages_chunk(body: &serde_json::Value, room: &RoomId) -> Vec<ChatEvent> {
    let mut evs: Vec<ChatEvent> = body["chunk"].as_array().cloned().unwrap_or_default().iter()
        .filter(|e| e["type"] == "m.room.message")
        .filter_map(|e| Some(ChatEvent {
            event_id: e["event_id"].as_str()?.to_string(),
            room_id: room.clone(),
            sender: UserId::new(e["sender"].as_str()?),
            content: e["content"]["body"].as_str().unwrap_or("").to_string(),
            timestamp: Utc::now(),
            kind: ChatEventKind::Message,
        }))
        .collect();
    evs.reverse(); // /messages dir=b is newest-first; callers want oldest-first
    evs
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_messages_response_oldest_first() {
        let body = serde_json::json!({"chunk":[
            {"type":"m.room.message","event_id":"$b","sender":"@p:occitane.guilhem","origin_server_ts":2,"content":{"msgtype":"m.text","body":"second"}},
            {"type":"m.room.message","event_id":"$a","sender":"@p:occitane.guilhem","origin_server_ts":1,"content":{"msgtype":"m.text","body":"first"}}
        ]});
        let evs = parse_messages_chunk(&body, &RoomId::new("!r:occitane.guilhem"));
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].content, "first"); // dir=b returns newest-first; we reverse to oldest-first
        assert_eq!(evs[1].content, "second");
    }
}
