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

    /// Share the underlying appservice client (same Matrix token) — used to back the
    /// Matrix MCP tool server so MCP actions and inbound handling speak as one identity.
    pub fn appservice_client(&self) -> Arc<AppserviceClient> {
        Arc::clone(&self.client)
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
            .error_for_status()
            .map_err(|e| charradissa_core::error::CharradissaError::Backend(
                format!("Farga component list HTTP error: {}", e)
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
        //    No component owner: the sender keeps its default creator power level.
        if let Err(e) = self.client.create_or_join_aliased_room(project, &format!("{} project", project), None).await {
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

            if system_prompt.is_empty() {
                tracing::warn!("no system prompt resolved for '{}', skipping component room", component);
                continue;
            }

            // Create-or-join component room. The component agent owns its room
            // (PL 100) — Charradissa #20. The room alias uses the bare component
            // name, but the agent's Matrix localpart for "charradissa" is
            // "charradissa-agent" (see AGENT_LOCAL_PARTS), so normalize for the
            // power-level grant.
            let owner_localpart = agent_localpart_for_component(component);
            match self.client.create_or_join_aliased_room(component, &format!("{} agent", component), Some(owner_localpart)).await {
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

    /// Ensure a DM room exists between Guilhem (the appservice sender,
    /// `@charradissa`, display name "Guilhem") and each component agent.
    ///
    /// Charradissa #22. There is no separate `@guilhem` Matrix user — the
    /// appservice sender *is* Guilhem on the wire — so the DMs are owned by, and
    /// the `m.direct` tag is written for, the sender. Idempotent: the existing
    /// `m.direct` account_data is the source of truth; partners already recorded
    /// there are reused and never re-created. Returns the partner → room_id map.
    pub async fn provision_dm_rooms(&self, farga_url: &str) -> Result<HashMap<String, String>> {
        let bot = self.client.bot_user_id().to_string();
        let server = self.client.server_name().to_string();

        // 1. Read the sender's current m.direct (user_id -> [room_id, ...]).
        let mut direct = match self.client.get_account_data(&bot, "m.direct").await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("reading m.direct failed, assuming empty: {}", e);
                serde_json::json!({})
            }
        };
        if !direct.is_object() {
            direct = serde_json::json!({});
        }

        let mut result: HashMap<String, String> = HashMap::new();
        let mut created_any = false;

        // 2. For each component agent: reuse the recorded DM or create one.
        //    Scoped so the `&mut` borrow of `direct` ends before we read it back below.
        {
            let map_obj = direct.as_object_mut()
                .expect("direct is an object by construction");
            for lp in charradissa_core::registration::COMPONENT_AGENT_LOCALPARTS {
                let partner = format!("@{}:{}", lp, server);
                if let Some(existing) = map_obj.get(&partner)
                    .and_then(|v| v.as_array())
                    .and_then(|rooms| rooms.iter().find_map(|r| r.as_str()))
                {
                    result.insert(partner.clone(), existing.to_string());
                    continue;
                }
                match self.client.create_dm_room(&partner).await {
                    Ok(room_id) => {
                        tracing::info!("provisioned DM room {} with {}", room_id.as_str(), partner);
                        map_obj.insert(partner.clone(), serde_json::json!([room_id.as_str()]));
                        result.insert(partner.clone(), room_id.as_str().to_string());
                        created_any = true;
                    }
                    Err(e) => tracing::warn!("DM room creation failed for {}: {}", partner, e),
                }
            }
        }

        // 3. Persist the updated m.direct for the sender (only if it changed).
        if created_any {
            if let Err(e) = self.client.set_account_data(&bot, "m.direct", &direct).await {
                tracing::warn!("persisting m.direct failed: {}", e);
            }
        }

        // 4. Observability signal to Farga (best-effort, ignore errors).
        let mapping: Vec<String> = result.iter()
            .map(|(partner, room)| format!("{} → {}", partner, room))
            .collect();
        let signal_url = format!("{}/signals", farga_url);
        let _ = reqwest::Client::new().post(&signal_url)
            .json(&serde_json::json!({
                "project": "occitan",
                "source": "charradissa-dm-provisioning",
                "signals": [{
                    "project": "occitan",
                    "source": "charradissa-dm-provisioning",
                    "content": format!(
                        "provisioned {} DM rooms (Guilhem ↔ component agents): {}",
                        result.len(), mapping.join(", ")
                    )
                }]
            }))
            .send().await;

        Ok(result)
    }
}

/// Map a project component name to the component agent's Matrix localpart.
///
/// Identical for every component except `charradissa`, whose agent identity is
/// `charradissa-agent` (the bare `charradissa` localpart is the appservice
/// sender). Keeps the PL-100 owner grant aligned with [`AGENT_LOCAL_PARTS`].
fn agent_localpart_for_component(component: &str) -> &str {
    match component {
        "charradissa" => "charradissa-agent",
        other => other,
    }
}

#[async_trait]
impl ChatBackend for MatrixBackend {
    async fn send_message(&self, room: &RoomId, content: &str) -> Result<()> {
        self.client.send_message(room, content).await
    }

    async fn send_message_as(&self, room: &RoomId, content: &str, sender_localpart: Option<&str>) -> Result<()> {
        self.client.send_message_as(room, content, sender_localpart).await
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

    /// Build a MatrixBackend pointed at the given homeserver URL (for test use).
    fn backend_for_test(homeserver: &str) -> MatrixBackend {
        MatrixBackend::new(
            homeserver.to_string(),
            "test-token".to_string(),
            "@charradissa:occitane.guilhem".to_string(),
            "occitane.guilhem".to_string(),
        )
    }

    fn test_params(mock_uri: &str) -> RoomProvisioningParams {
        RoomProvisioningParams {
            farga_url: mock_uri.to_string(),
            fondament_url: mock_uri.to_string(),
            anthropic_api_key: "test-key".to_string(),
            dispatcher_url: "http://dispatcher:9090/mcp".to_string(),
            amassada_url: "http://amassada:7700".to_string(),
        }
    }

    /// Happy path: Farga returns ["amassada"], Fondament returns a non-empty prompt,
    /// Matrix join returns 404 (room not found), then createRoom returns a room_id.
    /// Expected: one entry in the returned map.
    #[tokio::test]
    async fn provision_project_rooms_happy_path() {
        use wiremock::{Mock, MockServer, ResponseTemplate};
        use wiremock::matchers::{method, path};

        let mock = MockServer::start().await;

        // Farga: component list for "test-project" → ["amassada"]
        Mock::given(method("GET"))
            .and(path("/context/components/test-project"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!(["amassada"]))
            )
            .mount(&mock)
            .await;

        // Fondament: resolve fondament/amassada-agent → non-empty system prompt
        Mock::given(method("GET"))
            .and(path("/resolve/fondament/amassada-agent"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("You are an Amassada session orchestrator.")
            )
            .mount(&mock)
            .await;

        // Matrix join for project room → 404 (triggers create_room)
        Mock::given(method("POST"))
            .and(path("/_matrix/client/v3/join/%23test-project%3Aoccitane.guilhem"))
            .respond_with(
                ResponseTemplate::new(404)
                    .set_body_json(serde_json::json!({"errcode": "M_NOT_FOUND"}))
            )
            .mount(&mock)
            .await;

        // Matrix join for component room → 404 (triggers create_room)
        Mock::given(method("POST"))
            .and(path("/_matrix/client/v3/join/%23amassada%3Aoccitane.guilhem"))
            .respond_with(
                ResponseTemplate::new(404)
                    .set_body_json(serde_json::json!({"errcode": "M_NOT_FOUND"}))
            )
            .mount(&mock)
            .await;

        // Matrix createRoom → used for both project and component rooms
        Mock::given(method("POST"))
            .and(path("/_matrix/client/v3/createRoom"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"room_id": "!amassada:occitane.guilhem"}))
            )
            .mount(&mock)
            .await;

        let backend = backend_for_test(&mock.uri());
        let params = test_params(&mock.uri());

        let result = backend.provision_project_rooms("test-project", &params).await;
        assert!(result.is_ok(), "provision_project_rooms should succeed: {:?}", result.err());
        let map = result.unwrap();
        assert_eq!(map.len(), 1, "expected exactly one component room in the map");
    }

    /// Empty-prompt skip: Farga returns ["amassada"], Fondament returns 404.
    /// Expected: zero entries in the map (component room creation is skipped).
    #[tokio::test]
    async fn provision_project_rooms_skips_empty_prompt() {
        use wiremock::{Mock, MockServer, ResponseTemplate};
        use wiremock::matchers::{method, path};

        let mock = MockServer::start().await;

        // Farga: component list for "test-project" → ["amassada"]
        Mock::given(method("GET"))
            .and(path("/context/components/test-project"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!(["amassada"]))
            )
            .mount(&mock)
            .await;

        // Fondament: 404 → empty system_prompt → component is skipped
        Mock::given(method("GET"))
            .and(path("/resolve/fondament/amassada-agent"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock)
            .await;

        // Matrix join for project room → 404 → create_room
        Mock::given(method("POST"))
            .and(path("/_matrix/client/v3/join/%23test-project%3Aoccitane.guilhem"))
            .respond_with(
                ResponseTemplate::new(404)
                    .set_body_json(serde_json::json!({"errcode": "M_NOT_FOUND"}))
            )
            .mount(&mock)
            .await;

        // Matrix createRoom → for the project room only (component is skipped before create)
        Mock::given(method("POST"))
            .and(path("/_matrix/client/v3/createRoom"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"room_id": "!test-project:occitane.guilhem"}))
            )
            .mount(&mock)
            .await;

        let backend = backend_for_test(&mock.uri());
        let params = test_params(&mock.uri());

        let result = backend.provision_project_rooms("test-project", &params).await;
        assert!(result.is_ok(), "provision_project_rooms should succeed: {:?}", result.err());
        let map = result.unwrap();
        assert_eq!(map.len(), 0, "component room should be skipped when Fondament returns empty prompt");
    }

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

    #[test]
    fn charradissa_component_maps_to_agent_localpart() {
        // The bare `charradissa` localpart is the sender; the agent is `charradissa-agent`.
        assert_eq!(agent_localpart_for_component("charradissa"), "charradissa-agent");
        assert_eq!(agent_localpart_for_component("amassada"), "amassada");
        assert_eq!(agent_localpart_for_component("gardian"), "gardian");
    }

    /// Path of the sender's m.direct account_data endpoint, used by the DM tests.
    const M_DIRECT_PATH: &str =
        "/_matrix/client/v3/user/%40charradissa%3Aoccitane.guilhem/account_data/m.direct";

    /// Charradissa #22: with no recorded DMs, a room is created for each of the
    /// seven component agents. The `.expect(7)` on the createRoom mock is verified
    /// when the MockServer is dropped at end of test.
    #[tokio::test]
    async fn provision_dm_rooms_creates_for_all_seven_when_absent() {
        use wiremock::{Mock, MockServer, ResponseTemplate};
        use wiremock::matchers::{method, path};

        let mock = MockServer::start().await;

        // m.direct does not exist yet → 404 (treated as empty).
        Mock::given(method("GET"))
            .and(path(M_DIRECT_PATH))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock).await;

        // Exactly seven DM rooms must be created (id value is irrelevant here).
        Mock::given(method("POST"))
            .and(path("/_matrix/client/v3/createRoom"))
            .respond_with(ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "room_id": "!dm:occitane.guilhem" })))
            .expect(7)
            .mount(&mock).await;

        // m.direct is persisted afterwards.
        Mock::given(method("PUT"))
            .and(path(M_DIRECT_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&mock).await;

        let backend = backend_for_test(&mock.uri());
        let result = backend.provision_dm_rooms(&mock.uri()).await.expect("provision should succeed");

        assert_eq!(result.len(), 7, "expected a DM for each of the 7 component agents");
        // MockServer drop verifies the createRoom `.expect(7)`.
    }

    /// Charradissa #22 idempotency: calling provision twice when m.direct already
    /// records all seven DMs creates no new rooms (no duplicates). The `.expect(0)`
    /// on the createRoom mock fails the test if any DM is (re-)created.
    #[tokio::test]
    async fn provision_dm_rooms_is_idempotent() {
        use wiremock::{Mock, MockServer, ResponseTemplate};
        use wiremock::matchers::{method, path};

        let mock = MockServer::start().await;

        // m.direct already records a DM room for every component agent.
        let mut direct = serde_json::Map::new();
        for lp in charradissa_core::registration::COMPONENT_AGENT_LOCALPARTS {
            direct.insert(
                format!("@{}:occitane.guilhem", lp),
                serde_json::json!([format!("!existing-{}:occitane.guilhem", lp)]),
            );
        }
        let direct_body = serde_json::Value::Object(direct);

        Mock::given(method("GET"))
            .and(path(M_DIRECT_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(direct_body))
            .mount(&mock).await;

        // No DM room may be created across either call.
        Mock::given(method("POST"))
            .and(path("/_matrix/client/v3/createRoom"))
            .respond_with(ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "room_id": "!should-not-happen:occitane.guilhem" })))
            .expect(0)
            .mount(&mock).await;

        let backend = backend_for_test(&mock.uri());

        let first = backend.provision_dm_rooms(&mock.uri()).await.expect("first provision");
        let second = backend.provision_dm_rooms(&mock.uri()).await.expect("second provision");

        assert_eq!(first.len(), 7);
        assert_eq!(second.len(), 7);
        // Existing rooms are reused, not re-created.
        assert_eq!(
            second.get("@amassada:occitane.guilhem").map(String::as_str),
            Some("!existing-amassada:occitane.guilhem")
        );
        // MockServer drop verifies the createRoom `.expect(0)`.
    }
}
