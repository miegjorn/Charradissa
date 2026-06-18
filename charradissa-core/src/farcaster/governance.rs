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
