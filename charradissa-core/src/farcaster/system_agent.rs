use async_trait::async_trait;
use crate::error::Result;
use super::milestone::MilestoneEvent;
use super::observation::ObservationEvent;

#[async_trait]
pub trait SystemAgent: Send + Sync {
    fn name(&self) -> &str;
    async fn on_milestone(&self, event: &MilestoneEvent) -> Result<()>;

    /// Handles any observation event. Default delegates milestones to on_milestone and
    /// ignores domain digests, providing Phase I backward compatibility.
    async fn on_observation(&self, event: &ObservationEvent) -> Result<()> {
        match event {
            ObservationEvent::Milestone(m) => self.on_milestone(m).await,
            ObservationEvent::DomainDigest(_) => Ok(()),
        }
    }

    async fn tick(&self) -> Result<()>;
}
