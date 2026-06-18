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
    pub digest_buffer: Mutex<Vec<DigestEntry>>,
    pub(crate) daily_reactive_token_budget: u32,
    pub(crate) daily_digest_token_budget: u32,
    pub reactive_tokens_used: AtomicU32,
    pub digest_tokens_used: AtomicU32,
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
        // 1. Accumulate to event_buffer (hold lock briefly, release before any await)
        {
            let mut buf = self.event_buffer.lock().await;
            let project_buf = buf.entry(event.project_id().clone()).or_default();
            if project_buf.len() >= EVENT_BUFFER_CAP { project_buf.pop_front(); }
            project_buf.push_back(event.clone());
        }

        // 2. Filter — skip insignificant events
        if !Self::is_significant(event) { return Ok(()); }

        // 3. Budget check — defer to digest if exhausted
        if self.reactive_tokens_used.load(Ordering::Relaxed) >= self.daily_reactive_token_budget {
            tracing::info!("farcaster: reactive budget exhausted, deferring to digest");
            self.digest_buffer.lock().await.push(DigestEntry {
                project_id: event.project_id().clone(),
                connection_summary: format!("deferred (budget): {}", event.summary()),
                involved_projects: vec![event.project_id().clone()],
                concurrence: vec![],
                urgency: Urgency::Low,
                whispered_at: None,
            });
            return Ok(());
        }

        // 4. Build snapshot (lock released before analyze call)
        let snapshot = self.build_snapshot().await;

        // 5. Analyze — best-effort, log and return on failure
        let (connections, tokens_used) = match self.analyzer.analyze_cross_space(event, &snapshot).await {
            Ok(result) => result,
            Err(e) => {
                tracing::error!("farcaster: analyze_cross_space failed: {}", e);
                return Ok(());
            }
        };
        self.reactive_tokens_used.fetch_add(tokens_used, Ordering::Relaxed);

        // 6. Whisper Medium/High urgency connections
        let now = Utc::now();
        for conn in &connections {
            if matches!(conn.urgency, Urgency::Medium | Urgency::High) {
                if let Some(agent_id) = self.project_agent_ids.get(&conn.to_project) {
                    let msg = format!(
                        "[farcaster] {}: {}\nSuggested: {}",
                        conn.from_project,
                        event.summary(),
                        conn.summary
                    );
                    if let Err(e) = self.backend.send_dm(agent_id, &msg).await {
                        tracing::warn!("farcaster: dm delivery failed for {}: {}", conn.to_project, e);
                    }
                }
            }
        }

        // 7. Record all connections to digest_buffer
        {
            let mut digest = self.digest_buffer.lock().await;
            for conn in connections {
                let is_whispered = matches!(conn.urgency, Urgency::Medium | Urgency::High);
                let concurrence = if is_whispered {
                    self.project_agent_ids.get(&conn.to_project).map(|uid| {
                        vec![AgentConcurrence {
                            project_id: conn.to_project.to_string(),
                            agent_address: uid.to_string(),
                            concurrence_type: ConcurrenceType::Whispered,
                            note: None,
                        }]
                    }).unwrap_or_default()
                } else {
                    vec![]
                };
                digest.push(DigestEntry {
                    project_id: conn.from_project.clone(),
                    connection_summary: conn.summary.clone(),
                    involved_projects: vec![conn.from_project, conn.to_project],
                    concurrence,
                    urgency: conn.urgency,
                    whispered_at: if is_whispered { Some(now) } else { None },
                });
            }
        }

        Ok(())
    }

    // Digest path — implemented in Task 8
    pub(crate) async fn run_tick(&self) -> Result<()> {
        // 1. Drain digest_buffer atomically
        let entries = {
            let mut buf = self.digest_buffer.lock().await;
            if buf.is_empty() { return Ok(()); }
            std::mem::take(&mut *buf)
        };

        // 2. Budget check — defer to next tick if exhausted
        if self.digest_tokens_used.load(Ordering::Relaxed) >= self.daily_digest_token_budget {
            tracing::info!("farcaster: digest budget exhausted, deferring to next tick");
            self.digest_buffer.lock().await.extend(entries);
            return Ok(());
        }

        // 3. Synthesize with Opus — requeue on failure
        let (synthesis, tokens_used) = match self.analyzer.synthesize_digest(&entries).await {
            Ok(result) => result,
            Err(e) => {
                tracing::error!("farcaster: synthesize_digest failed: {}", e);
                self.digest_buffer.lock().await.extend(entries);
                return Ok(());
            }
        };
        self.digest_tokens_used.fetch_add(tokens_used, Ordering::Relaxed);

        // 4. Broadcast to #farcaster (best-effort)
        let farcaster_room = RoomId::new("#farcaster");
        if let Err(e) = self.backend.send_message(&farcaster_room, &format_digest(&synthesis)).await {
            tracing::warn!("farcaster: broadcast failed: {}", e);
        }

        // 5. Farga submission if verdict is "submit"
        if synthesis.farga_verdict == "submit" {
            let period_end = Utc::now();
            let period_start = *self.last_digest_at.lock().await;

            let payload = super::analyzer::DigestPayload {
                title: synthesis.farga_title.clone().unwrap_or_default(),
                narrative: synthesis.farga_narrative.clone().unwrap_or_default(),
                lessons: synthesis.lessons.clone(),
                open_questions: synthesis.open_questions.clone(),
                period_start,
                period_end,
                projects_observed: self.projects.clone(),
                concurrence: entries.iter().flat_map(|e| e.concurrence.iter().cloned()).collect(),
            };

            let content = serde_json::to_string(&payload)
                .map_err(|e| crate::error::CharradissaError::Dispatch(e.to_string()))?;

            let signals = vec![Signal {
                project: "system".to_string(),
                content,
                source: "farcaster".to_string(),
            }];

            let system_project = ProjectId::new("system");
            if let Err(e) = self.farga.write_signals(&system_project, signals).await {
                tracing::error!("farcaster: farga submission failed, re-queuing: {}", e);
                self.digest_buffer.lock().await.extend(entries);
                return Ok(());
            }
        }

        // 6. Update last_digest_at
        *self.last_digest_at.lock().await = Utc::now();
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
