use async_trait::async_trait;
use crate::error::Result;
use super::observation::DomainDigestEvent;

/// Implemented by the level above a FarcasterAgent in the fractal hierarchy.
/// A domain-level FarcasterAgent emits here after each digest synthesis; the
/// cross-domain Farcaster implements this to receive those digests.
#[async_trait]
pub trait UpstreamObserver: Send + Sync {
    async fn on_domain_digest(&self, event: DomainDigestEvent) -> Result<()>;
}
