// placeholder — filled in Task 3
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use crate::error::Result;
use crate::types::{ChatEvent, CompositionAddress, RoomId, RoomOptions, SpaceId, UserId};

#[async_trait]
pub trait ChatBackend: Send + Sync {
    async fn send_message(&self, room: &RoomId, content: &str) -> Result<()>;
    /// Send as a specific virtual user in the appservice namespace.
    /// Default delegates to `send_message` (ignores `sender_localpart`).
    async fn send_message_as(&self, room: &RoomId, content: &str, sender_localpart: Option<&str>) -> Result<()> {
        let _ = sender_localpart;
        self.send_message(room, content).await
    }
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
    async fn join_room(&self, room: &RoomId) -> Result<()> { Ok(()) }
    async fn joined_rooms(&self) -> Result<Vec<RoomId>> { Ok(vec![]) }
    /// Send a typing indicator for the bot user in `room`.
    /// `typing: true` = start indicator (auto-clears after `timeout_ms`).
    /// `typing: false` = clear immediately. Non-fatal — impls must not propagate errors.
    async fn set_typing(&self, room: &RoomId, user_id: &str, typing: bool, timeout_ms: u32) -> Result<()> {
        let _ = (room, user_id, typing, timeout_ms);
        Ok(())
    }

    /// Upload raw bytes to the Matrix media server. Returns the `mxc://` URI.
    /// Default is a no-op that returns an error — only Matrix backends implement this.
    async fn upload_media(&self, content_type: &str, data: Vec<u8>) -> Result<String> {
        let _ = (content_type, data);
        Err(crate::error::CharradissaError::Backend("upload_media not supported by this backend".into()))
    }

    /// Send an image message (`m.image`) to a room via an `mxc://` URI.
    /// Default is a no-op — only Matrix backends implement this.
    async fn send_image(&self, room: &RoomId, mxc_uri: &str, filename: &str) -> Result<()> {
        let _ = (room, mxc_uri, filename);
        Err(crate::error::CharradissaError::Backend("send_image not supported by this backend".into()))
    }
}
