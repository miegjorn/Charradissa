use std::sync::Arc;
use std::time::Duration;
use chrono::Utc;
use tokio::time::interval;
use crate::backend::ChatBackend;
use crate::farga::{FargaWriter, Signal};
use crate::farcaster::milestone::MilestoneEvent;
use crate::farcaster::observation::ObservationEvent;
use crate::farcaster::system_agent::SystemAgent;
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

/// Format a room's recent events into a single durable digest signal body.
pub fn harvest_digest(room: &RoomId, events: &[ChatEvent]) -> String {
    let mut s = format!("[room harvest: {}]\n", room.as_str());
    for e in events { s.push_str(&format!("{}: {}\n", e.sender, e.content)); }
    if s.len() > 8000 {
        let mut boundary = 8000;
        while boundary > 0 && !s.is_char_boundary(boundary) { boundary -= 1; }
        s.truncate(boundary);
        s.push_str("\n…(truncated)");
    }
    s
}

pub struct ConciergeAgent {
    backend: Arc<dyn ChatBackend>,
    farga: Arc<dyn FargaWriter>,
    projects: Vec<ProjectId>,
    project_agent_ids: std::collections::HashMap<ProjectId, UserId>,
    archival_interval_hours: u64,
    convergence_interval_hours: u64,
    daily_token_budget: u32,
    system_agents: Vec<Box<dyn SystemAgent>>,
    system_agent_tick_intervals: Vec<Duration>,
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
        Self {
            backend, farga, projects, project_agent_ids,
            archival_interval_hours, convergence_interval_hours, daily_token_budget,
            system_agents: Vec::new(),
            system_agent_tick_intervals: Vec::new(),
        }
    }

    pub fn register_system_agent(&mut self, agent: Box<dyn SystemAgent>, tick_interval: Duration) {
        self.system_agents.push(agent);
        self.system_agent_tick_intervals.push(tick_interval);
    }

    pub async fn dispatch_milestone(&self, event: &MilestoneEvent) {
        for agent in &self.system_agents {
            if let Err(e) = agent.on_milestone(event).await {
                tracing::error!("[{}] on_milestone error: {}", agent.name(), e);
            }
        }
    }

    /// Dispatch any observation event (project milestone or domain digest) to all system agents.
    /// Use this instead of dispatch_milestone when Phase II fractal routing is needed.
    pub async fn dispatch_observation(&self, event: &ObservationEvent) {
        for agent in &self.system_agents {
            if let Err(e) = agent.on_observation(event).await {
                tracing::error!("[{}] on_observation error: {}", agent.name(), e);
            }
        }
    }

    pub async fn run_system_agent_ticks(&self) {
        let mut ticker = interval(Duration::from_secs(60));
        let mut next_tick: Vec<tokio::time::Instant> = self.system_agent_tick_intervals
            .iter()
            .map(|d| tokio::time::Instant::now() + *d)
            .collect();
        loop {
            ticker.tick().await;
            let now = tokio::time::Instant::now();
            for (i, agent) in self.system_agents.iter().enumerate() {
                if now >= next_tick[i] {
                    if let Err(e) = agent.tick().await {
                        tracing::error!("[{}] tick error: {}", agent.name(), e);
                    }
                    next_tick[i] = now + self.system_agent_tick_intervals[i];
                }
            }
        }
    }

    pub async fn run_archival_loop(&self) {
        let mut ticker = interval(Duration::from_secs(self.archival_interval_hours * 3600));
        loop {
            ticker.tick().await;
            let since = Utc::now() - chrono::Duration::hours(self.archival_interval_hours as i64);
            let rooms = self.backend.joined_rooms().await.unwrap_or_default();
            tracing::info!("concierge archival: sweeping {} joined rooms", rooms.len());
            for room in &rooms {
                match self.backend.room_history(room, since).await {
                    Ok(events) if !events.is_empty() => {
                        let sig = Signal {
                            project: "occitan".into(),
                            content: harvest_digest(room, &events),
                            source: "concierge-archival".into(),
                        };
                        if let Err(e) = self.farga.write_signals(&ProjectId::new("occitan"), vec![sig]).await {
                            tracing::error!("concierge: farga write failed for {}: {}", room, e);
                        }
                    }
                    Ok(_) => {}
                    Err(e) => tracing::error!("concierge: room_history failed for {}: {}", room, e),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatEventKind, UserId};
    use chrono::Utc;

    #[test]
    fn harvest_digest_contains_sender_content_and_room() {
        let room = RoomId::new("!abc:occitane.guilhem");
        let events = vec![
            ChatEvent {
                event_id: "$1".into(),
                room_id: room.clone(),
                sender: UserId::new("@p:occitane.guilhem"),
                content: "hello world".into(),
                timestamp: Utc::now(),
                kind: ChatEventKind::Message,
            },
        ];
        let digest = harvest_digest(&room, &events);
        assert!(digest.contains("!abc:occitane.guilhem"), "must include room id");
        assert!(digest.contains("@p:occitane.guilhem"), "must include sender");
        assert!(digest.contains("hello world"), "must include content");
    }

    #[test]
    fn harvest_digest_truncates_at_8000() {
        let room = RoomId::new("!r:s");
        let events = vec![
            ChatEvent {
                event_id: "$1".into(),
                room_id: room.clone(),
                sender: UserId::new("@u:s"),
                content: "x".repeat(9000),
                timestamp: Utc::now(),
                kind: ChatEventKind::Message,
            },
        ];
        let digest = harvest_digest(&room, &events);
        assert!(digest.len() <= 8030, "truncated digest should not exceed 8000 + truncation suffix");
        assert!(digest.contains("truncated"), "must include truncation marker");
    }
}
