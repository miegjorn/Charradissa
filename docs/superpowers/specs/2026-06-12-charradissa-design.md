# Charradissa Design Spec
_2026-06-12_

## 1. Purpose

Charradissa is a daemon that hosts multiple AI agents as persistent members of a chat system (Matrix by default). Each agent lives in one or more rooms. Rooms are the substrate for human–agent collaboration and, when complexity warrants it, full Amassada multi-agent sessions.

Charradissa is **dual-mode**: a lightweight tool loop for single-turn interactions, and a full Amassada session engine for structured multi-agent work. The chat backend is abstracted — Matrix ships first, IRC and others follow the same trait.

---

## 2. Crate Structure

```
charradissa/
├── Cargo.toml                      # workspace root
├── charradissa-core/               # daemon logic, traits, agent tiers, tool loop
├── charradissa-matrix/             # MatrixBackend — Appservice API (feature = "matrix")
├── charradissa-irc/                # IrcBackend — future (feature = "irc")
├── charradissa-jira/               # JiraTaskManager — TaskManager impl (feature = "jira")
└── charradissa-daemon/             # binary — wires backends, config, tokio runtime
```

`charradissa-core` depends on `amassada-core` as a library. The session path calls into it directly — no separate process, no HTTP boundary.

---

## 3. ChatBackend Trait

All chat operations are mediated through a single trait. `charradissa-matrix` implements it via the Matrix Appservice API (one HTTP webhook endpoint, namespace of `@charradissa-*:homeserver` users — no per-bot WebSocket connections).

```rust
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
    fn event_stream(&self) -> impl Stream<Item = ChatEvent> + Send;
}
```

---

## 4. Agent Tiers & Provisioning

### Persistent tiers (provisioned at daemon startup)

| Tier | Matrix ID | Lives in |
|---|---|---|
| OrgAgent | `@org.<org>:homeserver` | `#<org>-general` |
| ProjectAgent | `@project.<name>:homeserver` | `#<project>` |
| ConciergeAgent | `@concierge:homeserver` | silent member of every `#<project>` |

Persistent agents are registered once and remain online for the daemon's lifetime.

### Specialists (JIT)

`@specialist-<uuid>:homeserver` — created via `register_agent()` when invited, deregistered via `deregister_agent()` when kicked. No account persists after the conversation ends. History is preserved in Farga, not in the Matrix account.

### Invitation model

- **Private consultation**: agent invokes a direct API call (mini Amassada session or single-turn dispatch). No Matrix room created, invisible to the main room. Result injected as context for the requesting agent.
- **Public invitation**: agent calls `request_invitation(address, reason)` → ProjectAgent evaluates → calls `invite()` if approved → specialist joins `#<project>` as a visible member. ProjectAgent is the sole gatekeeper of the public invitation pool.
- Specialists can request sub-invitations via `request_invitation()` to the ProjectAgent — they cannot self-invite. Flat delegation model (v1).

---

## 5. Room Topology

```
Matrix Space: <org>/<project>
├── #<project>                        ← main room: ProjectAgent + humans + specialists
├── #<project>-infra-approval         ← approval rooms, one per tool category
├── #<project>-code-approval
├── #<project>-db-approval
└── #<project>-session-<uuid>         ← ephemeral: Amassada sessions (born and deleted)
                                         #<project>-impl-<ticket_id> for task dispatch

Matrix Space: <org>
└── #<org>-general                    ← OrgAgent, ConciergeAgent (silent)
```

ConciergeAgent joins every `#<project>` room at creation as a standard member (visible in the member list). It never posts to any room — all output is via `send_dm()` to ProjectAgent or OrgAgent.

---

## 6. Message Flow

### Simple path (single-turn, default)

```
ChatEvent
→ on_message(room, event)
→ should_respond()              ← mention / name regex / DM
→ generate_reply(room_id)
    build_system_prompt()       ← CompositionAddress → Fondament → assembled system prompt
    messages.create()           ← non-streaming, tool_use
    tool loop (max 5 rounds):
      for each tool_use block:
        requires_approval?
          yes → post to #<project>-<cat>-approval
                block until /approve <id> | /reject <id>
                timeout → auto-reject (configurable, default 60 min)
          no  → execute_tool() → inject tool_result
      until text-only response
→ room_send()
```

### Session path (agent-initiated or human slash command)

```
agent calls start_session(canvas_id, goal)
  OR human sends /session <canvas_id> "<goal>"
→ create_room(#<project>-session-<uuid>) → add_to_space()
→ invite relevant agents as Matrix members
→ CharradissaTransport::new(room_id, backend)    ← implements amassada_core::Transport
→ amassada_core::run(canvas, goal, transport)
→ session closes (Amassada close signal)
→ ConciergeAgent sweep:
    room_history(since: session_start) → extract decisions + artifacts → Farga::write()
    delete_room(#<project>-session-<uuid>)
```

`CharradissaTransport` is the seam: implements `amassada_core::Transport`, translating Matrix room events into Amassada's session event model and back.

---

## 7. Approval Queue

Write-requiring tool calls are gated through category-specific approval rooms.

**Flow:**
1. Tool loop identifies `requires_approval: true` on a tool call
2. Posts a pending action to `#<project>-<category>-approval` with a unique `<id>`, description, and proposed parameters
3. Tool loop blocks (tokio channel `oneshot`)
4. Human responds with `/approve <id>` or `/reject <id> [reason]`
5. Charradissa parses the command, resolves the oneshot, tool loop resumes with result
6. If no response within `approval.timeout_minutes`: auto-reject

All approvals are logged with timestamp, approver identity, and decision — audit trail in Farga.

---

## 8. ConciergeAgent

ConciergeAgent runs two scheduled jobs. Its daily LLM token budget is hard-capped in config before any job fires.

### Job 1 — Room archival (per room, every 24h or on session close)

```
for each #<project> room:
  events = room_history(since: now - 24h)
  if events non-empty:
    signals = extract_signals(events)     ← decisions, blockers, artifacts, patterns
    farga.write_signals(project, signals)
    prune_room(events)                    ← delete messages older than 24h
```

### Job 2 — Cross-project convergence sweep (configurable interval, default 6h)

```
summaries = [ farga.recent_signals(project, since: last_sweep) for each project ]
convergence = llm_call(concierge_persona, summaries)
for each opportunity in convergence:
  target = project_agent_user_id(opportunity.project)   ← looked up from daemon's agent registry
  send_dm(target, whisper)               ← never posts to any room
```

### Farga write boundary

```rust
pub trait FargaWriter: Send + Sync {
    async fn write_signals(&self, project: &ProjectId, signals: Vec<Signal>) -> Result<()>;
    async fn recent_signals(&self, project: &ProjectId, since: Duration) -> Result<Vec<Signal>>;
}
```

Charradissa writes raw signals. Farga's own internal agent handles dedup, conflict resolution, and lazy resolution index optimization independently.

---

## 9. Task Management & Implementation Dispatch

Task operations are abstracted behind `TaskManager` (Jira default via `charradissa-jira`).

```rust
#[async_trait]
pub trait TaskManager: Send + Sync {
    async fn create_task(&self, project: &ProjectId, opts: &TaskOptions) -> Result<TaskId>;
    async fn assign_task(&self, task: &TaskId, assignee: &Assignee) -> Result<()>;
    async fn update_status(&self, task: &TaskId, status: TaskStatus) -> Result<()>;
    async fn get_task(&self, task: &TaskId) -> Result<Task>;
    async fn list_open(&self, project: &ProjectId) -> Result<Vec<Task>>;
}
```

`Assignee` is either a human user ID or a `CompositionAddress` (agent).

### Implementation dispatch (ticket assigned to an agent)

```
ProjectAgent calls dispatch_implementation(ticket_id, canvas?)
→ task_manager.update_status(ticket_id, InProgress)
→ create_room(#<project>-impl-<ticket_id>) → add_to_space()
→ run Amassada with implement-session canvas (or caller-specified)
→ session closes
→ Concierge archives artifacts → Farga
→ task_manager.update_status(ticket_id, InReview)
→ post implementation summary to #<project>
→ delete_room(#<project>-impl-<ticket_id>)
```

Human approval for write actions inside impl sessions routes through `#<project>-code-approval` as normal — impl sessions are not exempt.

---

## 10. Cor MCP Tools

Four tools expose Charradissa to Claude Code sessions (human-driven):

```rust
/// Sends a visible message to #<project>
post(project: &str, message: &str) -> Result<()>

/// DM to ProjectAgent — response returned, never visible in room
ask(project: &str, question: &str) -> Result<String>

/// Dispatches a fix task; routes through approval queue if write tools needed
fix(project: &str, bug_id: &str, context: Option<&str>) -> Result<()>

/// Returns open tasks (Jira), pipeline status, active session, last Concierge sweep
health(project: &str) -> Result<ProjectHealth>
```

`health` reads from `TaskManager::list_open()` — consistent with the abstraction, not a direct Jira call.

---

## 11. Configuration (`charradissa.toml`)

```toml
[org]
name = "acme"
homeserver = "https://matrix.acme.internal"

[backend]
type = "matrix"                       # "irc" when charradissa-irc ships

[concierge]
archival_interval_hours = 24
convergence_interval_hours = 6
daily_token_budget = 50_000

[approval]
timeout_minutes = 60                  # auto-reject if no /approve within window

[tasks]
type = "jira"
base_url = "https://acme.atlassian.net"
project_key = "PROJ"

[projects]
# auto-discovered from Farga, or listed explicitly
autodiscover = true
```

---

## 12. Key Crates

- `tokio` — async runtime
- `axum` — HTTP server (Appservice webhook endpoint)
- `serde` + `serde_yaml` / `toml` — config and event parsing
- `async-trait` — ChatBackend, TaskManager, FargaWriter traits
- `anthropic` (or `reqwest` against Anthropic API) — agent dispatch
- `amassada-core` — session engine (library dependency)
- `matrix-sdk` or raw Appservice HTTP — Matrix backend

---

## 13. Out of Scope (v1)

- E2EE (Matrix encrypted rooms)
- Federation across homeservers
- IRC backend implementation
- Linear / GitHub Issues as TaskManager backends
- ConciergeAgent ML-based pattern detection (v1 uses LLM sweep only)
- Specialist invitation depth beyond flat (no sub-specialist chains)
- Web UI for approval queue (CLI + Matrix room only)
