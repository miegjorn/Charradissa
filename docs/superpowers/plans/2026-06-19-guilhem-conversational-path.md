# Guilhem Conversational Path (Phase B) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make guilhem reply to Matrix messages in Element — incoming room message → Claude reply grounded in recent room history → sent back into the room.

**Architecture:** The appservice already receives transactions (`handle_transaction`) but only logs them. This plan wires that inbound path to a `Responder` that assembles guilhem's persona + recent room history (fetched via a real `room_history`) + the latest message, calls Claude (reusing the `call_claude` HTTP pattern from `claude_analyzer.rs`), and sends the reply via the working `AppserviceClient::send_message`. Room history is the working memory; Farga durability is the concierge's job (already built) and out of scope here.

**Tech Stack:** Rust, tokio, axum, reqwest, async-trait, serde_json. Matrix client-server API via the appservice token. Anthropic `/v1/messages`.

## Global Constraints

- Matrix homeserver `server_name` is `occitane.guilhem`; the HTTP endpoint is `http://synapse:8008` (these are DIFFERENT — never derive the server_name from the URL host).
- The appservice authenticates to synapse with `Authorization: Bearer <as_token>`; synapse authenticates to the appservice with the `hs_token` (same value in our deploy).
- Anthropic calls: `x-api-key` + `anthropic-version: 2023-06-01`, model ids exactly `claude-sonnet-4-6` / `claude-opus-4-8` / `claude-haiku-4-5-20251001`.
- v1 is a SINGLE-TURN reply (no tool-execution loop — `tool_loop.rs` has no executor yet). Tools/approval-in-chat, sessions, missions, specialists are explicitly out of scope.
- No new external crates.

---

## DECISIONS TO CONFIRM BEFORE EXECUTION

1. **Guilhem's Matrix identity.** Recommended: provision a dedicated `@guilhem:occitane.guilhem` (add `@guilhem` to the appservice registration namespace + one synapse restart — same bootstrap path we already used). Alternative (zero registration change): reply as the existing appservice sender `@charradissa:occitane.guilhem` with display name "Guilhem". The plan below assumes **`@guilhem`** (Task 1a). If you prefer the sender, drop Task 1a and set `self_user_id = @charradissa:occitane.guilhem`.
2. **Reply model.** Default `claude-sonnet-4-6`. Opus if you want guilhem sharper at higher cost.
3. **should_respond in the shared room.** v1 = respond to every non-self human message (PL is the only human; rooms are quiet). Mention-gating deferred.

---

## File Structure

- `charradissa-core/src/config.rs` — add `server_name` field (Task 1).
- `charradissa-matrix/src/client.rs` — fix user_id formatting; add `room_history` GET; add `join_room`, `set_display_name` (Tasks 1, 2, 6).
- `charradissa-matrix/src/backend.rs` — implement `room_history` (Task 2).
- `charradissa-core/src/responder.rs` — NEW: persona, prompt assembly, `should_respond`, Claude call (Tasks 3, 4).
- `charradissa-matrix/src/appservice.rs` — validate `hs_token`, route message events to the responder (Task 5).
- `charradissa-daemon/src/main.rs` — construct responder + wire state; startup room provisioning (Tasks 5, 6).

---

### Task 1: Correct the server_name (fix malformed user IDs)

**Files:**
- Modify: `charradissa-core/src/config.rs`
- Modify: `charradissa-matrix/src/client.rs:75-77`
- Modify: `charradissa-daemon/src/main.rs:29-30`
- Test: inline `#[cfg(test)]` in `client.rs`

**Interfaces:**
- Produces: `OrgConfig.server_name: String`; `fn user_id(local_part: &str, server_name: &str) -> UserId`.

- [ ] **Step 1: Write the failing test** (in `charradissa-matrix/src/client.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn user_id_uses_server_name_not_url() {
        // server_name is occitane.guilhem even though the HTTP host is synapse:8008
        assert_eq!(user_id("guilhem", "occitane.guilhem").as_str(), "@guilhem:occitane.guilhem");
    }
}
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo test -p charradissa-matrix user_id_uses_server_name -- --nocapture`
Expected: FAIL — `user_id` not found.

- [ ] **Step 3: Implement**

In `client.rs`, add a free function and use it in `register_agent`:
```rust
use charradissa_core::types::UserId;
pub fn user_id(local_part: &str, server_name: &str) -> UserId {
    UserId::new(&format!("@{}:{}", local_part, server_name))
}
```
Add `server_name: String` to `AppserviceClient` (constructor param), and in `register_agent` replace the `format!("@{}:{}", local_part, self.homeserver.trim_start_matches(...))` with `Ok(user_id(local_part, &self.server_name))`.

In `config.rs`, add to the `[org]` config struct:
```rust
#[serde(default = "default_server_name")]
pub server_name: String,
```
with `fn default_server_name() -> String { "occitane.guilhem".into() }`.

In `main.rs`, build `bot_user_id` from config:
```rust
let server_name = config.org.server_name.clone();
let bot_user_id = format!("@guilhem:{}", server_name); // or @charradissa per Decision 1
```
and thread `server_name` into `MatrixBackend::new(...)` (add the param) → `AppserviceClient::new(...)`.

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p charradissa-matrix user_id_uses_server_name` → PASS
Run: `cargo build -p charradissa-daemon` → compiles.

- [ ] **Step 5: Commit**

```bash
git add charradissa-core/src/config.rs charradissa-matrix/src/client.rs charradissa-daemon/src/main.rs
git commit -m "fix(charradissa): derive Matrix user IDs from server_name, not the HS URL"
```

### Task 1a: Add `@guilhem` to the appservice registration (only if Decision 1 = @guilhem)

**Files:** operational — the registration file on synapse `/data` + values.

- [ ] **Step 1:** Append a namespace regex `@guilhem:occitane\.guilhem` (exclusive: true) to `charradissa-registration.yaml` on synapse `/data` (via `kubectl exec`), then delete the synapse pod to reload. Expected: synapse logs `Loaded application service` with the new regex; pod READY, 0 restarts.
- [ ] **Step 2: Commit** — none (operational state on the PVC); note it in the deploy runbook.

### Task 2: Real `room_history` (room = working memory)

**Files:**
- Modify: `charradissa-matrix/src/client.rs` (add `room_messages`)
- Modify: `charradissa-matrix/src/backend.rs:62-64`
- Test: inline `#[cfg(test)]` in `backend.rs` for the event-parsing helper

**Interfaces:**
- Consumes: `AppserviceClient`.
- Produces: `AppserviceClient::room_messages(&self, room: &RoomId, limit: u32) -> Result<Vec<ChatEvent>>`; `MatrixBackend::room_history` returns real events (oldest-first).

- [ ] **Step 1: Write the failing test** (parsing helper, no network)

```rust
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
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p charradissa-matrix parses_messages_response` → FAIL (`parse_messages_chunk` missing).

- [ ] **Step 3: Implement**

In `backend.rs` add a pure parser, and in `client.rs` the GET. Parser:
```rust
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
```
`client.rs`:
```rust
pub async fn room_messages(&self, room: &RoomId, limit: u32) -> Result<serde_json::Value> {
    let url = format!("{}/_matrix/client/v3/rooms/{}/messages?dir=b&limit={}",
        self.homeserver, room.as_str(), limit);
    let resp = self.client.get(&url).header("Authorization", self.auth_header())
        .send().await.map_err(|e| CharradissaError::Backend(e.to_string()))?;
    resp.json().await.map_err(|e| CharradissaError::Backend(e.to_string()))
}
```
`backend.rs` `room_history` (named const for the context window — Decision 3, ~20 msgs):
```rust
/// Number of recent messages fed to guilhem as conversational context each turn.
pub const HISTORY_LIMIT: u32 = 20;

async fn room_history(&self, room: &RoomId, _since: DateTime<Utc>) -> Result<Vec<ChatEvent>> {
    let body = self.client.room_messages(room, HISTORY_LIMIT).await?;
    Ok(parse_messages_chunk(&body, room))
}
```

- [ ] **Step 4: Run, verify pass** — `cargo test -p charradissa-matrix parses_messages_response` → PASS; `cargo build` ok.

- [ ] **Step 5: Commit**

```bash
git add charradissa-matrix/src/client.rs charradissa-matrix/src/backend.rs
git commit -m "feat(charradissa): real room_history via /messages (room as working memory)"
```

### Task 3: Responder — persona + prompt assembly + Claude call

**Files:**
- Create: `charradissa-core/src/responder.rs`
- Modify: `charradissa-core/src/lib.rs` (add `pub mod responder;`)
- Test: inline `#[cfg(test)]` in `responder.rs`

**Interfaces:**
- Produces: `Responder::new(api_key, model, server_name)`; `Responder::build_user_prompt(history: &[ChatEvent], latest: &ChatEvent) -> String`; `Responder::reply(history: &[ChatEvent], latest: &ChatEvent) -> Result<String>`; `const GUILHEM_SYSTEM: &str`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatEvent, ChatEventKind, RoomId, UserId};
    use chrono::Utc;
    fn ev(sender: &str, body: &str) -> ChatEvent {
        ChatEvent { event_id: "$x".into(), room_id: RoomId::new("!r:occitane.guilhem"),
            sender: UserId::new(sender), content: body.into(), timestamp: Utc::now(), kind: ChatEventKind::Message }
    }
    #[test]
    fn prompt_includes_history_and_latest() {
        let r = Responder::new("k".into(), "claude-sonnet-4-6".into(), "occitane.guilhem".into());
        let hist = vec![ev("@p:occitane.guilhem", "hello"), ev("@guilhem:occitane.guilhem", "hi")];
        let latest = ev("@p:occitane.guilhem", "what is farga?");
        let p = r.build_user_prompt(&hist, &latest);
        assert!(p.contains("hello") && p.contains("what is farga?"));
        assert!(p.contains("@p:occitane.guilhem"));
    }
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p charradissa-core prompt_includes_history` → FAIL.

- [ ] **Step 3: Implement** (`responder.rs`)

```rust
use crate::error::{CharradissaError, Result};
use crate::types::ChatEvent;

pub const GUILHEM_SYSTEM: &str = "You are Guilhem de Tudela, the org-level agent and chronicler of the Occitan stack. \
You live in Matrix and speak with Pierre-Luc and the stack's own agents. You are aware of the stack's components \
(Farga = durable memory/coherence, Amassada = sessions, Charradissa = your Matrix presence, Synapse = homeserver). \
Be substantive, honest, and concise. The room you are in is your working memory since the last concierge sweep; \
durable memory lives in Farga.";

pub struct Responder { client: reqwest::Client, api_key: String, model: String, pub server_name: String }

impl Responder {
    pub fn new(api_key: String, model: String, server_name: String) -> Self {
        Self { client: reqwest::Client::new(), api_key, model, server_name }
    }
    pub fn build_user_prompt(&self, history: &[ChatEvent], latest: &ChatEvent) -> String {
        let mut s = String::from("Recent conversation (oldest first):\n");
        for e in history { s.push_str(&format!("{}: {}\n", e.sender, e.content)); }
        s.push_str(&format!("\nLatest message from {}:\n{}\n\nReply as Guilhem.", latest.sender, latest.content));
        s
    }
    pub async fn reply(&self, history: &[ChatEvent], latest: &ChatEvent) -> Result<String> {
        let user = self.build_user_prompt(history, latest);
        let body = serde_json::json!({ "model": self.model, "max_tokens": 1024,
            "system": GUILHEM_SYSTEM, "messages": [{"role":"user","content": user}] });
        let resp = self.client.post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key).header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json").json(&body).send().await
            .map_err(|e| CharradissaError::Dispatch(e.to_string()))?;
        let data: serde_json::Value = resp.json().await.map_err(|e| CharradissaError::Dispatch(e.to_string()))?;
        if let Some(err) = data.get("error") { return Err(CharradissaError::Dispatch(err.to_string())); }
        Ok(data["content"][0]["text"].as_str().unwrap_or("").to_string())
    }
}
```
Add `pub mod responder;` to `lib.rs`.

- [ ] **Step 4: Run, verify pass** — `cargo test -p charradissa-core prompt_includes_history` → PASS; `cargo build` ok.

- [ ] **Step 5: Commit**

```bash
git add charradissa-core/src/responder.rs charradissa-core/src/lib.rs
git commit -m "feat(charradissa): Responder — guilhem persona + room-context prompt + Claude call"
```

### Task 4: `should_respond` (loop safety)

**Files:**
- Modify: `charradissa-core/src/responder.rs` (add fn + tests)

**Interfaces:**
- Produces: `fn should_respond(event: &ChatEvent, self_user_id: &str) -> bool`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn ignores_self_and_nonmessages() {
    let me = "@guilhem:occitane.guilhem";
    assert!(!should_respond(&ev(me, "my own message"), me));        // no self-loop
    assert!(should_respond(&ev("@p:occitane.guilhem", "hi"), me));  // human → respond
    let mut join = ev("@p:occitane.guilhem", ""); join.kind = ChatEventKind::MemberJoin;
    assert!(!should_respond(&join, me));                            // only real messages
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p charradissa-core ignores_self_and_nonmessages` → FAIL.

- [ ] **Step 3: Implement**

```rust
use crate::types::ChatEventKind;
pub fn should_respond(event: &ChatEvent, self_user_id: &str) -> bool {
    event.sender.as_str() != self_user_id
        && matches!(event.kind, ChatEventKind::Message | ChatEventKind::Mention)
}
```

- [ ] **Step 4: Run, verify pass** — PASS.

- [ ] **Step 5: Commit**

```bash
git add charradissa-core/src/responder.rs
git commit -m "feat(charradissa): should_respond — ignore self and non-message events"
```

### Task 5: Wire `handle_transaction` → responder (validate hs_token, reply)

**Files:**
- Modify: `charradissa-matrix/src/appservice.rs`
- Modify: `charradissa-daemon/src/main.rs:100-111`

**Interfaces:**
- Consumes: `Responder`, `MatrixBackend` (as `Arc<dyn ChatBackend>`), `should_respond`, `parse_matrix_event`.
- Produces: extended `AppserviceState { hs_token, responder: Arc<Responder>, backend: Arc<dyn ChatBackend>, self_user_id: String }`.

- [ ] **Step 1: Write the failing test** (token gate, pure)

```rust
#[test]
fn rejects_wrong_hs_token() {
    assert!(token_ok(Some("good"), "good"));   // correct token accepted
    assert!(!token_ok(Some("bad"), "good"));   // wrong token rejected
    assert!(!token_ok(None, "good"));          // missing token rejected
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p charradissa-matrix rejects_wrong_hs_token` → FAIL.

- [ ] **Step 3: Implement**

In `appservice.rs`:
```rust
use std::sync::Arc;
use charradissa_core::backend::ChatBackend;
use charradissa_core::responder::{Responder, should_respond};
use axum::extract::Query;

#[derive(Clone)]
pub struct AppserviceState {
    pub hs_token: String,
    pub responder: Arc<Responder>,
    pub backend: Arc<dyn ChatBackend>,
    pub self_user_id: String,
}

pub fn token_ok(provided: Option<&str>, expected: &str) -> bool {
    provided == Some(expected)
}

pub async fn handle_transaction(
    State(state): State<AppserviceState>,
    Query(q): Query<std::collections::HashMap<String, String>>,
    Path(_txn): Path<String>,
    Json(body): Json<Value>,
) -> StatusCode {
    if !token_ok(q.get("access_token").map(|s| s.as_str()), &state.hs_token) {
        return StatusCode::FORBIDDEN;
    }
    let events = body["events"].as_array().cloned().unwrap_or_default();
    for raw in events {
        if let Some(ev) = parse_matrix_event(&raw) {
            if !should_respond(&ev, &state.self_user_id) { continue; }
            let (responder, backend) = (state.responder.clone(), state.backend.clone());
            tokio::spawn(async move {
                let history = backend.room_history(&ev.room_id, chrono::Utc::now()).await.unwrap_or_default();
                match responder.reply(&history, &ev).await {
                    Ok(text) if !text.trim().is_empty() => { let _ = backend.send_message(&ev.room_id, &text).await; }
                    Ok(_) => {}
                    Err(e) => tracing::error!("guilhem reply failed: {}", e),
                }
            });
        }
    }
    StatusCode::OK
}
```
In `main.rs`, build the state:
```rust
let responder = Arc::new(Responder::new(anthropic_api_key.clone(), "claude-sonnet-4-6".into(), server_name.clone()));
let self_user_id = bot_user_id.clone();
let appservice_state = AppserviceState {
    hs_token: as_token.clone(), responder,
    backend: Arc::clone(&backend) as Arc<dyn ChatBackend>, self_user_id,
};
```
(Note: the `:txnId` route already exists; keep it. `Query` extractor must precede `Json`.)

- [ ] **Step 2b: Note** — `room_history` ignores `since` for now (fetches last 20); acceptable for v1.

- [ ] **Step 4: Run, verify pass** — `cargo test -p charradissa-matrix rejects_wrong_hs_token` → PASS; `cargo build -p charradissa-daemon` ok.

- [ ] **Step 5: Commit**

```bash
git add charradissa-matrix/src/appservice.rs charradissa-daemon/src/main.rs
git commit -m "feat(charradissa): route inbound messages to guilhem responder, validate hs_token"
```

### Task 6: Startup provisioning — guilhem joins #occitane-general with a display name

**Files:**
- Modify: `charradissa-matrix/src/client.rs` (add `join_room`, `set_display_name`)
- Modify: `charradissa-daemon/src/main.rs` (call them at startup)

**Interfaces:**
- Produces: `AppserviceClient::join_room(&self, room_alias_or_id: &str) -> Result<RoomId>`; `AppserviceClient::set_display_name(&self, user_id: &str, name: &str) -> Result<()>`.

- [ ] **Step 1: Implement client methods** (HTTP; verified manually). NO new crates — use a tiny manual percent-encoder for the path segments (Matrix IDs contain `@`, `:`, `#`, `!`).

First add a unit-tested helper + test:
```rust
/// Percent-encode the characters that appear in Matrix IDs/aliases for use in a URL path segment.
pub fn pct(s: &str) -> String {
    s.chars().map(|c| match c {
        '@' => "%40".into(), ':' => "%3A".into(), '#' => "%23".into(),
        '!' => "%21".into(), '/' => "%2F".into(), ' ' => "%20".into(),
        c => c.to_string(),
    }).collect()
}
#[cfg(test)]
mod pct_tests {
    use super::pct;
    #[test]
    fn encodes_matrix_id() {
        assert_eq!(pct("@guilhem:occitane.guilhem"), "%40guilhem%3Aoccitane.guilhem");
        assert_eq!(pct("#occitan-general:occitane.guilhem"), "%23occitan-general%3Aoccitane.guilhem");
    }
}
```
Then the methods:
```rust
pub async fn join_room(&self, alias_or_id: &str) -> Result<RoomId> {
    let url = format!("{}/_matrix/client/v3/join/{}", self.homeserver, pct(alias_or_id));
    let resp = self.client.post(&url).header("Authorization", self.auth_header())
        .json(&serde_json::json!({})).send().await
        .map_err(|e| CharradissaError::Backend(e.to_string()))?;
    let j: serde_json::Value = resp.json().await.map_err(|e| CharradissaError::Backend(e.to_string()))?;
    Ok(RoomId::new(j["room_id"].as_str().unwrap_or(alias_or_id)))
}
pub async fn set_display_name(&self, user_id: &str, name: &str) -> Result<()> {
    let url = format!("{}/_matrix/client/v3/profile/{}/displayname", self.homeserver, pct(user_id));
    self.client.put(&url).header("Authorization", self.auth_header())
        .json(&serde_json::json!({"displayname": name})).send().await
        .map_err(|e| CharradissaError::Backend(e.to_string()))?;
    Ok(())
}
```

- [ ] **Step 2: Wire startup** in `main.rs` before `axum::serve`:

```rust
// best-effort: create #occitane-general if missing, join it, set display name
let general_alias = format!("#{}-general", config.org.name); // #occitan-general
if let Err(e) = backend.send_message(&RoomId::new(&general_alias), "Guilhem is present.").await {
    tracing::warn!("could not greet {}: {} (room may need creating/inviting)", general_alias, e);
}
```
(Provisioning the room/space and the join handshake is finicky over the appservice API; for v1 the pragmatic path is: create the room once from Element, invite `@guilhem:occitane.guilhem`, and rely on auto-join via an `m.room.member` invite event handled in `handle_transaction` — add: on `ChatEventKind::MemberJoin`/invite for self, call `join_room`. Keep this minimal and verify manually.)

- [ ] **Step 3: Commit**

```bash
git add charradissa-matrix/src/client.rs charradissa-daemon/src/main.rs
git commit -m "feat(charradissa): guilhem joins its room and sets a display name"
```

### Task 7: Build image, deploy, verify in Element

**Files:** none (operational).

- [ ] **Step 1:** Rebuild the charradissa image (Dockerfile uses `rust:1.90-slim` + amassada build-context) and `kind load docker-image ghcr.io/occitan/charradissa:latest --name occitan`.
- [ ] **Step 2:** Restart the deployment: `kubectl rollout restart deploy/charradissa -n occitan-system`; confirm `1/1 Running`, logs show `webhook listening`.
- [ ] **Step 3: Manual verification** — in Element (homeserver `http://localhost:8008`), register a user, create a room, invite `@guilhem:occitane.guilhem` (or `@charradissa:…`), send "what's the state of the stack?". Expected: guilhem replies in-room within a few seconds, and a follow-up ("and since yesterday?") shows it used room history.
- [ ] **Step 4:** Confirm no self-reply loop (guilhem does not answer its own messages).

---

## Self-Review notes

- Coverage: incoming→reply (T5), room-as-memory (T2), persona (T3), loop safety (T4), identity correctness (T1), presence (T6), end-to-end (T7). The three flagged decisions gate T1/T1a/T5 model.
- Out of scope (tracked, not built): tool-execution loop + approval-in-chat, sessions/missions, specialist invitation, `send_dm` real impl, `since`-accurate history pagination, Fondament-driven persona assembly (v1 uses an inline persona).
- Risk: appservice user join/invite handshake is the fiddliest part (T6) — verify manually and iterate; the reply path (T2–T5) is the load-bearing slice and is unit-tested where pure.
