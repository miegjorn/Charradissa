// placeholder — filled in Task 4
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::oneshot;
use uuid::Uuid;
use crate::error::{CharradissaError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalOutcome {
    Approved,
    Rejected(String),
}

pub struct PendingApproval {
    pub id: String,
    pub category: String,
    pub description: String,
    pub params: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
    tx: oneshot::Sender<ApprovalOutcome>,
}

pub struct ApprovalQueue {
    pending: HashMap<String, PendingApproval>,
    timeout_minutes: u64,
}

impl ApprovalQueue {
    pub fn new(timeout_minutes: u64) -> Self {
        Self { pending: HashMap::new(), timeout_minutes }
    }

    pub fn create_pending(
        &mut self,
        category: String,
        description: String,
        params: serde_json::Value,
    ) -> (String, oneshot::Receiver<ApprovalOutcome>) {
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending.insert(id.clone(), PendingApproval {
            id: id.clone(),
            category,
            description,
            params,
            created_at: chrono::Utc::now(),
            tx,
        });
        (id, rx)
    }

    pub fn resolve(&mut self, id: &str, outcome: ApprovalOutcome) -> Result<()> {
        let entry = self.pending.remove(id)
            .ok_or_else(|| CharradissaError::Backend(format!("unknown approval id: {}", id)))?;
        let _ = entry.tx.send(outcome);
        Ok(())
    }

    pub fn list_pending(&self) -> Vec<(&str, &str, &str)> {
        self.pending.values()
            .map(|p| (p.id.as_str(), p.category.as_str(), p.description.as_str()))
            .collect()
    }

    pub fn timeout_minutes(&self) -> u64 { self.timeout_minutes }
}

/// Serializable snapshot of a pending approval (no tx channel — loaded from disk).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PendingApprovalRecord {
    pub id: String,
    pub room_id: String,
    pub category: String,
    pub description: String,
    pub params: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub status: ApprovalStatus,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Rejected(String),
}

/// Cross-space approval queue backed by Farga's generic KV store
/// (`GET`/`PUT /kv/*path`), one entry per approval id under the `approval`
/// namespace -- same convention `corrier-core::routing` and
/// `caissa-cli::tick_poller` already use. State lives in Farga, not in any
/// process's memory, so this is safe to consume from multiple replicas of
/// the same daemon (see this plan's Global Constraints).
pub struct PersistentApprovalQueue {
    farga_url: String,
}

impl PersistentApprovalQueue {
    pub fn new(farga_url: String) -> Self {
        Self { farga_url }
    }

    fn kv_url(&self, id: &str) -> String {
        format!("{}/kv/approval/{}", self.farga_url.trim_end_matches('/'), id)
    }

    fn namespace_url(&self) -> String {
        format!("{}/kv/approval", self.farga_url.trim_end_matches('/'))
    }

    pub async fn register(
        &self,
        id: &str,
        room_id: &str,
        category: &str,
        description: &str,
        params: serde_json::Value,
    ) -> Result<()> {
        let record = PendingApprovalRecord {
            id: id.to_string(),
            room_id: room_id.to_string(),
            category: category.to_string(),
            description: description.to_string(),
            params,
            created_at: chrono::Utc::now(),
            status: ApprovalStatus::Pending,
        };
        let body = serde_json::json!({ "value": record });
        let resp = reqwest::Client::new()
            .put(self.kv_url(id))
            .json(&body)
            .send()
            .await
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(CharradissaError::Backend(format!(
                "farga approval PUT returned {}",
                resp.status()
            )));
        }
        Ok(())
    }

    pub async fn get(&self, id: &str) -> Option<PendingApprovalRecord> {
        let resp = reqwest::Client::new().get(self.kv_url(id)).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let json: serde_json::Value = resp.json().await.ok()?;
        let value = json.get("value")?;
        serde_json::from_value(value.clone()).ok()
    }

    async fn update_status(&self, id: &str, status: ApprovalStatus) -> Result<()> {
        let mut record = self.get(id).await
            .ok_or_else(|| CharradissaError::Backend(format!("unknown approval id: {}", id)))?;
        record.status = status;
        let body = serde_json::json!({ "value": record });
        let resp = reqwest::Client::new()
            .put(self.kv_url(id))
            .json(&body)
            .send()
            .await
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(CharradissaError::Backend(format!(
                "farga approval PUT (update) returned {}",
                resp.status()
            )));
        }
        Ok(())
    }

    pub async fn approve(&self, id: &str) -> Result<()> {
        self.update_status(id, ApprovalStatus::Approved).await
    }

    pub async fn reject(&self, id: &str, reason: String) -> Result<()> {
        self.update_status(id, ApprovalStatus::Rejected(reason)).await
    }

    pub async fn list_all(&self) -> Vec<PendingApprovalRecord> {
        let resp = match reqwest::Client::new().get(self.namespace_url()).send().await {
            Ok(r) if r.status().is_success() => r,
            _ => return vec![],
        };
        #[derive(serde::Deserialize)]
        struct KvListEntry {
            value: PendingApprovalRecord,
        }
        let entries: Vec<KvListEntry> = resp.json().await.unwrap_or_default();
        entries.into_iter().map(|e| e.value).collect()
    }

    pub async fn list_pending(&self) -> Vec<PendingApprovalRecord> {
        self.list_all()
            .await
            .into_iter()
            .filter(|r| r.status == ApprovalStatus::Pending)
            .collect()
    }
}
