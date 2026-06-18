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
