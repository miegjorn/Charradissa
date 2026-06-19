# Governance Plan 4: Integration Wiring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the `SessionComposition` output from Plan 3 into runnable `Canvas` objects and a lifecycle state type that gates sessions on mission budget and threads the counter-session canvas through for High/Critical governance deliberations.

**Architecture:** Two new files inside the existing `governance/` module — `room.rs` owns all canvas-mutation logic (address parsing, participant substitution, budget scaling, moderator override with tracing); `state.rs` owns the lifecycle state machine (budget gate, active rooms). Neither file touches async code or HTTP; both are pure Rust with serde and tracing. The existing `MissionEngine` / `SessionRunner` infra runs the canvases without modification.

**Tech Stack:** Rust, serde (already in workspace), tracing (already in workspace), amassada-core crate at `/Users/bedardpl/project/Amassada`.

---

## Context for the Implementer

### What was built in Plans 1–3

The `governance/` module already has:
- `risk.rs` — `RiskTier`, `RiskFactors`, `RiskScore`, `compute_risk_score`
- `config.rs` — `GovernanceConfig` (parses `governance:` YAML block), `GovernanceBudgetConfig`, `TierMinimums`
- `composition.rs` — `SessionComposition`, `BudgetEnvelope`, `compose_session`, `check_constitution`
- `mod.rs` — re-exports all of the above
- `tests/governance_tests.rs` — 19 governance tests (all passing as of Plan 3)

Baseline: **54 tests passing**, 0 failed.

### Key types from the existing module

```rust
// governance/composition.rs
pub struct SessionComposition {
    pub risk_score: f32,
    pub tier: RiskTier,
    pub primary_session: Vec<String>,   // Fondament addresses e.g. "auth-service+adversarial"
    pub counter_session: Option<Vec<String>>,  // None at Low/Medium; Some at High/Critical
    pub budget: BudgetEnvelope,
    pub moderator_override: Option<String>,  // If Some, overrides the moderator domain
}

pub struct BudgetEnvelope {
    pub recommended_tokens: u32,
    pub minimum_tokens: u32,
}
```

```rust
// governance/config.rs
pub struct GovernanceConfig {
    pub risk_weights: RiskWeights,
    pub tier_thresholds: TierThresholds,
    pub budget: GovernanceBudgetConfig,
    pub tier_minimums: TierMinimums,
}
pub struct GovernanceBudgetConfig {
    pub daily_tokens: u32,
    pub per_session_cap: u32,
    pub counter_session_cap: u32,  // ← used as the counter canvas budget
}
```

### Address formats for Fondament

Two formats exist in `primary_session` and `counter_session` address lists:
- `"stances/realist"` — generic stance: persona = last path segment after `/`
- `"auth-service+adversarial"` — project-specific: persona = part after `+`
- `"stances/moderator"` — the moderator slot (always generic, `is_moderator() == true`)

Moderator override (when `composition.moderator_override = Some(addr)`) replaces the domain field of any participant whose persona is `"moderator"`, leaving the persona itself unchanged.

### Canvas and ParticipantDef (canvas.rs)

```rust
pub struct Canvas {
    pub id: String,
    pub version: String,
    pub mode: CanvasMode,
    pub selector: SelectorMeta,
    pub initial_participants: Vec<ParticipantDef>,
    pub budget: BudgetConfig,
    pub consultation: ConsultationConfig,
    pub rounds: RoundsConfig,
    pub human: HumanConfig,
    pub output: OutputConfig,
}

pub struct ParticipantDef {
    pub persona: String,
    pub domain: String,
    pub model: Option<String>,
    pub authority: Option<String>,
}

impl ParticipantDef {
    pub fn is_moderator(&self) -> bool { self.persona == "moderator" }
}
```

### MissionBudget (mission/types.rs)

```rust
pub struct MissionBudget {
    pub total_tokens: u64,
    pub discretionary: u64,
    pub discretionary_strategize_spent: u64,
    pub discretionary_evaluate_spent: u64,
    pub deployable: u64,
    pub deployable_spent: u64,
}

impl MissionBudget {
    pub fn deployable_remaining(&self) -> u64 {
        self.deployable.saturating_sub(self.deployable_spent)
    }
}
```

### scale_canvas_budget (mission/session_runner.rs)

```rust
pub fn scale_canvas_budget(canvas: Canvas, budget_tokens: u64) -> Canvas
```

Scales all pool values proportionally to a new total. Already pub — import via `crate::mission::session_runner::scale_canvas_budget`.

---

## Task 1: Canvas composition from SessionComposition (`governance/room.rs`)

**Files:**
- Create: `Amassada/crates/amassada-core/src/governance/room.rs`
- Modify: `Amassada/crates/amassada-core/src/governance/mod.rs`
- Modify: `Amassada/crates/amassada-core/tests/governance_tests.rs`

### What `room.rs` does

**`address_to_participant(addr: &str) -> ParticipantDef`** — parses a Fondament address string into a `ParticipantDef`:
- `"stances/realist"` → persona: `"realist"`, domain: `"stances/realist"`
- `"auth-service+adversarial"` → persona: `"adversarial"`, domain: `"auth-service+adversarial"`
- Rule: if `+` is present, persona = substring after last `+`; else persona = substring after last `/`; fallback to the full string

**`compose_governance_canvas(base: Canvas, composition: &SessionComposition) -> Canvas`** — produces a canvas ready to run:
1. Replace `initial_participants` from `composition.primary_session` addresses using `address_to_participant`
2. Scale budget to `composition.budget.recommended_tokens` using `scale_canvas_budget`
3. If `composition.moderator_override = Some(addr)`: replace the domain of any participant whose persona is `"moderator"` with `addr`, and emit `tracing::info!` with old/new domain

- [ ] **Step 1: Write failing tests**

Append to `Amassada/crates/amassada-core/tests/governance_tests.rs`:

```rust
use amassada_core::governance::{address_to_participant, compose_governance_canvas};
use amassada_core::canvas::Canvas;

fn governance_canvas() -> Canvas {
    Canvas::from_yaml(include_str!("../../../canvases/stdlib/governance-deliberation.yaml"))
        .unwrap()
}

fn make_composition_for_room(
    primary: Vec<String>,
    counter: Option<Vec<String>>,
    override_addr: Option<String>,
    recommended: u32,
    minimum: u32,
) -> amassada_core::governance::SessionComposition {
    amassada_core::governance::SessionComposition {
        risk_score: 0.5,
        tier: amassada_core::governance::RiskTier::Medium,
        primary_session: primary,
        counter_session: counter,
        budget: amassada_core::governance::BudgetEnvelope { recommended_tokens: recommended, minimum_tokens: minimum },
        moderator_override: override_addr,
    }
}

#[test]
fn address_to_participant_parses_generic_stance() {
    let p = address_to_participant("stances/realist");
    assert_eq!(p.persona, "realist");
    assert_eq!(p.domain, "stances/realist");
    assert!(p.model.is_none());
    assert!(p.authority.is_none());
}

#[test]
fn address_to_participant_parses_project_specific() {
    let p = address_to_participant("auth-service+adversarial");
    assert_eq!(p.persona, "adversarial");
    assert_eq!(p.domain, "auth-service+adversarial");
}

#[test]
fn address_to_participant_parses_moderator_slot() {
    let p = address_to_participant("stances/moderator");
    assert!(p.is_moderator());
    assert_eq!(p.domain, "stances/moderator");
}

#[test]
fn compose_governance_canvas_replaces_participants() {
    let base = governance_canvas();
    let primary = vec![
        "stances/realist".into(),
        "stances/adversarial".into(),
        "stances/moderator".into(),
    ];
    let comp = make_composition_for_room(primary, None, None, 5000, 2000);
    let result = compose_governance_canvas(base, &comp);
    assert_eq!(result.initial_participants.len(), 3);
    assert_eq!(result.initial_participants[0].persona, "realist");
    assert_eq!(result.initial_participants[1].persona, "adversarial");
    assert!(result.initial_participants[2].is_moderator());
}

#[test]
fn compose_governance_canvas_scales_budget() {
    let base = governance_canvas(); // total_tokens = 15000
    let comp = make_composition_for_room(vec!["stances/moderator".into()], None, None, 5000, 2000);
    let result = compose_governance_canvas(base, &comp);
    assert_eq!(result.budget.total_tokens, 5000);
}

#[test]
fn compose_governance_canvas_applies_moderator_override() {
    let base = governance_canvas();
    let comp = make_composition_for_room(
        vec!["stances/realist".into(), "stances/moderator".into()],
        None,
        Some("special-projects+moderator".into()),
        5000,
        2000,
    );
    let result = compose_governance_canvas(base, &comp);
    let mod_p = result.initial_participants.iter().find(|p| p.is_moderator()).unwrap();
    // Domain overridden, persona unchanged
    assert_eq!(mod_p.domain, "special-projects+moderator");
    assert_eq!(mod_p.persona, "moderator");
}

#[test]
fn compose_governance_canvas_no_override_keeps_original_moderator_domain() {
    let base = governance_canvas();
    let comp = make_composition_for_room(
        vec!["stances/moderator".into()],
        None,
        None,
        5000,
        2000,
    );
    let result = compose_governance_canvas(base, &comp);
    let mod_p = result.initial_participants.iter().find(|p| p.is_moderator()).unwrap();
    assert_eq!(mod_p.domain, "stances/moderator");
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cd /Users/bedardpl/project/Amassada && cargo test -p amassada-core governance 2>&1 | grep -E "^error|FAILED" | head -10
```

Expected: compile errors — `address_to_participant` and `compose_governance_canvas` don't exist yet.

- [ ] **Step 3: Create governance/room.rs**

Create `Amassada/crates/amassada-core/src/governance/room.rs`:

```rust
use crate::canvas::ParticipantDef;
use crate::canvas::Canvas;
use crate::governance::composition::SessionComposition;
use crate::mission::session_runner::scale_canvas_budget;

pub fn address_to_participant(addr: &str) -> ParticipantDef {
    let persona = if let Some(pos) = addr.rfind('+') {
        &addr[pos + 1..]
    } else if let Some(pos) = addr.rfind('/') {
        &addr[pos + 1..]
    } else {
        addr
    };
    ParticipantDef {
        persona: persona.to_string(),
        domain: addr.to_string(),
        model: None,
        authority: None,
    }
}

pub fn compose_governance_canvas(mut base: Canvas, composition: &SessionComposition) -> Canvas {
    base.initial_participants = composition.primary_session
        .iter()
        .map(|addr| address_to_participant(addr))
        .collect();

    base = scale_canvas_budget(base, composition.budget.recommended_tokens as u64);

    if let Some(override_addr) = &composition.moderator_override {
        for p in &mut base.initial_participants {
            if p.is_moderator() {
                tracing::info!(
                    old_domain = %p.domain,
                    new_domain = %override_addr,
                    "governance moderator override applied"
                );
                p.domain = override_addr.clone();
            }
        }
    }

    base
}
```

- [ ] **Step 4: Add pub mod room to governance/mod.rs**

Read `Amassada/crates/amassada-core/src/governance/mod.rs` first, then add:

```rust
pub mod room;
```

And add to the re-exports:

```rust
pub use room::{address_to_participant, compose_governance_canvas};
```

- [ ] **Step 5: Run tests**

```bash
cd /Users/bedardpl/project/Amassada && cargo test -p amassada-core governance 2>&1 | tail -10
```

Expected: all prior governance tests + 7 new room tests pass (54 + 7 = 61 total across the suite).

- [ ] **Step 6: Commit**

```bash
cd /Users/bedardpl/project/Amassada && git add crates/amassada-core/src/governance/room.rs crates/amassada-core/src/governance/mod.rs crates/amassada-core/tests/governance_tests.rs && git commit -m "feat: add governance canvas composition and moderator override"
```

---

## Task 2: GovernanceSessionState + budget gating (`governance/state.rs`)

**Files:**
- Create: `Amassada/crates/amassada-core/src/governance/state.rs`
- Modify: `Amassada/crates/amassada-core/src/governance/mod.rs`
- Modify: `Amassada/crates/amassada-core/tests/governance_tests.rs`

### What `state.rs` does

**`GovernanceRoomSet`** — holds the canvas(es) ready to run:
```rust
pub struct GovernanceRoomSet {
    pub primary: Canvas,
    pub counter: Option<Canvas>,
}
```

**`GovernanceSessionState`** — lifecycle state:
```rust
pub enum GovernanceSessionState {
    PendingBudget {
        composition: SessionComposition,
        shortfall: u32,
    },
    Active {
        composition: SessionComposition,
        rooms: GovernanceRoomSet,
    },
}
```

**`init_governance_state`** — budget gate then canvas assembly:
1. If `mission_budget.deployable_remaining() < composition.budget.minimum_tokens as u64` → `PendingBudget { shortfall: gap_as_u32 }`
2. Otherwise build `primary` canvas via `compose_governance_canvas`
3. If `composition.counter_session = Some(addrs)`: build `counter` canvas — start with `base_canvas.clone()`, replace participants from `addrs`, scale budget to `config.budget.counter_session_cap`
4. Return `Active { composition, rooms: GovernanceRoomSet { primary, counter } }`

- [ ] **Step 1: Write failing tests**

Append to `Amassada/crates/amassada-core/tests/governance_tests.rs`:

```rust
use amassada_core::governance::{GovernanceSessionState, init_governance_state};
use amassada_core::mission::types::MissionBudget;

fn mission_budget_with_remaining(remaining: u64) -> MissionBudget {
    // Total chosen so that deployable (80%) equals remaining after spending the rest
    // deployable = total * 0.8, so total = remaining / 0.8
    // Simpler: set total to a large value and spend down
    let mut b = MissionBudget::new(100_000);
    // deployable = 80_000; spend (80_000 - remaining)
    b.deployable_spent = 80_000u64.saturating_sub(remaining);
    b
}

fn low_tier_comp() -> amassada_core::governance::SessionComposition {
    amassada_core::governance::SessionComposition {
        risk_score: 0.20,
        tier: amassada_core::governance::RiskTier::Low,
        primary_session: vec![
            "stances/realist".into(),
            "stances/realist".into(),
            "stances/moderator".into(),
        ],
        counter_session: None,
        budget: amassada_core::governance::BudgetEnvelope {
            recommended_tokens: 4000,
            minimum_tokens: 2000,
        },
        moderator_override: None,
    }
}

fn high_tier_comp() -> amassada_core::governance::SessionComposition {
    amassada_core::governance::SessionComposition {
        risk_score: 0.65,
        tier: amassada_core::governance::RiskTier::High,
        primary_session: vec![
            "stances/adversarial".into(),
            "stances/adversarial".into(),
            "stances/realist".into(),
            "stances/moderator".into(),
        ],
        counter_session: Some(vec![
            "stances/builder".into(),
            "stances/dreamer".into(),
        ]),
        budget: amassada_core::governance::BudgetEnvelope {
            recommended_tokens: 15000,
            minimum_tokens: 8000,
        },
        moderator_override: None,
    }
}

#[test]
fn init_governance_state_pending_when_budget_short() {
    let budget = mission_budget_with_remaining(1000); // 1000 remaining
    let comp = low_tier_comp(); // minimum_tokens = 2000
    let state = init_governance_state(comp, &budget, governance_canvas(), &default_config());
    assert!(matches!(state, GovernanceSessionState::PendingBudget { .. }),
        "expected PendingBudget when remaining < minimum");
}

#[test]
fn init_governance_state_shortfall_is_correct() {
    let budget = mission_budget_with_remaining(1000); // 1000 remaining
    let comp = low_tier_comp(); // minimum_tokens = 2000; shortfall = 1000
    let state = init_governance_state(comp, &budget, governance_canvas(), &default_config());
    if let GovernanceSessionState::PendingBudget { shortfall, .. } = state {
        assert_eq!(shortfall, 1000, "shortfall = minimum - remaining = 2000 - 1000 = 1000");
    } else {
        panic!("expected PendingBudget");
    }
}

#[test]
fn init_governance_state_active_when_budget_sufficient() {
    let budget = mission_budget_with_remaining(10_000); // 10000 remaining
    let comp = low_tier_comp(); // minimum_tokens = 2000
    let state = init_governance_state(comp, &budget, governance_canvas(), &default_config());
    assert!(matches!(state, GovernanceSessionState::Active { .. }),
        "expected Active when remaining >= minimum");
}

#[test]
fn init_governance_state_exactly_at_minimum_is_active() {
    let budget = mission_budget_with_remaining(2000); // exactly minimum
    let comp = low_tier_comp(); // minimum_tokens = 2000
    let state = init_governance_state(comp, &budget, governance_canvas(), &default_config());
    assert!(matches!(state, GovernanceSessionState::Active { .. }),
        "exactly meeting minimum should be Active, not PendingBudget");
}

#[test]
fn init_governance_state_no_counter_for_low_tier() {
    let budget = mission_budget_with_remaining(10_000);
    let comp = low_tier_comp(); // No counter_session
    let state = init_governance_state(comp, &budget, governance_canvas(), &default_config());
    if let GovernanceSessionState::Active { rooms, .. } = state {
        assert!(rooms.counter.is_none(), "Low tier must have no counter canvas");
    } else {
        panic!("expected Active");
    }
}

#[test]
fn init_governance_state_has_counter_for_high_tier() {
    let budget = mission_budget_with_remaining(20_000);
    let comp = high_tier_comp(); // Has counter_session: builder, dreamer
    let state = init_governance_state(comp, &budget, governance_canvas(), &default_config());
    if let GovernanceSessionState::Active { rooms, .. } = state {
        assert!(rooms.counter.is_some(), "High tier must have a counter canvas");
        let counter = rooms.counter.unwrap();
        assert_eq!(
            counter.initial_participants.len(), 2,
            "Counter canvas should have builder and dreamer"
        );
        assert_eq!(counter.initial_participants[0].persona, "builder");
        assert_eq!(counter.initial_participants[1].persona, "dreamer");
    } else {
        panic!("expected Active");
    }
}

#[test]
fn init_governance_state_primary_has_correct_participant_count() {
    let budget = mission_budget_with_remaining(20_000);
    let comp = high_tier_comp(); // 4 primary slots
    let state = init_governance_state(comp, &budget, governance_canvas(), &default_config());
    if let GovernanceSessionState::Active { rooms, .. } = state {
        assert_eq!(rooms.primary.initial_participants.len(), 4,
            "High tier primary: adversarial, adversarial, realist, moderator");
    } else {
        panic!("expected Active");
    }
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cd /Users/bedardpl/project/Amassada && cargo test -p amassada-core governance 2>&1 | grep -E "^error|FAILED" | head -10
```

Expected: compile errors — `GovernanceSessionState` and `init_governance_state` don't exist yet.

- [ ] **Step 3: Create governance/state.rs**

Create `Amassada/crates/amassada-core/src/governance/state.rs`:

```rust
use crate::canvas::Canvas;
use crate::governance::composition::SessionComposition;
use crate::governance::config::GovernanceConfig;
use crate::governance::room::{address_to_participant, compose_governance_canvas};
use crate::mission::session_runner::scale_canvas_budget;
use crate::mission::types::MissionBudget;

pub struct GovernanceRoomSet {
    pub primary: Canvas,
    pub counter: Option<Canvas>,
}

pub enum GovernanceSessionState {
    PendingBudget {
        composition: SessionComposition,
        shortfall: u32,
    },
    Active {
        composition: SessionComposition,
        rooms: GovernanceRoomSet,
    },
}

pub fn init_governance_state(
    composition: SessionComposition,
    mission_budget: &MissionBudget,
    base_canvas: Canvas,
    config: &GovernanceConfig,
) -> GovernanceSessionState {
    let minimum = composition.budget.minimum_tokens as u64;
    let remaining = mission_budget.deployable_remaining();

    if remaining < minimum {
        let shortfall = (minimum - remaining).min(u32::MAX as u64) as u32;
        return GovernanceSessionState::PendingBudget { composition, shortfall };
    }

    let primary = compose_governance_canvas(base_canvas.clone(), &composition);

    let counter = composition.counter_session.as_ref().map(|addrs| {
        let mut c = base_canvas.clone();
        c.initial_participants = addrs.iter().map(|a| address_to_participant(a)).collect();
        scale_canvas_budget(c, config.budget.counter_session_cap as u64)
    });

    GovernanceSessionState::Active {
        composition,
        rooms: GovernanceRoomSet { primary, counter },
    }
}
```

- [ ] **Step 4: Add pub mod state to governance/mod.rs**

Read `Amassada/crates/amassada-core/src/governance/mod.rs` first. The file currently has `pub mod risk`, `pub mod config`, `pub mod composition`, `pub mod room` (added in Task 1). Add:

```rust
pub mod state;
```

And add to re-exports:

```rust
pub use state::{GovernanceRoomSet, GovernanceSessionState, init_governance_state};
```

- [ ] **Step 5: Add MissionBudget to mission re-exports so tests can import it**

Check if `MissionBudget` is accessible via `amassada_core::mission::types::MissionBudget`. It should be — `crate::mission` is a pub mod in lib.rs, and `types` is a pub mod inside mission. No changes needed to lib.rs.

- [ ] **Step 6: Run tests**

```bash
cd /Users/bedardpl/project/Amassada && cargo test -p amassada-core 2>&1 | grep "test result"
```

Expected: all prior tests + 7 (Task 1) + 7 (Task 2) = 68 total passing.

If you get a compile error about `GovernanceRoomSet` or `GovernanceSessionState` not being Debug/Clone, that's fine — they don't need those derives for the tests. The tests only use `matches!()` and field access.

If you get a compile error about `counter_session` being moved out of `composition` while `composition` is used after: restructure to extract the counter address list before consuming `composition`:

```rust
// If the borrow checker complains, restructure:
let counter_addrs = composition.counter_session.clone();  // clone the addresses
let primary = compose_governance_canvas(base_canvas.clone(), &composition);
let counter = counter_addrs.map(|addrs| {
    let mut c = base_canvas.clone();
    c.initial_participants = addrs.iter().map(|a| address_to_participant(a)).collect();
    scale_canvas_budget(c, config.budget.counter_session_cap as u64)
});
GovernanceSessionState::Active { composition, rooms: GovernanceRoomSet { primary, counter } }
```

- [ ] **Step 7: Run full suite to confirm no regressions**

```bash
cd /Users/bedardpl/project/Amassada && cargo test -p amassada-core 2>&1 | tail -5
```

Expected: 0 failures.

- [ ] **Step 8: Commit**

```bash
cd /Users/bedardpl/project/Amassada && git add crates/amassada-core/src/governance/state.rs crates/amassada-core/src/governance/mod.rs crates/amassada-core/tests/governance_tests.rs && git commit -m "feat: add GovernanceSessionState with budget gating and counter-session rooms"
```

---

## Self-Review

**Spec coverage:**

| Spec requirement | Covered by |
|---|---|
| Room creation from `SessionComposition` | Task 1 `compose_governance_canvas` |
| `GovernanceSessionState` lifecycle | Task 2 `GovernanceSessionState` enum |
| Budget gating (deployable_remaining < minimum → PendingBudget) | Task 2 `init_governance_state` |
| Moderator override logging (tracing::info!) | Task 1 in `compose_governance_canvas` |
| Counter-session canvas at High/Critical | Task 2 `GovernanceRoomSet.counter` |
| Address parsing (generic stance, project-specific) | Task 1 `address_to_participant` |

**Placeholder scan:** None found — all steps have complete code.

**Type consistency check:**
- `GovernanceRoomSet { primary: Canvas, counter: Option<Canvas> }` defined in Task 2 state.rs → used in `GovernanceSessionState::Active` in the same file → accessed in Task 2 tests via field access — consistent
- `init_governance_state(composition, mission_budget, base_canvas, config)` defined and called with same signature in tests — consistent
- `compose_governance_canvas` defined in Task 1 room.rs → called from Task 2 state.rs → consistent
- `address_to_participant` defined in Task 1 → imported via `crate::governance::room` in state.rs → consistent
- `mission_budget_with_remaining` helper in tests creates a `MissionBudget` with `deployable_spent = 80_000 - remaining` — note that `MissionBudget::new(100_000)` sets `deployable = 80_000`. If `remaining > 80_000` then `80_000.saturating_sub(remaining) = 0` (no spend). Tests use values ≤ 80_000 so this is safe.
- The `governance_canvas()` test helper defined in Task 1 tests uses `include_str!("../../../canvases/stdlib/governance-deliberation.yaml")` — Task 2 tests reuse this helper (it's already in scope in the same file) — no redefinition needed
- `default_config()` also defined in Task 2 tests appended section — check that it's not already defined from an earlier governance test; if it is, do NOT redefine it, just use the existing one

**Note on counter canvas budget:** The counter canvas is scaled to `config.budget.counter_session_cap` (default: 10_000). The primary canvas is scaled to `composition.budget.recommended_tokens`. This gives the counter session a fixed cap independent of the primary's dynamic budget — intentional, matching the design.

**Note on `default_config()` in tests:** The test helpers `governance_canvas()` and `default_config()` were defined in Task 1 (room tests). When adding Task 2 tests, read the existing test file first to check if these helpers are already defined. Do NOT redefine them — just use them. Add `use` imports only for types not yet in scope.
