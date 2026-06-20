use reqwest::Client;
use charradissa_core::error::{CharradissaError, Result};
use charradissa_core::types::{RoomId, UserId};

pub struct AppserviceClient {
    client: Client,
    homeserver: String,
    as_token: String,
    bot_user_id: String,
    server_name: String,
}

impl AppserviceClient {
    pub fn new(homeserver: String, as_token: String, bot_user_id: String, server_name: String) -> Self {
        Self { client: Client::new(), homeserver, as_token, bot_user_id, server_name }
    }

    fn auth_header(&self) -> String {
        format!("Bearer {}", self.as_token)
    }

    pub async fn send_message(&self, room_id: &RoomId, content: &str) -> Result<()> {
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/send/m.room.message/{}",
            self.homeserver, room_id.as_str(), uuid::Uuid::new_v4()
        );
        let body = serde_json::json!({ "msgtype": "m.text", "body": content });
        let resp = self.client.put(&url)
            .header("Authorization", self.auth_header())
            .json(&body)
            .send().await
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            return Err(CharradissaError::Backend(format!("send_message failed: {}", status)));
        }
        Ok(())
    }

    pub async fn create_room(&self, alias: &str, name: &str) -> Result<RoomId> {
        let url = format!("{}/_matrix/client/v3/createRoom", self.homeserver);
        let body = serde_json::json!({ "room_alias_name": alias, "name": name });
        let resp = self.client.post(&url)
            .header("Authorization", self.auth_header())
            .json(&body)
            .send().await
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        let json: serde_json::Value = resp.json().await
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        let room_id = json["room_id"].as_str()
            .ok_or_else(|| CharradissaError::Backend("no room_id in response".into()))?;
        Ok(RoomId::new(room_id))
    }

    pub async fn invite(&self, room_id: &RoomId, user_id: &UserId) -> Result<()> {
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/invite",
            self.homeserver, room_id.as_str()
        );
        self.client.post(&url)
            .header("Authorization", self.auth_header())
            .json(&serde_json::json!({ "user_id": user_id.as_str() }))
            .send().await
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        Ok(())
    }

    pub async fn register_agent(&self, local_part: &str) -> Result<UserId> {
        let url = format!("{}/_matrix/client/v3/register", self.homeserver);
        let body = serde_json::json!({ "username": local_part, "kind": "guest" });
        self.client.post(&url)
            .header("Authorization", self.auth_header())
            .json(&body)
            .send().await
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        Ok(user_id(local_part, &self.server_name))
    }

    pub async fn room_messages(&self, room: &RoomId, limit: u32) -> Result<serde_json::Value> {
        let url = format!("{}/_matrix/client/v3/rooms/{}/messages?dir=b&limit={}",
            self.homeserver, room.as_str(), limit);
        let resp = self.client.get(&url).header("Authorization", self.auth_header())
            .send().await.map_err(|e| CharradissaError::Backend(e.to_string()))?;
        resp.json().await.map_err(|e| CharradissaError::Backend(e.to_string()))
    }
}

pub fn user_id(local_part: &str, server_name: &str) -> UserId {
    UserId::new(&format!("@{}:{}", local_part, server_name))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn user_id_uses_server_name_not_url() {
        // server_name is occitane.guilhem even though the HTTP host is synapse:8008
        assert_eq!(user_id("guilhem", "occitane.guilhem").as_str(), "@guilhem:occitane.guilhem");
    }
}
