use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use charradissa_core::backend::ChatBackend;
use charradissa_core::error::Result;
use charradissa_core::responder::Responder;
use charradissa_core::types::*;
use crate::client::AppserviceClient;
use serde_json;

pub struct RoomProvisioningParams {
    pub farga_url: String,
    pub fondament_url: String,
    pub anthropic_api_key: String,
    pub dispatcher_url: String,
    pub amassada_url: String,
}

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

    /// Grant kick power (PL 50) to all component agents in every room Charradissa is in.
    /// Idempotent — rooms where agents already have PL ≥ 50 are left untouched.
    pub async fn provision_agent_kick_power(&self) -> Result<()> {
        let rooms = self.client.joined_rooms().await?;
        tracing::info!("provisioning kick power in {} rooms", rooms.len());
        for room in &rooms {
            if let Err(e) = self.client.grant_agent_kick_power(room).await {
                tracing::warn!("kick power grant failed for {}: {}", room.as_str(), e);
            }
        }
        Ok(())
    }

    /// Discover project components from Farga, resolve system prompts from Fondament,
    /// create-or-join aliased rooms, and return a room_id → Responder map.
    pub async fn provision_project_rooms(
        &self,
        project: &str,
        params: &RoomProvisioningParams,
    ) -> Result<HashMap<RoomId, Arc<Responder>>> {
        let http = reqwest::Client::new();

        // 1. Fetch component list from Farga.
        let components_url = format!("{}/context/components/{}", params.farga_url, project);
        let components: Vec<String> = http.get(&components_url)
            .send().await
            .map_err(|e| charradissa_core::error::CharradissaError::Backend(
                format!("Farga component list failed: {}", e)
            ))?
            .json().await
            .map_err(|e| charradissa_core::error::CharradissaError::Backend(
                format!("Farga component list parse failed: {}", e)
            ))?;

        if components.is_empty() {
            tracing::warn!("no components found for project '{}' in Farga", project);
            return Ok(HashMap::new());
        }

        // 2. Create-or-join project room (no Responder — Guilhem HTTP handles it).
        if let Err(e) = self.client.create_or_join_aliased_room(project, &format!("{} project", project)).await {
            tracing::warn!("project room provisioning failed for '{}': {}", project, e);
        } else {
            tracing::info!("project room #{}:… ready", project);
        }

        // 3. For each component: resolve system prompt + create-or-join component room.
        let mut map = HashMap::new();
        let server_name = self.client.server_name().to_string();

        for component in &components {
            // Fetch system prompt from Fondament (best-effort).
            let fondament_id = format!("fondament/{}-agent", component);
            let resolve_url = format!("{}/resolve/{}", params.fondament_url, fondament_id);
            let system_prompt = match http.get(&resolve_url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    resp.text().await.unwrap_or_default()
                }
                Ok(resp) => {
                    tracing::warn!("Fondament resolve {} returned {}", fondament_id, resp.status());
                    String::new()
                }
                Err(e) => {
                    tracing::warn!("Fondament resolve {} failed: {}", fondament_id, e);
                    String::new()
                }
            };

            // Create-or-join component room.
            match self.client.create_or_join_aliased_room(component, &format!("{} agent", component)).await {
                Ok(room_id) => {
                    tracing::info!("component room #{}: {} ready", component, room_id.as_str());
                    let responder = Arc::new(Responder::with_config(
                        params.anthropic_api_key.clone(),
                        "claude-sonnet-4-6".into(),
                        server_name.clone(),
                        params.farga_url.clone(),
                        params.dispatcher_url.clone(),
                        params.amassada_url.clone(),
                        system_prompt,
                        false,
                    ));
                    map.insert(room_id, responder);
                }
                Err(e) => {
                    tracing::warn!("component room provisioning failed for '{}': {}", component, e);
                }
            }
        }

        // 4. Write observability signal to Farga (best-effort, ignore errors).
        let room_names: Vec<&str> = map.keys().map(|r| r.as_str()).collect();
        let signal_url = format!("{}/signals", params.farga_url);
        let _ = http.post(&signal_url)
            .json(&serde_json::json!({
                "project": project,
                "source": "charradissa-provisioning",
                "signals": [{
                    "project": project,
                    "source": "charradissa-provisioning",
                    "content": format!("provisioned {} component rooms: {}", room_names.len(), room_names.join(", "))
                }]
            }))
            .send().await;

        Ok(map)
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
        self.client.kick_user(room, user, reason).await
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

    async fn set_typing(&self, room: &RoomId, user_id: &str, typing: bool, timeout_ms: u32) -> Result<()> {
        self.client.set_typing(room, user_id, typing, timeout_ms).await
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
    fn room_provisioning_params_holds_all_fields() {
        let p = RoomProvisioningParams {
            farga_url: "http://farga:7500".into(),
            fondament_url: "http://fondament:7800".into(),
            anthropic_api_key: "key".into(),
            dispatcher_url: "http://dispatcher:9090/mcp".into(),
            amassada_url: "http://amassada:7700".into(),
        };
        assert_eq!(p.farga_url, "http://farga:7500");
        assert_eq!(p.fondament_url, "http://fondament:7800");
    }

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
