# Governance Session Composition — Plan 1: Foundational Types

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the four missing Fondament stance YAMLs, define `GovernanceContribution` and supporting types in `charradissa-core`, add `first_observed_at` to `DigestEntry`, and wire Farcaster's digest path to emit a `GovernanceContribution` when `farga_verdict == "submit"`.

**Architecture:** Four tasks across two repos (Fondament and Charradissa). Fondament gets four stance YAML files. `charradissa-core` gets a new `farcaster/governance.rs` module with the governance types, a field addition to `DigestEntry`, and a new trait method on `FargaWriter`. The existing `write_signals` Farga submission in `run_tick` is replaced by `submit_governance_contribution`, which has a default implementation that serializes to a `Signal` — existing mocks inherit it without changes.

**Tech Stack:** Rust, `serde`/`serde_json`, `chrono`, `async-trait`; YAML for Fondament definitions.

---

## File Structure

**Create:**
- `Fondament/definitions/stances/builder.yaml`
- `Fondament/definitions/stances/realist.yaml`
- `Fondament/definitions/stances/dreamer.yaml`
- `Fondament/definitions/stances/moderator.yaml`
- `charradissa-core/src/farcaster/governance.rs` — `GovernanceContribution`, `FargaLayer`, `ReversibilityLevel`, `ImpactScope`

**Modify:**
- `charradissa-core/src/farcaster/analyzer.rs` — add `first_observed_at: DateTime<Utc>` to `DigestEntry`
- `charradissa-core/src/farcaster/agent.rs` — set `first_observed_at` in both `DigestEntry` construction sites; replace Farga submission block in `run_tick` with `submit_governance_contribution` call; remove now-unused `Signal` and `DigestPayload` imports
- `charradissa-core/src/farcaster/mod.rs` — declare and re-export governance module
- `charradissa-core/src/farga.rs` — add `submit_governance_contribution` default async method to `FargaWriter` trait
- `charradissa-core/tests/farcaster_tests.rs` — add `first_observed_at` to 4 `DigestEntry` literals; fix `tick_submits_to_farga_on_submit_verdict` source assertion; add 4 new tests

---

## Task 1: Fondament stance definitions

**Files:**
- Create: `Fondament/definitions/stances/builder.yaml`
- Create: `Fondament/definitions/stances/realist.yaml`
- Create: `Fondament/definitions/stances/dreamer.yaml`
- Create: `Fondament/definitions/stances/moderator.yaml`
- Modify: `Fondament/fondament-core/tests/resolver_tests.rs`

- [ ] **Step 1: Write failing tests**

Add to `Fondament/fondament-core/tests/resolver_tests.rs` (after the existing `resolves_role_address_to_agent` test):

```rust
fn make_tree_with_stances() -> DefinitionTree {
    let dir = TempDir::new().unwrap();
    let stances: &[(&str, &str)] = &[
        ("stances/builder.yaml", "id: stances/builder\nkind: stance\ncontext: |\n  Construct solutions.\n"),
        ("stances/realist.yaml", "id: stances/realist\nkind: stance\ncontext: |\n  Assess feasibility.\n"),
        ("stances/dreamer.yaml", "id: stances/dreamer\nkind: stance\ncontext: |\n  Explore without constraint.\n"),
        ("stances/moderator.yaml", "id: stances/moderator\nkind: stance\ncontext: |\n  Hold the process.\n"),
    ];
    for (path, content) in stances {
        let full = dir.path().join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, content).unwrap();
    }
    DefinitionTree::load(dir.path()).unwrap()
}

#[tokio::test]
async fn resolves_builder_stance_context() {
    let tree = make_tree_with_stances();
    let farga = MockFarga;
    let address: CompositionAddress = "fondament/stances/builder".parse().unwrap();
    let agent = resolve(&address, &tree, &farga, "acme").await.unwrap();
    assert!(agent.system_prompt.contains("Construct solutions"));
}

#[tokio::test]
async fn all_four_stances_resolve_without_error() {
    let tree = make_tree_with_stances();
    let farga = MockFarga;
    for stance in &["builder", "realist", "dreamer", "moderator"] {
        let addr: CompositionAddress = format!("fondament/stances/{}", stance).parse().unwrap();
        let agent = resolve(&addr, &tree, &farga, "acme").await.unwrap();
        assert!(!agent.system_prompt.is_empty(), "stance {} produced empty prompt", stance);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd /Users/bedardpl/project/Fondament && cargo test -p fondament-core --test resolver_tests 2>&1 | tail -10
```
Expected: `resolves_builder_stance_context` and `all_four_stances_resolve_without_error` fail (files don't exist yet).

- [ ] **Step 3: Write the four YAML files**

Write `Fondament/definitions/stances/builder.yaml`:
```yaml
id: stances/builder
kind: stance
context: |
  Construct solutions. Your role is to find the path forward — make things
  work, surface viable alternatives, build on what exists. Criticism without
  a proposed alternative is incomplete.
```

Write `Fondament/definitions/stances/realist.yaml`:
```yaml
id: stances/realist
kind: stance
context: |
  Assess feasibility. Identify real constraints, cut scope to what can
  actually be delivered, and distinguish genuine blockers from hypothetical
  concerns. Your role is clarity, not pessimism.
```

Write `Fondament/definitions/stances/dreamer.yaml`:
```yaml
id: stances/dreamer
kind: stance
context: |
  Explore without constraint. Generate alternatives, challenge assumptions
  about what is fixed, think past current limits. Your role is to expand
  the solution space before it contracts.
```

Write `Fondament/definitions/stances/moderator.yaml`:
```yaml
id: stances/moderator
kind: stance
context: |
  Hold the process. Balance voices, ensure all perspectives surface, and
  synthesize without advocating for any position. Your role is the quality
  of the conversation, not its conclusion.
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd /Users/bedardpl/project/Fondament && cargo test -p fondament-core --test resolver_tests 2>&1 | tail -10
```
Expected: all 3 resolver tests pass.

- [ ] **Step 5: Commit**

```bash
cd /Users/bedardpl/project/Fondament && git add definitions/stances/ fondament-core/tests/resolver_tests.rs && git commit -m "feat: add builder, realist, dreamer, moderator stance definitions"
```

---

## Task 2: GovernanceContribution types

**Files:**
- Create: `charradissa-core/src/farcaster/governance.rs`
- Modify: `charradissa-core/src/farcaster/mod.rs`
- Modify: `charradissa-core/tests/farcaster_tests.rs`

- [ ] **Step 1: Write failing tests**

Add to `charradissa-core/tests/farcaster_tests.rs` (near the top with existing use statements, add):
```rust
use charradissa_core::farcaster::governance::{
    GovernanceContribution, FargaLayer, ReversibilityLevel, ImpactScope,
};
```

Add at the bottom of the test file:
```rust
#[test]
fn governance_contribution_serializes_round_trip() {
    use charradissa_core::farcaster::concurrence::{AgentConcurrence, ConcurrenceType};

    let contrib = GovernanceContribution {
        title: "Auth pattern discovered".into(),
        narrative: "Two projects independently chose RS256".into(),
        lessons: vec!["Use RS256 for JWT signing".into()],
        open_questions: vec!["Should we centralize key rotation?".into()],
        involved_projects: vec![ProjectId::new("auth"), ProjectId::new("gateway")],
        concurrence: vec![AgentConcurrence {
            project_id: "auth".into(),
            agent_address: "auth+adversarial".into(),
            concurrence_type: ConcurrenceType::Whispered,
            note: None,
        }],
        target_layer: FargaLayer::ProjectLevel,
        first_observed_at: chrono::Utc::now(),
        last_observed_at: chrono::Utc::now(),
        event_count: 3,
        reversibility: Some(ReversibilityLevel::FullyReversible),
        impact: None,
    };

    let json = serde_json::to_string(&contrib).unwrap();
    let decoded: GovernanceContribution = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.title, "Auth pattern discovered");
    assert_eq!(decoded.event_count, 3);
    assert!(decoded.impact.is_none());
    assert_eq!(decoded.reversibility, Some(ReversibilityLevel::FullyReversible));
}

#[test]
fn farga_layer_variants_serialize_to_distinct_strings() {
    let variants = [FargaLayer::OrgLevel, FargaLayer::InitiativeLevel, FargaLayer::ProjectLevel];
    let serialized: Vec<String> = variants.iter()
        .map(|v| serde_json::to_string(v).unwrap())
        .collect();
    let unique: std::collections::HashSet<_> = serialized.iter().collect();
    assert_eq!(unique.len(), 3);
    assert_eq!(serde_json::to_string(&FargaLayer::OrgLevel).unwrap(), r#""OrgLevel""#);
}

#[test]
fn all_reversibility_and_impact_variants_serialize() {
    let rev = [
        ReversibilityLevel::FullyReversible,
        ReversibilityLevel::EffectsLinger,
        ReversibilityLevel::CostlyReversible,
        ReversibilityLevel::Irreversible,
    ];
    let unique_rev: std::collections::HashSet<_> = rev.iter()
        .map(|r| serde_json::to_string(r).unwrap())
        .collect();
    assert_eq!(unique_rev.len(), 4);

    let imp = [
        ImpactScope::Contained,
        ImpactScope::CrossProject,
        ImpactScope::DomainWide,
        ImpactScope::OrgWide,
    ];
    let unique_imp: std::collections::HashSet<_> = imp.iter()
        .map(|i| serde_json::to_string(i).unwrap())
        .collect();
    assert_eq!(unique_imp.len(), 4);
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core --test farcaster_tests governance_ 2>&1 | tail -10
```
Expected: compile error — `charradissa_core::farcaster::governance` does not exist.

- [ ] **Step 3: Write governance.rs**

Create `charradissa-core/src/farcaster/governance.rs`:
```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::types::ProjectId;
use super::concurrence::AgentConcurrence;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FargaLayer {
    OrgLevel,
    InitiativeLevel,
    ProjectLevel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReversibilityLevel {
    FullyReversible,
    EffectsLinger,
    CostlyReversible,
    Irreversible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImpactScope {
    Contained,
    CrossProject,
    DomainWide,
    OrgWide,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceContribution {
    pub title: String,
    pub narrative: String,
    pub lessons: Vec<String>,
    pub open_questions: Vec<String>,
    pub involved_projects: Vec<ProjectId>,
    pub concurrence: Vec<AgentConcurrence>,
    pub target_layer: FargaLayer,
    pub first_observed_at: DateTime<Utc>,
    pub last_observed_at: DateTime<Utc>,
    pub event_count: u32,
    /// Set to None at submission — filled by Farga librarian assessment (Plan 2)
    pub reversibility: Option<ReversibilityLevel>,
    /// Set to None at submission — filled by Farga librarian assessment (Plan 2)
    pub impact: Option<ImpactScope>,
}
```

- [ ] **Step 4: Add module declaration and re-exports to mod.rs**

In `charradissa-core/src/farcaster/mod.rs`, add after `pub mod claude_analyzer;`:
```rust
pub mod governance;
pub use governance::{GovernanceContribution, FargaLayer, ReversibilityLevel, ImpactScope};
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core --test farcaster_tests governance_ 2>&1 | tail -10
```
Expected: all 3 governance type tests pass.

- [ ] **Step 6: Commit**

```bash
cd /Users/bedardpl/project/Charradissa && git add charradissa-core/src/farcaster/governance.rs charradissa-core/src/farcaster/mod.rs charradissa-core/tests/farcaster_tests.rs && git commit -m "feat: add GovernanceContribution, FargaLayer, ReversibilityLevel, ImpactScope types"
```

---

## Task 3: DigestEntry.first_observed_at

**Files:**
- Modify: `charradissa-core/src/farcaster/analyzer.rs`
- Modify: `charradissa-core/src/farcaster/agent.rs`
- Modify: `charradissa-core/tests/farcaster_tests.rs`

- [ ] **Step 1: Add field to DigestEntry**

In `charradissa-core/src/farcaster/analyzer.rs`, change:
```rust
#[derive(Debug, Clone)]
pub struct DigestEntry {
    pub project_id: ProjectId,
    pub connection_summary: String,
    pub involved_projects: Vec<ProjectId>,
    pub concurrence: Vec<AgentConcurrence>,
    pub urgency: Urgency,
    pub whispered_at: Option<DateTime<Utc>>,
}
```
to:
```rust
#[derive(Debug, Clone)]
pub struct DigestEntry {
    pub project_id: ProjectId,
    pub connection_summary: String,
    pub involved_projects: Vec<ProjectId>,
    pub concurrence: Vec<AgentConcurrence>,
    pub urgency: Urgency,
    pub whispered_at: Option<DateTime<Utc>>,
    pub first_observed_at: DateTime<Utc>,
}
```

- [ ] **Step 2: Fix DigestEntry construction in agent.rs**

In `charradissa-core/src/farcaster/agent.rs`, there are two `DigestEntry { ... }` construction sites.

**Site 1** — deferred budget path (around line 106):
```rust
self.digest_buffer.lock().await.push(DigestEntry {
    project_id: event.project_id().clone(),
    connection_summary: format!("deferred (budget): {}", event.summary()),
    involved_projects: vec![event.project_id().clone()],
    concurrence: vec![],
    urgency: Urgency::Low,
    whispered_at: None,
    first_observed_at: Utc::now(),
});
```

**Site 2** — connection recording loop (around line 165):
```rust
digest.push(DigestEntry {
    project_id: conn.from_project.clone(),
    connection_summary: conn.summary.clone(),
    involved_projects: vec![conn.from_project, conn.to_project],
    concurrence,
    urgency: conn.urgency,
    whispered_at: if is_whispered { Some(now) } else { None },
    first_observed_at: now,
});
```

- [ ] **Step 3: Fix DigestEntry construction in tests**

In `charradissa-core/tests/farcaster_tests.rs`, there are 4 `DigestEntry { ... }` literals. Add `first_observed_at: chrono::Utc::now()` to each one.

**Test: tick_synthesizes_and_broadcasts_digest** (around line 318):
```rust
buf.push(DigestEntry {
    project_id: ProjectId::new("alpha"),
    connection_summary: "shared auth approach".into(),
    involved_projects: vec![ProjectId::new("alpha"), ProjectId::new("beta")],
    concurrence: vec![],
    urgency: Urgency::Medium,
    whispered_at: Some(chrono::Utc::now()),
    first_observed_at: chrono::Utc::now(),
});
```

**Test: tick_submits_to_farga_on_submit_verdict** (around line 352):
```rust
buf.push(DigestEntry {
    project_id: ProjectId::new("alpha"),
    connection_summary: "important pattern".into(),
    involved_projects: vec![ProjectId::new("alpha")],
    concurrence: vec![],
    urgency: Urgency::High,
    whispered_at: None,
    first_observed_at: chrono::Utc::now(),
});
```

**Test: tick_requeues_entries_on_farga_failure** (around line 409):
```rust
buf.push(DigestEntry {
    project_id: ProjectId::new("alpha"),
    connection_summary: "test entry".into(),
    involved_projects: vec![],
    concurrence: vec![],
    urgency: Urgency::Low,
    whispered_at: None,
    first_observed_at: chrono::Utc::now(),
});
```

**Test: digest_budget_exhausted_requeues_buffer_without_calling_analyzer** (around line 477):
```rust
buf.push(DigestEntry {
    project_id: ProjectId::new("alpha"),
    connection_summary: "pending entry".into(),
    involved_projects: vec![],
    concurrence: vec![],
    urgency: Urgency::Low,
    whispered_at: None,
    first_observed_at: chrono::Utc::now(),
});
```

- [ ] **Step 4: Run all tests**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core 2>&1 | tail -20
```
Expected: all 18 tests pass (15 existing + 3 governance from Task 2). Zero regressions.

- [ ] **Step 5: Commit**

```bash
cd /Users/bedardpl/project/Charradissa && git add charradissa-core/src/farcaster/analyzer.rs charradissa-core/src/farcaster/agent.rs charradissa-core/tests/farcaster_tests.rs && git commit -m "feat: add first_observed_at to DigestEntry for velocity tracking"
```

---

## Task 4: FargaWriter::submit_governance_contribution + wire run_tick

**Files:**
- Modify: `charradissa-core/src/farga.rs`
- Modify: `charradissa-core/src/farcaster/agent.rs`
- Modify: `charradissa-core/tests/farcaster_tests.rs`

- [ ] **Step 1: Write failing test**

Add to `charradissa-core/tests/farcaster_tests.rs`:
```rust
#[tokio::test]
async fn tick_emits_governance_contribution_on_submit_verdict() {
    let projects = vec![ProjectId::new("alpha"), ProjectId::new("beta")];
    let (agent, _, _, farga_calls, analyzer) = make_agent(projects, HashMap::new());

    {
        let mut buf = agent.digest_buffer.lock().await;
        buf.push(DigestEntry {
            project_id: ProjectId::new("alpha"),
            connection_summary: "shared auth pattern".into(),
            involved_projects: vec![ProjectId::new("alpha"), ProjectId::new("beta")],
            concurrence: vec![],
            urgency: Urgency::High,
            whispered_at: None,
            first_observed_at: chrono::Utc::now(),
        });
    }

    analyzer.queue_digest(DigestSynthesis {
        connections: vec!["alpha and beta both chose RS256".into()],
        lessons: vec!["Use RS256 org-wide".into()],
        open_questions: vec![],
        farga_verdict: "submit".into(),
        farga_title: Some("JWT Signing Pattern".into()),
        farga_narrative: Some("Two projects independently converged on RS256.".into()),
    }, 300);

    agent.tick().await.unwrap();

    let calls = farga_calls.lock().await;
    assert_eq!(calls.len(), 1);
    let (project_id, signals) = &calls[0];
    assert_eq!(project_id.as_str(), "system");
    assert_eq!(signals.len(), 1);
    // Default impl uses source "farcaster-governance" to distinguish governance submissions
    assert_eq!(signals[0].source, "farcaster-governance");
    // Content is a serialized GovernanceContribution
    let parsed: GovernanceContribution =
        serde_json::from_str(&signals[0].content).unwrap();
    assert_eq!(parsed.title, "JWT Signing Pattern");
    assert_eq!(parsed.event_count, 1);
    assert_eq!(parsed.target_layer, FargaLayer::ProjectLevel);
    assert!(parsed.reversibility.is_none());
    assert!(parsed.impact.is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core --test farcaster_tests tick_emits_governance 2>&1 | tail -10
```
Expected: compile error or test failure (method doesn't exist yet).

- [ ] **Step 3: Add submit_governance_contribution to FargaWriter trait**

In `charradissa-core/src/farga.rs`, add the import at the top (after existing use statements):
```rust
use crate::farcaster::governance::GovernanceContribution;
```

Add `submit_governance_contribution` as a default method to the `FargaWriter` trait (inside the trait block, after `recent_signals`):
```rust
async fn submit_governance_contribution(
    &self,
    contribution: GovernanceContribution,
) -> Result<()> {
    let content = serde_json::to_string(&contribution)
        .map_err(|e| crate::error::CharradissaError::Dispatch(e.to_string()))?;
    let signal = Signal {
        project: "system".to_string(),
        content,
        source: "farcaster-governance".to_string(),
    };
    self.write_signals(&ProjectId::new("system"), vec![signal]).await
}
```

The full updated `FargaWriter` trait looks like:
```rust
#[async_trait]
pub trait FargaWriter: Send + Sync {
    async fn write_signals(&self, project: &ProjectId, signals: Vec<Signal>) -> Result<()>;
    async fn recent_signals(&self, project: &ProjectId, since: Duration) -> Result<Vec<Signal>>;
    async fn submit_governance_contribution(
        &self,
        contribution: GovernanceContribution,
    ) -> Result<()> {
        let content = serde_json::to_string(&contribution)
            .map_err(|e| crate::error::CharradissaError::Dispatch(e.to_string()))?;
        let signal = Signal {
            project: "system".to_string(),
            content,
            source: "farcaster-governance".to_string(),
        };
        self.write_signals(&ProjectId::new("system"), vec![signal]).await
    }
}
```

- [ ] **Step 4: Update run_tick in agent.rs**

In `charradissa-core/src/farcaster/agent.rs`:

**Update the import line** (remove `Signal` and `DigestPayload` — both are unused after this change):
```rust
// Before:
use crate::farga::{FargaWriter, Signal};
// ...
use super::analyzer::{
    CrossSpaceSnapshot, DigestEntry, DigestPayload, DigestSynthesis, FarcasterAnalyzer,
    ProjectSnapshot,
};

// After:
use crate::farga::FargaWriter;
// ...
use super::analyzer::{
    CrossSpaceSnapshot, DigestEntry, DigestSynthesis, FarcasterAnalyzer, ProjectSnapshot,
};
```

**Add governance import** after the existing `use super::` lines:
```rust
use super::governance::{GovernanceContribution, FargaLayer};
```

**Replace the Farga submission block in run_tick** (the entire `if synthesis.farga_verdict == "submit" { ... }` block, currently lines 213–242):
```rust
if synthesis.farga_verdict == "submit" {
    let period_end = Utc::now();
    let period_start = *self.last_digest_at.lock().await;

    let first_observed_at = entries.iter()
        .map(|e| e.first_observed_at)
        .min()
        .unwrap_or(period_start);
    let last_observed_at = entries.iter()
        .map(|e| e.first_observed_at)
        .max()
        .unwrap_or(period_end);

    let involved_projects: Vec<ProjectId> = {
        let mut seen = std::collections::HashSet::new();
        entries.iter()
            .flat_map(|e| e.involved_projects.iter().cloned())
            .filter(|p| seen.insert(p.clone()))
            .collect()
    };

    let contribution = GovernanceContribution {
        title: synthesis.farga_title.clone().unwrap_or_default(),
        narrative: synthesis.farga_narrative.clone().unwrap_or_default(),
        lessons: synthesis.lessons.clone(),
        open_questions: synthesis.open_questions.clone(),
        involved_projects,
        concurrence: entries.iter()
            .flat_map(|e| e.concurrence.iter().cloned())
            .collect(),
        target_layer: FargaLayer::ProjectLevel,
        first_observed_at,
        last_observed_at,
        event_count: entries.len() as u32,
        reversibility: None,
        impact: None,
    };

    if let Err(e) = self.farga.submit_governance_contribution(contribution).await {
        tracing::error!("farcaster: governance submission failed, re-queuing: {}", e);
        self.digest_buffer.lock().await.extend(entries);
        return Ok(());
    }
}
```

- [ ] **Step 5: Fix the now-broken tick_submits_to_farga_on_submit_verdict test**

In `charradissa-core/tests/farcaster_tests.rs`, find `tick_submits_to_farga_on_submit_verdict` and update the source assertion from:
```rust
assert_eq!(signals[0].source, "farcaster");
```
to:
```rust
assert_eq!(signals[0].source, "farcaster-governance");
```
Also update the content assertion — the content is now a serialized `GovernanceContribution` (not a `DigestPayload`). Replace:
```rust
assert!(signals[0].content.contains("Replan Budget Lessons"),
    "content should include title");
```
with:
```rust
let parsed: GovernanceContribution =
    serde_json::from_str(&signals[0].content).unwrap();
assert_eq!(parsed.title, "Replan Budget Lessons");
```
Add `GovernanceContribution` and `FargaLayer` to the use statement at the top of the test file (already added in Task 2).

- [ ] **Step 6: Build and run all tests**

```bash
cd /Users/bedardpl/project/Charradissa && cargo build -p charradissa-core 2>&1 | tail -10
```
Expected: clean build.

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core 2>&1 | tail -25
```
Expected: 19 tests pass, 1 ignored. Tests:
- 15 from Farcaster Tasks 1–9 (tick_submits_to_farga_on_submit_verdict now updated)
- 3 from Task 2 of this plan (governance types)
- 1 new: tick_emits_governance_contribution_on_submit_verdict

- [ ] **Step 7: Commit**

```bash
cd /Users/bedardpl/project/Charradissa && git add charradissa-core/src/farga.rs charradissa-core/src/farcaster/agent.rs charradissa-core/tests/farcaster_tests.rs && git commit -m "feat: wire Farcaster to emit GovernanceContribution via submit_governance_contribution"
```

---

## Done

Plan 1 complete. The next plans in this series:

- **Plan 2 — Farga governance API:** `submit_governance_contribution` HTTP endpoint, `LibrarianAssessment` storage, precedent query, org config governance block
- **Plan 3 — Amassada governance canvas:** `RiskScore` computation, `SessionComposition`, tier → stance distribution, constitutional enforcement
- **Plan 4 — Integration wiring:** Room creation from `SessionComposition`, `GovernanceSessionState` lifecycle, moderator override logging
