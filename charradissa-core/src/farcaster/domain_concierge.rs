use std::collections::HashMap;
use std::sync::Arc;
use async_trait::async_trait;
use crate::backend::ChatBackend;
use crate::error::Result;
use crate::farga::FargaWriter;
use crate::types::{ProjectId, UserId};
use super::agent::FarcasterAgent;
use super::analyzer::FarcasterAnalyzer;
use super::milestone::MilestoneEvent;
use super::observation::ObservationEvent;
use super::system_agent::SystemAgent;
use super::upstream::UpstreamObserver;

/// A domain-scoped Farcaster hub. Wraps a FarcasterAgent scoped to the projects
/// in one domain, and wires it to a CrossDomainFarcaster upstream so that domain
/// digests propagate up the fractal hierarchy.
pub struct DomainConcierge {
    pub domain: String,
    pub(crate) farcaster: FarcasterAgent,
}

impl DomainConcierge {
    pub fn new(
        domain: String,
        backend: Arc<dyn ChatBackend>,
        farga: Arc<dyn FargaWriter>,
        analyzer: Arc<dyn FarcasterAnalyzer>,
        projects: Vec<ProjectId>,
        project_agent_ids: HashMap<ProjectId, UserId>,
        upstream: Arc<dyn UpstreamObserver>,
    ) -> Self {
        let farcaster = FarcasterAgent::new(backend, farga, analyzer, projects, project_agent_ids)
            .with_domain(domain.clone())
            .with_upstream(upstream);
        Self { domain, farcaster }
    }
}

#[async_trait]
impl SystemAgent for DomainConcierge {
    fn name(&self) -> &str { &self.domain }

    async fn on_milestone(&self, event: &MilestoneEvent) -> Result<()> {
        self.farcaster.handle_milestone(event).await
    }

    async fn on_observation(&self, event: &ObservationEvent) -> Result<()> {
        match event {
            ObservationEvent::Milestone(m) => self.farcaster.handle_milestone(m).await,
            // Domain concierges observe project-level milestones, not peer domain digests.
            ObservationEvent::DomainDigest(_) => Ok(()),
        }
    }

    async fn tick(&self) -> Result<()> {
        self.farcaster.run_tick().await
    }
}

/// Maps project IDs to domain names derived from Fondament composition addresses.
/// The domain is the first path component of the composition address:
/// e.g., "engineering/api-gateway" → domain "engineering".
pub struct DomainRegistry {
    project_to_domain: HashMap<ProjectId, String>,
}

impl DomainRegistry {
    pub fn new(project_to_domain: HashMap<ProjectId, String>) -> Self {
        Self { project_to_domain }
    }

    /// Build from (project_id → composition_address) pairs.
    pub fn from_composition_addresses(addresses: HashMap<ProjectId, String>) -> Self {
        let project_to_domain = addresses.into_iter()
            .map(|(pid, addr)| {
                let domain = addr.split('/').next().unwrap_or("default").to_string();
                (pid, domain)
            })
            .collect();
        Self { project_to_domain }
    }

    pub fn domain_for(&self, project: &ProjectId) -> Option<&str> {
        self.project_to_domain.get(project).map(|s| s.as_str())
    }

    pub fn projects_in_domain(&self, domain: &str) -> Vec<ProjectId> {
        self.project_to_domain.iter()
            .filter(|(_, d)| d.as_str() == domain)
            .map(|(pid, _)| pid.clone())
            .collect()
    }

    pub fn all_domains(&self) -> Vec<String> {
        let mut domains: Vec<String> = self.project_to_domain.values()
            .cloned()
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        domains.sort();
        domains
    }
}
