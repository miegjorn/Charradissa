use reqwest::Client;
use charradissa_core::error::{CharradissaError, Result};
use charradissa_core::types::{RoomId, UserId};

/// Component agent local parts in the Charradissa appservice namespace.
/// Kept here as the canonical source — matches charradissa-registration.yaml namespaces.
/// Charradissa sets these users to PL 50 (kick power) in every room it creates or joins.
pub const AGENT_LOCAL_PARTS: &[&str] = &[
    "guilhem", "gardian", "fondament", "farga", "amassada", "cor", "caissa", "charradissa", "nervi",
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
        self.send_message_as(room_id, content, None).await
    }

    /// Send a message optionally impersonating a virtual user in the appservice namespace.
    /// When `sender_localpart` is Some, adds `?user_id=@{localpart}:{server_name}` so the
    /// message appears as that user rather than the appservice bot.
    pub async fn send_message_as(&self, room_id: &RoomId, content: &str, sender_localpart: Option<&str>) -> Result<()> {
        let mut url = format!(
            "{}/_matrix/client/v3/rooms/{}/send/m.room.message/{}",
            self.homeserver, pct(room_id.as_str()), uuid::Uuid::new_v4()
        );
        if let Some(localpart) = sender_localpart {
            let mxid = format!("@{}:{}", localpart, self.server_name);
            url.push_str(&format!("?user_id={}", pct(&mxid)));
        }
        let body = markdown_body(content);
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
        self.create_room_with_owner(alias, name, None).await
    }

    /// Create an aliased room with power levels preset at creation time.
    ///
    /// All component agents get PL 50 (kick power) in every room.
    /// When `owner_localpart` is `Some`, the room is a component room and that
    /// agent is its owner — granted PL 100 — while the AS sender (@charradissa,
    /// the substrate bot) is set to PL 50 (moderator). When `owner_localpart`
    /// is `None` (project rooms, etc.) the sender keeps its default PL 100.
    pub async fn create_room_with_owner(
        &self,
        alias: &str,
        name: &str,
        owner_localpart: Option<&str>,
    ) -> Result<RoomId> {
        let url = format!("{}/_matrix/client/v3/createRoom", self.homeserver);
        // Base: every component agent gets PL 50 (kick power).
        let mut users: serde_json::Map<String, serde_json::Value> = AGENT_LOCAL_PARTS.iter()
            .map(|lp| (format!("@{}:{}", lp, self.server_name), serde_json::json!(50)))
            .collect();
        if let Some(owner) = owner_localpart {
            // The owning component agent is the room admin (PL 100).
            users.insert(format!("@{}:{}", owner, self.server_name), serde_json::json!(100));
            // Guilhem (the appservice sender) is a moderator here, not the admin.
            // The sender sets these levels as room creator at creation time.
            users.insert(self.bot_user_id.clone(), serde_json::json!(50));
        }
        let users: serde_json::Value = users.into();
        let body = serde_json::json!({
            "room_alias_name": alias,
            "name": name,
            "power_level_content_override": { "users": users, "kick": 50 }
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

    /// Create a direct-message (1:1) room and invite `partner_user_id`.
    ///
    /// Unlike [`create_room_with_owner`] this sets no alias and no power-level
    /// override: it sends `is_direct: true` with the `trusted_private_chat`
    /// preset so the invited partner shares control of the DM. Used by the DM
    /// fabric (Charradissa #22).
    pub async fn create_dm_room(&self, partner_user_id: &str) -> Result<RoomId> {
        let url = format!("{}/_matrix/client/v3/createRoom", self.homeserver);
        let body = serde_json::json!({
            "is_direct": true,
            "preset": "trusted_private_chat",
            "invite": [partner_user_id],
        });
        let resp = self.client.post(&url)
            .header("Authorization", self.auth_header())
            .json(&body)
            .send().await
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        let json: serde_json::Value = resp.json().await
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        let room_id = json["room_id"].as_str()
            .ok_or_else(|| CharradissaError::Backend("no room_id in DM create response".into()))?;
        Ok(RoomId::new(room_id))
    }

    /// Read a user's account_data event of `event_type`. Returns an empty JSON
    /// object when the event does not exist (HTTP 404) so callers can treat
    /// "never set" and "set to empty" uniformly.
    pub async fn get_account_data(&self, user_id: &str, event_type: &str) -> Result<serde_json::Value> {
        let url = format!(
            "{}/_matrix/client/v3/user/{}/account_data/{}",
            self.homeserver, pct(user_id), event_type
        );
        let resp = self.client.get(&url)
            .header("Authorization", self.auth_header())
            .send().await
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        if resp.status().as_u16() == 404 {
            return Ok(serde_json::json!({}));
        }
        let resp = resp.error_for_status()
            .map_err(|e| CharradissaError::Backend(format!("get_account_data: {}", e)))?;
        resp.json().await.map_err(|e| CharradissaError::Backend(e.to_string()))
    }

    /// Write a user's account_data event of `event_type`.
    pub async fn set_account_data(&self, user_id: &str, event_type: &str, value: &serde_json::Value) -> Result<()> {
        let url = format!(
            "{}/_matrix/client/v3/user/{}/account_data/{}",
            self.homeserver, pct(user_id), event_type
        );
        let resp = self.client.put(&url)
            .header("Authorization", self.auth_header())
            .json(value)
            .send().await
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            return Err(CharradissaError::Backend(format!("set_account_data failed: {}", status)));
        }
        Ok(())
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

    pub async fn leave_room(&self, room_id: &RoomId) -> Result<()> {
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/leave",
            self.homeserver, pct(room_id.as_str())
        );
        let resp = self.client.post(&url)
            .header("Authorization", self.auth_header())
            .json(&serde_json::json!({}))
            .send().await
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            return Err(CharradissaError::Backend(format!("leave failed: {}", status)));
        }
        Ok(())
    }

    pub async fn invite(&self, room_id: &RoomId, user_id: &UserId) -> Result<()> {
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/invite",
            self.homeserver, pct(room_id.as_str())
        );
        let resp = self.client.post(&url)
            .header("Authorization", self.auth_header())
            .json(&serde_json::json!({ "user_id": user_id.as_str() }))
            .send().await
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        // Surface Synapse's rejection (e.g. M_FORBIDDEN when the appservice's power
        // level is insufficient) instead of silently succeeding — matrix_invite must
        // fail gracefully and report the boundary.
        if !resp.status().is_success() {
            let status = resp.status();
            return Err(CharradissaError::Backend(format!("invite failed: {}", status)));
        }
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

    /// Find a DM room shared with `target_user_id` by scanning joined rooms for one
    /// that has exactly 2 members: the appservice bot and the target user.
    /// Returns `None` if no such room is found.
    pub async fn find_dm_room(&self, target_user_id: &str) -> Result<Option<RoomId>> {
        let rooms = self.joined_rooms().await?;
        for room in rooms {
            let url = format!(
                "{}/_matrix/client/v3/rooms/{}/joined_members",
                self.homeserver, pct(room.as_str())
            );
            let resp = self.client.get(&url)
                .header("Authorization", self.auth_header())
                .send().await
                .map_err(|e| CharradissaError::Backend(e.to_string()))?;
            let Ok(json) = resp.json::<serde_json::Value>().await else { continue };
            let Some(joined) = json["joined"].as_object() else { continue };
            if joined.len() == 2
                && joined.contains_key(target_user_id)
                && joined.contains_key(&self.bot_user_id)
            {
                return Ok(Some(room));
            }
        }
        Ok(None)
    }

    pub async fn room_messages(&self, room: &RoomId, limit: u32) -> Result<serde_json::Value> {
        let url = format!("{}/_matrix/client/v3/rooms/{}/messages?dir=b&limit={}",
            self.homeserver, pct(room.as_str()), limit);
        let resp = self.client.get(&url).header("Authorization", self.auth_header())
            .send().await.map_err(|e| CharradissaError::Backend(e.to_string()))?;
        let resp = resp.error_for_status().map_err(|e| CharradissaError::Backend(e.to_string()))?;
        resp.json().await.map_err(|e| CharradissaError::Backend(e.to_string()))
    }

    /// Fetch up to `limit` recent messages from a room, returned in chronological order
    /// as `(sender_mxid, body)` pairs. Only `m.room.message` events with a text body
    /// are included; state events and redacted messages are silently skipped.
    pub async fn get_messages(&self, room: &RoomId, limit: u32) -> Result<Vec<(String, String)>> {
        let raw = self.room_messages(room, limit).await?;
        let mut msgs: Vec<(String, String)> = raw["chunk"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e["type"].as_str() == Some("m.room.message"))
            .filter_map(|e| {
                let sender = e["sender"].as_str()?.to_string();
                let body = e["content"]["body"].as_str()?.to_string();
                Some((sender, body))
            })
            .collect();
        // dir=b returns newest-first; reverse to chronological order.
        msgs.reverse();
        Ok(msgs)
    }

    pub fn bot_user_id(&self) -> &str {
        &self.bot_user_id
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
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

    /// Join `#{alias_local}:{server_name}` if it exists; create it with `name` on 404.
    /// Returns the room_id in both cases. Idempotent.
    pub async fn create_or_join_aliased_room(
        &self,
        alias_local: &str,
        name: &str,
        owner_localpart: Option<&str>,
    ) -> Result<RoomId> {
        let alias = format!("#{}:{}", alias_local, self.server_name);
        let url = format!("{}/_matrix/client/v3/join/{}", self.homeserver, pct(&alias));
        let resp = self.client.post(&url)
            .header("Authorization", self.auth_header())
            .json(&serde_json::json!({}))
            .send().await
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        if resp.status().is_success() {
            let json: serde_json::Value = resp.json().await
                .map_err(|e| CharradissaError::Backend(e.to_string()))?;
            let room_id = json["room_id"].as_str()
                .ok_or_else(|| CharradissaError::Backend("no room_id in join response".into()))?;
            return Ok(RoomId::new(room_id));
        }
        if resp.status().as_u16() == 404 || resp.status().as_u16() == 400 {
            return self.create_room_with_owner(alias_local, name, owner_localpart).await;
        }
        let status = resp.status();
        Err(CharradissaError::Backend(format!("create_or_join_aliased_room failed: {}", status)))
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

    /// PUT /_matrix/client/v3/rooms/{roomId}/typing/{userId}
    /// Best-effort — logs a warning on failure, never returns an error.
    pub async fn set_typing(&self, room_id: &RoomId, user_id: &str, typing: bool, timeout_ms: u32) -> Result<()> {
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/typing/{}",
            self.homeserver, pct(room_id.as_str()), pct(user_id)
        );
        let body = if typing {
            serde_json::json!({ "typing": true, "timeout": timeout_ms })
        } else {
            serde_json::json!({ "typing": false })
        };
        let resp = self.client.put(&url)
            .header("Authorization", self.auth_header())
            .json(&body)
            .send().await
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        if !resp.status().is_success() {
            tracing::warn!("set_typing failed for {} in {}: {}", user_id, room_id.as_str(), resp.status());
        }
        Ok(())
    }

    /// Upload binary content (e.g. a PNG image) to the Matrix media server.
    /// Returns the `mxc://` URI for use in subsequent `m.image` events.
    pub async fn upload_media(&self, content_type: &str, data: Vec<u8>) -> Result<String> {
        let url = format!("{}/_matrix/media/v3/upload", self.homeserver);
        let resp = self.client.post(&url)
            .header("Authorization", self.auth_header())
            .header("Content-Type", content_type)
            .body(data)
            .send().await
            .map_err(|e| CharradissaError::Backend(format!("upload_media request: {}", e)))?;
        let json: serde_json::Value = resp.json().await
            .map_err(|e| CharradissaError::Backend(format!("upload_media parse: {}", e)))?;
        let mxc = json["content_uri"].as_str()
            .ok_or_else(|| CharradissaError::Backend("no content_uri in upload response".into()))?;
        Ok(mxc.to_string())
    }

    /// Send an `m.image` event to a room referencing an already-uploaded `mxc://` URI.
    /// `sender_localpart` selects which virtual user posts (appends `?user_id=`); `None` uses the bot.
    /// `filename` infers the mimetype for the `info` block (SVG → `image/svg+xml`, else `image/png`).
    pub async fn send_image(&self, room_id: &RoomId, mxc_uri: &str, filename: &str, sender_localpart: Option<&str>) -> Result<()> {
        let txn = uuid::Uuid::new_v4();
        let mut url = format!(
            "{}/_matrix/client/v3/rooms/{}/send/m.room.message/{}",
            self.homeserver, pct(room_id.as_str()), txn
        );
        if let Some(lp) = sender_localpart {
            url.push_str(&format!("?user_id={}", pct(&format!("@{}:{}", lp, self.server_name))));
        }
        let mimetype = if filename.ends_with(".svg") { "image/svg+xml" } else { "image/png" };
        let body = serde_json::json!({
            "msgtype": "m.image",
            "body": filename,
            "url": mxc_uri,
            "info": { "mimetype": mimetype },
        });
        let resp = self.client.put(&url)
            .header("Authorization", self.auth_header())
            .json(&body)
            .send().await
            .map_err(|e| CharradissaError::Backend(format!("send_image: {}", e)))?;
        if !resp.status().is_success() {
            return Err(CharradissaError::Backend(format!("send_image failed: {}", resp.status())));
        }
        Ok(())
    }
}

pub fn user_id(local_part: &str, server_name: &str) -> UserId {
    UserId::new(&format!("@{}:{}", local_part, server_name))
}

/// Build a Matrix `m.room.message` body for `content`.
///
/// If the content renders to HTML that differs from the plain text (i.e. the
/// content actually contains markdown), the body includes `format` and
/// `formatted_body` so Matrix clients render it. Otherwise plain text only.
fn markdown_body(content: &str) -> serde_json::Value {
    let html = render_markdown(content);
    // Only send formatted_body when there is actual markup — avoids cluttering
    // plain-prose messages with an identical HTML copy.
    if html_differs_from_plain(content, &html) {
        serde_json::json!({
            "msgtype": "m.text",
            "body": content,
            "format": "org.matrix.custom.html",
            "formatted_body": html,
        })
    } else {
        serde_json::json!({ "msgtype": "m.text", "body": content })
    }
}

fn render_markdown(content: &str) -> String {
    use pulldown_cmark::{html, Options, Parser};
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;
    let parser = Parser::new_ext(content, opts);
    let mut html_out = String::new();
    html::push_html(&mut html_out, parser);
    html_out
}

/// Returns true when the rendered HTML adds something beyond a plain paragraph
/// wrap (pulldown-cmark wraps bare text in `<p>…</p>\n` even with no markdown).
fn html_differs_from_plain(plain: &str, html: &str) -> bool {
    // Normalise: strip the outer <p>…</p>\n that pulldown-cmark adds to any
    // single-paragraph input, then compare to the original.
    let trimmed = html.trim();
    let unwrapped = trimmed
        .strip_prefix("<p>")
        .and_then(|s| s.strip_suffix("</p>"))
        .unwrap_or(trimmed);
    unwrapped != plain.trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_id_uses_server_name_not_url() {
        // server_name is occitane.guilhem even though the HTTP host is synapse:8008
        assert_eq!(user_id("guilhem", "occitane.guilhem").as_str(), "@guilhem:occitane.guilhem");
    }

    // ── markdown_body ─────────────────────────────────────────────────────────

    #[test]
    fn plain_prose_has_no_formatted_body() {
        let body = markdown_body("hello world");
        assert!(body.get("formatted_body").is_none());
        assert!(body.get("format").is_none());
        assert_eq!(body["body"], "hello world");
    }

    #[test]
    fn bold_text_produces_formatted_body() {
        let body = markdown_body("**important**");
        assert_eq!(body["format"], "org.matrix.custom.html");
        let html = body["formatted_body"].as_str().unwrap();
        assert!(html.contains("<strong>important</strong>"), "got: {html}");
    }

    #[test]
    fn code_block_produces_formatted_body() {
        let body = markdown_body("```rust\nfn main() {}\n```");
        assert_eq!(body["format"], "org.matrix.custom.html");
        let html = body["formatted_body"].as_str().unwrap();
        assert!(html.contains("<pre><code"), "got: {html}");
    }

    #[test]
    fn plain_body_preserved_alongside_html() {
        let md = "**bold** text";
        let body = markdown_body(md);
        assert_eq!(body["body"], md);
    }

    #[test]
    fn mermaid_fence_produces_language_mermaid_class() {
        // Element Web renders <pre><code class="language-mermaid"> as a diagram.
        // pulldown-cmark preserves the fence language as a CSS class, so Mermaid
        // just works without any extra handling.
        let md = "```mermaid\ngraph LR\n  A --> B\n```";
        let body = markdown_body(md);
        assert_eq!(body["format"], "org.matrix.custom.html");
        let html = body["formatted_body"].as_str().unwrap();
        assert!(html.contains(r#"class="language-mermaid""#), "got: {html}");
    }

    #[test]
    fn bullet_list_produces_formatted_body() {
        let body = markdown_body("- item one\n- item two");
        assert_eq!(body["format"], "org.matrix.custom.html");
        let html = body["formatted_body"].as_str().unwrap();
        assert!(html.contains("<ul>") && html.contains("<li>"), "got: {html}");
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

    #[test]
    fn create_or_join_aliased_room_builds_correct_alias() {
        // The alias format must be #{local}:{server} — verify via pct encoding.
        let alias = format!("#{}:{}", "amassada", "occitane.guilhem");
        assert_eq!(pct(&alias), "%23amassada%3Aoccitane.guilhem");
    }

    #[tokio::test]
    async fn create_or_join_aliased_room_joins_existing_room() {
        use wiremock::{Mock, MockServer, ResponseTemplate};
        use wiremock::matchers::{method, path};

        let mock_server = MockServer::start().await;

        // Mock the join endpoint to return success with a room_id
        Mock::given(method("POST"))
            .and(path("/_matrix/client/v3/join/%23amassada%3Aoccitane.guilhem"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "room_id": "!testroom:occitane.guilhem" }))
            )
            .mount(&mock_server)
            .await;

        let client = AppserviceClient::new(
            mock_server.uri(),
            "test_token".to_string(),
            "@bot:occitane.guilhem".to_string(),
            "occitane.guilhem".to_string(),
        );

        let result = client.create_or_join_aliased_room("amassada", "Amassada Room", None).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_str(), "!testroom:occitane.guilhem");
    }

    #[tokio::test]
    async fn create_or_join_aliased_room_creates_when_not_found() {
        use wiremock::{Mock, MockServer, ResponseTemplate};
        use wiremock::matchers::{method, path};

        let mock_server = MockServer::start().await;

        // Mock the join endpoint to return 400 with M_NOT_FOUND
        Mock::given(method("POST"))
            .and(path("/_matrix/client/v3/join/%23amassada%3Aoccitane.guilhem"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_json(serde_json::json!({
                        "errcode": "M_NOT_FOUND",
                        "error": "Room alias not found"
                    }))
            )
            .mount(&mock_server)
            .await;

        // Mock the create room endpoint
        Mock::given(method("POST"))
            .and(path("/_matrix/client/v3/createRoom"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "room_id": "!newroom:occitane.guilhem" }))
            )
            .mount(&mock_server)
            .await;

        let client = AppserviceClient::new(
            mock_server.uri(),
            "test_token".to_string(),
            "@bot:occitane.guilhem".to_string(),
            "occitane.guilhem".to_string(),
        );

        let result = client.create_or_join_aliased_room("amassada", "Amassada Room", None).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_str(), "!newroom:occitane.guilhem");
    }

    /// Charradissa #20: a component room must grant the owning agent PL 100 and
    /// the AS sender (@charradissa, the substrate bot) PL 50 at creation time.
    #[tokio::test]
    async fn create_room_with_owner_grants_owner_100_and_sender_50() {
        use wiremock::{Mock, MockServer, ResponseTemplate};
        use wiremock::matchers::{method, path, body_partial_json};

        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/_matrix/client/v3/createRoom"))
            .and(body_partial_json(serde_json::json!({
                "power_level_content_override": {
                    "users": {
                        "@amassada:occitane.guilhem": 100,
                        "@charradissa:occitane.guilhem": 50
                    }
                }
            })))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "room_id": "!amassada:occitane.guilhem" }))
            )
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = AppserviceClient::new(
            mock_server.uri(),
            "test_token".to_string(),
            "@charradissa:occitane.guilhem".to_string(),
            "occitane.guilhem".to_string(),
        );

        // If the power-level body doesn't match, no mock matches and this errors.
        let result = client.create_room_with_owner("amassada", "amassada agent", Some("amassada")).await;
        assert!(result.is_ok(), "create_room_with_owner should match the PL body: {:?}", result.err());
        assert_eq!(result.unwrap().as_str(), "!amassada:occitane.guilhem");
    }

    /// Charradissa #22: a DM room is created with `is_direct: true` and invites
    /// the partner agent.
    #[tokio::test]
    async fn create_dm_room_sets_is_direct_and_invites_partner() {
        use wiremock::{Mock, MockServer, ResponseTemplate};
        use wiremock::matchers::{method, path, body_partial_json};

        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/_matrix/client/v3/createRoom"))
            .and(body_partial_json(serde_json::json!({
                "is_direct": true,
                "invite": ["@amassada:occitane.guilhem"]
            })))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "room_id": "!dm:occitane.guilhem" }))
            )
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = AppserviceClient::new(
            mock_server.uri(),
            "test_token".to_string(),
            "@charradissa:occitane.guilhem".to_string(),
            "occitane.guilhem".to_string(),
        );

        let result = client.create_dm_room("@amassada:occitane.guilhem").await;
        assert!(result.is_ok(), "create_dm_room should match is_direct body: {:?}", result.err());
        assert_eq!(result.unwrap().as_str(), "!dm:occitane.guilhem");
    }
}
