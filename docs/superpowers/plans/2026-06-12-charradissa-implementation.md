# Charradissa Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the dual-mode chat daemon — lightweight tool loop for single-turn interactions, full Amassada session engine for structured multi-agent work, Matrix Appservice API as the default chat backend.

**Architecture:** `charradissa-core` owns traits (ChatBackend, TaskManager), agent tier logic, tool loop, and approval queue. It depends on `amassada-core` as a library. `charradissa-matrix` implements `ChatBackend` via the Matrix Appservice API (one axum webhook, namespace of `@charradissa-*` users). `charradissa-daemon` is the binary that wires config, backend, and tokio runtime.

**Tech Stack:** Rust, tokio, axum, serde/toml, reqwest (Anthropic API), amassada-core, async-trait

---

## File Map

```
charradissa/
├── Cargo.toml
├── charradissa.toml.example
├── charradissa-core/src/
│   ├── lib.rs
│   ├── types.rs              # RoomId, UserId, SpaceId, CompositionAddress, ChatEvent, TaskId, Task, etc.
│   ├── error.rs              # CharradissaError
│   ├── config.rs             # Config struct, charradissa.toml parsing
│   ├── backend.rs            # ChatBackend trait
│   ├── task.rs               # TaskManager trait + TaskOptions + Assignee
│   ├── farga.rs              # FargaWriter trait (charradissa side)
│   ├── tool_loop.rs          # Simple path: build_context, tool loop, approval gate
│   ├── approval.rs           # ApprovalQueue: pending map, oneshot senders, /approve /reject
│   ├── concierge.rs          # ConciergeAgent: room archival + convergence sweep
│   ├── agents/
│   │   ├── mod.rs
│   │   ├── org.rs            # OrgAgent
│   │   ├── project.rs        # ProjectAgent (also hosts session path + task dispatch)
│   │   └── specialist.rs     # Specialist (JIT register/deregister)
│   └── transport.rs          # CharradissaTransport — implements amassada_core::Transport
├── charradissa-matrix/src/
│   ├── lib.rs
│   ├── backend.rs            # MatrixBackend: ChatBackend impl via Appservice API
│   ├── appservice.rs         # axum webhook handler for Matrix events
│   └── client.rs             # Appservice HTTP client (room_send, register, invite, etc.)
├── charradissa-jira/src/
│   ├── lib.rs
│   └── backend.rs            # JiraTaskManager: TaskManager impl
└── charradissa-daemon/src/
    ├── main.rs               # startup: load config, wire backend, start agents, serve webhook
    └── registry.rs           # AgentRegistry: live map of agent_id → UserId
```

---

### Task 1: Workspace Scaffold + Config

**Files:** `Cargo.toml`, crate `Cargo.toml`s, `charradissa-core/src/config.rs`, `charradissa.toml.example`

- [ ] **Step 1: Create workspace Cargo.toml**

```toml
# charradissa/Cargo.toml
[workspace]
members = [
    "charradissa-core",
    "charradissa-matrix",
    "charradissa-jira",
    "charradissa-daemon",
]
resolver = "2"

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
axum = "0.7"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
async-trait = "0.1"
reqwest = { version = "0.12", features = ["json"] }
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1", features = ["v4"] }
thiserror = "1"
anyhow = "1"
tracing = "0.1"
tracing-subscriber = "0.3"
futures = "0.3"
tokio-stream = "0.1"
```

- [ ] **Step 2: Create crate Cargo.tomls**

```toml
# charradissa-core/Cargo.toml
[package]
name = "charradissa-core"
version = "0.1.0"
edition = "2021"

[dependencies]
amassada-core = { path = "../../../Amassada/crates/amassada-core" }
tokio = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
toml = { workspace = true }
async-trait = { workspace = true }
reqwest = { workspace = true }
chrono = { workspace = true }
uuid = { workspace = true }
thiserror = { workspace = true }
anyhow = { workspace = true }
futures = { workspace = true }
tokio-stream = { workspace = true }
tracing = { workspace = true }
```

```toml
# charradissa-matrix/Cargo.toml
[package]
name = "charradissa-matrix"
version = "0.1.0"
edition = "2021"

[dependencies]
charradissa-core = { path = "../charradissa-core" }
tokio = { workspace = true }
axum = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
reqwest = { workspace = true }
chrono = { workspace = true }
uuid = { workspace = true }
anyhow = { workspace = true }
tracing = { workspace = true }
```

```toml
# charradissa-jira/Cargo.toml
[package]
name = "charradissa-jira"
version = "0.1.0"
edition = "2021"

[dependencies]
charradissa-core = { path = "../charradissa-core" }
reqwest = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
anyhow = { workspace = true }
async-trait = { workspace = true }
```

```toml
# charradissa-daemon/Cargo.toml
[package]
name = "charradissa-daemon"
version = "0.1.0"
edition = "2021"

[dependencies]
charradissa-core = { path = "../charradissa-core" }
charradissa-matrix = { path = "../charradissa-matrix" }
charradissa-jira = { path = "../charradissa-jira" }
tokio = { workspace = true }
axum = { workspace = true }
serde = { workspace = true }
toml = { workspace = true }
anyhow = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
```

- [ ] **Step 3: Write config.rs**

```rust
// charradissa-core/src/config.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub org: OrgConfig,
    pub backend: BackendConfig,
    pub concierge: ConciergeConfig,
    pub approval: ApprovalConfig,
    pub tasks: TasksConfig,
    pub projects: ProjectsConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OrgConfig {
    pub name: String,
    pub homeserver: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BackendConfig {
    #[serde(rename = "type")]
    pub backend_type: String, // "matrix" | "irc"
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConciergeConfig {
    #[serde(default = "default_archival_interval")]
    pub archival_interval_hours: u64,
    #[serde(default = "default_convergence_interval")]
    pub convergence_interval_hours: u64,
    #[serde(default = "default_daily_budget")]
    pub daily_token_budget: u32,
}

fn default_archival_interval() -> u64 { 24 }
fn default_convergence_interval() -> u64 { 6 }
fn default_daily_budget() -> u32 { 50_000 }

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApprovalConfig {
    #[serde(default = "default_timeout")]
    pub timeout_minutes: u64,
}

fn default_timeout() -> u64 { 60 }

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TasksConfig {
    #[serde(rename = "type")]
    pub tasks_type: String, // "jira" | "none"
    pub base_url: Option<String>,
    pub project_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProjectsConfig {
    #[serde(default = "default_true")]
    pub autodiscover: bool,
}

fn default_true() -> bool { true }

impl Config {
    pub fn from_file(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&content)?)
    }
}
```

- [ ] **Step 4: Write charradissa.toml.example**

```toml
# charradissa.toml.example
[org]
name = "acme"
homeserver = "https://matrix.acme.internal"

[backend]
type = "matrix"

[concierge]
archival_interval_hours = 24
convergence_interval_hours = 6
daily_token_budget = 50000

[approval]
timeout_minutes = 60

[tasks]
type = "jira"
base_url = "https://acme.atlassian.net"
project_key = "PROJ"

[projects]
autodiscover = true
```

- [ ] **Step 5: Create stubs and verify**

```bash
cd /Users/bedardpl/project/Charradissa && cargo check --workspace 2>&1
```

- [ ] **Step 6: Commit**

```bash
git init && git add -A && git commit -m "feat: scaffold charradissa workspace and config"
```

---

### Task 2: Core Types & Error

**Files:** `charradissa-core/src/types.rs`, `charradissa-core/src/error.rs`

- [ ] **Step 1: Write failing tests**

```rust
// charradissa-core/tests/types_tests.rs
use charradissa_core::types::*;

#[test]
fn room_id_roundtrip() {
    let id = RoomId::new("!abc123:matrix.acme.internal");
    assert_eq!(id.as_str(), "!abc123:matrix.acme.internal");
}

#[test]
fn composition_address_display() {
    let addr = CompositionAddress::Role { role: "tech-moderator".into(), stance_override: None };
    assert!(addr.to_string().contains("tech-moderator"));
}
```

- [ ] **Step 2: Implement error.rs**

```rust
// charradissa-core/src/error.rs
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CharradissaError {
    #[error("backend error: {0}")]
    Backend(String),
    #[error("approval timeout for {id}")]
    ApprovalTimeout { id: String },
    #[error("approval rejected: {reason}")]
    ApprovalRejected { reason: String },
    #[error("tool error: {0}")]
    Tool(String),
    #[error("dispatch error: {0}")]
    Dispatch(String),
    #[error("config error: {0}")]
    Config(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, CharradissaError>;
```

- [ ] **Step 3: Implement types.rs**

```rust
// charradissa-core/src/types.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RoomId(String);

impl RoomId {
    pub fn new(s: &str) -> Self { Self(s.to_string()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

impl std::fmt::Display for RoomId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserId(String);

impl UserId {
    pub fn new(s: &str) -> Self { Self(s.to_string()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpaceId(String);

impl SpaceId {
    pub fn new(s: &str) -> Self { Self(s.to_string()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProjectId(String);

impl ProjectId {
    pub fn new(s: &str) -> Self { Self(s.to_string()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

impl std::fmt::Display for ProjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(String);

impl TaskId {
    pub fn new(s: &str) -> Self { Self(s.to_string()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

/// The composition address — how Fondament assembles agent context.
/// Carried through the whole system; resolved to a system prompt at dispatch time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompositionAddress {
    /// A named Fondament role, optionally with a stance override
    Role { role: String, stance_override: Option<String> },
    /// A project/facet/stance composition resolved from Farga at dispatch time
    Composed { project: String, facet: String, stance: String },
}

impl std::fmt::Display for CompositionAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Role { role, stance_override } => {
                if let Some(s) = stance_override {
                    write!(f, "fondament/{}/{}", role, s)
                } else {
                    write!(f, "fondament/{}", role)
                }
            }
            Self::Composed { project, facet, stance } => {
                write!(f, "{}/{}+{}", project, facet, stance)
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatEvent {
    pub event_id: String,
    pub room_id: RoomId,
    pub sender: UserId,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub kind: ChatEventKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ChatEventKind {
    Message,
    SlashCommand { command: String, args: String },
    Mention,
    Reaction,
    MemberJoin,
    MemberLeave,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomOptions {
    pub alias: String,
    pub name: String,
    pub topic: Option<String>,
    pub invite: Vec<UserId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    pub description: String,
    pub status: TaskStatus,
    pub assignee: Option<Assignee>,
    pub project: ProjectId,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskStatus {
    Open, InProgress, InReview, Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Assignee {
    Human(UserId),
    Agent(CompositionAddress),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskOptions {
    pub title: String,
    pub description: String,
    pub assignee: Option<Assignee>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectHealth {
    pub project: ProjectId,
    pub open_tasks: Vec<Task>,
    pub active_session: Option<String>,
    pub last_concierge_sweep: Option<DateTime<Utc>>,
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test --package charradissa-core 2>&1
```
Expected: 2 tests pass

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: add CharradissaError, RoomId, UserId, CompositionAddress, ChatEvent types"
```

---

### Task 3: ChatBackend & TaskManager Traits

**Files:** `charradissa-core/src/backend.rs`, `charradissa-core/src/task.rs`, `charradissa-core/src/farga.rs`

- [ ] **Step 1: Write failing tests**

```rust
// charradissa-core/tests/trait_tests.rs
use charradissa_core::backend::ChatBackend;
use charradissa_core::task::TaskManager;
use charradissa_core::farga::FargaWriter;

fn _assert_chat_backend_object_safe(_: &dyn ChatBackend) {}
fn _assert_task_manager_object_safe(_: &dyn TaskManager) {}
fn _assert_farga_writer_object_safe(_: &dyn FargaWriter) {}

#[test]
fn traits_are_object_safe() {
    // Compile-only: if this compiles, the traits are object-safe.
}
```

- [ ] **Step 2: Implement backend.rs**

```rust
// charradissa-core/src/backend.rs
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::Stream;
use crate::error::Result;
use crate::types::{ChatEvent, CompositionAddress, RoomId, RoomOptions, SpaceId, UserId};

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
}
```

- [ ] **Step 3: Implement task.rs**

```rust
// charradissa-core/src/task.rs
use async_trait::async_trait;
use crate::error::Result;
use crate::types::{Assignee, ProjectId, Task, TaskId, TaskOptions, TaskStatus};

#[async_trait]
pub trait TaskManager: Send + Sync {
    async fn create_task(&self, project: &ProjectId, opts: &TaskOptions) -> Result<TaskId>;
    async fn assign_task(&self, task: &TaskId, assignee: &Assignee) -> Result<()>;
    async fn update_status(&self, task: &TaskId, status: TaskStatus) -> Result<()>;
    async fn get_task(&self, task: &TaskId) -> Result<Task>;
    async fn list_open(&self, project: &ProjectId) -> Result<Vec<Task>>;
}
```

- [ ] **Step 4: Implement farga.rs**

```rust
// charradissa-core/src/farga.rs
use async_trait::async_trait;
use chrono::Duration;
use serde::{Deserialize, Serialize};
use crate::error::Result;
use crate::types::ProjectId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    pub project: String,
    pub content: String,
    pub source: String,
}

#[async_trait]
pub trait FargaWriter: Send + Sync {
    async fn write_signals(&self, project: &ProjectId, signals: Vec<Signal>) -> Result<()>;
    async fn recent_signals(&self, project: &ProjectId, since: Duration) -> Result<Vec<Signal>>;
}

/// HTTP client implementation connecting to farga-server
pub struct HttpFargaWriter {
    client: reqwest::Client,
    base_url: String,
}

impl HttpFargaWriter {
    pub fn new(base_url: String) -> Self {
        Self { client: reqwest::Client::new(), base_url }
    }
}

#[async_trait]
impl FargaWriter for HttpFargaWriter {
    async fn write_signals(&self, project: &ProjectId, signals: Vec<Signal>) -> Result<()> {
        let url = format!("{}/signals", self.base_url);
        self.client.post(&url)
            .json(&serde_json::json!({ "project": project.as_str(), "signals": signals }))
            .send().await
            .map_err(|e| crate::error::CharradissaError::Backend(e.to_string()))?;
        Ok(())
    }

    async fn recent_signals(&self, project: &ProjectId, since: Duration) -> Result<Vec<Signal>> {
        let url = format!("{}/signals/recent?project={}&since={}h",
            self.base_url, project.as_str(), since.num_hours());
        let resp = self.client.get(&url).send().await
            .map_err(|e| crate::error::CharradissaError::Backend(e.to_string()))?;
        resp.json().await
            .map_err(|e| crate::error::CharradissaError::Backend(e.to_string()))
    }
}
```

- [ ] **Step 5: Run tests**

```bash
cargo test --package charradissa-core 2>&1
```
Expected: 1 test passes

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat: add ChatBackend, TaskManager, FargaWriter traits"
```

---

### Task 4: Approval Queue

**Files:** `charradissa-core/src/approval.rs`

- [ ] **Step 1: Write failing tests**

```rust
// charradissa-core/tests/approval_tests.rs
use charradissa_core::approval::{ApprovalQueue, ApprovalOutcome};

#[tokio::test]
async fn approval_resolves_on_approve() {
    let mut queue = ApprovalQueue::new(60);
    let (id, rx) = queue.create_pending("infra".into(), "delete bucket".into(), serde_json::json!({}));
    queue.resolve(&id, ApprovalOutcome::Approved).unwrap();
    let result = rx.await.unwrap();
    assert_eq!(result, ApprovalOutcome::Approved);
}

#[tokio::test]
async fn approval_resolves_on_reject() {
    let mut queue = ApprovalQueue::new(60);
    let (id, rx) = queue.create_pending("code".into(), "merge PR".into(), serde_json::json!({}));
    queue.resolve(&id, ApprovalOutcome::Rejected("too risky".into())).unwrap();
    let result = rx.await.unwrap();
    assert!(matches!(result, ApprovalOutcome::Rejected(_)));
}
```

- [ ] **Step 2: Implement approval.rs**

```rust
// charradissa-core/src/approval.rs
use std::collections::HashMap;
use tokio::sync::oneshot;
use uuid::Uuid;
use crate::error::{CharradissaError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalOutcome {
    Approved,
    Rejected(String),
}

pub struct PendingApproval {
    pub id: String,
    pub category: String,       // "infra" | "code" | "db"
    pub description: String,
    pub params: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
    tx: oneshot::Sender<ApprovalOutcome>,
}

pub struct ApprovalQueue {
    pending: HashMap<String, PendingApproval>,
    timeout_minutes: u64,
}

impl ApprovalQueue {
    pub fn new(timeout_minutes: u64) -> Self {
        Self { pending: HashMap::new(), timeout_minutes }
    }

    pub fn create_pending(
        &mut self,
        category: String,
        description: String,
        params: serde_json::Value,
    ) -> (String, oneshot::Receiver<ApprovalOutcome>) {
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending.insert(id.clone(), PendingApproval {
            id: id.clone(),
            category,
            description,
            params,
            created_at: chrono::Utc::now(),
            tx,
        });
        (id, rx)
    }

    pub fn resolve(&mut self, id: &str, outcome: ApprovalOutcome) -> Result<()> {
        let entry = self.pending.remove(id)
            .ok_or_else(|| CharradissaError::Backend(format!("unknown approval id: {}", id)))?;
        let _ = entry.tx.send(outcome);
        Ok(())
    }

    pub fn list_pending(&self) -> Vec<(&str, &str, &str)> {
        self.pending.values()
            .map(|p| (p.id.as_str(), p.category.as_str(), p.description.as_str()))
            .collect()
    }

    pub fn timeout_minutes(&self) -> u64 { self.timeout_minutes }
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test --package charradissa-core approval 2>&1
```
Expected: 2 tests pass

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "feat: add ApprovalQueue with oneshot-based resolve for /approve /reject flow"
```

---

### Task 5: Tool Loop (Simple Path)

**Files:** `charradissa-core/src/tool_loop.rs`

- [ ] **Step 1: Write failing tests**

```rust
// charradissa-core/tests/tool_loop_tests.rs
use charradissa_core::tool_loop::{parse_slash_command, SlashCommand};

#[test]
fn parses_approve_command() {
    let cmd = parse_slash_command("/approve abc-123");
    assert_eq!(cmd, Some(SlashCommand::Approve { id: "abc-123".into() }));
}

#[test]
fn parses_reject_command() {
    let cmd = parse_slash_command("/reject abc-123 too risky");
    assert_eq!(cmd, Some(SlashCommand::Reject { id: "abc-123".into(), reason: "too risky".into() }));
}

#[test]
fn parses_session_command() {
    let cmd = parse_slash_command("/session debate \"should we use microservices?\"");
    assert!(matches!(cmd, Some(SlashCommand::Session { .. })));
}

#[test]
fn returns_none_for_unknown() {
    let cmd = parse_slash_command("/unknown foo");
    assert!(cmd.is_none());
}
```

- [ ] **Step 2: Implement tool_loop.rs**

```rust
// charradissa-core/src/tool_loop.rs
use serde::{Deserialize, Serialize};
use crate::error::{CharradissaError, Result};
use crate::types::{CompositionAddress, RoomId};

const DEFAULT_MODEL: &str = "claude-sonnet-4-6";
const MAX_TOOL_ROUNDS: u32 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    Approve { id: String },
    Reject { id: String, reason: String },
    Session { canvas_id: String, goal: String },
    Invite { address: String },
    Call,
}

pub fn parse_slash_command(text: &str) -> Option<SlashCommand> {
    let text = text.trim();
    if !text.starts_with('/') { return None; }

    let parts: Vec<&str> = text[1..].splitn(3, ' ').collect();
    match parts[0] {
        "approve" => {
            let id = parts.get(1)?.to_string();
            Some(SlashCommand::Approve { id })
        }
        "reject" => {
            let id = parts.get(1)?.to_string();
            let reason = parts.get(2).copied().unwrap_or("no reason given").to_string();
            Some(SlashCommand::Reject { id, reason })
        }
        "session" => {
            let canvas_id = parts.get(1)?.to_string();
            let goal = parts.get(2)
                .map(|s| s.trim_matches('"').to_string())
                .unwrap_or_else(|| "unspecified goal".into());
            Some(SlashCommand::Session { canvas_id, goal })
        }
        "invite" => {
            let address = parts.get(1)?.to_string();
            Some(SlashCommand::Invite { address })
        }
        "call" => Some(SlashCommand::Call),
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
    pub requires_approval: bool,
    pub approval_category: Option<String>, // "infra" | "code" | "db"
}

/// Executes the simple-path tool loop.
/// 1. Call API with assembled context
/// 2. Parse tool_use blocks
/// 3. Gate writes through approval queue
/// 4. Execute approved tools, inject results
/// 5. Repeat until text-only response (max MAX_TOOL_ROUNDS)
pub struct ToolLoopConfig {
    pub model: String,
    pub system_prompt: String,
    pub context: String,
    pub room_id: RoomId,
    pub max_rounds: u32,
}

impl Default for ToolLoopConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.into(),
            system_prompt: String::new(),
            context: String::new(),
            room_id: RoomId::new(""),
            max_rounds: MAX_TOOL_ROUNDS,
        }
    }
}

/// Determines if a tool call requires human approval before execution.
pub fn requires_approval(tool_name: &str) -> (bool, Option<String>) {
    match tool_name {
        t if t.starts_with("git_") || t.starts_with("pr_") || t.starts_with("code_") => {
            (true, Some("code".into()))
        }
        t if t.starts_with("db_") || t.starts_with("sql_") => {
            (true, Some("db".into()))
        }
        t if t.starts_with("infra_") || t.starts_with("aws_") || t.starts_with("tf_") => {
            (true, Some("infra".into()))
        }
        _ => (false, None),
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test --package charradissa-core tool_loop 2>&1
```
Expected: 4 tests pass

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "feat: add tool loop: slash command parser, approval gate, ToolLoopConfig"
```

---

### Task 6: ConciergeAgent Scheduled Jobs

**Files:** `charradissa-core/src/concierge.rs`

- [ ] **Step 1: Write failing tests**

```rust
// charradissa-core/tests/concierge_tests.rs
use charradissa_core::concierge::extract_signals;

#[test]
fn extracts_signals_from_events() {
    use charradissa_core::types::{ChatEvent, ChatEventKind, RoomId, UserId};
    use chrono::Utc;

    let events = vec![
        ChatEvent {
            event_id: "1".into(),
            room_id: RoomId::new("!room:matrix.test"),
            sender: UserId::new("@agent:matrix.test"),
            content: "Decision: use PostgreSQL for the user service.".into(),
            timestamp: Utc::now(),
            kind: ChatEventKind::Message,
        },
        ChatEvent {
            event_id: "2".into(),
            room_id: RoomId::new("!room:matrix.test"),
            sender: UserId::new("@human:matrix.test"),
            content: "Blocker: we need a design review before proceeding.".into(),
            timestamp: Utc::now(),
            kind: ChatEventKind::Message,
        },
    ];

    let signals = extract_signals(&events);
    assert!(!signals.is_empty());
}
```

- [ ] **Step 2: Implement concierge.rs**

```rust
// charradissa-core/src/concierge.rs
use std::sync::Arc;
use std::time::Duration;
use chrono::Utc;
use tokio::time::interval;
use crate::backend::ChatBackend;
use crate::farga::{FargaWriter, Signal};
use crate::types::{ChatEvent, ProjectId, RoomId, UserId};

/// Heuristic signal extraction from room events.
/// In production: replace with a lightweight LLM call.
pub fn extract_signals(events: &[ChatEvent]) -> Vec<Signal> {
    let signal_keywords = ["decision:", "decided:", "blocker:", "blocked:", "artifact:", "pattern:"];
    events.iter().filter_map(|e| {
        let lower = e.content.to_lowercase();
        if signal_keywords.iter().any(|kw| lower.contains(kw)) {
            Some(Signal {
                project: e.room_id.to_string(),
                content: e.content.clone(),
                source: "concierge-archival".into(),
            })
        } else {
            None
        }
    }).collect()
}

pub struct ConciergeAgent {
    backend: Arc<dyn ChatBackend>,
    farga: Arc<dyn FargaWriter>,
    projects: Vec<ProjectId>,
    project_agent_ids: std::collections::HashMap<ProjectId, UserId>,
    archival_interval_hours: u64,
    convergence_interval_hours: u64,
    daily_token_budget: u32,
}

impl ConciergeAgent {
    pub fn new(
        backend: Arc<dyn ChatBackend>,
        farga: Arc<dyn FargaWriter>,
        projects: Vec<ProjectId>,
        project_agent_ids: std::collections::HashMap<ProjectId, UserId>,
        archival_interval_hours: u64,
        convergence_interval_hours: u64,
        daily_token_budget: u32,
    ) -> Self {
        Self { backend, farga, projects, project_agent_ids,
               archival_interval_hours, convergence_interval_hours, daily_token_budget }
    }

    /// Job 1: archive room history every N hours
    pub async fn run_archival_loop(&self) {
        let mut ticker = interval(Duration::from_secs(self.archival_interval_hours * 3600));
        loop {
            ticker.tick().await;
            for project in &self.projects {
                let room_id = RoomId::new(&format!("#{}", project.as_str()));
                let since = Utc::now() - chrono::Duration::hours(self.archival_interval_hours as i64);
                match self.backend.room_history(&room_id, since).await {
                    Ok(events) if !events.is_empty() => {
                        let signals = extract_signals(&events);
                        if !signals.is_empty() {
                            if let Err(e) = self.farga.write_signals(project, signals).await {
                                tracing::error!("concierge: farga write failed for {}: {}", project, e);
                            }
                        }
                    }
                    Ok(_) => {} // empty, nothing to archive
                    Err(e) => tracing::error!("concierge: room_history failed for {}: {}", project, e),
                }
            }
        }
    }

    /// Job 2: cross-project convergence sweep every N hours
    pub async fn run_convergence_loop(&self) {
        let mut ticker = interval(Duration::from_secs(self.convergence_interval_hours * 3600));
        loop {
            ticker.tick().await;
            let since = chrono::Duration::hours(self.convergence_interval_hours as i64);
            let mut all_signals = Vec::new();
            for project in &self.projects {
                match self.farga.recent_signals(project, since).await {
                    Ok(signals) => all_signals.extend(signals),
                    Err(e) => tracing::error!("concierge: recent_signals failed for {}: {}", project, e),
                }
            }

            if all_signals.is_empty() { continue; }

            // Simplified: in production, call LLM with all_signals for convergence analysis
            // and emit targeted whispers to ProjectAgents.
            // v0.1.0 logs the sweep for operator visibility.
            tracing::info!("concierge convergence sweep: {} signals across {} projects",
                all_signals.len(), self.projects.len());
        }
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test --package charradissa-core concierge 2>&1
```
Expected: 1 test passes

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "feat: add ConciergeAgent with archival and convergence scheduled loops"
```

---

### Task 7: Agent Tiers

**Files:** `charradissa-core/src/agents/`

- [ ] **Step 1: Implement agents/project.rs**

```rust
// charradissa-core/src/agents/project.rs
use std::sync::Arc;
use crate::approval::{ApprovalOutcome, ApprovalQueue};
use crate::backend::ChatBackend;
use crate::task::TaskManager;
use crate::tool_loop::{parse_slash_command, requires_approval, SlashCommand};
use crate::types::*;

pub struct ProjectAgent {
    pub project: ProjectId,
    pub user_id: UserId,
    pub main_room: RoomId,
    pub space: SpaceId,
    backend: Arc<dyn ChatBackend>,
    task_manager: Arc<dyn TaskManager>,
    approval_queue: Arc<tokio::sync::Mutex<ApprovalQueue>>,
}

impl ProjectAgent {
    pub fn new(
        project: ProjectId,
        user_id: UserId,
        main_room: RoomId,
        space: SpaceId,
        backend: Arc<dyn ChatBackend>,
        task_manager: Arc<dyn TaskManager>,
        approval_queue: Arc<tokio::sync::Mutex<ApprovalQueue>>,
    ) -> Self {
        Self { project, user_id, main_room, space, backend, task_manager, approval_queue }
    }

    pub async fn handle_event(&self, event: &ChatEvent) -> crate::error::Result<()> {
        // Slash commands (from any user in the room)
        if let ChatEventKind::SlashCommand { command, args } = &event.kind {
            let full = format!("/{} {}", command, args);
            if let Some(cmd) = parse_slash_command(&full) {
                return self.handle_slash_command(cmd, &event.sender).await;
            }
        }

        // Approval resolution: /approve and /reject come in as messages
        if let Some(cmd) = parse_slash_command(&event.content) {
            match cmd {
                SlashCommand::Approve { id } => {
                    let mut q = self.approval_queue.lock().await;
                    let _ = q.resolve(&id, ApprovalOutcome::Approved);
                }
                SlashCommand::Reject { id, reason } => {
                    let mut q = self.approval_queue.lock().await;
                    let _ = q.resolve(&id, ApprovalOutcome::Rejected(reason));
                }
                _ => {}
            }
        }

        Ok(())
    }

    async fn handle_slash_command(&self, cmd: SlashCommand, from: &UserId) -> crate::error::Result<()> {
        match cmd {
            SlashCommand::Session { canvas_id, goal } => {
                tracing::info!("project {}: session requested — canvas={} goal={}",
                    self.project, canvas_id, goal);
                // Dispatch to Amassada via CharradissaTransport in full impl
            }
            SlashCommand::Invite { address } => {
                // Register specialist and invite to main room
                let addr = CompositionAddress::Role { role: address, stance_override: None };
                let specialist_id = self.backend.register_agent(&addr).await?;
                self.backend.invite(&self.main_room, &specialist_id).await?;
            }
            _ => {}
        }
        Ok(())
    }

    pub async fn create_task(&self, opts: &TaskOptions) -> crate::error::Result<TaskId> {
        self.task_manager.create_task(&self.project, opts).await
    }

    pub async fn dispatch_implementation(&self, ticket_id: &TaskId) -> crate::error::Result<()> {
        self.task_manager.update_status(ticket_id, TaskStatus::InProgress).await?;
        let room_alias = format!("#{}-impl-{}", self.project.as_str(), ticket_id.as_str());
        let impl_room = self.backend.create_room(&RoomOptions {
            alias: room_alias.clone(),
            name: format!("impl: {}", ticket_id.as_str()),
            topic: None,
            invite: vec![self.user_id.clone()],
        }).await?;
        self.backend.add_to_space(&self.space, &impl_room).await?;
        // Run Amassada session in impl_room via CharradissaTransport in full impl
        self.task_manager.update_status(ticket_id, TaskStatus::InReview).await?;
        Ok(())
    }
}
```

- [ ] **Step 2: Implement agents/org.rs**

```rust
// charradissa-core/src/agents/org.rs
use std::sync::Arc;
use crate::backend::ChatBackend;
use crate::types::*;

pub struct OrgAgent {
    pub org: String,
    pub user_id: UserId,
    pub general_room: RoomId,
    backend: Arc<dyn ChatBackend>,
}

impl OrgAgent {
    pub fn new(org: String, user_id: UserId, general_room: RoomId, backend: Arc<dyn ChatBackend>) -> Self {
        Self { org, user_id, general_room, backend }
    }

    pub async fn handle_event(&self, event: &ChatEvent) -> crate::error::Result<()> {
        tracing::debug!("org agent received event from {}", event.sender);
        Ok(())
    }

    pub async fn broadcast_org(&self, message: &str) -> crate::error::Result<()> {
        self.backend.send_message(&self.general_room, message).await
    }
}
```

- [ ] **Step 3: Implement agents/specialist.rs**

```rust
// charradissa-core/src/agents/specialist.rs
use std::sync::Arc;
use uuid::Uuid;
use crate::backend::ChatBackend;
use crate::types::*;

pub struct Specialist {
    pub user_id: UserId,
    pub address: CompositionAddress,
    pub room_id: RoomId,
    backend: Arc<dyn ChatBackend>,
}

impl Specialist {
    pub async fn provision(
        address: CompositionAddress,
        room_id: RoomId,
        backend: Arc<dyn ChatBackend>,
    ) -> crate::error::Result<Self> {
        let user_id = backend.register_agent(&address).await?;
        backend.invite(&room_id, &user_id).await?;
        Ok(Self { user_id, address, room_id, backend })
    }

    pub async fn deprovision(self) -> crate::error::Result<()> {
        self.backend.kick(&self.room_id, &self.user_id, "session complete").await?;
        self.backend.deregister_agent(&self.user_id).await?;
        Ok(())
    }
}
```

- [ ] **Step 4: Implement agents/mod.rs**

```rust
// charradissa-core/src/agents/mod.rs
pub mod org;
pub mod project;
pub mod specialist;
```

- [ ] **Step 5: Build**

```bash
cargo build --package charradissa-core 2>&1
```

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat: add OrgAgent, ProjectAgent (with task dispatch), Specialist JIT agents"
```

---

### Task 8: CharradissaTransport

**Files:** `charradissa-core/src/transport.rs`

- [ ] **Step 1: Write failing tests**

```rust
// charradissa-core/tests/transport_tests.rs
// CharradissaTransport object-safety and trait-impl check
use charradissa_core::transport::CharradissaTransport;
use amassada_core::transport::Transport;

fn _assert_amassada_transport(_: &dyn Transport) {}

#[test]
fn charradissa_transport_implements_amassada_transport() {
    // Compile-only: if this compiles, CharradissaTransport satisfies the amassada Transport trait
}
```

- [ ] **Step 2: Implement transport.rs**

```rust
// charradissa-core/src/transport.rs
use async_trait::async_trait;
use std::sync::Arc;
use amassada_core::channels::consult::{ConsultRequest, ConsultResponse};
use amassada_core::error::Result as AmassadaResult;
use amassada_core::error::AmassadaError;
use amassada_core::transport::Transport;
use amassada_core::types::{AgentId, HumanInput, SessionEvent, SessionOutput, WhisperMsg};
use crate::backend::ChatBackend;
use crate::types::RoomId;

/// Implements the amassada_core Transport trait for Matrix rooms.
/// Translates Amassada session events into Matrix room operations.
pub struct CharradissaTransport {
    room_id: RoomId,
    backend: Arc<dyn ChatBackend>,
}

impl CharradissaTransport {
    pub fn new(room_id: RoomId, backend: Arc<dyn ChatBackend>) -> Self {
        Self { room_id, backend }
    }

    fn format_event(event: &SessionEvent) -> String {
        match event {
            SessionEvent::TurnCompleted { record } => {
                format!("**[{} / {}]**\n{}", record.agent_id, record.persona, record.content)
            }
            SessionEvent::RoundStarted { round } => {
                format!("--- Round {} ---", round)
            }
            SessionEvent::BtwEmitted { from, to, content } => {
                format!("*[btw from {} to {}]* {}", from, to, content)
            }
            SessionEvent::ApprovalRequested { reason } => {
                format!("⏸ **Approval requested**: {}", reason)
            }
            SessionEvent::ArtifactCompleted { title, .. } => {
                format!("✅ Artifact ready: **{}**", title)
            }
            SessionEvent::SessionCompleted => "🎉 Session complete.".into(),
            SessionEvent::ModeratorAction { action } => {
                format!("*[moderator: {}]*", action)
            }
            _ => format!("{:?}", event),
        }
    }
}

#[async_trait]
impl Transport for CharradissaTransport {
    async fn broadcast(&self, event: &SessionEvent) -> AmassadaResult<()> {
        let text = Self::format_event(event);
        self.backend.send_message(&self.room_id, &text).await
            .map_err(|e| AmassadaError::Transport(e.to_string()))
    }

    async fn consult(&self, req: &ConsultRequest) -> AmassadaResult<ConsultResponse> {
        // In Matrix mode: create a DM-style consultation via private API call
        // v0.1.0: stub that returns a placeholder
        Ok(ConsultResponse {
            from: req.target.clone(),
            content: "[consultation pending full Matrix ConsultRuntime]".into(),
        })
    }

    async fn whisper(&self, agent: &AgentId, msg: &WhisperMsg) -> AmassadaResult<()> {
        tracing::debug!("[whisper in room {} → {}] {}", self.room_id, agent, msg.content);
        Ok(())
    }

    async fn recv_human(&self) -> Option<HumanInput> {
        // In Matrix mode: polling channel populated by Matrix event handler
        // v0.1.0: stub (no blocking poll; human input drives through event_stream)
        None
    }

    async fn emit_output(&self, output: &SessionOutput) -> AmassadaResult<()> {
        let summary = format!("**Session Output** ({})\n\n{}",
            output.canvas_id,
            output.artifacts.iter()
                .map(|a| format!("### {}\n{}", a.title, a.content))
                .collect::<Vec<_>>()
                .join("\n\n")
        );
        self.backend.send_message(&self.room_id, &summary).await
            .map_err(|e| AmassadaError::Transport(e.to_string()))
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test --package charradissa-core transport 2>&1
```

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "feat: add CharradissaTransport implementing amassada_core::Transport for Matrix rooms"
```

---

### Task 9: MatrixBackend (Appservice API) + Stub JiraTaskManager

**Files:** `charradissa-matrix/src/`, `charradissa-jira/src/`

- [ ] **Step 1: Implement charradissa-matrix/src/client.rs**

```rust
// charradissa-matrix/src/client.rs
use reqwest::Client;
use serde::{Deserialize, Serialize};
use charradissa_core::error::{CharradissaError, Result};
use charradissa_core::types::{RoomId, SpaceId, UserId};

pub struct AppserviceClient {
    client: Client,
    homeserver: String,
    as_token: String,
    bot_user_id: String,
}

impl AppserviceClient {
    pub fn new(homeserver: String, as_token: String, bot_user_id: String) -> Self {
        Self { client: Client::new(), homeserver, as_token, bot_user_id }
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
        let user_id = format!("@{}:{}", local_part,
            self.homeserver.trim_start_matches("https://").trim_start_matches("http://"));
        Ok(UserId::new(&user_id))
    }
}
```

- [ ] **Step 2: Implement charradissa-matrix/src/backend.rs**

```rust
// charradissa-matrix/src/backend.rs
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::stream;
use std::sync::Arc;
use charradissa_core::backend::ChatBackend;
use charradissa_core::error::{CharradissaError, Result};
use charradissa_core::types::*;
use crate::client::AppserviceClient;

pub struct MatrixBackend {
    client: Arc<AppserviceClient>,
}

impl MatrixBackend {
    pub fn new(homeserver: String, as_token: String, bot_user_id: String) -> Self {
        Self { client: Arc::new(AppserviceClient::new(homeserver, as_token, bot_user_id)) }
    }
}

#[async_trait]
impl ChatBackend for MatrixBackend {
    async fn send_message(&self, room: &RoomId, content: &str) -> Result<()> {
        self.client.send_message(room, content).await
    }

    async fn send_dm(&self, user: &UserId, content: &str) -> Result<()> {
        tracing::debug!("DM to {}: {}", user, content);
        Ok(())  // Create DM room and send — stub for v0.1.0
    }

    async fn create_room(&self, opts: &RoomOptions) -> Result<RoomId> {
        self.client.create_room(&opts.alias, &opts.name).await
    }

    async fn create_space(&self, name: &str) -> Result<SpaceId> {
        Ok(SpaceId::new(&format!("!space-{}:homeserver", name)))  // stub
    }

    async fn add_to_space(&self, space: &SpaceId, room: &RoomId) -> Result<()> {
        tracing::debug!("add {} to space {}", room, space.as_str());
        Ok(())  // Matrix space child event — stub for v0.1.0
    }

    async fn invite(&self, room: &RoomId, user: &UserId) -> Result<()> {
        self.client.invite(room, user).await
    }

    async fn kick(&self, room: &RoomId, user: &UserId, reason: &str) -> Result<()> {
        tracing::info!("kick {} from {} ({})", user, room, reason);
        Ok(())  // Appservice kick endpoint — stub for v0.1.0
    }

    async fn register_agent(&self, address: &CompositionAddress) -> Result<UserId> {
        let local_part = format!("charradissa-{}", uuid::Uuid::new_v4());
        self.client.register_agent(&local_part).await
    }

    async fn deregister_agent(&self, user: &UserId) -> Result<()> {
        tracing::info!("deregister agent: {}", user);
        Ok(())
    }

    async fn room_history(&self, room: &RoomId, since: DateTime<Utc>) -> Result<Vec<ChatEvent>> {
        Ok(vec![])  // Matrix /messages endpoint — stub for v0.1.0
    }

    async fn delete_room(&self, room: &RoomId) -> Result<()> {
        tracing::info!("delete room: {}", room);
        Ok(())
    }
}
```

- [ ] **Step 3: Implement charradissa-matrix/src/appservice.rs**

```rust
// charradissa-matrix/src/appservice.rs
use axum::{extract::{Path, State}, http::StatusCode, Json};
use charradissa_core::types::{ChatEvent, ChatEventKind, RoomId, UserId};
use chrono::Utc;
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppserviceState {
    pub hs_token: String,           // homeserver→appservice auth token
}

/// Matrix Appservice PUT /_matrix/app/v1/transactions/{txnId}
pub async fn handle_transaction(
    State(state): State<AppserviceState>,
    Path(txn_id): Path<String>,
    Json(body): Json<Value>,
) -> StatusCode {
    let events = body["events"].as_array().cloned().unwrap_or_default();
    for event in events {
        if let Some(event) = parse_matrix_event(&event) {
            tracing::info!("appservice event from {}: {}", event.sender, event.content);
            // In full impl: route to ProjectAgent or OrgAgent via AgentRegistry
        }
    }
    StatusCode::OK
}

fn parse_matrix_event(event: &Value) -> Option<ChatEvent> {
    let event_id = event["event_id"].as_str()?.to_string();
    let room_id = RoomId::new(event["room_id"].as_str()?);
    let sender = UserId::new(event["sender"].as_str()?);
    let content_body = event["content"]["body"].as_str().unwrap_or("").to_string();
    let ts = event["origin_server_ts"].as_u64().unwrap_or(0);

    let kind = if content_body.starts_with('/') {
        let parts: Vec<&str> = content_body.splitn(2, ' ').collect();
        ChatEventKind::SlashCommand {
            command: parts[0][1..].to_string(),
            args: parts.get(1).copied().unwrap_or("").to_string(),
        }
    } else {
        ChatEventKind::Message
    };

    Some(ChatEvent {
        event_id,
        room_id,
        sender,
        content: content_body,
        timestamp: Utc::now(),
        kind,
    })
}
```

- [ ] **Step 4: Implement JiraTaskManager stub**

```rust
// charradissa-jira/src/backend.rs
use async_trait::async_trait;
use charradissa_core::error::{CharradissaError, Result};
use charradissa_core::task::TaskManager;
use charradissa_core::types::*;

pub struct JiraTaskManager {
    client: reqwest::Client,
    base_url: String,
    project_key: String,
    api_token: String,
    email: String,
}

impl JiraTaskManager {
    pub fn new(base_url: String, project_key: String, api_token: String, email: String) -> Self {
        Self { client: reqwest::Client::new(), base_url, project_key, api_token, email }
    }

    fn auth(&self) -> String {
        use base64::Engine;
        let creds = format!("{}:{}", self.email, self.api_token);
        format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(creds.as_bytes()))
    }
}

#[async_trait]
impl TaskManager for JiraTaskManager {
    async fn create_task(&self, project: &ProjectId, opts: &TaskOptions) -> Result<TaskId> {
        let url = format!("{}/rest/api/3/issue", self.base_url);
        let body = serde_json::json!({
            "fields": {
                "project": { "key": self.project_key },
                "summary": opts.title,
                "description": {
                    "type": "doc",
                    "version": 1,
                    "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": opts.description }] }]
                },
                "issuetype": { "name": "Task" }
            }
        });
        let resp = self.client.post(&url)
            .header("Authorization", self.auth())
            .header("Content-Type", "application/json")
            .json(&body)
            .send().await
            .map_err(|e| CharradissaError::Tool(e.to_string()))?;
        let json: serde_json::Value = resp.json().await
            .map_err(|e| CharradissaError::Tool(e.to_string()))?;
        let id = json["id"].as_str()
            .ok_or_else(|| CharradissaError::Tool("no id in Jira response".into()))?;
        Ok(TaskId::new(id))
    }

    async fn assign_task(&self, task: &TaskId, assignee: &Assignee) -> Result<()> {
        tracing::info!("assign task {} to {:?}", task.as_str(), assignee);
        Ok(())
    }

    async fn update_status(&self, task: &TaskId, status: TaskStatus) -> Result<()> {
        tracing::info!("update task {} → {:?}", task.as_str(), status);
        Ok(())  // Jira transition endpoint — stub for v0.1.0
    }

    async fn get_task(&self, task: &TaskId) -> Result<Task> {
        Err(CharradissaError::Tool("get_task: not implemented".into()))
    }

    async fn list_open(&self, project: &ProjectId) -> Result<Vec<Task>> {
        Ok(vec![]) // Jira JQL query — stub for v0.1.0
    }
}
```

- [ ] **Step 5: Build workspace**

```bash
cargo build --workspace 2>&1
```
Expected: all crates build (JiraTaskManager requires `base64` dep — add `base64 = "0.22"` to jira Cargo.toml if not available)

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat: add MatrixBackend (Appservice API), appservice webhook handler, JiraTaskManager stub"
```

---

### Task 10: charradissa-daemon Binary

**Files:** `charradissa-daemon/src/main.rs`, `charradissa-daemon/src/registry.rs`

- [ ] **Step 1: Implement registry.rs**

```rust
// charradissa-daemon/src/registry.rs
use std::collections::HashMap;
use charradissa_core::types::{ProjectId, UserId};

pub struct AgentRegistry {
    project_agents: HashMap<ProjectId, UserId>,
    org_agent: Option<UserId>,
    concierge: Option<UserId>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self { project_agents: HashMap::new(), org_agent: None, concierge: None }
    }

    pub fn register_project_agent(&mut self, project: ProjectId, user_id: UserId) {
        self.project_agents.insert(project, user_id);
    }

    pub fn register_org_agent(&mut self, user_id: UserId) {
        self.org_agent = Some(user_id);
    }

    pub fn register_concierge(&mut self, user_id: UserId) {
        self.concierge = Some(user_id);
    }

    pub fn project_agent(&self, project: &ProjectId) -> Option<&UserId> {
        self.project_agents.get(project)
    }
}
```

- [ ] **Step 2: Implement main.rs**

```rust
// charradissa-daemon/src/main.rs
mod registry;

use std::sync::Arc;
use charradissa_core::config::Config;
use charradissa_matrix::backend::MatrixBackend;
use charradissa_matrix::appservice::AppserviceState;
use axum::{routing::put, Router};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config_path = std::env::var("CHARRADISSA_CONFIG")
        .unwrap_or("charradissa.toml".into());
    let config = Config::from_file(&config_path)
        .map_err(|e| anyhow::anyhow!("config error: {}", e))?;

    let as_token = std::env::var("MATRIX_AS_TOKEN")
        .unwrap_or_else(|_| "dev-token".into());
    let bot_user_id = format!("@charradissa:{}", 
        config.org.homeserver.trim_start_matches("https://").trim_start_matches("http://"));

    let backend = Arc::new(MatrixBackend::new(
        config.org.homeserver.clone(),
        as_token.clone(),
        bot_user_id,
    ));

    let mut registry = registry::AgentRegistry::new();
    tracing::info!("charradissa-daemon starting for org: {}", config.org.name);

    // Appservice webhook server (Matrix sends events here)
    let appservice_port = std::env::var("CHARRADISSA_PORT").unwrap_or("8448".into());
    let appservice_state = AppserviceState { hs_token: as_token };

    let app = Router::new()
        .route("/_matrix/app/v1/transactions/:txnId",
            put(charradissa_matrix::appservice::handle_transaction))
        .with_state(appservice_state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", appservice_port)).await?;
    tracing::info!("charradissa-daemon webhook listening on :{}", appservice_port);
    axum::serve(listener, app).await?;
    Ok(())
}
```

- [ ] **Step 3: Build and verify**

```bash
cargo build --workspace 2>&1
```
Expected: builds successfully

- [ ] **Step 4: Final commit**

```bash
git add -A && git commit -m "feat: add charradissa-daemon binary with Matrix Appservice webhook — charradissa v0.1.0 complete"
```

---

## Self-Review

**Spec coverage:**
- ✅ ChatBackend trait — all methods defined (Task 3)
- ✅ TaskManager trait + TaskOptions + Assignee + CompositionAddress (Tasks 2, 3)
- ✅ FargaWriter trait + HttpFargaWriter client (Task 3)
- ✅ OrgAgent, ProjectAgent, Specialist (JIT) — all three tiers (Task 7)
- ✅ ApprovalQueue — oneshot-based /approve /reject flow (Task 4)
- ✅ Tool loop — slash command parser, approval gate logic (Task 5)
- ✅ ConciergeAgent — archival job + convergence sweep + signal extraction (Task 6)
- ✅ MatrixBackend — Appservice API client, room operations (Task 9)
- ✅ Appservice webhook handler (parse_matrix_event, PUT /_matrix/app/v1/transactions/:txnId) (Task 9)
- ✅ CharradissaTransport — implements amassada_core::Transport for Matrix rooms (Task 8)
- ✅ JiraTaskManager — create_task with Jira REST API (Task 9)
- ✅ Config (charradissa.toml) with all sections (Task 1)
- ✅ charradissa-daemon binary, AgentRegistry, webhook server wired (Task 10)
- ⚠ Full tool loop dispatch (Anthropic API call + tool_use parsing) — ToolLoopConfig defined, dispatch stub; v0.2.0
- ⚠ MatrixBackend: send_dm, room_history, delete_room, kick, add_to_space — HTTP stubs; v0.2.0
- ⚠ CharradissaTransport::consult and recv_human — stubs; full Matrix-backed consult in v0.2.0
- ⚠ ConciergeAgent LLM convergence call — heuristic keyword extraction in v0.1.0, LLM in v0.2.0
- ⚠ charradissa-irc — out of scope v1 per spec
- ⚠ Cor MCP tools (post/ask/fix/health) — live in Cor as MCP, call charradissa HTTP; not in this repo
