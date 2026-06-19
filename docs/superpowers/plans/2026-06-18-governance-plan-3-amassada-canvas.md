# Governance Plan 3: Amassada Canvas Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a governance module to `amassada-core` that computes risk scores from six weighted factors, derives a `SessionComposition` (stance distribution + budget envelope) from the score, enforces constitutional rules (counter-session required at High/Critical), and provides a `governance-deliberation` canvas template.

**Architecture:** A new `governance/` module sub-tree under `amassada-core/src/` owns risk computation (`risk.rs`), org config parsing (`config.rs`), and session composition + constitutional enforcement (`composition.rs`). The canvas YAML at `canvases/stdlib/governance-deliberation.yaml` defines the deliberation template; actual participants are substituted at session creation time (Plan 4). All types are pure Rust structs with serde — no Axum or database dependencies in this plan.

**Tech Stack:** Rust, serde/serde_yaml (already in the workspace), amassada-core crate at `/Users/bedardpl/project/Amassada`.

---

## Context for the Implementer

### Amassada crate structure

```
Amassada/
  crates/amassada-core/src/
    lib.rs          -- pub mod declarations; add `pub mod governance` here
    canvas.rs       -- Canvas, CanvasLibrary, ParticipantDef
    budget.rs       -- BudgetLedger, PoolName, PoolState
    mission/
      types.rs      -- MissionBudget (has deployable_remaining()), SessionPlan
    ...
  crates/amassada-core/tests/
    canvas_tests.rs, budget_tests.rs, ...  -- existing tests
  canvases/stdlib/   -- canvas YAML files (debate.yaml, planning.yaml, etc.)
```

### Existing Canvas struct (canvas.rs)

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
    pub persona: String,   // e.g. "moderator", "adversarial", "realist"
    pub domain: String,    // Fondament address: "stances/realist" or "auth-service+adversarial"
    pub model: Option<String>,
    pub authority: Option<String>,
}
```

### serde_yaml availability

The workspace already uses `serde_yaml` for `Canvas::from_yaml`. Check `crates/amassada-core/Cargo.toml` to confirm. If somehow missing, add `serde_yaml = "0.9"`.

### Existing test baseline

```
35 tests total across amassada-core (17 + 6 + 4 + 2 + 2 + 1 + 1 + 2 = 35), 0 failures, 2 ignored.
```

### Governance config YAML shape (produced by Farga's GET /governance/config)

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
    daily_tokens: 50000
    per_session_cap: 15000
    counter_session_cap: 10000
  tier_minimums:
    low: 2000
    medium: 5000
    high: 8000
    critical: 12000
```

---

## Task 1: RiskScore computation

**Files:**
- Create: `Amassada/crates/amassada-core/src/governance/mod.rs`
- Create: `Amassada/crates/amassada-core/src/governance/risk.rs`
- Modify: `Amassada/crates/amassada-core/src/lib.rs`
- Create: `Amassada/crates/amassada-core/tests/governance_tests.rs`

### What the risk scorer does

Six pre-normalized factors (each `f32` in `[0, 1]`) are weighted and summed. Two boolean override flags enforce hard floor tiers: `is_org_wide → minimum Critical (≥ 0.80)`; `is_irreversible → minimum High (≥ 0.55)`. Result is a `(score: f32, tier: RiskTier)` pair.

Tier thresholds (configurable, defaults from spec):
- Low: score < 0.30
- Medium: 0.30 ≤ score < 0.55
- High: 0.55 ≤ score < 0.80
- Critical: score ≥ 0.80

- [ ] **Step 1: Add `pub mod governance` to lib.rs**

Open `Amassada/crates/amassada-core/src/lib.rs`. Add this line in alphabetical order with the other pub mods:

```rust
pub mod governance;
```

- [ ] **Step 2: Create governance/mod.rs**

Create `Amassada/crates/amassada-core/src/governance/mod.rs`:

```rust
pub mod risk;
pub mod config;
pub mod composition;

pub use risk::{RiskFactors, RiskScore, RiskTier, RiskWeights, TierThresholds, compute_risk_score};
pub use config::GovernanceConfig;
pub use composition::{BudgetEnvelope, ConstitutionViolation, SessionComposition, check_constitution, compose_session};
```

- [ ] **Step 3: Write failing tests**

Create `Amassada/crates/amassada-core/tests/governance_tests.rs` with just the risk tests for now:

```rust
use amassada_core::governance::{
    RiskFactors, RiskScore, RiskTier, RiskWeights, TierThresholds, compute_risk_score,
};

fn default_weights() -> RiskWeights {
    RiskWeights {
        primitive_proximity: 0.25,
        signal_concurrence: 0.20,
        signal_velocity: 0.15,
        reversibility: 0.20,
        impact: 0.15,
        precedent: 0.05,
    }
}

fn default_thresholds() -> TierThresholds {
    TierThresholds { medium: 0.30, high: 0.55, critical: 0.80 }
}

#[test]
fn low_risk_factors_produce_low_tier() {
    let factors = RiskFactors {
        primitive_proximity: 0.2,
        signal_concurrence: 0.1,
        signal_velocity: 0.1,
        reversibility: 0.0,
        impact: 0.1,
        precedent: 0.0,
        is_irreversible: false,
        is_org_wide: false,
    };
    let result = compute_risk_score(&factors, &default_weights(), &default_thresholds());
    assert_eq!(result.tier, RiskTier::Low);
    assert!(result.score < 0.30, "score was {}", result.score);
}

#[test]
fn high_risk_factors_produce_high_tier() {
    let factors = RiskFactors {
        primitive_proximity: 0.8,
        signal_concurrence: 0.7,
        signal_velocity: 0.6,
        reversibility: 0.7,
        impact: 0.7,
        precedent: 0.5,
        is_irreversible: false,
        is_org_wide: false,
    };
    let result = compute_risk_score(&factors, &default_weights(), &default_thresholds());
    assert_eq!(result.tier, RiskTier::High);
    assert!(result.score >= 0.55 && result.score < 0.80, "score was {}", result.score);
}

#[test]
fn org_wide_flag_forces_critical_regardless_of_score() {
    let factors = RiskFactors {
        primitive_proximity: 0.0,
        signal_concurrence: 0.0,
        signal_velocity: 0.0,
        reversibility: 0.0,
        impact: 0.0,
        precedent: 0.0,
        is_irreversible: false,
        is_org_wide: true,  // force Critical
    };
    let result = compute_risk_score(&factors, &default_weights(), &default_thresholds());
    assert_eq!(result.tier, RiskTier::Critical);
    assert!(result.score >= 0.80, "score should be floored to critical threshold, was {}", result.score);
}

#[test]
fn irreversible_flag_forces_minimum_high_tier() {
    let factors = RiskFactors {
        primitive_proximity: 0.0,
        signal_concurrence: 0.0,
        signal_velocity: 0.0,
        reversibility: 0.0,
        impact: 0.0,
        precedent: 0.0,
        is_irreversible: true,  // force minimum High
        is_org_wide: false,
    };
    let result = compute_risk_score(&factors, &default_weights(), &default_thresholds());
    assert_eq!(result.tier, RiskTier::High);
    assert!(result.score >= 0.55, "score should be floored to high threshold, was {}", result.score);
}

#[test]
fn org_wide_wins_over_irreversible() {
    // org_wide is a higher floor than irreversible; both set → Critical
    let factors = RiskFactors {
        primitive_proximity: 0.0,
        signal_concurrence: 0.0,
        signal_velocity: 0.0,
        reversibility: 0.0,
        impact: 0.0,
        precedent: 0.0,
        is_irreversible: true,
        is_org_wide: true,
    };
    let result = compute_risk_score(&factors, &default_weights(), &default_thresholds());
    assert_eq!(result.tier, RiskTier::Critical);
}

#[test]
fn weighted_sum_matches_manual_calculation() {
    let factors = RiskFactors {
        primitive_proximity: 1.0,
        signal_concurrence: 1.0,
        signal_velocity: 1.0,
        reversibility: 1.0,
        impact: 1.0,
        precedent: 1.0,
        is_irreversible: false,
        is_org_wide: false,
    };
    let weights = default_weights();
    let result = compute_risk_score(&factors, &weights, &default_thresholds());
    // All factors = 1.0, all weights sum to 1.0 → score = 1.0
    assert!((result.score - 1.0).abs() < 0.001, "expected 1.0, got {}", result.score);
    assert_eq!(result.tier, RiskTier::Critical);
}
```

- [ ] **Step 4: Run to verify failure**

```bash
cd /Users/bedardpl/project/Amassada && cargo test -p amassada-core governance 2>&1 | grep -E "^error|FAILED" | head -10
```

Expected: compile errors — types don't exist yet.

- [ ] **Step 5: Create governance/risk.rs**

Create `Amassada/crates/amassada-core/src/governance/risk.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskTier {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskWeights {
    pub primitive_proximity: f32,
    pub signal_concurrence: f32,
    pub signal_velocity: f32,
    pub reversibility: f32,
    pub impact: f32,
    pub precedent: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierThresholds {
    pub medium: f32,
    pub high: f32,
    pub critical: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskFactors {
    pub primitive_proximity: f32,
    pub signal_concurrence: f32,
    pub signal_velocity: f32,
    pub reversibility: f32,
    pub impact: f32,
    pub precedent: f32,
    /// True when LibrarianAssessment.reversibility == Irreversible → minimum High tier
    pub is_irreversible: bool,
    /// True when LibrarianAssessment.impact == OrgWide → minimum Critical tier
    pub is_org_wide: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskScore {
    pub score: f32,
    pub tier: RiskTier,
}

pub fn compute_risk_score(
    factors: &RiskFactors,
    weights: &RiskWeights,
    thresholds: &TierThresholds,
) -> RiskScore {
    let raw = factors.primitive_proximity * weights.primitive_proximity
        + factors.signal_concurrence * weights.signal_concurrence
        + factors.signal_velocity * weights.signal_velocity
        + factors.reversibility * weights.reversibility
        + factors.impact * weights.impact
        + factors.precedent * weights.precedent;

    // Apply hard floor overrides
    let score = if factors.is_org_wide {
        raw.max(thresholds.critical)
    } else if factors.is_irreversible {
        raw.max(thresholds.high)
    } else {
        raw
    };

    let tier = if score >= thresholds.critical {
        RiskTier::Critical
    } else if score >= thresholds.high {
        RiskTier::High
    } else if score >= thresholds.medium {
        RiskTier::Medium
    } else {
        RiskTier::Low
    };

    RiskScore { score, tier }
}
```

- [ ] **Step 6: Create stub files so the module compiles**

Create `Amassada/crates/amassada-core/src/governance/config.rs` (stub — will be filled in Task 2):

```rust
use serde::{Deserialize, Serialize};
use crate::governance::risk::{RiskWeights, TierThresholds};
use crate::error::{AmassadaError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierMinimums {
    pub low: u32,
    pub medium: u32,
    pub high: u32,
    pub critical: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceBudgetConfig {
    pub daily_tokens: u32,
    pub per_session_cap: u32,
    pub counter_session_cap: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceConfig {
    pub risk_weights: RiskWeights,
    pub tier_thresholds: TierThresholds,
    pub budget: GovernanceBudgetConfig,
    pub tier_minimums: TierMinimums,
}

impl GovernanceConfig {
    pub fn from_yaml(_yaml: &str) -> Result<Self> {
        Err(AmassadaError::CanvasParse("not implemented".into()))
    }

    pub fn default_weights() -> Self {
        Self {
            risk_weights: RiskWeights {
                primitive_proximity: 0.25,
                signal_concurrence: 0.20,
                signal_velocity: 0.15,
                reversibility: 0.20,
                impact: 0.15,
                precedent: 0.05,
            },
            tier_thresholds: TierThresholds { medium: 0.30, high: 0.55, critical: 0.80 },
            budget: GovernanceBudgetConfig {
                daily_tokens: 50_000,
                per_session_cap: 15_000,
                counter_session_cap: 10_000,
            },
            tier_minimums: TierMinimums { low: 2_000, medium: 5_000, high: 8_000, critical: 12_000 },
        }
    }
}
```

Create `Amassada/crates/amassada-core/src/governance/composition.rs` (stub — will be filled in Task 3):

```rust
use serde::{Deserialize, Serialize};
use crate::governance::risk::RiskTier;
use crate::governance::config::GovernanceConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetEnvelope {
    pub recommended_tokens: u32,
    pub minimum_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionComposition {
    pub risk_score: f32,
    pub tier: RiskTier,
    pub primary_session: Vec<String>,
    pub counter_session: Option<Vec<String>>,
    pub budget: BudgetEnvelope,
    pub moderator_override: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ConstitutionViolation {
    MissingCounterSession { tier: RiskTier },
}

impl std::fmt::Display for ConstitutionViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingCounterSession { tier } => write!(
                f, "counter-session is required for {:?} tier but was not provided", tier
            ),
        }
    }
}

pub fn compose_session(
    _risk_score: &crate::governance::risk::RiskScore,
    _involved_projects: &[String],
    _config: &GovernanceConfig,
) -> SessionComposition {
    unimplemented!("Task 3")
}

pub fn check_constitution(composition: &SessionComposition) -> Result<(), ConstitutionViolation> {
    match &composition.tier {
        RiskTier::High | RiskTier::Critical => {
            if composition.counter_session.is_none() {
                return Err(ConstitutionViolation::MissingCounterSession {
                    tier: composition.tier.clone(),
                });
            }
        }
        _ => {}
    }
    Ok(())
}
```

- [ ] **Step 7: Run risk tests**

```bash
cd /Users/bedardpl/project/Amassada && cargo test -p amassada-core governance 2>&1 | tail -10
```

Expected: 6 governance tests pass.

- [ ] **Step 8: Commit**

```bash
cd /Users/bedardpl/project/Amassada && git add crates/amassada-core/src/lib.rs crates/amassada-core/src/governance/ crates/amassada-core/tests/governance_tests.rs && git commit -m "feat: add governance module with RiskScore computation"
```

---

## Task 2: GovernanceConfig YAML parsing

**Files:**
- Modify: `Amassada/crates/amassada-core/src/governance/config.rs`
- Modify: `Amassada/crates/amassada-core/tests/governance_tests.rs`

Replace the stub `from_yaml` with a real implementation. The YAML has a top-level `governance:` key.

- [ ] **Step 1: Write failing tests**

Append to `Amassada/crates/amassada-core/tests/governance_tests.rs`:

```rust
use amassada_core::governance::GovernanceConfig;

const SAMPLE_CONFIG: &str = r#"
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
    daily_tokens: 50000
    per_session_cap: 15000
    counter_session_cap: 10000
  tier_minimums:
    low: 2000
    medium: 5000
    high: 8000
    critical: 12000
"#;

#[test]
fn governance_config_parses_from_yaml() {
    let config = GovernanceConfig::from_yaml(SAMPLE_CONFIG).unwrap();
    assert!((config.risk_weights.primitive_proximity - 0.25).abs() < 0.001);
    assert!((config.risk_weights.precedent - 0.05).abs() < 0.001);
    assert!((config.tier_thresholds.high - 0.55).abs() < 0.001);
    assert_eq!(config.budget.per_session_cap, 15_000);
    assert_eq!(config.tier_minimums.critical, 12_000);
}

#[test]
fn governance_config_weights_sum_to_one() {
    let config = GovernanceConfig::from_yaml(SAMPLE_CONFIG).unwrap();
    let w = &config.risk_weights;
    let sum = w.primitive_proximity + w.signal_concurrence + w.signal_velocity
        + w.reversibility + w.impact + w.precedent;
    assert!((sum - 1.0).abs() < 0.001, "weights must sum to 1.0, got {}", sum);
}

#[test]
fn governance_config_from_yaml_rejects_empty_string() {
    let result = GovernanceConfig::from_yaml("");
    assert!(result.is_err(), "empty YAML should fail to parse");
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cd /Users/bedardpl/project/Amassada && cargo test -p amassada-core governance_config 2>&1 | grep -E "FAILED|^error" | head -5
```

Expected: FAIL — `from_yaml` returns Err always.

- [ ] **Step 3: Implement from_yaml in config.rs**

Replace the entire content of `Amassada/crates/amassada-core/src/governance/config.rs` with:

```rust
use serde::{Deserialize, Serialize};
use crate::governance::risk::{RiskWeights, TierThresholds};
use crate::error::{AmassadaError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierMinimums {
    pub low: u32,
    pub medium: u32,
    pub high: u32,
    pub critical: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceBudgetConfig {
    pub daily_tokens: u32,
    pub per_session_cap: u32,
    pub counter_session_cap: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceConfig {
    pub risk_weights: RiskWeights,
    pub tier_thresholds: TierThresholds,
    pub budget: GovernanceBudgetConfig,
    pub tier_minimums: TierMinimums,
}

// Intermediate for deserializing the nested `governance:` wrapper key
#[derive(Deserialize)]
struct GovernanceConfigFile {
    governance: GovernanceConfigRaw,
}

#[derive(Deserialize)]
struct GovernanceConfigRaw {
    risk_weights: RiskWeights,
    tier_thresholds: TierThresholds,
    budget: GovernanceBudgetConfig,
    tier_minimums: TierMinimums,
}

impl GovernanceConfig {
    pub fn from_yaml(yaml: &str) -> Result<Self> {
        let file: GovernanceConfigFile = serde_yaml::from_str(yaml)
            .map_err(|e| AmassadaError::CanvasParse(e.to_string()))?;
        Ok(Self {
            risk_weights: file.governance.risk_weights,
            tier_thresholds: file.governance.tier_thresholds,
            budget: file.governance.budget,
            tier_minimums: file.governance.tier_minimums,
        })
    }

    pub fn default_weights() -> Self {
        Self {
            risk_weights: RiskWeights {
                primitive_proximity: 0.25,
                signal_concurrence: 0.20,
                signal_velocity: 0.15,
                reversibility: 0.20,
                impact: 0.15,
                precedent: 0.05,
            },
            tier_thresholds: TierThresholds { medium: 0.30, high: 0.55, critical: 0.80 },
            budget: GovernanceBudgetConfig {
                daily_tokens: 50_000,
                per_session_cap: 15_000,
                counter_session_cap: 10_000,
            },
            tier_minimums: TierMinimums { low: 2_000, medium: 5_000, high: 8_000, critical: 12_000 },
        }
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cd /Users/bedardpl/project/Amassada && cargo test -p amassada-core governance 2>&1 | tail -10
```

Expected: 9 governance tests pass (6 risk + 3 config).

- [ ] **Step 5: Commit**

```bash
cd /Users/bedardpl/project/Amassada && git add crates/amassada-core/src/governance/config.rs crates/amassada-core/tests/governance_tests.rs && git commit -m "feat: implement GovernanceConfig YAML parsing"
```

---

## Task 3: SessionComposition + slot resolution + constitutional enforcement

**Files:**
- Modify: `Amassada/crates/amassada-core/src/governance/composition.rs`
- Modify: `Amassada/crates/amassada-core/tests/governance_tests.rs`

### Slot resolution logic

Each tier maps to a set of stance slots. Slots are filled from `involved_projects` first (project-specific address: `"{project}+{stance}"`); remaining slots get generic addresses (`"stances/{stance}"`).

**Tier stance distributions:**
- **Low**: primary = `["realist", "realist", "moderator"]`, counter = none
- **Medium**: primary = `["realist", "adversarial", "builder", "moderator"]`, counter = none
- **High**: primary = `["adversarial", "adversarial", "realist", "moderator"]`, counter = `["builder", "dreamer"]`
- **Critical**: primary = `["adversarial", "adversarial", "realist", "builder", "dreamer", "moderator"]`, counter = `["builder", "dreamer"]`

Project-specific addresses are only assigned to non-moderator slots (the moderator slot always uses `"stances/moderator"`).

### Budget per tier (from GovernanceConfig)

Primary session budget: `min(config.budget.per_session_cap, config.tier_minimums[tier] * 2)` for recommended; `config.tier_minimums[tier]` for minimum.

Counter session (if present): `config.budget.counter_session_cap` recommended; `config.tier_minimums.medium` minimum (counter sessions are medium-weight deliberations regardless of tier).

- [ ] **Step 1: Write failing tests**

Append to `Amassada/crates/amassada-core/tests/governance_tests.rs`:

```rust
use amassada_core::governance::{
    BudgetEnvelope, SessionComposition, check_constitution, compose_session,
    ConstitutionViolation, GovernanceConfig, RiskScore, RiskTier,
};

fn default_config() -> GovernanceConfig {
    GovernanceConfig::default_weights()
}

fn risk_score(tier: RiskTier, score: f32) -> RiskScore {
    RiskScore { score, tier }
}

#[test]
fn low_tier_produces_no_counter_session() {
    let rs = risk_score(RiskTier::Low, 0.20);
    let comp = compose_session(&rs, &["auth".into()], &default_config());
    assert_eq!(comp.tier, RiskTier::Low);
    assert!(comp.counter_session.is_none());
    // Low tier: realist, realist, moderator
    assert_eq!(comp.primary_session.len(), 3);
}

#[test]
fn medium_tier_produces_no_counter_session() {
    let rs = risk_score(RiskTier::Medium, 0.40);
    let comp = compose_session(&rs, &[], &default_config());
    assert_eq!(comp.tier, RiskTier::Medium);
    assert!(comp.counter_session.is_none());
    // Medium tier: realist, adversarial, builder, moderator
    assert_eq!(comp.primary_session.len(), 4);
}

#[test]
fn high_tier_produces_counter_session() {
    let rs = risk_score(RiskTier::High, 0.65);
    let comp = compose_session(&rs, &[], &default_config());
    assert_eq!(comp.tier, RiskTier::High);
    assert!(comp.counter_session.is_some());
    let counter = comp.counter_session.as_ref().unwrap();
    // Counter: builder, dreamer
    assert_eq!(counter.len(), 2);
}

#[test]
fn critical_tier_produces_counter_session() {
    let rs = risk_score(RiskTier::Critical, 0.90);
    let comp = compose_session(&rs, &[], &default_config());
    assert_eq!(comp.tier, RiskTier::Critical);
    assert!(comp.counter_session.is_some());
    // Primary: adversarial, adversarial, realist, builder, dreamer, moderator
    assert_eq!(comp.primary_session.len(), 6);
}

#[test]
fn involved_projects_fill_non_moderator_slots_first() {
    let rs = risk_score(RiskTier::High, 0.65);
    let projects = vec!["auth-service".into(), "api-gateway".into()];
    let comp = compose_session(&rs, &projects, &default_config());
    // First two adversarial slots should get project-specific addresses
    assert!(comp.primary_session[0].contains("auth-service"), "first slot: {}", comp.primary_session[0]);
    assert!(comp.primary_session[1].contains("api-gateway"), "second slot: {}", comp.primary_session[1]);
    // Third slot (realist) falls back to generic since projects exhausted
    assert!(comp.primary_session[2].starts_with("stances/"), "third slot: {}", comp.primary_session[2]);
    // Last slot is always stances/moderator
    assert_eq!(comp.primary_session.last().unwrap(), "stances/moderator");
}

#[test]
fn moderator_slot_always_generic() {
    let rs = risk_score(RiskTier::Critical, 0.85);
    let projects: Vec<String> = (0..10).map(|i| format!("proj-{}", i)).collect(); // many projects
    let comp = compose_session(&rs, &projects, &default_config());
    assert_eq!(comp.primary_session.last().unwrap(), "stances/moderator");
}

#[test]
fn constitution_passes_for_high_with_counter() {
    let comp = SessionComposition {
        risk_score: 0.65,
        tier: RiskTier::High,
        primary_session: vec!["stances/adversarial".into()],
        counter_session: Some(vec!["stances/builder".into()]),
        budget: BudgetEnvelope { recommended_tokens: 8000, minimum_tokens: 8000 },
        moderator_override: None,
    };
    assert!(check_constitution(&comp).is_ok());
}

#[test]
fn constitution_fails_for_high_without_counter() {
    let comp = SessionComposition {
        risk_score: 0.65,
        tier: RiskTier::High,
        primary_session: vec!["stances/adversarial".into()],
        counter_session: None,  // missing!
        budget: BudgetEnvelope { recommended_tokens: 8000, minimum_tokens: 8000 },
        moderator_override: None,
    };
    assert!(check_constitution(&comp).is_err());
}

#[test]
fn constitution_passes_for_low_without_counter() {
    let comp = SessionComposition {
        risk_score: 0.20,
        tier: RiskTier::Low,
        primary_session: vec!["stances/realist".into()],
        counter_session: None,
        budget: BudgetEnvelope { recommended_tokens: 2000, minimum_tokens: 2000 },
        moderator_override: None,
    };
    assert!(check_constitution(&comp).is_ok());
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cd /Users/bedardpl/project/Amassada && cargo test -p amassada-core governance 2>&1 | grep -E "FAILED|panicked" | head -10
```

Expected: FAIL on the `compose_session` tests (unimplemented!).

- [ ] **Step 3: Implement compose_session in composition.rs**

Replace the entire content of `Amassada/crates/amassada-core/src/governance/composition.rs` with:

```rust
use serde::{Deserialize, Serialize};
use crate::governance::risk::{RiskScore, RiskTier};
use crate::governance::config::GovernanceConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetEnvelope {
    pub recommended_tokens: u32,
    pub minimum_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionComposition {
    pub risk_score: f32,
    pub tier: RiskTier,
    pub primary_session: Vec<String>,
    pub counter_session: Option<Vec<String>>,
    pub budget: BudgetEnvelope,
    pub moderator_override: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ConstitutionViolation {
    MissingCounterSession { tier: RiskTier },
}

impl std::fmt::Display for ConstitutionViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingCounterSession { tier } => write!(
                f, "counter-session required for {:?} tier", tier
            ),
        }
    }
}

// Stance slots per tier. Moderator is always last and always generic.
// Other slots are filled from involved_projects first.
fn stance_slots(tier: &RiskTier) -> Vec<&'static str> {
    match tier {
        RiskTier::Low => vec!["realist", "realist", "moderator"],
        RiskTier::Medium => vec!["realist", "adversarial", "builder", "moderator"],
        RiskTier::High => vec!["adversarial", "adversarial", "realist", "moderator"],
        RiskTier::Critical => vec!["adversarial", "adversarial", "realist", "builder", "dreamer", "moderator"],
    }
}

fn counter_stances(tier: &RiskTier) -> Option<Vec<&'static str>> {
    match tier {
        RiskTier::High | RiskTier::Critical => Some(vec!["builder", "dreamer"]),
        _ => None,
    }
}

fn resolve_address(stance: &str, projects: &[String], project_idx: &mut usize) -> String {
    if stance == "moderator" {
        return "stances/moderator".into();
    }
    if *project_idx < projects.len() {
        let addr = format!("{}+{}", projects[*project_idx], stance);
        *project_idx += 1;
        addr
    } else {
        format!("stances/{}", stance)
    }
}

pub fn compose_session(
    risk_score: &RiskScore,
    involved_projects: &[String],
    config: &GovernanceConfig,
) -> SessionComposition {
    let primary_stances = stance_slots(&risk_score.tier);
    let mut project_idx = 0;
    let primary_session = primary_stances
        .iter()
        .map(|s| resolve_address(s, involved_projects, &mut project_idx))
        .collect();

    let counter_session = counter_stances(&risk_score.tier).map(|stances| {
        let mut ci = 0usize;
        stances.iter().map(|s| resolve_address(s, &[], &mut ci)).collect()
    });

    let min_tokens = match risk_score.tier {
        RiskTier::Low => config.tier_minimums.low,
        RiskTier::Medium => config.tier_minimums.medium,
        RiskTier::High => config.tier_minimums.high,
        RiskTier::Critical => config.tier_minimums.critical,
    };
    let recommended_tokens = config.budget.per_session_cap.min(min_tokens * 2);

    SessionComposition {
        risk_score: risk_score.score,
        tier: risk_score.tier.clone(),
        primary_session,
        counter_session,
        budget: BudgetEnvelope { recommended_tokens, minimum_tokens: min_tokens },
        moderator_override: None,
    }
}

pub fn check_constitution(composition: &SessionComposition) -> Result<(), ConstitutionViolation> {
    match &composition.tier {
        RiskTier::High | RiskTier::Critical => {
            if composition.counter_session.is_none() {
                return Err(ConstitutionViolation::MissingCounterSession {
                    tier: composition.tier.clone(),
                });
            }
        }
        _ => {}
    }
    Ok(())
}
```

- [ ] **Step 4: Run tests**

```bash
cd /Users/bedardpl/project/Amassada && cargo test -p amassada-core governance 2>&1 | tail -15
```

Expected: 18 governance tests pass (6 risk + 3 config + 9 composition/constitution).

- [ ] **Step 5: Run full suite to check no regressions**

```bash
cd /Users/bedardpl/project/Amassada && cargo test -p amassada-core 2>&1 | tail -10
```

Expected: all 35 original tests + 18 new = 53 total, 0 failures.

- [ ] **Step 6: Commit**

```bash
cd /Users/bedardpl/project/Amassada && git add crates/amassada-core/src/governance/composition.rs crates/amassada-core/tests/governance_tests.rs && git commit -m "feat: add SessionComposition, slot resolution, and constitutional enforcement"
```

---

## Task 4: Governance canvas YAML + canvas load test

**Files:**
- Create: `Amassada/canvases/stdlib/governance-deliberation.yaml`
- Modify: `Amassada/crates/amassada-core/tests/governance_tests.rs`

The governance canvas is a template. Its `initial_participants` define a canonical Medium-tier layout (the safe default when tier isn't known at load time). Plan 4 replaces participants dynamically from `SessionComposition.primary_session`. The canvas must parse cleanly via `Canvas::from_yaml`.

- [ ] **Step 1: Write failing test**

Append to `Amassada/crates/amassada-core/tests/governance_tests.rs`:

```rust
use amassada_core::canvas::Canvas;

#[test]
fn governance_deliberation_canvas_parses() {
    let yaml = include_str!("../../../canvases/stdlib/governance-deliberation.yaml");
    let canvas = Canvas::from_yaml(yaml).expect("governance-deliberation canvas must parse");
    assert_eq!(canvas.id, "governance-deliberation");
    assert!(canvas.human.slot, "governance sessions always have a human slot");
    assert!(!canvas.output.sections.is_empty(), "output must have sections");
    // Must have at least a moderator participant in the template
    let has_moderator = canvas.initial_participants.iter().any(|p| p.is_moderator());
    assert!(has_moderator, "canvas must include a moderator participant");
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cd /Users/bedardpl/project/Amassada && cargo test -p amassada-core governance_deliberation_canvas 2>&1 | grep -E "FAILED|panicked|^error" | head -5
```

Expected: FAIL — file doesn't exist.

- [ ] **Step 3: Create governance-deliberation.yaml**

Create `Amassada/canvases/stdlib/governance-deliberation.yaml`:

```yaml
id: governance-deliberation
version: "1.0.0"
mode: auto
selector:
  description: "Governance deliberation on a proposed Fondament archetype change; risk-scored session composition"
  tags: [governance, archetype, fondament, risk, deliberation, pattern]
  examples:
    - "deliberate on cross-project JWT signing standard"
    - "governance review of a new agent role definition"
    - "evaluate whether to standardize error handling across projects"
initial_participants:
  - persona: moderator
    domain: stances/moderator
  - persona: adversarial
    domain: stances/adversarial
  - persona: realist
    domain: stances/realist
  - persona: builder
    domain: stances/builder
budget:
  total_tokens: 15000
  pools:
    main_session: 11000
    consultations: 3000
    mod_whisper: 1000
consultation:
  max_turns: 2
  min_response_tokens: 50
rounds:
  min: 3
  max: 6
  convergence_modifier: 0.7
  context_window: 20
human:
  slot: true
  advisory_window_turns: 2
output:
  format: markdown
  sections:
    - id: proposed_change
      title: "Proposed Archetype Change"
      required: true
    - id: risk_assessment
      title: "Risk Assessment"
      required: true
    - id: agent_votes
      title: "Agent Votes"
      required: true
    - id: impact_analysis
      title: "Impact Analysis"
      required: false
    - id: recommendation
      title: "Recommendation"
      required: true
```

- [ ] **Step 4: Run tests**

```bash
cd /Users/bedardpl/project/Amassada && cargo test -p amassada-core governance 2>&1 | tail -10
```

Expected: 19 governance tests pass (18 from Tasks 1-3 + 1 canvas load test).

- [ ] **Step 5: Run full suite**

```bash
cd /Users/bedardpl/project/Amassada && cargo test -p amassada-core 2>&1 | grep "test result" | head -15
```

Expected: all tests pass (35 original + 19 new = 54 total).

- [ ] **Step 6: Commit**

```bash
cd /Users/bedardpl/project/Amassada && git add canvases/stdlib/governance-deliberation.yaml crates/amassada-core/tests/governance_tests.rs && git commit -m "feat: add governance-deliberation canvas template"
```

---

## Self-Review

**Spec coverage:**

| Spec requirement | Covered by |
|---|---|
| `RiskScore` computation | Task 1 `compute_risk_score` with 6 weighted factors |
| Hard floor overrides (OrgWide→Critical, Irreversible→High) | Task 1 `is_org_wide` / `is_irreversible` flags |
| `SessionComposition` struct | Task 3 (risk_score, tier, primary_session, counter_session, budget, moderator_override) |
| Tier → stance distribution | Task 3 `stance_slots()` per tier |
| Affected-project-first slot filling | Task 3 `resolve_address()` with project_idx |
| Counter-session required at High/Critical | Task 3 `counter_stances()` + `check_constitution()` |
| Constitutional enforcement | Task 3 `check_constitution()` returns Err for High/Critical without counter |
| `BudgetEnvelope` from tier_minimums and per_session_cap | Task 3 budget computation |
| Configurable weights via GovernanceConfig | Task 2 `GovernanceConfig::from_yaml` |
| Governance canvas YAML | Task 4 |

**Placeholder scan:** None found — all steps have complete code.

**Type consistency check:**
- `RiskTier` defined in Task 1 `risk.rs` → used in Task 3 `composition.rs` and governance_tests.rs — consistent
- `RiskScore { score, tier }` defined in Task 1 → `compose_session(risk_score: &RiskScore, ...)` in Task 3 — consistent
- `GovernanceConfig { risk_weights, tier_thresholds, budget, tier_minimums }` defined in Task 2 stub and finalized in Task 2 implementation → used in Task 3 `compose_session` — consistent
- `BudgetEnvelope { recommended_tokens, minimum_tokens }` defined in Task 1 stub (composition.rs) and kept identical in Task 3 — consistent

**Note on counter-session address resolution:** Counter slots (`builder`, `dreamer`) intentionally do NOT get project-specific addresses — they are builder/dreamer stance agents responding to the primary session's risks, not project-specific agents. The `project_idx = 0; resolve_address(s, &[], &mut ci)` in Task 3 implements this correctly (empty project list → always falls to `"stances/{stance}"`).
