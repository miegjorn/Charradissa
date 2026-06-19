// placeholder — filled in Task 3
use async_trait::async_trait;
use chrono::Duration;
use serde::{Deserialize, Serialize};
use crate::error::Result;
use crate::types::ProjectId;
use crate::farcaster::governance::GovernanceContribution;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    pub project: String,
    pub content: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GovernanceOutcome {
    Approved,
    Rejected,
    Deferred,
    ApprovedWithConditions,
}

impl GovernanceOutcome {
    pub fn as_status_str(&self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Deferred => "deferred",
            Self::ApprovedWithConditions => "approved_with_conditions",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceDecision {
    pub node_id: String,
    pub outcome: GovernanceOutcome,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssessmentResult {
    pub status: String,
    pub reversibility: Option<String>,
    pub impact: Option<String>,
}

#[async_trait]
pub trait FargaWriter: Send + Sync {
    async fn write_signals(&self, project: &ProjectId, signals: Vec<Signal>) -> Result<()>;
    async fn recent_signals(&self, project: &ProjectId, since: Duration) -> Result<Vec<Signal>>;
    // Fallback for testing and non-HTTP backends. Flattens the contribution into
    // a Signal write. Real HTTP backends override this with a dedicated endpoint.
    async fn submit_governance_contribution(
        &self,
        contribution: GovernanceContribution,
    ) -> Result<String> {
        let content = serde_json::to_string(&contribution)
            .map_err(|e| crate::error::CharradissaError::Dispatch(e.to_string()))?;
        let signal = Signal {
            project: "system".to_string(),
            content,
            source: "farcaster-governance".to_string(),
        };
        self.write_signals(&ProjectId::new("system"), vec![signal]).await?;
        Ok(String::new())
    }
    async fn submit_governance_decision(&self, decision: GovernanceDecision) -> Result<()>;
    async fn get_assessment(&self, _node_id: &str) -> Result<Option<AssessmentResult>> {
        Ok(None)
    }
}

pub struct HttpFargaWriter {
    client: reqwest::Client,
    base_url: String,
}

impl HttpFargaWriter {
    pub fn new(base_url: String) -> Self {
        Self { client: reqwest::Client::new(), base_url }
    }
}

#[async_trait]
impl FargaWriter for HttpFargaWriter {
    async fn write_signals(&self, project: &ProjectId, signals: Vec<Signal>) -> Result<()> {
        let url = format!("{}/signals", self.base_url);
        self.client.post(&url)
            .json(&serde_json::json!({ "project": project.as_str(), "signals": signals }))
            .send().await
            .map_err(|e| crate::error::CharradissaError::Backend(e.to_string()))?
            .error_for_status()
            .map_err(|e| crate::error::CharradissaError::Backend(e.to_string()))?;
        Ok(())
    }

    async fn recent_signals(&self, project: &ProjectId, since: Duration) -> Result<Vec<Signal>> {
        let url = format!("{}/signals/recent?project={}&since={}h",
            self.base_url, project.as_str(), since.num_hours());
        let resp = self.client.get(&url).send().await
            .map_err(|e| crate::error::CharradissaError::Backend(e.to_string()))?;
        resp.json().await
            .map_err(|e| crate::error::CharradissaError::Backend(e.to_string()))
    }

    async fn submit_governance_contribution(
        &self,
        contribution: GovernanceContribution,
    ) -> Result<String> {
        let url = format!("{}/governance", self.base_url);
        let resp = self.client
            .post(&url)
            .json(&contribution)
            .send()
            .await
            .map_err(|e| crate::error::CharradissaError::Backend(e.to_string()))?
            .error_for_status()
            .map_err(|e| crate::error::CharradissaError::Backend(e.to_string()))?;
        let json: serde_json::Value = resp.json().await
            .map_err(|e| crate::error::CharradissaError::Backend(e.to_string()))?;
        Ok(json["id"].as_str().unwrap_or("").to_string())
    }

    async fn submit_governance_decision(&self, decision: GovernanceDecision) -> Result<()> {
        let url = format!("{}/governance/decisions", self.base_url);
        self.client
            .post(&url)
            .json(&serde_json::json!({
                "node_id": decision.node_id,
                "outcome": decision.outcome.as_status_str(),
                "rationale": decision.rationale,
            }))
            .send()
            .await
            .map_err(|e| crate::error::CharradissaError::Backend(e.to_string()))?
            .error_for_status()
            .map_err(|e| crate::error::CharradissaError::Backend(e.to_string()))?;
        Ok(())
    }

    async fn get_assessment(&self, node_id: &str) -> Result<Option<AssessmentResult>> {
        let url = format!("{}/governance/assessments/{}", self.base_url, node_id);
        let resp = self.client.get(&url).send().await
            .map_err(|e| crate::error::CharradissaError::Backend(e.to_string()))?;
        if resp.status().as_u16() == 404 { return Ok(None); }
        resp.error_for_status_ref()
            .map_err(|e| crate::error::CharradissaError::Backend(e.to_string()))?;
        let json: serde_json::Value = resp.json().await
            .map_err(|e| crate::error::CharradissaError::Backend(e.to_string()))?;
        Ok(Some(AssessmentResult {
            status: json["status"].as_str().unwrap_or("pending").to_string(),
            reversibility: json["reversibility"].as_str().map(|s| s.to_string()),
            impact: json["impact"].as_str().map(|s| s.to_string()),
        }))
    }
}
