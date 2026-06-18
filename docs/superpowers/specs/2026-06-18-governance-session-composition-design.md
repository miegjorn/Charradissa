# Governance Session Composition

**Date:** 2026-06-18
**Status:** Draft
**Scope:** Cross-stack — Fondament (stance definitions), Farga (GovernanceContribution, LibrarianAssessment, org config), Amassada (governance canvas, SessionComposition, MissionEngine), Charradissa/Farcaster (submission enrichment, DigestEntry)

---

## Motivation

Farcaster can now submit cross-space lessons to Farga. But the path from "contribution received" to "governance session running" is undefined. Who decides the session shape? Who enforces minimum deliberation quality? Who gates on budget?

This spec answers those questions with a risk-scored, constitutionally-enforced session composition pipeline. The core principle: session composition is a function of measurable risk factors derived from the contribution itself — not hardcoded, not left to moderator discretion. The moderator focuses on goal and strategy; the system handles compositional calibration.

---

## New Concepts

### GovernanceContribution

A structured contribution type distinct from the raw `Signal` write. Used exclusively for patterns that may warrant archetype evolution — Farcaster submits one when `farga_verdict == "submit"` and the contribution carries lessons that could reshape Fondament definitions.

`Signal` writes continue for routine Farga enrichment (session artifacts, project context updates). `GovernanceContribution` is the governance-path type.

```rust
pub struct GovernanceContribution {
    pub title: String,
    pub narrative: String,
    pub lessons: Vec<String>,
    pub open_questions: Vec<String>,
    pub involved_projects: Vec<ProjectId>,
    pub concurrence: Vec<AgentConcurrence>,
    // Risk metadata — set by Farcaster at submission time
    pub target_layer: FargaLayer,
    pub first_observed_at: DateTime<Utc>,
    pub last_observed_at: DateTime<Utc>,
    pub event_count: u32,
    // Set to None at submission — filled by LibrarianAssessment
    pub reversibility: Option<ReversibilityLevel>,
    pub impact: Option<ImpactScope>,
}

pub enum FargaLayer { OrgLevel, InitiativeLevel, ProjectLevel }
```

### ReversibilityLevel

Assessed by the Farga librarian during preliminary evaluation. Farcaster cannot determine this — only the librarian can, reading the contribution against existing Farga context.

| Level | Meaning |
|---|---|
| `FullyReversible` | Clean definition revert, no downstream effects already in flight |
| `EffectsLinger` | Definition reverts cleanly, but agent behaviors shaped by it persist in active sessions |
| `CostlyReversible` | Revert requires coordinated changes across multiple project Farga contexts |
| `Irreversible` | Effects are baked into produced artifacts, external outputs, or approved MRs |

### ImpactScope

Also assessed by the librarian. The `extends` chain in Fondament is the key signal: a change to a widely-inherited definition has `DomainWide` or `OrgWide` impact even if the target layer appears local.

| Level | Meaning |
|---|---|
| `Contained` | One project, no cross-cutting effects |
| `CrossProject` | Multiple projects from the concurrence list — limited blast radius |
| `DomainWide` | All projects in a domain (e.g. all engineering, all data); typically triggered via Fondament `extends` chains |
| `OrgWide` | Org-level archetypes — all agent resolutions in the system are affected |

### LibrarianAssessment

The output of the Farga preliminary evaluation stage. Enriches the `GovernanceContribution` before risk scoring can proceed.

```rust
pub struct LibrarianAssessment {
    pub reversibility: ReversibilityLevel,
    pub impact: ImpactScope,
    pub routing: LibrarianRouting,
    pub notes: Option<String>,
}

pub enum LibrarianRouting {
    DirectIntegrate,   // low impact, fully reversible — no governance session needed
    OpenGovernance,    // risk scoring proceeds, session composition computed
    Reject { reason: String },
}
```

### RiskFactors + RiskScore

Six factors, three sourced from Farcaster at submission time, two from `LibrarianAssessment`, one from a Farga history query.

| Factor | Source | Notes |
|---|---|---|
| `primitive_proximity` | `GovernanceContribution.target_layer` | OrgLevel = 1.0, InitiativeLevel = 0.6, ProjectLevel = 0.2 |
| `signal_concurrence` | `AgentConcurrence` count | Normalized against org config threshold |
| `signal_velocity` | `(last_observed_at - first_observed_at) / event_count` | Inverted — fast spikes score higher risk |
| `reversibility` | `LibrarianAssessment` | Irreversible = 1.0, CostlyReversible = 0.7, EffectsLinger = 0.4, FullyReversible = 0.0 |
| `impact` | `LibrarianAssessment` | OrgWide = 1.0, DomainWide = 0.7, CrossProject = 0.4, Contained = 0.1 |
| `precedent` | Farga rejection history query | Higher rejection count for similar patterns = higher risk score |

Each factor normalizes to [0, 1]. The `RiskScore` is a weighted sum using org-configured weights. Two hard floor overrides bypass the aggregate:

- `impact == OrgWide` → minimum tier **Critical** regardless of weighted sum
- `reversibility == Irreversible` → minimum tier **High** regardless of weighted sum

### SessionComposition

Produced by Amassada's governance canvas at session creation time. Consumed by Charradissa for Matrix room setup.

```rust
pub struct SessionComposition {
    pub risk_score: f32,
    pub tier: RiskTier,
    pub primary_session: Vec<CompositionAddress>,
    pub counter_session: Option<Vec<CompositionAddress>>,
    pub budget: BudgetEnvelope,
    pub moderator_override: Option<String>,  // logged justification if composition overridden
}

pub struct BudgetEnvelope {
    pub recommended_tokens: u32,
    pub minimum_tokens: u32,
}

pub enum RiskTier { Low, Medium, High, Critical }
```

---

## Stance Distribution by Tier

| Tier | Score | Session shape |
|---|---|---|
| `Low` | < 0.30 | Realist-dominant, light budget, single session |
| `Medium` | 0.30–0.55 | Balanced realist + adversarial, one builder, single session |
| `High` | 0.55–0.80 | Adversarial-dominant primary → mandatory counter-session (builder + dreamer) |
| `Critical` | > 0.80 or floor override | Full spectrum (all stances), extended budget, both sessions |

### Counter-Session Mechanism

A High-adversarial session can produce paralysis — everything surfaces as risk, nothing gets approved. The counter-session is the structural answer.

**Primary session** (adversarial-dominant): surfaces risks, failure modes, and objections.

**Counter-session** (builder + dreamer): receives the primary session output as context. Responds specifically to the surfaced risks with constructive proposals and alternatives.

The human MR sees both outputs side by side. This prevents adversarial paralysis without suppressing legitimate concerns.

The counter-session is a second `SessionPlan` entry in the governance `MissionEngine` run. The meta-moderator allocates budget across both upfront. It is **not** optional at High/Critical tiers — it is constitutionally required.

---

## Agent Slot Resolution

For each stance slot, the resolver fills from the contribution's `involved_projects` first:

```
involved_projects: [auth-service, api-gateway]
High tier primary → 2 adversarial slots, 1 realist slot

Slot resolution:
  adversarial[0] → auth-service+adversarial
  adversarial[1] → api-gateway+adversarial
  realist[0]     → fondament/roles/senior-engineer+realist  (fallback — no third project)
```

Affected projects are represented in the session that debates their contribution. Generic Fondament role agents fill remaining slots when involved projects are exhausted.

---

## Two-Stage Governance Pipeline

```
Farcaster submits GovernanceContribution
    (reversibility: None, impact: None)
         ↓
Stage 1: Farga librarian preliminary assessment
    - Reads contribution + existing Farga context
    - Assesses reversibility + impact
    - Routes: DirectIntegrate | OpenGovernance | Reject
         ↓ (if OpenGovernance)
Stage 2: Risk scoring + session composition
    - Farga queries rejection history (precedent)
    - Amassada governance canvas receives contribution + LibrarianAssessment
    - Computes RiskFactors → RiskScore → RiskTier
    - Resolves stance slots → SessionComposition
    - Checks budget availability
         ↓
Stage 3: Governance MissionEngine run
    - SessionPlan[0]: primary debate (adversarial-dominant)
    - SessionPlan[1]: counter-session if High/Critical tier
    - Output: structured MR document (proposed change + agent votes + impact analysis)
         ↓
Stage 4: Human approval
    - MR on Farga repo
    - Human approves or rejects
    - If approved: Fondament definition updated, affected projects notified
```

### GovernanceSessionState

```rust
pub enum GovernanceSessionState {
    Queued,            // contribution received, pending librarian assessment
    PendingBudget,     // composition computed, budget pool insufficient — deferred
    Scheduled,         // budget confirmed, sessions ready to spawn
    Running,           // mission in progress
    AwaitingHuman,     // MR open, waiting for approval
    Closed(CloseReason),
}

pub enum CloseReason { Approved, Rejected, Deferred }
```

`PendingBudget` is visible to the human — they can see what's queued and why it hasn't started. A manual budget exception (itself a logged governance event in Farga) can unblock it.

---

## Budget Integration

The governance session is a `MissionEngine` run. No new budget machinery — it rides the existing `MissionBudget` / `BudgetLedger` rails.

- `GovernanceBudgetPool` → `MissionBudget.total_tokens`, sourced from Farga org config
- Per-session caps → `budget_slice` in each `SessionPlan`
- The existing `deployable_remaining()` check gates session spawning — `PendingBudget` state = that check returned false
- `BudgetLedger` pools (`MainSession`, `Consultations`, `ModWhisper`) account within each session as normal

If budget is insufficient for the tier's `minimum_tokens`: defer to `PendingBudget`. Never degrade silently to a lower tier — the human should know a session is waiting.

---

## Configurable Weights

Stored as structured data in the Farga org-level context. Read by Amassada at governance session creation. The org tunes its own risk sensitivity through the same governance cycle these weights govern.

```yaml
governance:
  risk_weights:
    primitive_proximity: 0.25
    signal_concurrence: 0.20
    signal_velocity: 0.15
    reversibility: 0.20
    impact: 0.15
    precedent: 0.05
  tier_thresholds:
    medium: 0.30
    high: 0.55
    critical: 0.80
  budget:
    daily_tokens: 50_000
    per_session_cap: 15_000
    counter_session_cap: 10_000
  tier_minimums:
    low: 2_000
    medium: 5_000
    high: 8_000
    critical: 12_000
```

Weights must sum to 1.0. Farga validates this on write.

---

## Moderator Authority

**The moderator controls:**
- Which specific agents (by `CompositionAddress`) fill each stance slot
- Budget allocation between primary and counter sessions
- Whether to merge or split a contribution before it enters the cycle
- Composition override (with logged justification in Farga)

**The moderator does not control:**
- The risk score computation — system-derived, not negotiable
- Minimum stance diversity at each tier — constitutionally enforced by Amassada at session creation
- Whether a counter-session is required at High/Critical — structural, not optional

---

## Prerequisites

Three things must exist before implementation begins:

### 1. Missing stance definitions (Fondament)

Four YAML files in `Fondament/definitions/stances/`:

**`builder.yaml`**
```yaml
id: stances/builder
kind: stance
context: |
  Construct solutions. Your role is to find the path forward — make things
  work, surface viable alternatives, build on what exists. Criticism without
  a proposed alternative is incomplete.
```

**`realist.yaml`**
```yaml
id: stances/realist
kind: stance
context: |
  Assess feasibility. Identify real constraints, cut scope to what can
  actually be delivered, and distinguish genuine blockers from hypothetical
  concerns. Your role is clarity, not pessimism.
```

**`dreamer.yaml`**
```yaml
id: stances/dreamer
kind: stance
context: |
  Explore without constraint. Generate alternatives, challenge assumptions
  about what is fixed, think past current limits. Your role is to expand
  the solution space before it contracts.
```

**`moderator.yaml`**
```yaml
id: stances/moderator
kind: stance
context: |
  Hold the process. Balance voices, ensure all perspectives surface, and
  synthesize without advocating for any position. Your role is the quality
  of the conversation, not its conclusion.
```

### 2. `first_observed_at` on `DigestEntry`

One field addition to `charradissa-core/src/farcaster/analyzer.rs`:

```rust
pub struct DigestEntry {
    // ... existing fields ...
    pub first_observed_at: DateTime<Utc>,  // when this pattern first entered the event buffer
}
```

Set by `handle_milestone()` in `FarcasterAgent` when the pattern first appears, not updated on subsequent observations.

### 3. Farga org context governance block

The YAML structure above must be readable from Farga's org layer. Requires Farga to support structured YAML in org context (vs. freeform text), or a dedicated `governance_config` field on the org record.

---

## What Does Not Change

- `FargaWriter` trait — `write_signals()` continues for routine enrichment. `GovernanceContribution` uses a separate write path (`submit_governance_contribution()`).
- `BudgetLedger` and `MissionBudget` — governance sessions use existing machinery unchanged.
- `MissionEngine` — governance sessions are standard `MissionEngine` runs with a governance canvas.
- `FarcasterAgent` reactive path — unchanged. Only the digest path submission enriches to `GovernanceContribution`.
- The human approval gate — unchanged. No system path to accept an archetype change without a human MR approval.

---

## Testing

- **`RiskScore` computation** — unit test each factor normalization and the weighted sum; test both hard floor overrides force correct minimum tier.
- **`SessionComposition` slot resolution** — test affected-project-first filling with and without enough involved projects to fill all slots.
- **Budget gating** — test that `PendingBudget` state is produced when `deployable_remaining()` < `minimum_tokens` for the tier.
- **Counter-session requirement** — test that High/Critical tiers always produce `counter_session: Some(...)` and Low/Medium produce `counter_session: None`.
- **Configurable weights** — test that changing weights in org config changes tier assignment for the same raw factors.
- **`LibrarianAssessment` routing** — test `DirectIntegrate` path skips risk scoring; `Reject` path closes the session immediately.
