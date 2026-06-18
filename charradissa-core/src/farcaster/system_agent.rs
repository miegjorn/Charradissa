use async_trait::async_trait;
use crate::error::Result;
use super::milestone::MilestoneEvent;

#[async_trait]
pub trait SystemAgent: Send + Sync {
    fn name(&self) -> &str;
    async fn on_milestone(&self, event: &MilestoneEvent) -> Result<()>;
    async fn tick(&self) -> Result<()>;
}
