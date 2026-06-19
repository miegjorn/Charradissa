use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::types::ProjectId;
use super::concurrence::AgentConcurrence;
use super::milestone::MilestoneEvent;

/// A domain digest emitted by a domain-level FarcasterAgent after synthesis.
/// This is the currency of the fractal hierarchy: a digest at level N becomes
/// an ObservationEvent at level N+1.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainDigestEvent {
    pub domain: String,
    pub title: String,
    pub narrative: String,
    pub lessons: Vec<String>,
    pub open_questions: Vec<String>,
    pub involved_projects: Vec<ProjectId>,
    pub concurrence: Vec<AgentConcurrence>,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    /// Set when the digest was successfully submitted to Farga.
    pub farga_node_id: Option<String>,
}

impl DomainDigestEvent {
    pub fn summary(&self) -> String {
        format!("domain digest [{}]: {}", self.domain, self.title)
    }
}

/// Union of a raw project-level milestone and an inbound digest from a lower-level Farcaster.
/// The same Concierge/Farcaster pattern applies to both — only the observation scope changes.
#[derive(Debug, Clone)]
pub enum ObservationEvent {
    Milestone(MilestoneEvent),
    DomainDigest(DomainDigestEvent),
}

impl ObservationEvent {
    pub fn summary(&self) -> String {
        match self {
            Self::Milestone(m) => m.summary(),
            Self::DomainDigest(d) => d.summary(),
        }
    }

    pub fn source_domain(&self) -> Option<&str> {
        match self {
            Self::DomainDigest(d) => Some(&d.domain),
            Self::Milestone(_) => None,
        }
    }
}
