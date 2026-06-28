# Project-Driven Room Provisioning Design

**Date:** 2026-06-27  
**Issue:** Charradissa#22 (parent: Charradissa#21)  
**Status:** Draft  
**Repos affected:** Farga, Fondament, Charradissa

---

## Problem

Charradissa currently hardcodes component agent identities in `AGENT_LOCAL_PARTS` and requires
manual `[component_agents]` config entries with pre-known room IDs. There is no project room
for Guilhem to live in. Adding a new project or component requires editing Charradissa's config
and redeploying — the system is not data-driven.

---

## Goal

Provisioning of Matrix rooms and component agent Responders is driven entirely by:

1. **Farga** — source of truth for which projects exist and which components each project has
2. **Fondament** — source of truth for each component agent's resolved system prompt

Adding a new project or component to the stack requires zero Charradissa config changes.

---

## Architecture

```
Farga /context/components/occitan
  → ["amassada", "gardian", "farga", "cor", "caissa", "fondament", "charradissa"]

Fondament /resolve/fondament/amassada-agent
  → composed system prompt (Farga org/project context + Fondament definition layers)

Charradissa at startup:
  → #occitan:occitane.guilhem        project room — Guilhem (@charradissa) lives here
  → #amassada:occitane.guilhem       component room — amassada Responder handles it
  → #gardian:occitane.guilhem        ...one per discovered component
```

### Guilhem's identity

`@charradissa:occitane.guilhem` IS Guilhem — no separate virtual `@guilhem` user.
Guilhem lives in the `#occitan` project room and is routed via the existing
`default_agent_url` (the Guilhem HTTP agent). Component virtual user identities
(`@amassada`, `@gardian`, etc.) are out of scope here — that is tracked in Charradissa#10.

### Room routing (unchanged logic, dynamic source)

| Room | Routing | Handler |
|------|---------|---------|
| `#occitan:occitane.guilhem` | `default_agent_url` | Guilhem HTTP |
| `#amassada:occitane.guilhem` | `component_agents[room_id]` | Amassada Responder |
| `#{component}:occitane.guilhem` | `component_agents[room_id]` | Component Responder |

The `AppserviceState.component_agents` map is built dynamically at startup from
provisioning results instead of from `charradissa.toml`.

---

## Changes per repo

### 1. Farga — component listing endpoint

**New endpoint:** `GET /context/components/:project`

Returns a JSON array of component names found under `docs/projects/{project}/`.
Implementation: scan the filesystem for subdirectories containing a `component.md`.

```json
["amassada", "caissa", "charradissa", "cor", "farga", "fondament", "gardian"]
```

Files touched:
- `farga-server/src/docs.rs` — add `list_components(project)` method
- `farga-server/src/routes/context.rs` — add `get_components` handler
- `farga-server/src/routes/mod.rs` — register `GET /context/components/:project`

### 2. Fondament — new `fondament-server` crate

Fondament is currently a library/CLI with no HTTP interface. A minimal server crate
exposes the existing `Fondament::resolve()` logic over HTTP.

**New crate:** `fondament-server`

**Endpoints:**

`GET /component-agents`  
Returns all definitions with `kind: component-agent` as a JSON array:
```json
[
  { "id": "fondament/amassada-agent", "component": "amassada" },
  { "id": "fondament/gardian-agent",  "component": "gardian" },
  ...
]
```

`GET /resolve/:id`  
Resolves a definition by its Fondament ID (e.g. `fondament/amassada-agent`).
Returns the composed system prompt as plain text. Resolution layers:
1. Farga org context
2. Farga initiative context
3. Farga project context (if component has a project association)
4. Fondament definition `context` + `extends` chain

**Environment variables:**
- `FONDAMENT_DEFINITIONS_PATH` — path to the `definitions/` directory. In Kubernetes this is a
  volume mount from a ConfigMap or an init-container that clones/copies the Fondament repo's
  `definitions/` tree. Default: `./definitions`
- `FARGA_URL` — for Farga context enrichment during resolution
- `FONDAMENT_PORT` — default `7800`

Files touched:
- New crate `fondament-server/` with `main.rs`, `Cargo.toml`
- Root `Cargo.toml` — add workspace member
- `Dockerfile` — build and expose the server binary
- `.github/workflows/build-guilhem.yml` — already exists, add fondament-server build step

### 3. Charradissa — dynamic provisioning

#### Config changes

`[component_agents]` in `charradissa.toml` and `ComponentAgentConfig` in `charradissa-core/src/config.rs`
are kept as an **optional fallback** for the transition period (already `#[serde(default)]`).
When `provision_project_rooms` succeeds, its results take precedence. When it fails entirely,
any manually configured `[component_agents]` entries are used instead.
They will be removed once the full pipeline is stable (follow-up cleanup issue).

New optional config section for per-project provisioning targets:
```toml
[provisioning]
projects = ["occitan"]          # projects to provision at startup; default: ["occitan"]
fondament_url = "http://fondament:7800"   # override; default: FONDAMENT_URL env var
```

`FONDAMENT_URL` env var is the primary configuration path (consistent with other services).

#### New AppserviceClient methods (`charradissa-matrix/src/client.rs`)

```rust
/// Join a room by alias, creating it with the given name if it doesn't exist.
/// Returns the room_id in both cases.
pub async fn create_or_join_aliased_room(&self, alias_local: &str, name: &str) -> Result<RoomId>
```

Implementation: attempt `join_room(#{alias_local}:{server_name})`, on 404 call `create_room(alias_local, name)`.

Remove `AGENT_LOCAL_PARTS` constant (or retain only for kick-power grant during
the transition period, to be removed when Charradissa#10 lands).

#### New MatrixBackend method (`charradissa-matrix/src/backend.rs`)

```rust
pub struct ProvisioningConfig {
    pub farga_url: String,
    pub fondament_url: String,
    pub anthropic_api_key: String,
    pub dispatcher_url: String,
    pub amassada_url: String,
}

pub async fn provision_project_rooms(
    &self,
    project: &str,
    cfg: &ProvisioningConfig,
) -> Result<HashMap<RoomId, Arc<Responder>>>
```

Steps:
1. `GET {farga_url}/context/components/{project}` → component list
2. Create-or-join `#{project}:{server_name}` (project room; no Responder, handled by Guilhem HTTP)
3. For each component:
   a. `GET {fondament_url}/resolve/fondament/{component}-agent` → system prompt (empty string if 404 — Fondament may not have an agent def for every component)
   b. Create-or-join `#{component}:{server_name}` → room_id
   c. Build `Responder::with_config(...)` using resolved system prompt
   d. Insert `room_id → Responder` into result map
4. Write a Farga signal to `{project}` project recording provisioned room IDs (observability)

**Error handling:**
- Farga unavailable at startup → log warning, skip dynamic provisioning, fall back to `[component_agents]` if present in config (backwards compat during rollout)
- Fondament unavailable → log warning per component, use empty system prompt
- Room creation fails → log error per room, continue with remaining rooms
- Individual failures never abort the full provisioning pass

#### main.rs changes (`charradissa-daemon/src/main.rs`)

Replace the static `config.component_agents` loading block with:
```rust
let component_agents = backend
    .provision_project_rooms("occitan", &farga_base_url, &fondament_url, ...)
    .await
    .unwrap_or_else(|e| {
        tracing::warn!("project room provisioning failed: {}", e);
        HashMap::new()
    });
```

The call happens in the existing startup sequence, after `ensure_registered` and
`provision_agent_kick_power`, before `axum::serve`.

---

## Idempotency

Room creation idempotency is handled by the "join-first, create-on-404" pattern:
- If `#amassada:occitane.guilhem` already exists, `join_room` succeeds and returns the room_id
- No duplicate rooms are ever created
- Responders are always rebuilt fresh from Fondament at each startup (no stale state)

---

## Observability

After provisioning completes, Charradissa writes a signal to Farga:

```json
{
  "project": "occitan",
  "source": "charradissa-provisioning",
  "content": "provisioned rooms: #occitan, #amassada, #gardian, #farga, #cor, #caissa, #fondament, #charradissa"
}
```

This gives Guilhem (reading from Farga) visibility into what was provisioned and when.

---

## Acceptance criteria

- [ ] `GET /context/components/occitan` returns the 7 component names from Farga
- [ ] `GET /resolve/fondament/amassada-agent` returns Amassada's resolved system prompt from Fondament
- [ ] Charradissa startup creates (or joins) `#occitan:occitane.guilhem` and one room per component
- [ ] Room IDs are not hardcoded anywhere in Charradissa config or source
- [ ] Re-running startup does not create duplicate rooms
- [ ] Adding a new component to `Farga/docs/projects/occitan/` results in a new room at next Charradissa startup with no other config changes
- [ ] Farga or Fondament being unavailable at startup logs a warning but does not crash Charradissa

---

## Out of scope

- Component agents responding under their own Matrix identity (`@amassada`, etc.) — tracked in Charradissa#10
- Federation with future generations (occitane.arnaut, etc.)
- Fondament definition hot-reload (Fondament already has a file watcher; exposing it via HTTP is a follow-up)
- Multi-project provisioning (the `projects` config list is wired but only `occitan` is used in Phase II)
