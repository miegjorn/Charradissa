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
