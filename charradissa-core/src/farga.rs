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

#[async_trait]
pub trait FargaWriter: Send + Sync {
    async fn write_signals(&self, project: &ProjectId, signals: Vec<Signal>) -> Result<()>;
    async fn recent_signals(&self, project: &ProjectId, since: Duration) -> Result<Vec<Signal>>;
    // Fallback for testing and non-HTTP backends. Flattens the contribution into
    // a Signal write. Real HTTP backends override this with a dedicated endpoint.
    async fn submit_governance_contribution(
        &self,
        contribution: GovernanceContribution,
    ) -> Result<()> {
        let content = serde_json::to_string(&contribution)
            .map_err(|e| crate::error::CharradissaError::Dispatch(e.to_string()))?;
        let signal = Signal {
            project: "system".to_string(),
            content,
            source: "farcaster-governance".to_string(),
        };
        self.write_signals(&ProjectId::new("system"), vec![signal]).await
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
}
