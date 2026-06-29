# Charradissa — Matrix-Chat Runtime

**Charradissa** (Occitan: *chat/conversation*) is the Matrix-chat runtime of the Occitan stack. It hosts multiple AI agents as persistent members of a Matrix chat system: projects live as rooms, agents live as room members. Charradissa consumes `amassada-core` as a library and implements its `Transport` trait for Matrix rooms.

Charradissa is **dual-mode**:
- A lightweight **tool loop** for single-turn, synchronous agent interactions.
- A full **Amassada session engine** for structured, multi-agent, multi-round collaborative work.

The chat backend is fully abstracted — Matrix ships first; IRC and others implement the same `ChatBackend` trait.

---

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [Crate Structure](#crate-structure)
3. [Agent Tiers](#agent-tiers)
4. [ConciergeAgent](#conciergeagent)
5. [Approval Flow](#approval-flow)
6. [Slash Commands](#slash-commands)
7. [CharradissaTransport](#charradissatransport)
8. [Jira Backend](#jira-backend)
9. [Configuration Reference](#configuration-reference)
10. [Environment Variables](#environment-variables)
11. [Matrix Appservice Setup](#matrix-appservice-setup)
12. [Matrix MCP Tool Server](#matrix-mcp-tool-server)
13. [Infrastructure](#infrastructure)
14. [Building and Running](#building-and-running)
15. [Testing](#testing)
16. [Out of Scope (v1)](#out-of-scope-v1)

---

## Architecture Overview

```
Human / Claude Code
        │
        │  Matrix events (PUT /_matrix/app/v1/transactions)
        ▼
charradissa-daemon  ──┬──  charradissa-matrix  (MatrixBackend, AppserviceClient)
                      ├──  charradissa-jira    (JiraTaskManager)
                      └──  charradissa-core    (ChatBackend trait, agent tiers,
                                                 tool loop, approval queue,
                                                 ConciergeAgent, CharradissaTransport)
                                    │
                                    │  library dep
                                    ▼
                             amassada-core     (Transport trait, session engine)
```

The daemon registers a namespace of Matrix virtual users (`@charradissa-*:homeserver`) via the Matrix Appservice protocol. Each agent is a virtual user joined to one or more rooms. Events arrive as HTTP `PUT` webhooks; the daemon parses them, routes them to the correct agent tier, runs the tool loop or an Amassada session, and posts replies back through `MatrixBackend`.

---

## Crate Structure

The workspace contains four crates:

### `charradissa-core`

The heart of the system. Backend-agnostic. Contains:

- **`config`** — `Config` struct and TOML deserialization (`OrgConfig`, `BackendConfig`, `ConciergeConfig`, `ApprovalConfig`, `TasksConfig`, `ProjectsConfig`).
- **`types`** — Newtype IDs (`RoomId`, `UserId`, `SpaceId`, `ProjectId`, `TaskId`), `CompositionAddress`, `ChatEvent`/`ChatEventKind`, `Task`/`TaskStatus`, `RoomOptions`, `ProjectHealth`.
- **`backend`** — `ChatBackend` async trait (13 methods, including `join_room`/`joined_rooms` with default no-op impls).
- **`task`** — `TaskManager` async trait (5 methods).
- **`farga`** — `FargaWriter` async trait + `HttpFargaWriter` implementation for publishing signals to Farga over HTTP.
- **`transport`** — `CharradissaTransport`: implements `amassada_core::Transport` for Matrix rooms.
- **`tool_loop`** — `SlashCommand` enum, `parse_slash_command()`, `ToolCall` struct, `ToolLoopConfig`, and `requires_approval()` — the approval gating function for tool calls.
- **`approval`** — `ApprovalQueue`, `PendingApproval`, `ApprovalOutcome`. Pending approvals are held by `tokio::sync::oneshot` channels; resolved by `/approve` or `/reject` slash commands.
- **`concierge`** — `ConciergeAgent` with archival and convergence loops; `extract_signals()` for harvesting structured signals from room history.
- **`agents`** — Three agent tier structs: `OrgAgent`, `ProjectAgent`, `Specialist`.
- **`error`** — `CharradissaError` enum (`Backend`, `ApprovalTimeout`, `ApprovalRejected`, `Tool`, `Dispatch`, `Config`, `Io`).

Dependencies: `amassada-core` (path dep), `tokio`, `serde`/`serde_json`, `toml`, `async-trait`, `reqwest`, `chrono`, `uuid`, `thiserror`, `anyhow`, `futures`, `tokio-stream`, `tracing`.

### `charradissa-matrix`

Matrix-specific implementation of `ChatBackend`. Four modules:

- **`client`** (`AppserviceClient`) — Thin `reqwest` wrapper over the Matrix Client-Server API v3: `send_message`, `create_room`, `invite`, `register_agent`. Uses `Bearer <as_token>` auth.
- **`backend`** (`MatrixBackend`) — Implements `ChatBackend` by delegating to `AppserviceClient`.
- **`appservice`** (`AppserviceState`, `handle_transaction`, `parse_matrix_event`) — Axum handler for `PUT /_matrix/app/v1/transactions/:txnId`. Parses Matrix events into `ChatEvent`, detects slash commands by body prefix.
- **`mcp`** (`MatrixMcp`) — JSON-RPC tool server backing the [Matrix MCP Tool Server](#matrix-mcp-tool-server) (`matrix_send`, `matrix_invite`, `matrix_kick`, `matrix_get_dm`), reusing `AppserviceClient`.

Dependencies: `charradissa-core`, `tokio`, `axum`, `serde_json`, `reqwest`, `chrono`, `uuid`, `async-trait`, `tracing`.

### `charradissa-jira`

Implements `TaskManager` via the Jira Cloud REST API v3.

- **`JiraTaskManager`** — `create_task` (POST `/rest/api/3/issue` with ADF description body), `assign_task` (logged, stub), `update_status` (logged, stub), `get_task` (returns error), `list_open` (returns empty vec). Authentication is HTTP Basic with base64-encoded `email:api_token`.

Dependencies: `charradissa-core`, `reqwest`, `serde`, `serde_json`, `anyhow`, `async-trait`, `tracing`, `base64 0.22`.

### `charradissa-daemon`

The runnable binary (`charradissa-daemon`). Entry point:

1. Reads `CHARRADISSA_CONFIG` (default: `charradissa.toml`).
2. Reads `MATRIX_AS_TOKEN` for the appservice token.
3. Constructs `MatrixBackend`.
4. Initialises an `AgentRegistry` (maps `ProjectId` → `UserId`, tracks org agent and concierge).
5. Binds an Axum HTTP server on `CHARRADISSA_LISTEN_PORT` (default: `8448`) and serves the Matrix Appservice webhook.

The `AgentRegistry` in `charradissa-daemon/src/registry.rs` is an in-process map maintained by the daemon for the lifetime of the process.

---

## Agent Tiers

### Persistent Agents (provisioned at startup)

| Tier | Matrix ID | Home Room | Role |
|---|---|---|---|
| `OrgAgent` | `@org.<org>:homeserver` | `#<org>-general` | Org-wide broadcast; receives all org-level events |
| `ProjectAgent` | `@project.<name>:homeserver` | `#<project>` | Manages a project room; handles slash commands; gates specialist invitations; dispatches implementation tasks |
| `ConciergeAgent` | `@concierge:homeserver` | Silent member of every `#<project>` | Runs archival and convergence background jobs; never posts to rooms |

Persistent agents are registered once at daemon start and remain online for the daemon's lifetime. The `AgentRegistry` tracks their `UserId`s for dispatch routing.

### Specialists (JIT / ephemeral)

Specialists have Matrix IDs of the form `@charradissa-<uuid>:homeserver`. They are:

- **Provisioned** by `Specialist::provision(address, room_id, backend)`: calls `backend.register_agent(&address)` then `backend.invite(&room_id, &user_id)`.
- **Deprovisioned** by `Specialist::deprovision()`: kicks the user from the room then calls `backend.deregister_agent()`.

The `CompositionAddress` enum determines what system prompt a specialist gets:
- `CompositionAddress::Role { role, stance_override }` — resolves to `fondament/<role>[/<stance>]` in Fondament.
- `CompositionAddress::Composed { project, facet, stance }` — resolves to `<project>/<facet>+<stance>`.

No Matrix account persists after a specialist's session ends. Conversation history is preserved in Farga, not the Matrix account.

### Invitation Model

- **Private consultation**: an agent calls the API directly (single-turn dispatch or mini-Amassada session). No Matrix room is created; the result is injected as context for the requesting agent.
- **Public invitation**: agent calls `request_invitation(address, reason)` → `ProjectAgent` evaluates → calls `backend.invite()` if approved → specialist joins `#<project>` as a visible member.
- Specialists may not self-invite. `ProjectAgent` is the sole gatekeeper of the public invitation pool. Sub-specialist invitation chains are out of scope for v1 (flat delegation model).

### Room Topology

```
Matrix Space: <org>/<project>
├── #<project>                        main room — ProjectAgent, humans, specialists
├── #<project>-code-approval          approval room for code/git/PR tool calls
├── #<project>-db-approval            approval room for db/sql tool calls
├── #<project>-infra-approval         approval room for infra/aws/tf tool calls
├── #<project>-session-<uuid>         ephemeral — Amassada sessions
└── #<project>-impl-<ticket_id>       ephemeral — implementation dispatch rooms

Matrix Space: <org>
└── #<org>-general                    OrgAgent, ConciergeAgent (silent)
```

---

## ConciergeAgent

`ConciergeAgent` (`charradissa-core/src/concierge.rs`) is a silent observer. It never posts to any room; all output goes via `send_dm()` to `ProjectAgent` or `OrgAgent`. It runs two background loops:

### Job 1 — Room Archival (every `archival_interval_hours`, default 24h)

```
for each project:
  events = room_history(since: now - archival_interval_hours)
  if events non-empty:
    signals = extract_signals(events)
    farga.write_signals(project, signals)
```

`extract_signals()` scans event content for keywords: `decision:`, `decided:`, `blocker:`, `blocked:`, `artifact:`, `pattern:`. Matching messages are emitted as `Signal { project, content, source }` and written to Farga via `HttpFargaWriter`.

### Job 2 — Cross-Project Convergence Sweep (every `convergence_interval_hours`, default 6h)

```
summaries = [ farga.recent_signals(project, since: last_sweep) for each project ]
if signals non-empty:
  convergence = llm_call(concierge_persona, summaries)
  for each opportunity:
    send_dm(target_project_agent_user_id, whisper)
```

The `daily_token_budget` caps total LLM token consumption before any job fires.

### Farga Integration

`FargaWriter` trait has two methods: `write_signals` and `recent_signals`. The `HttpFargaWriter` POSTs signals to `{base_url}/signals` and GETs recent signals from `{base_url}/signals/recent?project=<p>&since=<n>h`. Charradissa writes raw signals; Farga's own agent handles dedup, conflict resolution, and lazy resolution index optimization.

---

## Approval Flow

All write-class tool calls are gated through category-specific approval rooms before execution.

**Tool categories and their approval rooms:**

| Tool name prefix | Category | Approval room |
|---|---|---|
| `git_*`, `pr_*`, `code_*` | `code` | `#<project>-code-approval` |
| `db_*`, `sql_*` | `db` | `#<project>-db-approval` |
| `infra_*`, `aws_*`, `tf_*` | `infra` | `#<project>-infra-approval` |
| All others | — | No approval required |

**Flow:**

1. Tool loop calls `requires_approval(tool_name)` → returns `(true, Some("code"))` for gated tools.
2. `ApprovalQueue::create_pending(category, description, params)` — generates a UUID, stores a `PendingApproval`, returns a `tokio::sync::oneshot::Receiver<ApprovalOutcome>`.
3. The pending action is posted to `#<project>-<category>-approval` with the ID, description, and proposed parameters.
4. Tool loop blocks on the `oneshot` receiver.
5. A human posts `/approve <id>` or `/reject <id> [reason]` in the approval room.
6. `ProjectAgent::handle_event` parses the slash command and calls `ApprovalQueue::resolve(id, outcome)`, sending on the `oneshot`.
7. Tool loop receives `ApprovalOutcome::Approved` or `ApprovalOutcome::Rejected(reason)` and proceeds or aborts.
8. If no response arrives within `approval.timeout_minutes` (default 60), the queue times out and auto-rejects.

All approvals are logged with timestamp, approver identity, and decision for the audit trail in Farga.

---

## Slash Commands

Slash commands are parsed by `parse_slash_command()` in `charradissa-core/src/tool_loop.rs`. They are detected either as `ChatEventKind::SlashCommand` or by matching a `/`-prefixed message body.

| Command | Syntax | Handler | Effect |
|---|---|---|---|
| `/approve` | `/approve <id>` | `ProjectAgent` | Resolves the named pending approval with `Approved` |
| `/reject` | `/reject <id> [reason]` | `ProjectAgent` | Resolves the named pending approval with `Rejected(reason)` |
| `/session` | `/session <canvas_id> "<goal>"` | `ProjectAgent` | Initiates an Amassada session for the given canvas and goal |
| `/mission` | `/mission <budget_tokens> <goal>` | `ProjectAgent` | Runs a full `amassada_core::mission::engine::MissionEngine` with the given token budget and goal, submitting to Farga on a `FargaVerdict::Submit` outcome |
| `/invite` | `/invite <address>` | `ProjectAgent` | Provisions a specialist from a `CompositionAddress` string and invites it to the main room |
| `/call` | `/call` | (reserved) | Initiates a voice/video consultation (future) |

---

## CharradissaTransport

`CharradissaTransport` (`charradissa-core/src/transport.rs`) is the seam between the Matrix chat layer and Amassada's session engine. It implements `amassada_core::Transport` and translates session events into Matrix room operations:

| `SessionEvent` variant | Matrix action |
|---|---|
| `TurnCompleted { record }` | `send_message` — formatted as `**[agent_id / persona]**\ncontent` |
| `RoundStarted { round }` | `send_message` — formatted as `--- Round N ---` |
| `BtwEmitted { from, to, content }` | `send_message` — formatted as `*[btw from X to Y]* content` |
| `ApprovalRequested { reason }` | `send_message` — formatted as `Approval requested: reason` |
| `ArtifactCompleted { title }` | `send_message` — formatted as `Artifact ready: **title**` |
| `SessionCompleted` | `send_message` — `"Session complete."` |
| `ModeratorAction { action }` | `send_message` — formatted as `*[moderator: action]*` |
| `consult` | Stub — returns placeholder response |
| `whisper` | `tracing::debug` only (no-op in Matrix) |
| `recv_human` | Returns `None` (polling not yet implemented) |
| `emit_output` | `send_message` — formats all artifacts into a markdown summary |

---

## Jira Backend

`JiraTaskManager` (`charradissa-jira/src/backend.rs`) implements `TaskManager` against the Jira Cloud REST API v3.

**Construction:**
```rust
JiraTaskManager::new(base_url, project_key, api_token, email)
```

**Authentication:** HTTP Basic — `Authorization: Basic base64(<email>:<api_token>)`.

**`create_task`**: POSTs to `/rest/api/3/issue` with an ADF (`"type": "doc"`) description body. Returns the Jira issue ID.

**`assign_task`** and **`update_status`**: Stubbed — log via `tracing::info` and return `Ok(())`. Full Jira transition/assignment calls are planned.

**`get_task`**: Returns `CharradissaError::Tool("get_task: not implemented")`.

**`list_open`**: Returns an empty vec (full JQL query planned).

Required environment variables for Jira: `JIRA_API_TOKEN`, `JIRA_EMAIL` (passed to the constructor by the daemon).

---

## Configuration Reference

Charradissa is configured via a TOML file (default: `charradissa.toml`, overridden by `CHARRADISSA_CONFIG`).

```toml
[org]
name        = "acme"                         # Organisation slug; used in Matrix IDs and room aliases
homeserver  = "https://matrix.acme.internal" # Matrix homeserver base URL
server_name = "occitane.guilhem"             # Matrix server name used to build the bot's user ID (default: "occitane.guilhem")

[backend]
type = "matrix"   # Currently only "matrix" is implemented; "irc" is planned

[concierge]
archival_interval_hours   = 24     # How often the archival loop runs (default: 24)
convergence_interval_hours = 6     # How often the cross-project convergence sweep runs (default: 6)
daily_token_budget         = 50000 # Hard cap on LLM tokens per day across all concierge jobs (default: 50_000)

[approval]
timeout_minutes = 60   # Auto-reject pending approvals after this many minutes (default: 60)

[tasks]
type        = "jira"                        # "jira" or "none"
base_url    = "https://acme.atlassian.net"  # Required when type = "jira"
project_key = "PROJ"                        # Jira project key for issue creation

[projects]
autodiscover = true   # If true, projects are auto-discovered from Farga; if false, listed explicitly
```

All fields have documented defaults; the only required fields are `org.name`, `org.homeserver`, and `backend.type`.

### Agent routing config

```toml
# Per-room agent routing (optional).
[agents]
default = "http://guilhem.agents.svc.cluster.local:8080"   # fallback for unmatched rooms

# Component-agent routes: room_id → agent URL. The agent at that URL receives
# the standard { room_id, sender, content, history } body on POST /matrix/reply.
[agents.routes]
"!gardian-room:occitane.guilhem" = "http://gardian.agents.svc.cluster.local:8080"
"!farga-room:occitane.guilhem"   = "http://farga.agents.svc.cluster.local:7500"

# Project-backed agents: each [[agents.project]] block maps one or more rooms to
# an Amassada endpoint. Charradissa injects project_id into the turn body.
[[agents.project]]
type       = "amassada_backed"
rooms      = ["!room-projet-a:occitane.guilhem"]
project_id = "my-project"
endpoint   = "http://amassada.agents.svc.cluster.local:7700/sessions/{session_id}/message"
```

### Component agents (in-process responders)

`[[component_agents]]` wires a room directly to an in-process `Responder` rather than an HTTP pod. Use this for lightweight agents that do not need a separate container:

```toml
[[component_agents]]
room_id    = "!help-room:occitane.guilhem"
model      = "claude-opus-4-5"
system     = "You are a helpful assistant embedded in Matrix."
```

---

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `CHARRADISSA_CONFIG` | `charradissa.toml` | Path to the TOML config file |
| `MATRIX_AS_TOKEN` | `dev-token` | Matrix Appservice token (`as_token` in the registration YAML) |
| `MATRIX_HS_TOKEN` | `dev-hs-token` | Homeserver token (`hs_token` in the registration YAML); only read by `--generate-registration` |
| `CHARRADISSA_LISTEN_PORT` | `8448` | Port the daemon's Appservice webhook HTTP server listens on. Renamed from `CHARRADISSA_PORT` to avoid the kubelet service-link collision (issue #10) |
| `CHARRADISSA_URL` | `http://charradissa:<port>` | Callback URL Synapse posts transactions to (`url:` in the registration YAML); only read by `--generate-registration` |
| `JIRA_API_TOKEN` | — | Jira API token for `JiraTaskManager` auth |
| `JIRA_EMAIL` | — | Atlassian account email for Jira Basic auth |
| `FARGA_URL` | `http://farga:7500` | Base URL of the Farga HTTP API for signal writes |
| `GUILHEM_URL` | `http://guilhem.agents.svc.cluster.local:8080` | Base URL of the Guilhem pod's `/matrix/reply` endpoint |
| `ANTHROPIC_API_KEY` | — | API key for the Anthropic Claude API; required by any agent tier that makes LLM calls directly (concierge convergence sweep, Responder-backed component agents) |
| `AMASSADA_URL` | `http://amassada:7700` | Base URL of the Amassada service; used as the default origin for project-agent endpoints when no explicit `endpoint` is set in `[[agents.project]]` |
| `CHARRADISSA_DM_REGISTRY` | `charradissa-dm-registry.json` | Path to the DM room registry JSON read by the `matrix_get_dm` MCP tool (see [Matrix MCP Tool Server](#matrix-mcp-tool-server)). Provisioned by Charradissa#22; a missing file degrades to an empty registry |
| `RUST_LOG` | — | Log filter (e.g. `charradissa=debug,info`) via `tracing-subscriber` |

---

## Matrix Appservice Setup

Charradissa uses the Matrix Appservice protocol. No per-bot WebSocket connections. All inbound events arrive as a single `PUT` webhook; all outbound calls use `Authorization: Bearer <as_token>`.

The appservice is fully wired in the local kind cluster: charradissa runs at `1/1`, `@charradissa` and `@claude` users exist in Matrix, and the registration file lives in Synapse's `/data` directory. What follows is the reference setup for re-provisioning or new deployments.

### Registration YAML (register with your homeserver)

Generate the registration with the daemon rather than writing it by hand — this
is the source of truth for the file Synapse loads:

```sh
MATRIX_AS_TOKEN=<as-token> MATRIX_HS_TOKEN=<hs-token> \
  charradissa-daemon --generate-registration > charradissa-registration.yaml
```

The generated file looks like:

```yaml
id: charradissa
url: http://charradissa:8448
as_token: <MATRIX_AS_TOKEN>
hs_token: <MATRIX_HS_TOKEN>
sender_localpart: charradissa
namespaces:
  users:
    - exclusive: true
      regex: '@charradissa:occitane\.guilhem'
    - exclusive: true
      regex: '@charradissa-.*'
    - exclusive: true
      regex: '@gardian:occitane\.guilhem'
    - exclusive: true
      regex: '@fondament:occitane\.guilhem'
    - exclusive: true
      regex: '@farga:occitane\.guilhem'
    - exclusive: true
      regex: '@amassada:occitane\.guilhem'
    - exclusive: true
      regex: '@cor:occitane\.guilhem'
    - exclusive: true
      regex: '@caissa:occitane\.guilhem'
    - exclusive: true
      regex: '@charradissa-agent:occitane\.guilhem'
  rooms: []
  aliases: []
```

- `as_token` must match `MATRIX_AS_TOKEN` in the daemon environment; `hs_token`
  comes from `MATRIX_HS_TOKEN`. The callback `url` is taken from `CHARRADISSA_URL`
  (default `http://charradissa:<CHARRADISSA_LISTEN_PORT>`), and the server name in
  the regexes from `org.server_name` in the TOML config.
- `hs_token` is sent by the homeserver in the `Authorization` header on incoming requests; the daemon validates it via `AppserviceState.hs_token`.
- The `@charradissa-.*` namespace covers all specialist virtual users.
- The seven component agents (`@gardian`, `@fondament`, `@farga`, `@amassada`,
  `@cor`, `@caissa`, `@charradissa-agent`) each get an exclusive entry so they can
  respond as their own Matrix identity rather than as `@charradissa`. The set is
  defined by `COMPONENT_AGENT_LOCALPARTS` in `charradissa-core/src/registration.rs`.
- Persistent agents (`@org.*`, `@project.*`, `@concierge`) may require additional namespace entries or separate appservice registrations.

> **Deploying a namespace change:** Synapse reads this file once at startup from
> its data PVC (`/data/charradissa-registration.yaml`). After regenerating it,
> replace the file on the PVC and restart **both** Synapse and Charradissa (the
> `as_token`/`hs_token` must stay in sync across the two).

### Routing

Every inbound Matrix event is dispatched through one of three paths, checked in order:

1. **Project-agent path (`[[agents.project]]`)** — if the event's room ID matches an entry in `project_routes`, Charradissa POSTs to the configured Amassada endpoint:
   ```
   POST http://amassada:7700/sessions/{session_id}/message
   Body: { "content": "...", "sender": "...", "room_id": "...", "project_id": "..." }
   ```
   The `{session_id}` placeholder is substituted with a stable, URL-safe session handle derived from the room (`project-<hash>`, see `charradissa_core::routing::project_session_id`), so the project's conversation reuses the same Amassada session across turns and restarts. (`{room_id}` is still substituted with the raw room id for backward compatibility with older templates.) The `room_id` carried in the body is what Amassada uses to resolve the project from its registry; `project_id` is injected as the project Charradissa resolved from its own routing table.

2. **Component-agent path (`[agents.routes]`)** — if the room ID matches an entry in `agent_routes`, Charradissa POSTs to that component pod's endpoint:
   ```
   POST <component_url>/matrix/reply
   Body: { "room_id": "...", "sender": "...", "content": "...", "history": [...] }
   ```
   Component agents run as separate pods (e.g. `@gardian`, `@farga`) and handle their own persona logic.

3. **Default path → Guilhem** — all other rooms fall through to the default agent URL (`GUILHEM_URL`):
   ```
   POST http://guilhem.agents.svc.cluster.local:8080/matrix/reply
   Body: { "room_id": "...", "sender": "...", "content": "...", "history": [...] }
   ```
   Only this path sends Matrix typing indicators (`set_typing`) to signal that Guilhem is processing the message.

---

## Webhook Endpoint

The daemon exposes one route:

```
PUT /_matrix/app/v1/transactions/:txnId
```

`handle_transaction` in `charradissa-matrix/src/appservice.rs` receives a JSON body with an `events` array, parses each event via `parse_matrix_event()`, and dispatches to the appropriate agent tier.

---

## Matrix MCP Tool Server

> Charradissa#23 — lets agents *act* in Matrix, not only respond to inbound events.

The daemon serves a [Model Context Protocol](https://modelcontextprotocol.io) tool server over
JSON-RPC 2.0 at:

```
POST /mcp
```

This mirrors the stack convention (`dispatcher` at `:9090/mcp`). The full URL in-cluster is
`http://charradissa:<CHARRADISSA_LISTEN_PORT>/mcp`. The implementation is
`charradissa-matrix/src/mcp.rs` (`MatrixMcp`); the HTTP transport is `charradissa-daemon/src/mcp_api.rs`.

### Tools

| Tool | Body | Effect |
|---|---|---|
| `matrix_send` | `{room_id, content}` | Send a message to any room or DM by room ID |
| `matrix_invite` | `{room_id, user_id}` | Invite a user to a room |
| `matrix_kick` | `{room_id, user_id, reason?}` | Kick a user from a room |
| `matrix_get_dm` | `{agent}` | Resolve the DM room ID for a component agent (`farga` or `@farga:occitane.guilhem`) |

`tools/list`, `initialize`, and `ping` are also handled. Each `tools/call` returns the standard
MCP `result.content[0].text` envelope; failures (e.g. a Synapse `M_FORBIDDEN` when the caller's
power level is insufficient) come back as `isError: true` text rather than a transport error, so
`matrix_invite` / `matrix_kick` **respect the caller's actual power level and fail gracefully**.

### Authentication

The server acts with the appservice's Matrix token, read from `MATRIX_AS_TOKEN` at daemon startup
(or resolved via Gardian). It shares the same `AppserviceClient` as inbound handling, so MCP
actions and inbound replies speak as one Matrix identity.

### DM registry (`matrix_get_dm`)

`matrix_get_dm` reads a JSON object mapping each component agent's localpart to its DM room ID with
`@guilhem`, from the path in `CHARRADISSA_DM_REGISTRY` (default `charradissa-dm-registry.json`):

```json
{
  "farga":    "!abc:occitane.guilhem",
  "amassada": "!def:occitane.guilhem"
}
```

**Dependency — Charradissa#22.** This registry is *provisioned and persisted* by Charradissa#22
(DM room fabric), which was not yet merged when #23 landed. #23 ships the read side plus the agreed
storage contract (this JSON shape at this path); #22 only has to write the same shape to the same
location. Until then, a missing file resolves to an empty registry and `matrix_get_dm` reports
"no DM room registered" rather than failing — see `charradissa-core/src/dm_registry.rs`.

### Wiring into agents

Tool authority is declared in Fondament role files (`definitions/fondament/*.yaml`,
`tools.always_on`) under MCP server name `charradissa` — e.g. resolved as
`mcp__charradissa__matrix_send`. Guilhem and the seven component agents carry `matrix_send` +
`matrix_get_dm`. Registering the server URL in the Guilhem pod's MCP config is Caissa#33.

---

## Infrastructure

The `infra/charradissa/charradissa.toml` file is the local development configuration (used with docker-compose):

```toml
[org]
name       = "dev"
homeserver = "http://synapse:8008"   # Synapse container on the compose network

[backend]
type = "matrix"

[tasks]
type = "none"        # No Jira integration in local dev

[projects]
autodiscover = false
```

The local dev stack expects a Synapse container reachable at `http://synapse:8008`.

---

## Building and Running

### Prerequisites

- Rust stable (2021 edition)
- A Matrix homeserver (Synapse recommended) with an appservice registration

### Build

```sh
# All crates
cargo build --workspace

# Production binary only
cargo build -p charradissa-daemon --release
```

### Container image

`charradissa-core` has a path dependency on `amassada-core` (`../../Amassada/crates/amassada-core`),
so the Docker build needs the **Amassada repo as a named build context** — `Amassada` must sit
as a sibling of this repo. The builder uses `rust:1.90-slim`:

```sh
# run from the parent directory that holds both repos
docker build --build-context amassada=./Amassada \
  -t ghcr.io/miegjorn/charradissa:latest ./Charradissa
```

For local kind deployment, load it in afterwards:
`kind load docker-image ghcr.io/miegjorn/charradissa:latest --name occitan`.
See `Caissa/docs/install.md` (step 3e) for the full stack.

> **Architecture**: Charradissa is the Matrix transport layer only. When a message arrives,
> `handle_transaction` fetches room history and forwards the event to Guilhem's
> `POST /matrix/reply` endpoint (`GUILHEM_URL` env, defaults to
> `http://guilhem.agents.svc.cluster.local:8080`). Guilhem runs `claude --print` with his
> full persona, Farga MCP, and `Bash` (giving access to `gh`, `glab`, `git`), then returns
> the reply text. Charradissa posts it back to Matrix. `responder.rs` is retained for
> reference and its tests, but is no longer in the production reply path.
>
> **Retry and fallback**: if the call to Guilhem fails (network error, timeout, or a
> non-2xx/unparseable response), `handle_transaction` retries up to 3 attempts total
> with a 5-second delay between attempts. If all attempts fail, Charradissa posts a
> fallback message to the room ("Guilhem is unreachable right now — please try again
> in a moment.") instead of silently dropping the event.

### Run

```sh
export CHARRADISSA_CONFIG=charradissa.toml
export MATRIX_AS_TOKEN=<your-as-token>
export RUST_LOG=charradissa=debug,info

cargo run -p charradissa-daemon
# or
./target/release/charradissa-daemon
```

The daemon will log `charradissa-daemon starting for org: <name>` and then `charradissa-daemon webhook listening on :<port>`.

---

## Testing

The codebase uses the standard Rust test harness. Key testing strategies:

- **Unit tests** for `parse_slash_command`, `requires_approval`, and `ApprovalQueue` logic live in `charradissa-core`.
- **Integration tests** for `MatrixBackend` and `JiraTaskManager` require a live homeserver or a mock HTTP server (e.g. `wiremock`).
- **`ChatBackend` is a trait**: tests inject a mock backend (`Arc<dyn ChatBackend>`) rather than the real `MatrixBackend`, keeping agent tier tests isolated from network calls.
- **`TaskManager` is a trait**: the same pattern applies for `JiraTaskManager` — inject a mock to test `ProjectAgent` dispatch logic.

```sh
cargo test --workspace
```

---

## Out of Scope (v1)

- E2EE (Matrix encrypted rooms)
- Federation across homeservers
- IRC backend (`charradissa-irc` crate)
- Linear / GitHub Issues as `TaskManager` backends
- ML-based pattern detection in `ConciergeAgent` (v1 uses LLM sweep only)
- Specialist invitation depth beyond flat (no sub-specialist chains)
- Web UI for the approval queue (CLI + Matrix room only)
- Full `get_task` and `list_open` implementation in `JiraTaskManager`
- `recv_human` polling in `CharradissaTransport`
