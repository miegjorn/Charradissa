// placeholder — filled in Task 3
use async_trait::async_trait;
use chrono::Duration;
use serde::{Deserialize, Serialize};
use crate::error::Result;
use crate::types::ProjectId;

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
