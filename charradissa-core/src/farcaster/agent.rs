use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::Mutex;
use crate::backend::ChatBackend;
use crate::error::Result;
use crate::farga::{FargaWriter, Signal};
use crate::types::{ProjectId, RoomId, UserId};
use super::analyzer::{
    CrossSpaceSnapshot, DigestEntry, DigestPayload, DigestSynthesis, FarcasterAnalyzer,
    ProjectSnapshot,
};
use super::concurrence::{AgentConcurrence, ConcurrenceType, Urgency};
use super::milestone::MilestoneEvent;
use super::system_agent::SystemAgent;

const EVENT_BUFFER_CAP: usize = 50;

pub struct FarcasterAgent {
    pub(crate) backend: Arc<dyn ChatBackend>,
    pub(crate) farga: Arc<dyn FargaWriter>,
    pub(crate) analyzer: Arc<dyn FarcasterAnalyzer>,
    pub(crate) projects: Vec<ProjectId>,
    pub(crate) project_agent_ids: HashMap<ProjectId, UserId>,
    pub(crate) event_buffer: Mutex<HashMap<ProjectId, VecDeque<MilestoneEvent>>>,
    pub(crate) digest_buffer: Mutex<Vec<DigestEntry>>,
    pub(crate) daily_reactive_token_budget: u32,
    pub(crate) daily_digest_token_budget: u32,
    pub(crate) reactive_tokens_used: AtomicU32,
    pub(crate) digest_tokens_used: AtomicU32,
    pub(crate) digest_interval_hours: u64,
    pub(crate) last_digest_at: Mutex<DateTime<Utc>>,
}

impl FarcasterAgent {
    pub fn new(
        backend: Arc<dyn ChatBackend>,
        farga: Arc<dyn FargaWriter>,
        analyzer: Arc<dyn FarcasterAnalyzer>,
        projects: Vec<ProjectId>,
        project_agent_ids: HashMap<ProjectId, UserId>,
    ) -> Self {
        Self {
            backend,
            farga,
            analyzer,
            projects,
            project_agent_ids,
            event_buffer: Mutex::new(HashMap::new()),
            digest_buffer: Mutex::new(Vec::new()),
            daily_reactive_token_budget: 20_000,
            daily_digest_token_budget: 10_000,
            reactive_tokens_used: AtomicU32::new(0),
            digest_tokens_used: AtomicU32::new(0),
            digest_interval_hours: 6,
            last_digest_at: Mutex::new(Utc::now()),
        }
    }

    pub(crate) fn is_significant(event: &MilestoneEvent) -> bool {
        match event {
            MilestoneEvent::ArtifactProduced { .. } => true,
            MilestoneEvent::EvaluationCompleted { satisfied: true, .. } => true,
            MilestoneEvent::EvaluationCompleted { .. } => false,
            MilestoneEvent::ReplanTriggered { replan_count, .. } => *replan_count >= 2,
            MilestoneEvent::MissionCompleted { .. } => true,
        }
    }

    pub(crate) async fn build_snapshot(&self) -> CrossSpaceSnapshot {
        let buf = self.event_buffer.lock().await;
        let projects = self.projects.iter().map(|pid| {
            let recent_events = buf.get(pid)
                .map(|deque| {
                    deque.iter().rev().take(3).map(|e| e.summary()).collect()
                })
                .unwrap_or_default();
            ProjectSnapshot {
                project_id: pid.clone(),
                mission_goal: None,
                open_sub_objectives: vec![],
                recent_events,
            }
        }).collect();
        CrossSpaceSnapshot { projects }
    }

    // Reactive path — implemented in Task 7
    pub(crate) async fn handle_milestone(&self, event: &MilestoneEvent) -> Result<()> {
        let _ = event;
        Ok(())
    }

    // Digest path — implemented in Task 8
    pub(crate) async fn run_tick(&self) -> Result<()> {
        Ok(())
    }
}

fn format_digest(synthesis: &DigestSynthesis) -> String {
    let mut out = String::from("## Farcaster Digest\n\n");
    if !synthesis.connections.is_empty() {
        out.push_str("### Cross-Space Connections\n");
        for c in &synthesis.connections { out.push_str(&format!("- {}\n", c)); }
        out.push('\n');
    }
    if !synthesis.lessons.is_empty() {
        out.push_str("### Lessons\n");
        for l in &synthesis.lessons { out.push_str(&format!("- {}\n", l)); }
        out.push('\n');
    }
    if !synthesis.open_questions.is_empty() {
        out.push_str("### Open Questions\n");
        for q in &synthesis.open_questions { out.push_str(&format!("- {}\n", q)); }
    }
    out
}

#[async_trait]
impl SystemAgent for FarcasterAgent {
    fn name(&self) -> &str { "farcaster" }
    async fn on_milestone(&self, event: &MilestoneEvent) -> Result<()> {
        self.handle_milestone(event).await
    }
    async fn tick(&self) -> Result<()> {
        self.run_tick().await
    }
}
