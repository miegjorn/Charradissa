use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Urgency {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConcurrenceType {
    Observed,
    Whispered,
    Acknowledged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConcurrence {
    pub project_id: String,
    pub agent_address: String,
    pub concurrence_type: ConcurrenceType,
    pub note: Option<String>,
}
