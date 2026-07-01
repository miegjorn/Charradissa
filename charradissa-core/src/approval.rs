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

/// Cross-space approval queue backed by a JSON file.
/// Used by the HTTP API layer to expose approvals across rooms.
pub struct PersistentApprovalQueue {
    path: PathBuf,
}

impl PersistentApprovalQueue {
    pub fn new(path: PathBuf) -> Self { Self { path } }

    fn load(&self) -> Vec<PendingApprovalRecord> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self, records: &[PendingApprovalRecord]) -> Result<()> {
        let json = serde_json::to_string_pretty(records)
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        std::fs::write(&self.path, json)
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        Ok(())
    }

    pub fn register(&self, room_id: &str, category: &str, description: &str, params: serde_json::Value) -> Result<String> {
        let mut records = self.load();
        let id = Uuid::new_v4().to_string();
        records.push(PendingApprovalRecord {
            id: id.clone(),
            room_id: room_id.to_string(),
            category: category.to_string(),
            description: description.to_string(),
            params,
            created_at: chrono::Utc::now(),
            status: ApprovalStatus::Pending,
        });
        self.save(&records)?;
        Ok(id)
    }

    pub fn approve(&self, id: &str) -> Result<()> {
        let mut records = self.load();
        let record = records.iter_mut().find(|r| r.id == id)
            .ok_or_else(|| CharradissaError::Backend(format!("unknown approval id: {}", id)))?;
        record.status = ApprovalStatus::Approved;
        self.save(&records)
    }

    pub fn reject(&self, id: &str, reason: String) -> Result<()> {
        let mut records = self.load();
        let record = records.iter_mut().find(|r| r.id == id)
            .ok_or_else(|| CharradissaError::Backend(format!("unknown approval id: {}", id)))?;
        record.status = ApprovalStatus::Rejected(reason);
        self.save(&records)
    }

    pub fn list_pending(&self) -> Vec<PendingApprovalRecord> {
        self.load().into_iter()
            .filter(|r| r.status == ApprovalStatus::Pending)
            .collect()
    }

    pub fn list_all(&self) -> Vec<PendingApprovalRecord> {
        self.load()
    }

    pub fn get(&self, id: &str) -> Option<PendingApprovalRecord> {
        self.load().into_iter().find(|r| r.id == id)
    }
}
