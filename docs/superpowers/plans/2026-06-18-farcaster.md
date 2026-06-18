# Farcaster Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement FarcasterAgent — a SystemAgent that observes Amassada MilestoneEvents across all active projects, whispers cross-space insights to project agents, and periodically submits collective lessons to Farga.

**Architecture:** `FarcasterAgent` implements the new `SystemAgent` trait and is registered with `ConciergeAgent`. It has two paths: a reactive path (`on_milestone`) that uses Haiku to detect cross-project connections and DM project agents, and a digest path (`tick`) that uses Opus to synthesize accumulated observations into a `#farcaster` broadcast and a Farga contribution.

**Tech Stack:** Rust, `tokio` (async runtime, Mutex, AtomicU32), `async-trait`, `serde_json`, `reqwest` (Anthropic API calls), `charradissa-core`.

---

## File Structure

**Create:**
- `charradissa-core/src/farcaster/mod.rs` — module declarations + re-exports
- `charradissa-core/src/farcaster/milestone.rs` — `MilestoneEvent` enum + helpers
- `charradissa-core/src/farcaster/concurrence.rs` — `AgentConcurrence`, `ConcurrenceType`, `Urgency`
- `charradissa-core/src/farcaster/analyzer.rs` — `FarcasterAnalyzer` trait + data types
- `charradissa-core/src/farcaster/system_agent.rs` — `SystemAgent` trait
- `charradissa-core/src/farcaster/agent.rs` — `FarcasterAgent` struct, inherent methods, `SystemAgent` impl
- `charradissa-core/src/farcaster/claude_analyzer.rs` — `ClaudeFarcasterAnalyzer` (real LLM)
- `charradissa-core/tests/farcaster_tests.rs` — all FarcasterAgent tests

**Modify:**
- `charradissa-core/src/lib.rs` — add `pub mod farcaster`
- `charradissa-core/src/concierge.rs` — add `system_agents`, `register_system_agent`, `dispatch_milestone`, `run_system_agent_ticks`
- `charradissa-core/tests/concierge_tests.rs` — add `dispatch_milestone` fan-out test
- `charradissa-core/tests/trait_tests.rs` — add `SystemAgent` + `FarcasterAnalyzer` object-safety checks
- `charradissa-daemon/src/main.rs` — wire `FarcasterAgent` with milestone broadcast channel

---

## Task 1: MilestoneEvent enum + module scaffold

**Files:**
- Create: `charradissa-core/src/farcaster/mod.rs`
- Create: `charradissa-core/src/farcaster/milestone.rs`
- Modify: `charradissa-core/src/lib.rs`

- [ ] **Step 1: Write a failing test for MilestoneEvent helpers**

Create `charradissa-core/tests/farcaster_tests.rs`:
```rust
use charradissa_core::farcaster::milestone::MilestoneEvent;
use charradissa_core::types::ProjectId;

fn artifact_event() -> MilestoneEvent {
    MilestoneEvent::ArtifactProduced {
        mission_id: "m1".into(),
        session_id: "s1".into(),
        project_id: ProjectId::new("alpha"),
        canvas_id: "c1".into(),
        artifact_summary: "auth schema finalized".into(),
        sub_objective_ids: vec!["obj-1".into()],
    }
}

#[test]
fn milestone_project_id_returns_correct_project() {
    let ev = artifact_event();
    assert_eq!(ev.project_id(), &ProjectId::new("alpha"));
}

#[test]
fn milestone_summary_contains_canvas_and_summary() {
    let ev = artifact_event();
    let s = ev.summary();
    assert!(s.contains("c1"), "expected canvas id in summary: {}", s);
    assert!(s.contains("auth schema"), "expected artifact summary: {}", s);
}

#[test]
fn replan_event_summary_contains_count() {
    let ev = MilestoneEvent::ReplanTriggered {
        mission_id: "m1".into(),
        project_id: ProjectId::new("beta"),
        sub_objective_id: "obj-2".into(),
        reason: "test failed".into(),
        replan_count: 3,
    };
    assert!(ev.summary().contains("3"));
}
```

- [ ] **Step 2: Run test — verify it fails with "module not found"**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core --test farcaster_tests 2>&1 | head -20
```
Expected: compile error mentioning `farcaster` not found.

- [ ] **Step 3: Create the farcaster module scaffold**

Create `charradissa-core/src/farcaster/mod.rs`:
```rust
pub mod milestone;
pub mod concurrence;
pub mod analyzer;
pub mod system_agent;
pub mod agent;
pub mod claude_analyzer;

pub use milestone::MilestoneEvent;
pub use concurrence::{AgentConcurrence, ConcurrenceType, Urgency};
pub use analyzer::{
    FarcasterAnalyzer, CrossSpaceSnapshot, ProjectSnapshot,
    CrossSpaceConnection, DigestSynthesis, DigestEntry, DigestPayload,
};
pub use system_agent::SystemAgent;
pub use agent::FarcasterAgent;
pub use claude_analyzer::ClaudeFarcasterAnalyzer;
```

For now, stub out the other sub-modules as empty files:
```bash
touch charradissa-core/src/farcaster/concurrence.rs \
      charradissa-core/src/farcaster/analyzer.rs \
      charradissa-core/src/farcaster/system_agent.rs \
      charradissa-core/src/farcaster/agent.rs \
      charradissa-core/src/farcaster/claude_analyzer.rs
```

- [ ] **Step 4: Implement MilestoneEvent**

Create `charradissa-core/src/farcaster/milestone.rs`:
```rust
use serde::{Deserialize, Serialize};
use crate::types::ProjectId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MilestoneEvent {
    ArtifactProduced {
        mission_id: String,
        session_id: String,
        project_id: ProjectId,
        canvas_id: String,
        artifact_summary: String,
        sub_objective_ids: Vec<String>,
    },
    EvaluationCompleted {
        mission_id: String,
        project_id: ProjectId,
        sub_objective_id: String,
        satisfied: bool,
        reason: String,
    },
    ReplanTriggered {
        mission_id: String,
        project_id: ProjectId,
        sub_objective_id: String,
        reason: String,
        replan_count: u32,
    },
    MissionCompleted {
        mission_id: String,
        project_id: ProjectId,
        goal: String,
        completed_sub_objectives: Vec<String>,
        verdict: String,
    },
}

impl MilestoneEvent {
    pub fn project_id(&self) -> &ProjectId {
        match self {
            Self::ArtifactProduced { project_id, .. } => project_id,
            Self::EvaluationCompleted { project_id, .. } => project_id,
            Self::ReplanTriggered { project_id, .. } => project_id,
            Self::MissionCompleted { project_id, .. } => project_id,
        }
    }

    pub fn summary(&self) -> String {
        match self {
            Self::ArtifactProduced { canvas_id, artifact_summary, .. } =>
                format!("produced artifact on canvas {}: {}", canvas_id, artifact_summary),
            Self::EvaluationCompleted { sub_objective_id, satisfied, reason, .. } =>
                format!("evaluation for {} {}: {}",
                    sub_objective_id,
                    if *satisfied { "succeeded" } else { "failed" },
                    reason),
            Self::ReplanTriggered { sub_objective_id, reason, replan_count, .. } =>
                format!("replan #{} for {}: {}", replan_count, sub_objective_id, reason),
            Self::MissionCompleted { goal, verdict, .. } =>
                format!("mission '{}' completed ({})", goal, verdict),
        }
    }
}
```

- [ ] **Step 5: Add `pub mod farcaster` to lib.rs**

Append to `charradissa-core/src/lib.rs`:
```rust
pub mod farcaster;
```

The file should now end with:
```rust
pub mod concierge;
pub mod agents;
pub mod transport;
pub mod farcaster;
```

- [ ] **Step 6: Run tests — verify they pass**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core --test farcaster_tests 2>&1 | tail -20
```
Expected: 3 tests pass. (Other submodule stubs may cause compile errors — if so, stub them with `// placeholder` until the following tasks fill them in. Temporarily remove the `pub use` re-exports for unimplemented types in mod.rs.)

- [ ] **Step 7: Commit**

```bash
git add charradissa-core/src/lib.rs \
        charradissa-core/src/farcaster/ \
        charradissa-core/tests/farcaster_tests.rs
git commit -m "feat: add MilestoneEvent enum and farcaster module scaffold"
```

---

## Task 2: AgentConcurrence, ConcurrenceType, Urgency

**Files:**
- Create: `charradissa-core/src/farcaster/concurrence.rs`
- Modify: `charradissa-core/tests/farcaster_tests.rs`

- [ ] **Step 1: Write a failing test**

Add to `charradissa-core/tests/farcaster_tests.rs`:
```rust
use charradissa_core::farcaster::concurrence::{AgentConcurrence, ConcurrenceType, Urgency};

#[test]
fn urgency_derives_eq_and_debug() {
    assert_eq!(Urgency::High, Urgency::High);
    assert_ne!(Urgency::Low, Urgency::High);
}

#[test]
fn agent_concurrence_round_trips_json() {
    let c = AgentConcurrence {
        project_id: "alpha".into(),
        agent_address: "alpha/dev+builder".into(),
        concurrence_type: ConcurrenceType::Whispered,
        note: Some("confirmed via DM".into()),
    };
    let json = serde_json::to_string(&c).unwrap();
    let back: AgentConcurrence = serde_json::from_str(&json).unwrap();
    assert_eq!(back.project_id, "alpha");
    assert_eq!(back.concurrence_type, ConcurrenceType::Whispered);
}
```

- [ ] **Step 2: Run test — verify it fails with "module not found"**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core --test farcaster_tests concurrence 2>&1 | head -20
```
Expected: compile error.

- [ ] **Step 3: Implement concurrence.rs**

Write `charradissa-core/src/farcaster/concurrence.rs`:
```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Urgency {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConcurrenceType {
    Observed,
    Whispered,
    Acknowledged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConcurrence {
    pub project_id: String,
    pub agent_address: String,
    pub concurrence_type: ConcurrenceType,
    pub note: Option<String>,
}
```

- [ ] **Step 4: Run tests — verify they pass**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core --test farcaster_tests 2>&1 | tail -10
```
Expected: all farcaster_tests pass.

- [ ] **Step 5: Commit**

```bash
git add charradissa-core/src/farcaster/concurrence.rs charradissa-core/tests/farcaster_tests.rs
git commit -m "feat: add AgentConcurrence, ConcurrenceType, Urgency types"
```

---

## Task 3: FarcasterAnalyzer trait + data types

**Files:**
- Create: `charradissa-core/src/farcaster/analyzer.rs`
- Modify: `charradissa-core/tests/trait_tests.rs`

- [ ] **Step 1: Write a failing object-safety test**

Add to `charradissa-core/tests/trait_tests.rs`:
```rust
use charradissa_core::farcaster::analyzer::FarcasterAnalyzer;

fn _assert_farcaster_analyzer_object_safe(_: &dyn FarcasterAnalyzer) {}

#[test]
fn farcaster_analyzer_is_object_safe() {}
```

- [ ] **Step 2: Run test — verify compile error**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core --test trait_tests 2>&1 | head -20
```
Expected: compile error about `FarcasterAnalyzer` not found.

- [ ] **Step 3: Implement analyzer.rs**

Write `charradissa-core/src/farcaster/analyzer.rs`:
```rust
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::error::Result;
use crate::types::ProjectId;
use super::concurrence::{AgentConcurrence, Urgency};
use super::milestone::MilestoneEvent;

#[derive(Debug, Clone)]
pub struct ProjectSnapshot {
    pub project_id: ProjectId,
    pub mission_goal: Option<String>,
    pub open_sub_objectives: Vec<String>,
    pub recent_events: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CrossSpaceSnapshot {
    pub projects: Vec<ProjectSnapshot>,
}

#[derive(Debug, Clone)]
pub struct CrossSpaceConnection {
    pub from_project: ProjectId,
    pub to_project: ProjectId,
    pub connection_type: String,
    pub summary: String,
    pub urgency: Urgency,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigestSynthesis {
    pub connections: Vec<String>,
    pub lessons: Vec<String>,
    pub open_questions: Vec<String>,
    pub farga_verdict: String,
    pub farga_title: Option<String>,
    pub farga_narrative: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DigestEntry {
    pub project_id: ProjectId,
    pub connection_summary: String,
    pub involved_projects: Vec<ProjectId>,
    pub concurrence: Vec<AgentConcurrence>,
    pub urgency: Urgency,
    pub whispered_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigestPayload {
    pub title: String,
    pub narrative: String,
    pub lessons: Vec<String>,
    pub open_questions: Vec<String>,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub projects_observed: Vec<ProjectId>,
    pub concurrence: Vec<AgentConcurrence>,
}

#[async_trait]
pub trait FarcasterAnalyzer: Send + Sync {
    async fn analyze_cross_space(
        &self,
        triggering_event: &MilestoneEvent,
        snapshot: &CrossSpaceSnapshot,
    ) -> Result<(Vec<CrossSpaceConnection>, u32)>;

    async fn synthesize_digest(
        &self,
        entries: &[DigestEntry],
    ) -> Result<(DigestSynthesis, u32)>;
}
```

- [ ] **Step 4: Run tests — verify they pass**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core --test trait_tests 2>&1 | tail -10
```
Expected: all pass including new `farcaster_analyzer_is_object_safe`.

- [ ] **Step 5: Commit**

```bash
git add charradissa-core/src/farcaster/analyzer.rs charradissa-core/tests/trait_tests.rs
git commit -m "feat: add FarcasterAnalyzer trait and associated data types"
```

---

## Task 4: SystemAgent trait

**Files:**
- Create: `charradissa-core/src/farcaster/system_agent.rs`
- Modify: `charradissa-core/tests/trait_tests.rs`

- [ ] **Step 1: Write a failing object-safety test**

Add to `charradissa-core/tests/trait_tests.rs`:
```rust
use charradissa_core::farcaster::system_agent::SystemAgent;

fn _assert_system_agent_object_safe(_: &dyn SystemAgent) {}

#[test]
fn system_agent_is_object_safe() {}
```

- [ ] **Step 2: Run test — verify compile error**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core --test trait_tests 2>&1 | head -20
```

- [ ] **Step 3: Implement system_agent.rs**

Write `charradissa-core/src/farcaster/system_agent.rs`:
```rust
use async_trait::async_trait;
use crate::error::Result;
use super::milestone::MilestoneEvent;

#[async_trait]
pub trait SystemAgent: Send + Sync {
    fn name(&self) -> &str;
    async fn on_milestone(&self, event: &MilestoneEvent) -> Result<()>;
    async fn tick(&self) -> Result<()>;
}
```

- [ ] **Step 4: Run tests — verify they pass**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core --test trait_tests 2>&1 | tail -10
```
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add charradissa-core/src/farcaster/system_agent.rs charradissa-core/tests/trait_tests.rs
git commit -m "feat: add SystemAgent trait"
```

---

## Task 5: ConciergeAgent extension + dispatch test

**Files:**
- Modify: `charradissa-core/src/concierge.rs`
- Modify: `charradissa-core/tests/concierge_tests.rs`

- [ ] **Step 1: Write a failing test for dispatch_milestone fan-out**

Add to `charradissa-core/tests/concierge_tests.rs`:
```rust
use std::sync::Arc;
use async_trait::async_trait;
use charradissa_core::concierge::ConciergeAgent;
use charradissa_core::farcaster::milestone::MilestoneEvent;
use charradissa_core::farcaster::system_agent::SystemAgent;
use charradissa_core::types::ProjectId;
use charradissa_core::error::Result;

struct RecordingAgent {
    name_str: String,
    calls: Arc<tokio::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl SystemAgent for RecordingAgent {
    fn name(&self) -> &str { &self.name_str }
    async fn on_milestone(&self, event: &MilestoneEvent) -> Result<()> {
        self.calls.lock().await.push(event.summary());
        Ok(())
    }
    async fn tick(&self) -> Result<()> { Ok(()) }
}

#[tokio::test]
async fn dispatch_milestone_fans_out_to_all_agents() {
    use charradissa_core::backend::ChatBackend;
    use charradissa_core::farga::FargaWriter;

    // Use a stub backend and farga writer (no-op implementations).
    struct StubBackend;
    struct StubFarga;

    #[async_trait]
    impl ChatBackend for StubBackend {
        async fn send_message(&self, _: &charradissa_core::types::RoomId, _: &str) -> Result<()> { Ok(()) }
        async fn send_dm(&self, _: &charradissa_core::types::UserId, _: &str) -> Result<()> { Ok(()) }
        async fn create_room(&self, _: &charradissa_core::types::RoomOptions) -> Result<charradissa_core::types::RoomId> { Ok(charradissa_core::types::RoomId::new("!r:t")) }
        async fn create_space(&self, _: &str) -> Result<charradissa_core::types::SpaceId> { Ok(charradissa_core::types::SpaceId::new("!s:t")) }
        async fn add_to_space(&self, _: &charradissa_core::types::SpaceId, _: &charradissa_core::types::RoomId) -> Result<()> { Ok(()) }
        async fn invite(&self, _: &charradissa_core::types::RoomId, _: &charradissa_core::types::UserId) -> Result<()> { Ok(()) }
        async fn kick(&self, _: &charradissa_core::types::RoomId, _: &charradissa_core::types::UserId, _: &str) -> Result<()> { Ok(()) }
        async fn register_agent(&self, _: &charradissa_core::types::CompositionAddress) -> Result<charradissa_core::types::UserId> { Ok(charradissa_core::types::UserId::new("@x:t")) }
        async fn deregister_agent(&self, _: &charradissa_core::types::UserId) -> Result<()> { Ok(()) }
        async fn room_history(&self, _: &charradissa_core::types::RoomId, _: chrono::DateTime<chrono::Utc>) -> Result<Vec<charradissa_core::types::ChatEvent>> { Ok(vec![]) }
        async fn delete_room(&self, _: &charradissa_core::types::RoomId) -> Result<()> { Ok(()) }
    }

    #[async_trait]
    impl FargaWriter for StubFarga {
        async fn write_signals(&self, _: &charradissa_core::types::ProjectId, _: Vec<charradissa_core::farga::Signal>) -> Result<()> { Ok(()) }
        async fn recent_signals(&self, _: &charradissa_core::types::ProjectId, _: chrono::Duration) -> Result<Vec<charradissa_core::farga::Signal>> { Ok(vec![]) }
    }

    let calls_a = Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
    let calls_b = Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));

    let mut concierge = ConciergeAgent::new(
        Arc::new(StubBackend),
        Arc::new(StubFarga),
        vec![],
        std::collections::HashMap::new(),
        24, 6, 10_000,
    );

    concierge.register_system_agent(
        Box::new(RecordingAgent { name_str: "a".into(), calls: Arc::clone(&calls_a) }),
        std::time::Duration::from_secs(3600),
    );
    concierge.register_system_agent(
        Box::new(RecordingAgent { name_str: "b".into(), calls: Arc::clone(&calls_b) }),
        std::time::Duration::from_secs(3600),
    );

    let event = MilestoneEvent::MissionCompleted {
        mission_id: "m1".into(),
        project_id: ProjectId::new("alpha"),
        goal: "ship auth".into(),
        completed_sub_objectives: vec![],
        verdict: "submit".into(),
    };

    concierge.dispatch_milestone(&event).await;

    assert_eq!(calls_a.lock().await.len(), 1);
    assert_eq!(calls_b.lock().await.len(), 1);
    assert!(calls_a.lock().await[0].contains("ship auth"));
}
```

- [ ] **Step 2: Run test — verify it fails**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core --test concierge_tests dispatch_milestone 2>&1 | head -30
```
Expected: compile error — `register_system_agent` and `dispatch_milestone` not found.

- [ ] **Step 3: Update ConciergeAgent struct and impl**

Replace the contents of `charradissa-core/src/concierge.rs` with:
```rust
use std::sync::Arc;
use std::time::Duration;
use chrono::Utc;
use tokio::time::interval;
use crate::backend::ChatBackend;
use crate::farga::{FargaWriter, Signal};
use crate::farcaster::milestone::MilestoneEvent;
use crate::farcaster::system_agent::SystemAgent;
use crate::types::{ChatEvent, ProjectId, RoomId, UserId};

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
    system_agents: Vec<Box<dyn SystemAgent>>,
    system_agent_tick_intervals: Vec<Duration>,
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
        Self {
            backend, farga, projects, project_agent_ids,
            archival_interval_hours, convergence_interval_hours, daily_token_budget,
            system_agents: Vec::new(),
            system_agent_tick_intervals: Vec::new(),
        }
    }

    pub fn register_system_agent(&mut self, agent: Box<dyn SystemAgent>, tick_interval: Duration) {
        self.system_agents.push(agent);
        self.system_agent_tick_intervals.push(tick_interval);
    }

    pub async fn dispatch_milestone(&self, event: &MilestoneEvent) {
        for agent in &self.system_agents {
            if let Err(e) = agent.on_milestone(event).await {
                tracing::error!("[{}] on_milestone error: {}", agent.name(), e);
            }
        }
    }

    pub async fn run_system_agent_ticks(&self) {
        let mut ticker = interval(Duration::from_secs(60));
        let mut next_tick: Vec<tokio::time::Instant> = self.system_agent_tick_intervals
            .iter()
            .map(|d| tokio::time::Instant::now() + *d)
            .collect();
        loop {
            ticker.tick().await;
            let now = tokio::time::Instant::now();
            for (i, agent) in self.system_agents.iter().enumerate() {
                if now >= next_tick[i] {
                    if let Err(e) = agent.tick().await {
                        tracing::error!("[{}] tick error: {}", agent.name(), e);
                    }
                    next_tick[i] = now + self.system_agent_tick_intervals[i];
                }
            }
        }
    }

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
                    Ok(_) => {}
                    Err(e) => tracing::error!("concierge: room_history failed for {}: {}", project, e),
                }
            }
        }
    }

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
            tracing::info!("concierge convergence sweep: {} signals across {} projects",
                all_signals.len(), self.projects.len());
        }
    }
}
```

- [ ] **Step 4: Run all concierge tests**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core --test concierge_tests 2>&1 | tail -15
```
Expected: both `extracts_signals_from_events` and `dispatch_milestone_fans_out_to_all_agents` pass.

- [ ] **Step 5: Commit**

```bash
git add charradissa-core/src/concierge.rs charradissa-core/tests/concierge_tests.rs
git commit -m "feat: add SystemAgent registration and dispatch_milestone to ConciergeAgent"
```

---

## Task 6: FarcasterAgent struct + new()

**Files:**
- Create: `charradissa-core/src/farcaster/agent.rs` (partial — struct + new())
- Modify: `charradissa-core/tests/farcaster_tests.rs`

- [ ] **Step 1: Write a failing test for FarcasterAgent construction**

Add to `charradissa-core/tests/farcaster_tests.rs`:
```rust
// --- Shared test helpers (define once, used in Tasks 6-9) ---

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use async_trait::async_trait;
use charradissa_core::backend::ChatBackend;
use charradissa_core::error::{CharradissaError, Result};
use charradissa_core::farga::{FargaWriter, Signal};
use charradissa_core::farcaster::analyzer::{
    CrossSpaceConnection, CrossSpaceSnapshot, DigestEntry, DigestSynthesis, FarcasterAnalyzer,
};
use charradissa_core::farcaster::concurrence::Urgency;
use charradissa_core::farcaster::milestone::MilestoneEvent;
use charradissa_core::farcaster::FarcasterAgent;
use charradissa_core::types::{
    ChatEvent, CompositionAddress, ProjectId, RoomId, RoomOptions, SpaceId, UserId,
};

struct MockChatBackend {
    dms: Arc<tokio::sync::Mutex<Vec<(UserId, String)>>>,
    messages: Arc<tokio::sync::Mutex<Vec<(RoomId, String)>>>,
}

impl MockChatBackend {
    fn new() -> (Self, Arc<tokio::sync::Mutex<Vec<(UserId, String)>>>, Arc<tokio::sync::Mutex<Vec<(RoomId, String)>>>) {
        let dms = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let messages = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        (Self { dms: Arc::clone(&dms), messages: Arc::clone(&messages) }, dms, messages)
    }
}

#[async_trait]
impl ChatBackend for MockChatBackend {
    async fn send_message(&self, room: &RoomId, content: &str) -> Result<()> {
        self.messages.lock().await.push((room.clone(), content.to_string()));
        Ok(())
    }
    async fn send_dm(&self, user: &UserId, content: &str) -> Result<()> {
        self.dms.lock().await.push((user.clone(), content.to_string()));
        Ok(())
    }
    async fn create_room(&self, _: &RoomOptions) -> Result<RoomId> { Ok(RoomId::new("!r:t")) }
    async fn create_space(&self, _: &str) -> Result<SpaceId> { Ok(SpaceId::new("!s:t")) }
    async fn add_to_space(&self, _: &SpaceId, _: &RoomId) -> Result<()> { Ok(()) }
    async fn invite(&self, _: &RoomId, _: &UserId) -> Result<()> { Ok(()) }
    async fn kick(&self, _: &RoomId, _: &UserId, _: &str) -> Result<()> { Ok(()) }
    async fn register_agent(&self, _: &CompositionAddress) -> Result<UserId> { Ok(UserId::new("@x:t")) }
    async fn deregister_agent(&self, _: &UserId) -> Result<()> { Ok(()) }
    async fn room_history(&self, _: &RoomId, _: chrono::DateTime<chrono::Utc>) -> Result<Vec<ChatEvent>> { Ok(vec![]) }
    async fn delete_room(&self, _: &RoomId) -> Result<()> { Ok(()) }
}

struct MockFargaWriter {
    calls: Arc<tokio::sync::Mutex<Vec<(ProjectId, Vec<Signal>)>>>,
}

impl MockFargaWriter {
    fn new() -> (Self, Arc<tokio::sync::Mutex<Vec<(ProjectId, Vec<Signal>)>>>) {
        let calls = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        (Self { calls: Arc::clone(&calls) }, calls)
    }
}

#[async_trait]
impl FargaWriter for MockFargaWriter {
    async fn write_signals(&self, project: &ProjectId, signals: Vec<Signal>) -> Result<()> {
        self.calls.lock().await.push((project.clone(), signals));
        Ok(())
    }
    async fn recent_signals(&self, _: &ProjectId, _: chrono::Duration) -> Result<Vec<Signal>> {
        Ok(vec![])
    }
}

struct MockFarcasterAnalyzer {
    reactive: std::sync::Mutex<VecDeque<(Vec<CrossSpaceConnection>, u32)>>,
    digest: std::sync::Mutex<VecDeque<(DigestSynthesis, u32)>>,
}

impl MockFarcasterAnalyzer {
    fn new() -> Self {
        Self {
            reactive: std::sync::Mutex::new(VecDeque::new()),
            digest: std::sync::Mutex::new(VecDeque::new()),
        }
    }
    fn queue_reactive(&self, connections: Vec<CrossSpaceConnection>, tokens: u32) {
        self.reactive.lock().unwrap().push_back((connections, tokens));
    }
    fn queue_digest(&self, synthesis: DigestSynthesis, tokens: u32) {
        self.digest.lock().unwrap().push_back((synthesis, tokens));
    }
}

#[async_trait]
impl FarcasterAnalyzer for MockFarcasterAnalyzer {
    async fn analyze_cross_space(
        &self,
        _event: &MilestoneEvent,
        _snapshot: &CrossSpaceSnapshot,
    ) -> Result<(Vec<CrossSpaceConnection>, u32)> {
        self.reactive.lock().unwrap().pop_front()
            .ok_or_else(|| CharradissaError::Dispatch("no queued reactive response".into()))
    }
    async fn synthesize_digest(&self, _entries: &[DigestEntry]) -> Result<(DigestSynthesis, u32)> {
        self.digest.lock().unwrap().pop_front()
            .ok_or_else(|| CharradissaError::Dispatch("no queued digest response".into()))
    }
}

fn make_agent(projects: Vec<ProjectId>, agent_ids: HashMap<ProjectId, UserId>)
    -> (FarcasterAgent,
        Arc<tokio::sync::Mutex<Vec<(UserId, String)>>>,
        Arc<tokio::sync::Mutex<Vec<(RoomId, String)>>>,
        Arc<tokio::sync::Mutex<Vec<(ProjectId, Vec<Signal>)>>>,
        Arc<MockFarcasterAnalyzer>)
{
    let (backend, dms, messages) = MockChatBackend::new();
    let (farga, farga_calls) = MockFargaWriter::new();
    let analyzer = Arc::new(MockFarcasterAnalyzer::new());
    let agent = FarcasterAgent::new(
        Arc::new(backend),
        Arc::new(farga),
        Arc::clone(&analyzer) as Arc<dyn FarcasterAnalyzer>,
        projects,
        agent_ids,
    );
    (agent, dms, messages, farga_calls, analyzer)
}

// --- Task 6 test ---
#[tokio::test]
async fn farcaster_agent_new_initializes_with_defaults() {
    let (agent, _, _, _, _) = make_agent(
        vec![ProjectId::new("alpha")],
        HashMap::new(),
    );
    // If this compiles and runs, construction is correct.
    // We verify budgets through budget_cap_test in Task 9.
    drop(agent);
}
```

- [ ] **Step 2: Run test — verify compile error**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core --test farcaster_tests farcaster_agent_new 2>&1 | head -30
```
Expected: compile error — `FarcasterAgent` not defined in `agent.rs` yet.

- [ ] **Step 3: Implement FarcasterAgent struct and new()**

Write `charradissa-core/src/farcaster/agent.rs`:
```rust
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::Mutex;
use crate::backend::ChatBackend;
use crate::error::Result;
use crate::farga::{FargaWriter, Signal};
use crate::types::{ProjectId, RoomId, UserId};
use super::analyzer::{
    CrossSpaceSnapshot, DigestEntry, DigestPayload, DigestSynthesis, FarcasterAnalyzer,
    ProjectSnapshot,
};
use super::concurrence::{AgentConcurrence, ConcurrenceType, Urgency};
use super::milestone::MilestoneEvent;
use super::system_agent::SystemAgent;

const EVENT_BUFFER_CAP: usize = 50;

pub struct FarcasterAgent {
    pub(crate) backend: Arc<dyn ChatBackend>,
    pub(crate) farga: Arc<dyn FargaWriter>,
    pub(crate) analyzer: Arc<dyn FarcasterAnalyzer>,
    pub(crate) projects: Vec<ProjectId>,
    pub(crate) project_agent_ids: HashMap<ProjectId, UserId>,
    pub(crate) event_buffer: Mutex<HashMap<ProjectId, VecDeque<MilestoneEvent>>>,
    pub(crate) digest_buffer: Mutex<Vec<DigestEntry>>,
    pub(crate) daily_reactive_token_budget: u32,
    pub(crate) daily_digest_token_budget: u32,
    pub(crate) reactive_tokens_used: AtomicU32,
    pub(crate) digest_tokens_used: AtomicU32,
    pub(crate) digest_interval_hours: u64,
    pub(crate) last_digest_at: Mutex<DateTime<Utc>>,
}

impl FarcasterAgent {
    pub fn new(
        backend: Arc<dyn ChatBackend>,
        farga: Arc<dyn FargaWriter>,
        analyzer: Arc<dyn FarcasterAnalyzer>,
        projects: Vec<ProjectId>,
        project_agent_ids: HashMap<ProjectId, UserId>,
    ) -> Self {
        Self {
            backend,
            farga,
            analyzer,
            projects,
            project_agent_ids,
            event_buffer: Mutex::new(HashMap::new()),
            digest_buffer: Mutex::new(Vec::new()),
            daily_reactive_token_budget: 20_000,
            daily_digest_token_budget: 10_000,
            reactive_tokens_used: AtomicU32::new(0),
            digest_tokens_used: AtomicU32::new(0),
            digest_interval_hours: 6,
            last_digest_at: Mutex::new(Utc::now()),
        }
    }

    pub(crate) fn is_significant(event: &MilestoneEvent) -> bool {
        match event {
            MilestoneEvent::ArtifactProduced { .. } => true,
            MilestoneEvent::EvaluationCompleted { satisfied: true, .. } => true,
            MilestoneEvent::EvaluationCompleted { .. } => false,
            MilestoneEvent::ReplanTriggered { replan_count, .. } => *replan_count >= 2,
            MilestoneEvent::MissionCompleted { .. } => true,
        }
    }

    pub(crate) async fn build_snapshot(&self) -> CrossSpaceSnapshot {
        let buf = self.event_buffer.lock().await;
        let projects = self.projects.iter().map(|pid| {
            let recent_events = buf.get(pid)
                .map(|deque| {
                    deque.iter().rev().take(3).map(|e| e.summary()).collect()
                })
                .unwrap_or_default();
            ProjectSnapshot {
                project_id: pid.clone(),
                mission_goal: None,
                open_sub_objectives: vec![],
                recent_events,
            }
        }).collect();
        CrossSpaceSnapshot { projects }
    }

    // Reactive path — called by SystemAgent::on_milestone
    pub(crate) async fn handle_milestone(&self, event: &MilestoneEvent) -> Result<()> {
        // Will be implemented in Task 7
        let _ = event;
        Ok(())
    }

    // Digest path — called by SystemAgent::tick
    pub(crate) async fn run_tick(&self) -> Result<()> {
        // Will be implemented in Task 8
        Ok(())
    }
}

fn format_digest(synthesis: &DigestSynthesis) -> String {
    let mut out = String::from("## Farcaster Digest\n\n");
    if !synthesis.connections.is_empty() {
        out.push_str("### Cross-Space Connections\n");
        for c in &synthesis.connections { out.push_str(&format!("- {}\n", c)); }
        out.push('\n');
    }
    if !synthesis.lessons.is_empty() {
        out.push_str("### Lessons\n");
        for l in &synthesis.lessons { out.push_str(&format!("- {}\n", l)); }
        out.push('\n');
    }
    if !synthesis.open_questions.is_empty() {
        out.push_str("### Open Questions\n");
        for q in &synthesis.open_questions { out.push_str(&format!("- {}\n", q)); }
    }
    out
}

#[async_trait]
impl SystemAgent for FarcasterAgent {
    fn name(&self) -> &str { "farcaster" }
    async fn on_milestone(&self, event: &MilestoneEvent) -> Result<()> {
        self.handle_milestone(event).await
    }
    async fn tick(&self) -> Result<()> {
        self.run_tick().await
    }
}
```

Update `charradissa-core/src/farcaster/mod.rs` — ensure re-exports now include `FarcasterAgent` (it should already be there from Task 1, but verify the file compiles).

- [ ] **Step 4: Run tests — verify they pass**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core --test farcaster_tests farcaster_agent_new 2>&1 | tail -10
```
Expected: `farcaster_agent_new_initializes_with_defaults` passes.

- [ ] **Step 5: Commit**

```bash
git add charradissa-core/src/farcaster/agent.rs charradissa-core/tests/farcaster_tests.rs
git commit -m "feat: add FarcasterAgent struct, new(), SystemAgent impl skeleton"
```

---

## Task 7: Reactive path — handle_milestone()

**Files:**
- Modify: `charradissa-core/src/farcaster/agent.rs` (fill in `handle_milestone`)
- Modify: `charradissa-core/tests/farcaster_tests.rs`

- [ ] **Step 1: Write failing tests for the reactive path**

Add to `charradissa-core/tests/farcaster_tests.rs`:
```rust
#[tokio::test]
async fn insignificant_events_skip_analyzer() {
    let (agent, _, _, _, analyzer) = make_agent(
        vec![ProjectId::new("alpha"), ProjectId::new("beta")],
        HashMap::new(),
    );
    // EvaluationCompleted with satisfied=false is NOT significant
    let event = MilestoneEvent::EvaluationCompleted {
        mission_id: "m1".into(),
        project_id: ProjectId::new("alpha"),
        sub_objective_id: "obj-1".into(),
        satisfied: false,
        reason: "not done yet".into(),
    };
    agent.on_milestone(&event).await.unwrap();
    // Analyzer queue should still be empty (pop_front would return None if called)
    // Indirectly: digest_buffer should be empty (no deferred entry either)
    // Access via the analyzer — reactive queue was never consumed
    // (This test passes if no panic occurs AND digest_buffer is empty)
    let digest = agent.digest_buffer.lock().await;
    assert!(digest.is_empty(), "insignificant event should not populate digest_buffer");
}

#[tokio::test]
async fn significant_event_triggers_analysis_and_dm() {
    let projects = vec![ProjectId::new("alpha"), ProjectId::new("beta")];
    let beta_agent_id = UserId::new("@beta-agent:matrix.test");
    let mut ids = HashMap::new();
    ids.insert(ProjectId::new("beta"), beta_agent_id.clone());

    let (agent, dms, _, _, analyzer) = make_agent(projects, ids);

    // Queue a High-urgency connection from alpha → beta
    analyzer.queue_reactive(vec![
        CrossSpaceConnection {
            from_project: ProjectId::new("alpha"),
            to_project: ProjectId::new("beta"),
            connection_type: "solved_problem".into(),
            summary: "auth schema resolution is applicable here".into(),
            urgency: Urgency::High,
        },
    ], 100);

    let event = MilestoneEvent::ArtifactProduced {
        mission_id: "m1".into(),
        session_id: "s1".into(),
        project_id: ProjectId::new("alpha"),
        canvas_id: "auth".into(),
        artifact_summary: "finalized auth schema".into(),
        sub_objective_ids: vec![],
    };

    agent.on_milestone(&event).await.unwrap();

    // DM should have been sent to beta-agent
    let sent_dms = dms.lock().await;
    assert_eq!(sent_dms.len(), 1, "expected one DM to beta agent");
    assert_eq!(sent_dms[0].0, beta_agent_id);
    assert!(sent_dms[0].1.contains("[farcaster]"), "DM should be tagged");
    assert!(sent_dms[0].1.contains("auth schema resolution"), "DM should include connection summary");

    // DigestEntry should be recorded
    let digest = agent.digest_buffer.lock().await;
    assert_eq!(digest.len(), 1);
    assert!(digest[0].whispered_at.is_some(), "High urgency should set whispered_at");
}

#[tokio::test]
async fn low_urgency_connection_skips_dm_but_records_digest_entry() {
    let projects = vec![ProjectId::new("alpha"), ProjectId::new("beta")];
    let mut ids = HashMap::new();
    ids.insert(ProjectId::new("beta"), UserId::new("@beta:t"));
    let (agent, dms, _, _, analyzer) = make_agent(projects, ids);

    analyzer.queue_reactive(vec![
        CrossSpaceConnection {
            from_project: ProjectId::new("alpha"),
            to_project: ProjectId::new("beta"),
            connection_type: "convergence_opportunity".into(),
            summary: "might share a library eventually".into(),
            urgency: Urgency::Low,
        },
    ], 50);

    let event = MilestoneEvent::ArtifactProduced {
        mission_id: "m1".into(),
        session_id: "s1".into(),
        project_id: ProjectId::new("alpha"),
        canvas_id: "c1".into(),
        artifact_summary: "drafted new API".into(),
        sub_objective_ids: vec![],
    };
    agent.on_milestone(&event).await.unwrap();

    assert!(dms.lock().await.is_empty(), "Low urgency should not DM");
    let digest = agent.digest_buffer.lock().await;
    assert_eq!(digest.len(), 1);
    assert!(digest[0].whispered_at.is_none(), "Low urgency should not set whispered_at");
}
```

- [ ] **Step 2: Run tests — verify they fail**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core --test farcaster_tests 2>&1 | grep -E "FAILED|passed|error" | head -20
```
Expected: new tests fail (handle_milestone is a no-op stub).

- [ ] **Step 3: Implement handle_milestone() in agent.rs**

Replace the `handle_milestone` stub in `charradissa-core/src/farcaster/agent.rs`:
```rust
pub(crate) async fn handle_milestone(&self, event: &MilestoneEvent) -> Result<()> {
    // 1. Accumulate to event_buffer (hold lock briefly, release before any await)
    {
        let mut buf = self.event_buffer.lock().await;
        let project_buf = buf.entry(event.project_id().clone()).or_default();
        if project_buf.len() >= EVENT_BUFFER_CAP { project_buf.pop_front(); }
        project_buf.push_back(event.clone());
    }

    // 2. Filter — skip insignificant events
    if !Self::is_significant(event) { return Ok(()); }

    // 3. Budget check — defer to digest if exhausted
    if self.reactive_tokens_used.load(Ordering::Relaxed) >= self.daily_reactive_token_budget {
        tracing::info!("farcaster: reactive budget exhausted, deferring to digest");
        self.digest_buffer.lock().await.push(DigestEntry {
            project_id: event.project_id().clone(),
            connection_summary: format!("deferred (budget): {}", event.summary()),
            involved_projects: vec![event.project_id().clone()],
            concurrence: vec![],
            urgency: Urgency::Low,
            whispered_at: None,
        });
        return Ok(());
    }

    // 4. Build snapshot (lock released before analyze call)
    let snapshot = self.build_snapshot().await;

    // 5. Analyze with Haiku — best-effort, log and return on failure
    let (connections, tokens_used) = match self.analyzer.analyze_cross_space(event, &snapshot).await {
        Ok(result) => result,
        Err(e) => {
            tracing::error!("farcaster: analyze_cross_space failed: {}", e);
            return Ok(());
        }
    };
    self.reactive_tokens_used.fetch_add(tokens_used, Ordering::Relaxed);

    // 6. Whisper Medium/High urgency connections
    let now = Utc::now();
    for conn in &connections {
        if matches!(conn.urgency, Urgency::Medium | Urgency::High) {
            if let Some(agent_id) = self.project_agent_ids.get(&conn.to_project) {
                let msg = format!(
                    "[farcaster] {}: {}\nSuggested: {}",
                    conn.from_project,
                    event.summary(),
                    conn.summary
                );
                if let Err(e) = self.backend.send_dm(agent_id, &msg).await {
                    tracing::warn!("farcaster: dm delivery failed for {}: {}", conn.to_project, e);
                }
            }
        }
    }

    // 7. Record all connections to digest_buffer
    {
        let mut digest = self.digest_buffer.lock().await;
        for conn in connections {
            let is_whispered = matches!(conn.urgency, Urgency::Medium | Urgency::High);
            let concurrence = if is_whispered {
                self.project_agent_ids.get(&conn.to_project).map(|uid| {
                    vec![AgentConcurrence {
                        project_id: conn.to_project.to_string(),
                        agent_address: uid.to_string(),
                        concurrence_type: ConcurrenceType::Whispered,
                        note: None,
                    }]
                }).unwrap_or_default()
            } else {
                vec![]
            };
            digest.push(DigestEntry {
                project_id: conn.from_project.clone(),
                connection_summary: conn.summary.clone(),
                involved_projects: vec![conn.from_project, conn.to_project],
                concurrence,
                urgency: conn.urgency,
                whispered_at: if is_whispered { Some(now) } else { None },
            });
        }
    }

    Ok(())
}
```

- [ ] **Step 4: Run all farcaster tests — verify they pass**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core --test farcaster_tests 2>&1 | tail -15
```
Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add charradissa-core/src/farcaster/agent.rs charradissa-core/tests/farcaster_tests.rs
git commit -m "feat: implement FarcasterAgent reactive path (handle_milestone)"
```

---

## Task 8: Digest path — run_tick()

**Files:**
- Modify: `charradissa-core/src/farcaster/agent.rs` (fill in `run_tick`)
- Modify: `charradissa-core/tests/farcaster_tests.rs`

- [ ] **Step 1: Write failing tests for the digest path**

Add to `charradissa-core/tests/farcaster_tests.rs`:
```rust
#[tokio::test]
async fn tick_with_empty_buffer_does_nothing() {
    let (agent, _, _, farga_calls, _) = make_agent(vec![], HashMap::new());
    agent.tick().await.unwrap();
    assert!(farga_calls.lock().await.is_empty());
}

#[tokio::test]
async fn tick_synthesizes_and_broadcasts_digest() {
    let projects = vec![ProjectId::new("alpha")];
    let (agent, _, messages, _, analyzer) = make_agent(projects, HashMap::new());

    // Pre-populate digest_buffer directly
    {
        let mut buf = agent.digest_buffer.lock().await;
        buf.push(DigestEntry {
            project_id: ProjectId::new("alpha"),
            connection_summary: "shared auth approach".into(),
            involved_projects: vec![ProjectId::new("alpha"), ProjectId::new("beta")],
            concurrence: vec![],
            urgency: Urgency::Medium,
            whispered_at: Some(chrono::Utc::now()),
        });
    }

    analyzer.queue_digest(DigestSynthesis {
        connections: vec!["alpha and beta share auth approach".into()],
        lessons: vec!["centralize auth early".into()],
        open_questions: vec!["which library?".into()],
        farga_verdict: "skip".into(),  // skip Farga submission in this test
        farga_title: None,
        farga_narrative: None,
    }, 200);

    agent.tick().await.unwrap();

    let sent = messages.lock().await;
    assert_eq!(sent.len(), 1, "digest should be broadcast to one room");
    assert!(sent[0].1.contains("Farcaster Digest"), "broadcast should be formatted digest");
    assert!(sent[0].1.contains("centralize auth early"), "digest should include lesson");
}

#[tokio::test]
async fn tick_submits_to_farga_on_submit_verdict() {
    let projects = vec![ProjectId::new("alpha")];
    let (agent, _, _, farga_calls, analyzer) = make_agent(projects, HashMap::new());

    {
        let mut buf = agent.digest_buffer.lock().await;
        buf.push(DigestEntry {
            project_id: ProjectId::new("alpha"),
            connection_summary: "important pattern".into(),
            involved_projects: vec![ProjectId::new("alpha")],
            concurrence: vec![],
            urgency: Urgency::High,
            whispered_at: None,
        });
    }

    analyzer.queue_digest(DigestSynthesis {
        connections: vec!["alpha discovered replan pattern".into()],
        lessons: vec!["aggressive replan budgets waste tokens".into()],
        open_questions: vec![],
        farga_verdict: "submit".into(),
        farga_title: Some("Replan Budget Lessons".into()),
        farga_narrative: Some("Projects replanning aggressively see 40% token waste.".into()),
    }, 500);

    agent.tick().await.unwrap();

    let calls = farga_calls.lock().await;
    assert_eq!(calls.len(), 1, "expected one Farga write_signals call");
    let (project_id, signals) = &calls[0];
    assert_eq!(project_id.as_str(), "system");
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].source, "farcaster");
    // Verify narrative is in the serialized payload
    assert!(signals[0].content.contains("Replan Budget Lessons"),
        "content should include title");
}

#[tokio::test]
async fn tick_requeues_entries_on_farga_failure() {
    use charradissa_core::error::CharradissaError;

    struct FailingFarga;
    #[async_trait]
    impl FargaWriter for FailingFarga {
        async fn write_signals(&self, _: &ProjectId, _: Vec<Signal>) -> Result<()> {
            Err(CharradissaError::Backend("simulated failure".into()))
        }
        async fn recent_signals(&self, _: &ProjectId, _: chrono::Duration) -> Result<Vec<Signal>> { Ok(vec![]) }
    }

    let (backend, _, _) = MockChatBackend::new();
    let analyzer = Arc::new(MockFarcasterAnalyzer::new());
    let agent = FarcasterAgent::new(
        Arc::new(backend),
        Arc::new(FailingFarga),
        Arc::clone(&analyzer) as Arc<dyn FarcasterAnalyzer>,
        vec![],
        HashMap::new(),
    );

    {
        let mut buf = agent.digest_buffer.lock().await;
        buf.push(DigestEntry {
            project_id: ProjectId::new("alpha"),
            connection_summary: "test entry".into(),
            involved_projects: vec![],
            concurrence: vec![],
            urgency: Urgency::Low,
            whispered_at: None,
        });
    }

    analyzer.queue_digest(DigestSynthesis {
        connections: vec![],
        lessons: vec!["lesson".into()],
        open_questions: vec![],
        farga_verdict: "submit".into(),
        farga_title: Some("T".into()),
        farga_narrative: Some("N".into()),
    }, 100);

    agent.tick().await.unwrap();  // should not panic

    // Buffer should be re-queued due to failure
    let buf = agent.digest_buffer.lock().await;
    assert_eq!(buf.len(), 1, "entry should be re-queued after Farga failure");
}
```

- [ ] **Step 2: Run tests — verify they fail**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core --test farcaster_tests tick_ 2>&1 | grep -E "FAILED|passed" | head -10
```
Expected: new tick tests fail (run_tick is a no-op stub).

- [ ] **Step 3: Implement run_tick() in agent.rs**

Replace the `run_tick` stub in `charradissa-core/src/farcaster/agent.rs`:
```rust
pub(crate) async fn run_tick(&self) -> Result<()> {
    // 1. Drain digest_buffer atomically
    let entries = {
        let mut buf = self.digest_buffer.lock().await;
        if buf.is_empty() { return Ok(()); }
        std::mem::take(&mut *buf)
    };

    // 2. Budget check — defer to next tick if exhausted
    if self.digest_tokens_used.load(Ordering::Relaxed) >= self.daily_digest_token_budget {
        tracing::info!("farcaster: digest budget exhausted, deferring to next tick");
        self.digest_buffer.lock().await.extend(entries);
        return Ok(());
    }

    // 3. Synthesize with Opus — requeue on failure
    let (synthesis, tokens_used) = match self.analyzer.synthesize_digest(&entries).await {
        Ok(result) => result,
        Err(e) => {
            tracing::error!("farcaster: synthesize_digest failed: {}", e);
            self.digest_buffer.lock().await.extend(entries);
            return Ok(());
        }
    };
    self.digest_tokens_used.fetch_add(tokens_used, Ordering::Relaxed);

    // 4. Broadcast to #farcaster (best-effort)
    let farcaster_room = RoomId::new("#farcaster");
    if let Err(e) = self.backend.send_message(&farcaster_room, &format_digest(&synthesis)).await {
        tracing::warn!("farcaster: broadcast failed: {}", e);
    }

    // 5. Farga submission if verdict is "submit"
    if synthesis.farga_verdict == "submit" {
        let period_end = Utc::now();
        let period_start = *self.last_digest_at.lock().await;

        let payload = DigestPayload {
            title: synthesis.farga_title.clone().unwrap_or_default(),
            narrative: synthesis.farga_narrative.clone().unwrap_or_default(),
            lessons: synthesis.lessons.clone(),
            open_questions: synthesis.open_questions.clone(),
            period_start,
            period_end,
            projects_observed: self.projects.clone(),
            concurrence: entries.iter().flat_map(|e| e.concurrence.iter().cloned()).collect(),
        };

        let content = serde_json::to_string(&payload)
            .map_err(|e| crate::error::CharradissaError::Dispatch(e.to_string()))?;

        let signals = vec![Signal {
            project: "system".to_string(),
            content,
            source: "farcaster".to_string(),
        }];

        let system_project = ProjectId::new("system");
        if let Err(e) = self.farga.write_signals(&system_project, signals).await {
            tracing::error!("farcaster: farga submission failed, re-queuing: {}", e);
            self.digest_buffer.lock().await.extend(entries);
            return Ok(());
        }
    }

    // 6. Update last_digest_at
    *self.last_digest_at.lock().await = Utc::now();
    Ok(())
}
```

- [ ] **Step 4: Run all farcaster tests — verify they pass**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core --test farcaster_tests 2>&1 | tail -20
```
Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add charradissa-core/src/farcaster/agent.rs charradissa-core/tests/farcaster_tests.rs
git commit -m "feat: implement FarcasterAgent digest path (run_tick)"
```

---

## Task 9: Budget cap test

**Files:**
- Modify: `charradissa-core/tests/farcaster_tests.rs`

- [ ] **Step 1: Write the budget cap test**

Add to `charradissa-core/tests/farcaster_tests.rs`:
```rust
#[tokio::test]
async fn reactive_budget_exhausted_defers_to_digest_without_calling_analyzer() {
    let projects = vec![ProjectId::new("alpha"), ProjectId::new("beta")];
    let (agent, dms, _, _, _analyzer) = make_agent(projects, HashMap::new());

    // Exhaust the reactive budget
    agent.reactive_tokens_used.store(20_000, std::sync::atomic::Ordering::Relaxed);

    let event = MilestoneEvent::ArtifactProduced {
        mission_id: "m1".into(),
        session_id: "s1".into(),
        project_id: ProjectId::new("alpha"),
        canvas_id: "c1".into(),
        artifact_summary: "some artifact".into(),
        sub_objective_ids: vec![],
    };

    // No response queued in analyzer — would panic if called
    agent.on_milestone(&event).await.unwrap();

    // Should NOT have called analyzer (no queue pop) and NOT DM'd anyone
    assert!(dms.lock().await.is_empty(), "budget exhaustion should skip DMs");

    // But should have added a deferred entry to digest_buffer
    let digest = agent.digest_buffer.lock().await;
    assert_eq!(digest.len(), 1, "deferred entry should be in digest_buffer");
    assert!(digest[0].connection_summary.contains("deferred"),
        "deferred entry summary should indicate it was deferred");
}

#[tokio::test]
async fn digest_budget_exhausted_requeues_buffer_without_calling_analyzer() {
    let (agent, _, _, farga_calls, _analyzer) = make_agent(vec![], HashMap::new());

    // Exhaust the digest budget
    agent.digest_tokens_used.store(10_000, std::sync::atomic::Ordering::Relaxed);

    // Add an entry to the buffer
    {
        let mut buf = agent.digest_buffer.lock().await;
        buf.push(DigestEntry {
            project_id: ProjectId::new("alpha"),
            connection_summary: "pending entry".into(),
            involved_projects: vec![],
            concurrence: vec![],
            urgency: Urgency::Low,
            whispered_at: None,
        });
    }

    // No response queued in analyzer — would panic if called
    agent.tick().await.unwrap();

    // No Farga call
    assert!(farga_calls.lock().await.is_empty());

    // Buffer re-queued
    let buf = agent.digest_buffer.lock().await;
    assert_eq!(buf.len(), 1, "buffer should be re-queued when digest budget exhausted");
}
```

- [ ] **Step 2: Run tests — verify they pass**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core --test farcaster_tests budget 2>&1 | tail -10
```
Expected: both budget tests pass.

- [ ] **Step 3: Run full test suite — verify no regressions**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core 2>&1 | tail -20
```
Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add charradissa-core/tests/farcaster_tests.rs
git commit -m "test: add reactive and digest budget cap tests for FarcasterAgent"
```

---

## Task 10: ClaudeFarcasterAnalyzer (real LLM implementation)

**Files:**
- Create: `charradissa-core/src/farcaster/claude_analyzer.rs`

- [ ] **Step 1: Implement ClaudeFarcasterAnalyzer**

Write `charradissa-core/src/farcaster/claude_analyzer.rs`:
```rust
use async_trait::async_trait;
use serde::Deserialize;
use crate::error::{CharradissaError, Result};
use crate::types::ProjectId;
use super::analyzer::{
    CrossSpaceConnection, CrossSpaceSnapshot, DigestEntry, DigestSynthesis, FarcasterAnalyzer,
};
use super::concurrence::Urgency;
use super::milestone::MilestoneEvent;

const HAIKU_MODEL: &str = "claude-haiku-4-5-20251001";
const OPUS_MODEL: &str = "claude-opus-4-8";
const REACTIVE_MAX_TOKENS: u32 = 512;
const DIGEST_MAX_TOKENS: u32 = 4096;

pub struct ClaudeFarcasterAnalyzer {
    client: reqwest::Client,
    api_key: String,
}

impl ClaudeFarcasterAnalyzer {
    pub fn new(api_key: String) -> Self {
        Self { client: reqwest::Client::new(), api_key }
    }

    async fn call_claude(
        &self,
        model: &str,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> Result<(String, u32)> {
        let body = serde_json::json!({
            "model": model,
            "max_tokens": max_tokens,
            "system": system,
            "messages": [{"role": "user", "content": user}]
        });

        let resp = self.client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send().await
            .map_err(|e| CharradissaError::Dispatch(e.to_string()))?;

        let data: serde_json::Value = resp.json().await
            .map_err(|e| CharradissaError::Dispatch(e.to_string()))?;

        if let Some(err) = data.get("error") {
            return Err(CharradissaError::Dispatch(err.to_string()));
        }

        let text = data["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let tokens = data["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32;

        Ok((text, tokens))
    }
}

#[derive(Deserialize)]
struct ConnectionJson {
    from_project: String,
    to_project: String,
    connection_type: String,
    summary: String,
    urgency: String,
}

fn parse_urgency(s: &str) -> Urgency {
    match s.to_lowercase().as_str() {
        "high" => Urgency::High,
        "medium" => Urgency::Medium,
        _ => Urgency::Low,
    }
}

#[derive(Deserialize)]
struct SynthesisJson {
    connections: Vec<String>,
    lessons: Vec<String>,
    open_questions: Vec<String>,
    farga_verdict: String,
    farga_title: Option<String>,
    farga_narrative: Option<String>,
}

const REACTIVE_SYSTEM: &str = "\
You are a cross-project intelligence analyzer. You receive a triggering milestone event and a snapshot of active projects. Identify cross-space connections that are actionable and specific. Return ONLY a JSON array — no prose. If no meaningful connection exists, return [].

Each item: {\"from_project\": str, \"to_project\": str, \"connection_type\": str, \"summary\": str, \"urgency\": str}
connection_type: shared_dependency | solved_problem | conflict | convergence_opportunity
urgency: low | medium | high";

const DIGEST_SYSTEM: &str = "\
You are a cross-project intelligence synthesizer. Review these cross-space connections observed over the past period. Return ONLY a JSON object — no prose.

Format: {\"connections\": [...str], \"lessons\": [...str], \"open_questions\": [...str], \"farga_verdict\": \"submit\"|\"skip\", \"farga_title\": str|null, \"farga_narrative\": str|null}
Set farga_verdict to \"submit\" only when lessons are substantive enough to contribute to organizational memory.";

#[async_trait]
impl FarcasterAnalyzer for ClaudeFarcasterAnalyzer {
    async fn analyze_cross_space(
        &self,
        triggering_event: &MilestoneEvent,
        snapshot: &CrossSpaceSnapshot,
    ) -> Result<(Vec<CrossSpaceConnection>, u32)> {
        let snapshot_text: Vec<String> = snapshot.projects.iter().map(|p| {
            format!(
                "Project: {}\nGoal: {}\nOpen: {}\nRecent events:\n{}",
                p.project_id,
                p.mission_goal.as_deref().unwrap_or("(unknown)"),
                p.open_sub_objectives.join(", "),
                p.recent_events.join("\n  "),
            )
        }).collect();

        let user = format!(
            "Triggering event: {}\n\nProject snapshots:\n{}",
            triggering_event.summary(),
            snapshot_text.join("\n\n")
        );

        let (text, tokens) = self.call_claude(HAIKU_MODEL, REACTIVE_SYSTEM, &user, REACTIVE_MAX_TOKENS).await?;

        // Extract JSON array from response (may be wrapped in markdown code block)
        let json_str = extract_json(&text);
        let items: Vec<ConnectionJson> = serde_json::from_str(json_str)
            .map_err(|e| CharradissaError::Dispatch(format!("failed to parse reactive response: {}: {}", e, text)))?;

        let connections = items.into_iter().map(|item| CrossSpaceConnection {
            from_project: ProjectId::new(&item.from_project),
            to_project: ProjectId::new(&item.to_project),
            connection_type: item.connection_type,
            summary: item.summary,
            urgency: parse_urgency(&item.urgency),
        }).collect();

        Ok((connections, tokens))
    }

    async fn synthesize_digest(&self, entries: &[DigestEntry]) -> Result<(DigestSynthesis, u32)> {
        let entries_text: Vec<String> = entries.iter().map(|e| {
            format!(
                "- [{} → {}] {} (urgency: {:?}, whispered: {})",
                e.involved_projects.first().map(|p| p.as_str()).unwrap_or("?"),
                e.involved_projects.get(1).map(|p| p.as_str()).unwrap_or("?"),
                e.connection_summary,
                e.urgency,
                e.whispered_at.is_some()
            )
        }).collect();

        let user = format!("Connections observed:\n{}", entries_text.join("\n"));

        let (text, tokens) = self.call_claude(OPUS_MODEL, DIGEST_SYSTEM, &user, DIGEST_MAX_TOKENS).await?;

        let json_str = extract_json(&text);
        let parsed: SynthesisJson = serde_json::from_str(json_str)
            .map_err(|e| CharradissaError::Dispatch(format!("failed to parse digest response: {}: {}", e, text)))?;

        Ok((DigestSynthesis {
            connections: parsed.connections,
            lessons: parsed.lessons,
            open_questions: parsed.open_questions,
            farga_verdict: parsed.farga_verdict,
            farga_title: parsed.farga_title,
            farga_narrative: parsed.farga_narrative,
        }, tokens))
    }
}

fn extract_json(text: &str) -> &str {
    // Strip markdown code fences if present
    if let Some(start) = text.find("```json") {
        let after = &text[start + 7..];
        if let Some(end) = after.find("```") {
            return after[..end].trim();
        }
    }
    if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        if let Some(end) = after.find("```") {
            return after[..end].trim();
        }
    }
    // Find the first [ or { and return from there
    let start = text.find('[').or_else(|| text.find('{')).unwrap_or(0);
    &text[start..]
}
```

Note: `Urgency` does not implement `Copy` or `as u8`. Fix the `format!` in `synthesize_digest` to use `{:?}` for both occurrences:
```rust
format!(
    "- [{} → {}] {} (urgency: {:?}, whispered: {})",
    e.involved_projects.first().map(|p| p.as_str()).unwrap_or("?"),
    e.involved_projects.get(1).map(|p| p.as_str()).unwrap_or("?"),
    e.connection_summary,
    e.urgency,
    e.whispered_at.is_some()
)
```

- [ ] **Step 2: Verify it compiles**

```bash
cd /Users/bedardpl/project/Charradissa && cargo build -p charradissa-core 2>&1 | tail -20
```
Expected: clean build (no errors).

- [ ] **Step 3: Add integration smoke test (ignored by default)**

Add to `charradissa-core/tests/farcaster_tests.rs`:
```rust
/// Integration test — requires ANTHROPIC_API_KEY env var. Run with:
/// cargo test -p charradissa-core --test farcaster_tests integration_claude_analyzer -- --ignored
#[tokio::test]
#[ignore]
async fn integration_claude_analyzer_produces_connections() {
    use charradissa_core::farcaster::claude_analyzer::ClaudeFarcasterAnalyzer;
    use charradissa_core::farcaster::analyzer::{CrossSpaceSnapshot, ProjectSnapshot};

    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .expect("ANTHROPIC_API_KEY required for integration test");

    let analyzer = ClaudeFarcasterAnalyzer::new(api_key);

    let event = MilestoneEvent::ArtifactProduced {
        mission_id: "m1".into(),
        session_id: "s1".into(),
        project_id: ProjectId::new("auth-service"),
        canvas_id: "design".into(),
        artifact_summary: "JWT token validation logic finalized using RS256".into(),
        sub_objective_ids: vec![],
    };

    let snapshot = CrossSpaceSnapshot {
        projects: vec![
            ProjectSnapshot {
                project_id: ProjectId::new("auth-service"),
                mission_goal: Some("implement authentication".into()),
                open_sub_objectives: vec!["write tests".into()],
                recent_events: vec!["finalized JWT logic".into()],
            },
            ProjectSnapshot {
                project_id: ProjectId::new("api-gateway"),
                mission_goal: Some("route and validate API requests".into()),
                open_sub_objectives: vec!["token verification middleware".into()],
                recent_events: vec!["started middleware spike".into()],
            },
        ],
    };

    let (connections, tokens) = analyzer.analyze_cross_space(&event, &snapshot).await.unwrap();

    println!("connections: {:?}", connections.iter().map(|c| &c.summary).collect::<Vec<_>>());
    println!("tokens used: {}", tokens);

    // At minimum: the call should return without error. A real LLM should find a connection.
    // We assert the call completed. Manual review of output is needed.
    assert!(tokens > 0, "tokens should be consumed");
}
```

- [ ] **Step 4: Run non-ignored tests to confirm no regressions**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core 2>&1 | tail -15
```
Expected: all non-ignored tests pass.

- [ ] **Step 5: Commit**

```bash
git add charradissa-core/src/farcaster/claude_analyzer.rs charradissa-core/tests/farcaster_tests.rs
git commit -m "feat: add ClaudeFarcasterAnalyzer (Haiku reactive + Opus digest)"
```

---

## Task 11: Daemon wiring

**Files:**
- Modify: `charradissa-daemon/src/main.rs`

- [ ] **Step 1: Update main.rs to wire FarcasterAgent**

Replace `charradissa-daemon/src/main.rs` with:
```rust
mod registry;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use charradissa_core::config::Config;
use charradissa_core::concierge::ConciergeAgent;
use charradissa_core::farcaster::milestone::MilestoneEvent;
use charradissa_core::farcaster::FarcasterAgent;
use charradissa_core::farcaster::claude_analyzer::ClaudeFarcasterAnalyzer;
use charradissa_core::farga::HttpFargaWriter;
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

    // Milestone broadcast channel — sender is used by appservice handlers (future task),
    // receiver is consumed by the dispatch task below.
    let (milestone_tx, mut milestone_rx) =
        tokio::sync::broadcast::channel::<MilestoneEvent>(256);
    let _ = milestone_tx; // suppress unused warning until appservice wiring is added

    let anthropic_api_key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();
    let farga_base_url = std::env::var("FARGA_BASE_URL")
        .unwrap_or_else(|_| "http://localhost:9000".into());

    // ConciergeAgent owns the FarcasterAgent — no Arc needed for the agent itself.
    let mut concierge = ConciergeAgent::new(
        Arc::clone(&backend) as Arc<dyn charradissa_core::backend::ChatBackend>,
        Arc::new(HttpFargaWriter::new(farga_base_url.clone())),
        vec![],
        HashMap::new(),
        24, 6, 50_000,
    );

    concierge.register_system_agent(
        Box::new(FarcasterAgent::new(
            Arc::clone(&backend) as Arc<dyn charradissa_core::backend::ChatBackend>,
            Arc::new(HttpFargaWriter::new(farga_base_url)),
            Arc::new(ClaudeFarcasterAnalyzer::new(anthropic_api_key)),
            vec![], // projects populated from config in a future task
            HashMap::new(),
        )),
        Duration::from_secs(6 * 3600),
    );

    let concierge = Arc::new(concierge);

    // Dispatch milestones from the broadcast channel to all registered system agents.
    let concierge_dispatch = Arc::clone(&concierge);
    tokio::spawn(async move {
        loop {
            match milestone_rx.recv().await {
                Ok(event) => concierge_dispatch.dispatch_milestone(&event).await,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("farcaster: milestone receiver lagged, dropped {} events", n);
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    // Run system agent tick loop (polls every 60s, calls tick() when interval elapses).
    let concierge_ticks = Arc::clone(&concierge);
    tokio::spawn(async move {
        concierge_ticks.run_system_agent_ticks().await;
    });

    tracing::info!("charradissa-daemon starting for org: {}", config.org.name);

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

- [ ] **Step 2: Verify it compiles**

```bash
cd /Users/bedardpl/project/Charradissa && cargo build -p charradissa-daemon 2>&1 | tail -30
```
Expected: clean build. If there are import or unused variable warnings, fix them. The `milestone_tx` will be unused until appservice handlers are wired — add `let _ = milestone_tx;` after the channel creation to suppress the warning.

- [ ] **Step 3: Run full workspace check**

```bash
cd /Users/bedardpl/project/Charradissa && cargo check 2>&1 | tail -20
```
Expected: clean (no errors).

- [ ] **Step 4: Run all tests**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core 2>&1 | tail -20
```
Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add charradissa-daemon/src/main.rs
git commit -m "feat: wire FarcasterAgent into charradissa-daemon with milestone broadcast channel"
```

---

## Done

After Task 11 completes, the full Farcaster feature is implemented:
- `MilestoneEvent`, `AgentConcurrence`, `Urgency`, `ConcurrenceType` — new core types
- `SystemAgent`, `FarcasterAnalyzer` — new traits with object-safety verified
- `FarcasterAgent` — reactive and digest paths, budget control, Farga submission, retry on failure
- `ClaudeFarcasterAnalyzer` — real LLM impl (Haiku reactive, Opus digest)
- `ConciergeAgent` — extended with system agent registration and milestone dispatch
- `charradissa-daemon` — FarcasterAgent wired with milestone broadcast channel

Use `superpowers:finishing-a-development-branch` to push and open a PR.
