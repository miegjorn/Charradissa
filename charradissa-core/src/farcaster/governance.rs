use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::types::ProjectId;
use super::concurrence::AgentConcurrence;
use amassada_core::governance::{RiskFactors, GovernanceConfig, SessionComposition, compute_risk_score, compose_session};

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

pub fn derive_risk_factors(contrib: &GovernanceContribution) -> RiskFactors {
    let primitive_proximity = match contrib.target_layer {
        FargaLayer::OrgLevel => 1.0,
        FargaLayer::InitiativeLevel => 0.6,
        FargaLayer::ProjectLevel => 0.3,
    };

    let signal_concurrence = (contrib.concurrence.len() as f32 / 4.0).min(1.0);
    let signal_velocity = (contrib.event_count as f32 / 10.0).min(1.0);

    let reversibility = match &contrib.reversibility {
        None | Some(ReversibilityLevel::FullyReversible) => 0.0,
        Some(ReversibilityLevel::EffectsLinger) => 0.3,
        Some(ReversibilityLevel::CostlyReversible) => 0.6,
        Some(ReversibilityLevel::Irreversible) => 1.0,
    };

    let impact = match &contrib.impact {
        None | Some(ImpactScope::Contained) => 0.0,
        Some(ImpactScope::CrossProject) => 0.3,
        Some(ImpactScope::DomainWide) => 0.6,
        Some(ImpactScope::OrgWide) => 1.0,
    };

    RiskFactors {
        primitive_proximity,
        signal_concurrence,
        signal_velocity,
        reversibility,
        impact,
        precedent: 0.0,
        is_irreversible: contrib.reversibility == Some(ReversibilityLevel::Irreversible),
        is_org_wide: contrib.impact == Some(ImpactScope::OrgWide),
    }
}

pub fn evaluate_governance(
    contrib: &GovernanceContribution,
    config: &GovernanceConfig,
) -> SessionComposition {
    let factors = derive_risk_factors(contrib);
    let risk_score = compute_risk_score(&factors, &config.risk_weights, &config.tier_thresholds);
    let projects: Vec<String> = contrib.involved_projects.iter().map(|p| p.to_string()).collect();
    compose_session(&risk_score, &projects, config)
}
