// placeholder — filled in Task 6
use std::sync::Arc;
use std::time::Duration;
use chrono::Utc;
use tokio::time::interval;
use crate::backend::ChatBackend;
use crate::farga::{FargaWriter, Signal};
use crate::types::{ChatEvent, ProjectId, RoomId, UserId};

pub fn extract_signals(events: &[ChatEvent]) -> Vec<Signal> {
    let signal_keywords = ["decision:", "decided:", "blocker:", "blocked:", "artifact:", "pattern:"];
    events.iter().filter_map(|e| {
        let lower = e.content.to_lowercase();
        if signal_keywords.iter().any(|kw| lower.contains(kw)) {
            Some(Signal {
                project: e.room_id.to_string(),
                content: e.content.clone(),
                source: "concierge-archival".into(),
            })
        } else {
            None
        }
    }).collect()
}

pub struct ConciergeAgent {
    backend: Arc<dyn ChatBackend>,
    farga: Arc<dyn FargaWriter>,
    projects: Vec<ProjectId>,
    project_agent_ids: std::collections::HashMap<ProjectId, UserId>,
    archival_interval_hours: u64,
    convergence_interval_hours: u64,
    daily_token_budget: u32,
}

impl ConciergeAgent {
    pub fn new(
        backend: Arc<dyn ChatBackend>,
        farga: Arc<dyn FargaWriter>,
        projects: Vec<ProjectId>,
        project_agent_ids: std::collections::HashMap<ProjectId, UserId>,
        archival_interval_hours: u64,
        convergence_interval_hours: u64,
        daily_token_budget: u32,
    ) -> Self {
        Self { backend, farga, projects, project_agent_ids,
               archival_interval_hours, convergence_interval_hours, daily_token_budget }
    }

    pub async fn run_archival_loop(&self) {
        let mut ticker = interval(Duration::from_secs(self.archival_interval_hours * 3600));
        loop {
            ticker.tick().await;
            for project in &self.projects {
                let room_id = RoomId::new(&format!("#{}", project.as_str()));
                let since = Utc::now() - chrono::Duration::hours(self.archival_interval_hours as i64);
                match self.backend.room_history(&room_id, since).await {
                    Ok(events) if !events.is_empty() => {
                        let signals = extract_signals(&events);
                        if !signals.is_empty() {
                            if let Err(e) = self.farga.write_signals(project, signals).await {
                                tracing::error!("concierge: farga write failed for {}: {}", project, e);
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(e) => tracing::error!("concierge: room_history failed for {}: {}", project, e),
                }
            }
        }
    }

    pub async fn run_convergence_loop(&self) {
        let mut ticker = interval(Duration::from_secs(self.convergence_interval_hours * 3600));
        loop {
            ticker.tick().await;
            let since = chrono::Duration::hours(self.convergence_interval_hours as i64);
            let mut all_signals = Vec::new();
            for project in &self.projects {
                match self.farga.recent_signals(project, since).await {
                    Ok(signals) => all_signals.extend(signals),
                    Err(e) => tracing::error!("concierge: recent_signals failed for {}: {}", project, e),
                }
            }

            if all_signals.is_empty() { continue; }

            tracing::info!("concierge convergence sweep: {} signals across {} projects",
                all_signals.len(), self.projects.len());
        }
    }
}
