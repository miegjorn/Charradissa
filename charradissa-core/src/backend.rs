// placeholder — filled in Task 3
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use crate::error::Result;
use crate::types::{ChatEvent, CompositionAddress, RoomId, RoomOptions, SpaceId, UserId};

#[async_trait]
pub trait ChatBackend: Send + Sync {
    async fn send_message(&self, room: &RoomId, content: &str) -> Result<()>;
    async fn send_dm(&self, user: &UserId, content: &str) -> Result<()>;
    async fn create_room(&self, opts: &RoomOptions) -> Result<RoomId>;
    async fn create_space(&self, name: &str) -> Result<SpaceId>;
    async fn add_to_space(&self, space: &SpaceId, room: &RoomId) -> Result<()>;
    async fn invite(&self, room: &RoomId, user: &UserId) -> Result<()>;
    async fn kick(&self, room: &RoomId, user: &UserId, reason: &str) -> Result<()>;
    async fn register_agent(&self, address: &CompositionAddress) -> Result<UserId>;
    async fn deregister_agent(&self, user: &UserId) -> Result<()>;
    async fn room_history(&self, room: &RoomId, since: DateTime<Utc>) -> Result<Vec<ChatEvent>>;
    async fn delete_room(&self, room: &RoomId) -> Result<()>;
}
