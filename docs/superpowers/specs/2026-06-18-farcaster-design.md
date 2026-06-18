# Farcaster — Cross-Space Intelligence Agent

**Date:** 2026-06-18
**Status:** Draft
**Scope:** `charradissa-core` — new `SystemAgent` pattern + `FarcasterAgent` as first implementor

---

## Motivation

As the number of active projects in the Occitan stack grows, agents working within a single project context cannot see connections, conflicts, or solved problems in adjacent projects. A team debugging an auth replan failure doesn't know that another project resolved the same issue two sessions ago. A business domain insight that should reshape an engineering architecture never reaches the engineering agents.

Farcaster is the cross-space intelligence layer that closes this gap. It observes significant Amassada milestones across all active missions, detects cross-project connections, whispers the right project agents with targeted context, and periodically distils collective lessons into Farga contributions.

Farcaster is not infrastructure — it is an agent. The Concierge spawns and supervises it the same way a moderator spawns a specialist. This establishes a general **system agent** pattern that future automation agents (`FargaLibrarian`, `BudgetWarden`, etc.) will implement without touching Concierge internals.

---

## New Concepts

### SystemAgent

A `SystemAgent` is an agent spawned and supervised by the Concierge. It observes a scoped event stream and acts through the existing primitives (whisper, DM, Farga contribution). All system agents share one interface:

```rust
#[async_trait]
pub trait SystemAgent: Send + Sync {
    fn name(&self) -> &str;
    async fn on_milestone(&self, event: &MilestoneEvent) -> Result<()>;
    async fn tick(&self) -> Result<()>;  // called on digest interval
}
```

`ConciergeAgent` gains a `system_agents: Vec<Box<dyn SystemAgent>>` field and a `dispatch_to_system_agents()` method that fans milestone events out to all registered agents. Each system agent is async and self-contained — the Concierge delivers the event and moves on.

### MilestoneEvent

A lightweight translated event emitted by Charradissa when Amassada crosses a significant threshold. Defined in `charradissa-core` (not `amassada-core`) to keep the dependency direction clean:

```rust
pub enum MilestoneEvent {
    ArtifactProduced {
        mission_id: String,
        session_id: String,
        project_id: ProjectId,
        canvas_id: String,
        artifact_summary: String,   // truncated — not full text
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
        verdict: String,    // "submit" or "skip"
    },
}
```

Events are delivered via a `tokio::broadcast` channel owned by `charradissa-daemon`. Amassada does not know about Farcaster — the daemon translates `SessionEvent`/`MissionEvent` into `MilestoneEvent` at the boundary.

### AgentConcurrence

Captures which agents observed or were informed about a cross-space connection. Used as a strength signal in Farga contributions — the librarian weighs how many independent agents concurred when deciding whether to integrate a lesson into collective memory.

```rust
pub struct AgentConcurrence {
    pub project_id: String,
    pub agent_address: String,       // CompositionAddress as string
    pub concurrence_type: ConcurrenceType,
    pub note: Option<String>,
}

pub enum ConcurrenceType {
    Observed,      // same pattern independently hit in this project
    Whispered,     // Farcaster surfaced this connection to the agent
    Acknowledged,  // agent explicitly responded positively (v2 — not populated yet)
}
```

---

## FarcasterAgent

### Struct

```rust
pub struct FarcasterAgent {
    backend: Arc<dyn ChatBackend>,
    farga: Arc<dyn FargaWriter>,
    analyzer: Arc<dyn FarcasterAnalyzer>,   // trait over LLM calls — mockable in tests
    projects: Vec<ProjectId>,
    project_agent_ids: HashMap<ProjectId, UserId>,
    event_buffer: Mutex<HashMap<ProjectId, VecDeque<MilestoneEvent>>>,  // capped at 50/project
    digest_buffer: Mutex<Vec<DigestEntry>>,
    daily_reactive_token_budget: u32,
    daily_digest_token_budget: u32,
    reactive_tokens_used: AtomicU32,
    digest_tokens_used: AtomicU32,
    digest_interval_hours: u64,
    last_digest_at: Mutex<DateTime<Utc>>,
}

pub struct DigestEntry {
    pub project_id: ProjectId,
    pub connection_summary: String,
    pub involved_projects: Vec<ProjectId>,
    pub concurrence: Vec<AgentConcurrence>,
    pub urgency: Urgency,
    pub whispered_at: Option<DateTime<Utc>>,
}

pub enum Urgency { Low, Medium, High }
```

### FarcasterAnalyzer Trait

Abstracts both LLM calls behind a testable interface:

```rust
#[async_trait]
pub trait FarcasterAnalyzer: Send + Sync {
    async fn analyze_cross_space(
        &self,
        triggering_event: &MilestoneEvent,
        snapshot: &CrossSpaceSnapshot,
    ) -> Result<(Vec<CrossSpaceConnection>, u32)>;  // (connections, tokens_used)

    async fn synthesize_digest(
        &self,
        entries: &[DigestEntry],
    ) -> Result<(DigestSynthesis, u32)>;
}

pub struct CrossSpaceSnapshot {
    pub projects: Vec<ProjectSnapshot>,
}

pub struct ProjectSnapshot {
    pub project_id: ProjectId,
    pub mission_goal: Option<String>,
    pub open_sub_objectives: Vec<String>,
    pub recent_events: Vec<String>,   // last 3 milestone summaries as text
}

pub struct CrossSpaceConnection {
    pub from_project: ProjectId,
    pub to_project: ProjectId,
    pub connection_type: String,    // "shared_dependency" | "solved_problem" | "conflict" | "convergence_opportunity"
    pub summary: String,
    pub urgency: Urgency,
}

pub struct DigestSynthesis {
    pub connections: Vec<String>,
    pub lessons: Vec<String>,
    pub open_questions: Vec<String>,
    pub farga_verdict: String,      // "submit" | "skip"
    pub farga_title: Option<String>,
    pub farga_narrative: Option<String>,
}
```

`ClaudeFarcasterAnalyzer` implements this using Haiku for reactive analysis (fast, cheap, capped at 512 output tokens) and Opus for digest synthesis (capped at 4096 output tokens).

---

## Reactive Path

Triggered by `on_milestone()`. Runs only on significant event types:

| Event | Triggers analysis? |
|---|---|
| `ArtifactProduced` | Yes |
| `EvaluationCompleted { satisfied: true }` | Yes |
| `ReplanTriggered { replan_count >= 2 }` | Yes — persistent failure, peers may have solved this |
| `EvaluationCompleted { satisfied: false }` | No — noise, wait for replan |
| `MissionCompleted` | Yes — always |

**Steps:**

1. **Accumulate** — append event to `event_buffer[project_id]`. Buffer capped at 50 per project (LRU drop when full).

2. **Filter** — apply the table above. If not significant, stop.

3. **Budget check** — if `reactive_tokens_used >= daily_reactive_token_budget`, log and stop. Append to `digest_buffer` anyway so the signal isn't lost.

4. **Snapshot** — build `CrossSpaceSnapshot`: for each project, collect last 3 event summaries + mission goal + open sub-objectives. Kept intentionally short — this is the LLM's working context, not the full transcript.

5. **Analyze** — call `analyzer.analyze_cross_space(event, snapshot)`. The system prompt instructs Haiku to return only connections with meaningful cross-project relevance; empty array if nothing actionable.

6. **Whisper** — for each connection with `urgency >= Medium`:
   - Look up the project agent user ID for `to_project` from `project_agent_ids`
   - Send DM via `backend.send_dm()`:
     ```
     [farcaster] {from_project} just {event summary}.
     This may be relevant to your work on {sub_objective}.
     Suggested: {connection summary}
     ```
   - The whisper is informational — the project agent decides how to respond (create ticket, initiate DM with the other project's agent, flag for later, ignore)

7. **Record** — append all connections (regardless of urgency) to `digest_buffer` with `whispered_at` set for Medium/High urgency ones. Increment `reactive_tokens_used`.

**On failure:** if the LLM call or DM delivery fails, the event remains in the buffer and is included in the next digest cycle. No retry — the milestone is ephemeral, but the information persists.

---

## Digest Path

Triggered by `tick()`. Runs on `digest_interval_hours` cadence (default: 6h). Skips if `digest_buffer` is empty.

**Steps:**

1. **Collect** — drain `digest_buffer`. Includes:
   - All whispered connections (Medium/High urgency, `whispered_at` set)
   - All buffered low-urgency connections
   - `MissionCompleted` events with `verdict == "submit"` since last digest
   - `ReplanTriggered` where `replan_count >= REPLAN_LIMIT` (sub-objectives that went OutOfScope — persistent failures worth capturing)

2. **Budget check** — if `digest_tokens_used >= daily_digest_token_budget`, defer to next tick. Do not drop the buffer.

3. **Build concurrence list** — for each lesson candidate in the buffer:
   - `Observed`: projects that independently hit the same pattern (same canvas_id + similar sub_objective keyword overlap)
   - `Whispered`: projects that received a DM about this connection

4. **Synthesize** — call `analyzer.synthesize_digest(entries)` using Opus. Output: connections summary, lessons, open questions, Farga verdict.

5. **Broadcast** — post formatted Markdown digest to the `#farcaster` Matrix room. Humans and agents can subscribe passively. Best-effort — failure is logged, not retried.

6. **Farga submission** — if `farga_verdict == "submit"`:
   Serialize the synthesis into `Signal` entries and call `farga.write_signals(project, signals)`. The signals are scoped to a sentinel "system" project representing the cross-space layer:
   ```rust
   let signals = vec![
       Signal {
           project: "system".to_string(),
           content: serde_json::to_string(&DigestPayload {
               title: synthesis.farga_title,
               narrative: synthesis.farga_narrative,
               lessons: synthesis.lessons,
               open_questions: synthesis.open_questions,
               period_start, period_end,
               projects_observed,
               concurrence: Vec<AgentConcurrence>,
           })?,
           source: "farcaster".to_string(),
       }
   ];
   farga.write_signals(&system_project_id, signals).await?;
   ```
   If submission fails, retry on next tick (buffer preserved). `DigestPayload` is a local struct — JSON-serialized into `Signal.content`.

7. **Reset** — clear `digest_buffer`, update `last_digest_at`, reset daily token counters at midnight UTC.

---

## ConciergeAgent Changes

```rust
pub struct ConciergeAgent {
    // ... existing fields unchanged ...
    system_agents: Vec<Box<dyn SystemAgent>>,
    system_agent_intervals: Vec<Duration>,    // per-agent tick interval
}

impl ConciergeAgent {
    pub fn register_system_agent(
        &mut self,
        agent: Box<dyn SystemAgent>,
        tick_interval: Duration,
    ) { ... }

    pub async fn dispatch_milestone(&self, event: &MilestoneEvent) {
        for agent in &self.system_agents {
            if let Err(e) = agent.on_milestone(event).await {
                tracing::error!("[{}] on_milestone error: {}", agent.name(), e);
            }
        }
    }

    pub async fn run_system_agent_ticks(&self) {
        // spawns one tokio task per system agent, each sleeping their interval
    }
}
```

The `charradissa-daemon` registers `FarcasterAgent` at startup:
```rust
concierge.register_system_agent(
    Box::new(FarcasterAgent::new(...)),
    Duration::from_secs(6 * 3600),
);
```

---

## Phase II Note: Fractal Hierarchy

This spec implements one level of a fractal architecture. In Phase II:

- Multiple domain-scoped Concierge instances replace the single global one (one per Fondament domain: `business`, `engineering`, `data`, `infra`)
- Each instance runs its own FarcasterAgent scoped to its domain's projects
- Domain digests become `ObservationEvent`s consumed by a cross-domain Farcaster at the level above
- `MilestoneEvent` is generalized to `ObservationEvent` (union of raw milestone or inbound digest)
- `SystemAgent::on_milestone()` becomes `SystemAgent::on_observation()`
- Farga sits at the root, receiving domain-level digests and governing archetype evolution across all domains

A business domain insight can cascade into engineering → infra → data through this hierarchy. Cross-domain resonance is detected at the domain Farcaster level and escalated to Farga for the governance cycle.

---

## Budget Defaults

| Parameter | Default |
|---|---|
| `daily_reactive_token_budget` | 20,000 |
| `daily_digest_token_budget` | 10,000 |
| `digest_interval_hours` | 6 |
| `event_buffer_cap_per_project` | 50 |
| Reactive max output tokens (Haiku) | 512 |
| Digest max output tokens (Opus) | 4,096 |

---

## Testing

- **`MockSystemAgent`** — records `on_milestone()` calls and `tick()` invocations. Used to test `ConciergeAgent::dispatch_milestone()` fan-out.
- **`MockFarcasterAnalyzer`** — returns pre-baked `CrossSpaceConnection` lists and `DigestSynthesis` values. Follows the same dequeue pattern as `MockEvaluator` and `MockMetaModerator`.
- **Reactive path test** — inject two `MilestoneEvent`s from different projects with overlapping keywords, assert `MockFarcasterAnalyzer::analyze_cross_space()` is called with correct snapshot, assert `backend.send_dm()` is called for the target project agent.
- **Digest path test** — pre-populate `digest_buffer`, call `tick()`, assert `farga.write_signals()` is called with a contribution containing the correct concurrence list.
- **Budget cap test** — exhaust `daily_reactive_token_budget`, assert further `on_milestone()` calls skip the LLM but still append to `digest_buffer`.
- **Integration smoke test** (`#[ignore]`, requires `ANTHROPIC_API_KEY`) — two mock projects emit overlapping milestones, assert at least one connection produced and one DM sent.

---

## What Does Not Change

- `SessionEngine` and `MissionEngine` — unchanged. They emit `SessionEvent`/`MissionEvent` as before. `charradissa-daemon` translates these to `MilestoneEvent` at the boundary.
- `ConciergeAgent` archival and convergence loops — unchanged.
- `FargaWriter` trait — unchanged. Farcaster uses `write_signals()` as-is.
- `ChatBackend` trait — unchanged. Farcaster uses `send_dm()` which already exists.
