use reqwest::Client;
use charradissa_core::error::{CharradissaError, Result};
use charradissa_core::types::{RoomId, UserId};

/// Component agent local parts in the Charradissa appservice namespace.
/// Kept here as the canonical source — matches charradissa-registration.yaml namespaces.
/// Charradissa sets these users to PL 50 (kick power) in every room it creates or joins.
pub const AGENT_LOCAL_PARTS: &[&str] = &[
    "gardian", "fondament", "farga", "amassada", "cor", "caissa", "charradissa-agent",
];

pub struct AppserviceClient {
    client: Client,
    homeserver: String,
    as_token: String,
    bot_user_id: String,
    server_name: String,
}

/// Percent-encode a Matrix path segment (room ID / user ID / alias).
/// Encodes `!`, `#`, `@`, `:` so they survive as a URL path component.
pub fn pct(s: &str) -> String {
    s.chars().map(|c| match c {
        '!' | '#' | '@' | ':' | '/' | '?' | '&' | '=' | '+' | ' ' => {
            format!("%{:02X}", c as u32)
        }
        _ => c.to_string(),
    }).collect()
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
        // Pre-set power levels at creation: all component agents get PL 50 (kick power).
        // The bot_user_id (sender) is already PL 100 as the room creator.
        let agent_users: serde_json::Value = AGENT_LOCAL_PARTS.iter()
            .map(|lp| (format!("@{}:{}", lp, self.server_name), serde_json::json!(50)))
            .collect::<serde_json::Map<_, _>>()
            .into();
        let body = serde_json::json!({
            "room_alias_name": alias,
            "name": name,
            "power_level_content_override": { "users": agent_users, "kick": 50 }
        });
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

    /// Read the current m.room.power_levels state for a room.
    pub async fn get_power_levels(&self, room_id: &RoomId) -> Result<serde_json::Value> {
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/state/m.room.power_levels",
            self.homeserver, pct(room_id.as_str())
        );
        let resp = self.client.get(&url)
            .header("Authorization", self.auth_header())
            .send().await
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        resp.error_for_status()
            .map_err(|e| CharradissaError::Backend(format!("get_power_levels: {}", e)))?
            .json().await
            .map_err(|e| CharradissaError::Backend(e.to_string()))
    }

    /// Grant kick power (PL 50) to all component agents in a room.
    /// Reads the current power_levels state first to avoid clobbering other settings.
    pub async fn grant_agent_kick_power(&self, room_id: &RoomId) -> Result<()> {
        let mut pl = self.get_power_levels(room_id).await?;
        let users = pl["users"].as_object_mut()
            .ok_or_else(|| CharradissaError::Backend("power_levels has no users map".into()))?;
        let mut changed = false;
        for lp in AGENT_LOCAL_PARTS {
            let uid = format!("@{}:{}", lp, self.server_name);
            let current = users.get(&uid).and_then(|v| v.as_i64()).unwrap_or(0);
            if current < 50 {
                users.insert(uid, serde_json::json!(50));
                changed = true;
            }
        }
        if !changed {
            return Ok(());
        }
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/state/m.room.power_levels",
            self.homeserver, pct(room_id.as_str())
        );
        let resp = self.client.put(&url)
            .header("Authorization", self.auth_header())
            .json(&pl)
            .send().await
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            return Err(CharradissaError::Backend(format!("set_power_levels failed: {}", status)));
        }
        Ok(())
    }

    /// Kick a user from a room.
    pub async fn kick_user(&self, room_id: &RoomId, user_id: &UserId, reason: &str) -> Result<()> {
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/kick",
            self.homeserver, pct(room_id.as_str())
        );
        let body = serde_json::json!({ "user_id": user_id.as_str(), "reason": reason });
        let resp = self.client.post(&url)
            .header("Authorization", self.auth_header())
            .json(&body)
            .send().await
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            return Err(CharradissaError::Backend(format!("kick failed: {}", status)));
        }
        Ok(())
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

    /// Materialize the appservice's own sender user so profile writes (display name)
    /// succeed. Without this synapse has no profile row and the displayname PUT 500s.
    /// Best-effort and idempotent (an already-registered user is fine).
    pub async fn register_self(&self) -> Result<()> {
        let local_part = self
            .bot_user_id
            .trim_start_matches('@')
            .split(':')
            .next()
            .unwrap_or("charradissa")
            .to_string();
        let url = format!("{}/_matrix/client/v3/register", self.homeserver);
        let body = serde_json::json!({
            "type": "m.login.application_service",
            "username": local_part,
        });
        self.client
            .post(&url)
            .header("Authorization", self.auth_header())
            .json(&body)
            .send()
            .await
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        Ok(())
    }

    pub async fn room_messages(&self, room: &RoomId, limit: u32) -> Result<serde_json::Value> {
        let url = format!("{}/_matrix/client/v3/rooms/{}/messages?dir=b&limit={}",
            self.homeserver, pct(room.as_str()), limit);
        let resp = self.client.get(&url).header("Authorization", self.auth_header())
            .send().await.map_err(|e| CharradissaError::Backend(e.to_string()))?;
        let resp = resp.error_for_status().map_err(|e| CharradissaError::Backend(e.to_string()))?;
        resp.json().await.map_err(|e| CharradissaError::Backend(e.to_string()))
    }

    pub fn bot_user_id(&self) -> &str {
        &self.bot_user_id
    }

    pub async fn joined_rooms(&self) -> Result<Vec<RoomId>> {
        let url = format!("{}/_matrix/client/v3/joined_rooms", self.homeserver);
        let resp = self.client.get(&url).header("Authorization", self.auth_header())
            .send().await.map_err(|e| CharradissaError::Backend(e.to_string()))?;
        let resp = resp.error_for_status().map_err(|e| CharradissaError::Backend(e.to_string()))?;
        let j: serde_json::Value = resp.json().await.map_err(|e| CharradissaError::Backend(e.to_string()))?;
        Ok(j["joined_rooms"].as_array().cloned().unwrap_or_default()
            .iter().filter_map(|v| v.as_str().map(RoomId::new)).collect())
    }

    pub async fn join_room(&self, alias_or_id: &str) -> Result<RoomId> {
        let url = format!("{}/_matrix/client/v3/join/{}", self.homeserver, pct(alias_or_id));
        let resp = self.client.post(&url)
            .header("Authorization", self.auth_header())
            .json(&serde_json::json!({}))
            .send().await
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            return Err(CharradissaError::Backend(format!("join_room failed: {}", status)));
        }
        let json: serde_json::Value = resp.json().await
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        let room_id = json["room_id"].as_str()
            .ok_or_else(|| CharradissaError::Backend("no room_id in join response".into()))?;
        Ok(RoomId::new(room_id))
    }

    pub async fn set_display_name(&self, user_id: &str, name: &str) -> Result<()> {
        let url = format!(
            "{}/_matrix/client/v3/profile/{}/displayname",
            self.homeserver, pct(user_id)
        );
        let body = serde_json::json!({ "displayname": name });
        let resp = self.client.put(&url)
            .header("Authorization", self.auth_header())
            .json(&body)
            .send().await
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            return Err(CharradissaError::Backend(format!("set_display_name failed: {}", status)));
        }
        Ok(())
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

    #[test]
    fn pct_encodes_matrix_sigils_and_colon() {
        // Room IDs and aliases must be percent-encoded to survive as a URL path segment.
        assert_eq!(pct("!room:server"), "%21room%3Aserver");
        assert_eq!(pct("#alias:server"), "%23alias%3Aserver");
        assert_eq!(pct("@user:server"), "%40user%3Aserver");
        // Plain ASCII letters are passed through unchanged.
        assert_eq!(pct("plain"), "plain");
    }
}
