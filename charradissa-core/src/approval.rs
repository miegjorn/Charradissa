// placeholder — filled in Task 4
use std::collections::HashMap;
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
