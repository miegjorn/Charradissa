use charradissa_core::farcaster::milestone::MilestoneEvent;
use charradissa_core::farcaster::concurrence::{AgentConcurrence, ConcurrenceType, Urgency};
use charradissa_core::types::ProjectId;

// --- Shared test helpers (Tasks 6-9) ---

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use async_trait::async_trait;
use charradissa_core::backend::ChatBackend;
use charradissa_core::error::{CharradissaError, Result};
use charradissa_core::farga::{FargaWriter, Signal};
use charradissa_core::farcaster::analyzer::{
    CrossSpaceConnection, CrossSpaceSnapshot, DigestEntry, DigestSynthesis, FarcasterAnalyzer,
};
use charradissa_core::farcaster::milestone::MilestoneEvent as _MilestoneEvent;
use charradissa_core::farcaster::FarcasterAgent;
use charradissa_core::types::{
    ChatEvent, CompositionAddress, RoomId, RoomOptions, SpaceId, UserId,
};

struct MockChatBackend {
    dms: Arc<tokio::sync::Mutex<Vec<(UserId, String)>>>,
    messages: Arc<tokio::sync::Mutex<Vec<(RoomId, String)>>>,
}

impl MockChatBackend {
    fn new() -> (Self, Arc<tokio::sync::Mutex<Vec<(UserId, String)>>>, Arc<tokio::sync::Mutex<Vec<(RoomId, String)>>>) {
        let dms = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let messages = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        (Self { dms: Arc::clone(&dms), messages: Arc::clone(&messages) }, dms, messages)
    }
}

#[async_trait]
impl ChatBackend for MockChatBackend {
    async fn send_message(&self, room: &RoomId, content: &str) -> Result<()> {
        self.messages.lock().await.push((room.clone(), content.to_string()));
        Ok(())
    }
    async fn send_dm(&self, user: &UserId, content: &str) -> Result<()> {
        self.dms.lock().await.push((user.clone(), content.to_string()));
        Ok(())
    }
    async fn create_room(&self, _: &RoomOptions) -> Result<RoomId> { Ok(RoomId::new("!r:t")) }
    async fn create_space(&self, _: &str) -> Result<SpaceId> { Ok(SpaceId::new("!s:t")) }
    async fn add_to_space(&self, _: &SpaceId, _: &RoomId) -> Result<()> { Ok(()) }
    async fn invite(&self, _: &RoomId, _: &UserId) -> Result<()> { Ok(()) }
    async fn kick(&self, _: &RoomId, _: &UserId, _: &str) -> Result<()> { Ok(()) }
    async fn register_agent(&self, _: &CompositionAddress) -> Result<UserId> { Ok(UserId::new("@x:t")) }
    async fn deregister_agent(&self, _: &UserId) -> Result<()> { Ok(()) }
    async fn room_history(&self, _: &RoomId, _: chrono::DateTime<chrono::Utc>) -> Result<Vec<ChatEvent>> { Ok(vec![]) }
    async fn delete_room(&self, _: &RoomId) -> Result<()> { Ok(()) }
}

struct MockFargaWriter {
    calls: Arc<tokio::sync::Mutex<Vec<(ProjectId, Vec<Signal>)>>>,
}

impl MockFargaWriter {
    fn new() -> (Self, Arc<tokio::sync::Mutex<Vec<(ProjectId, Vec<Signal>)>>>) {
        let calls = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        (Self { calls: Arc::clone(&calls) }, calls)
    }
}

#[async_trait]
impl FargaWriter for MockFargaWriter {
    async fn write_signals(&self, project: &ProjectId, signals: Vec<Signal>) -> Result<()> {
        self.calls.lock().await.push((project.clone(), signals));
        Ok(())
    }
    async fn recent_signals(&self, _: &ProjectId, _: chrono::Duration) -> Result<Vec<Signal>> {
        Ok(vec![])
    }
}

struct MockFarcasterAnalyzer {
    reactive: std::sync::Mutex<VecDeque<(Vec<CrossSpaceConnection>, u32)>>,
    digest: std::sync::Mutex<VecDeque<(DigestSynthesis, u32)>>,
}

impl MockFarcasterAnalyzer {
    fn new() -> Self {
        Self {
            reactive: std::sync::Mutex::new(VecDeque::new()),
            digest: std::sync::Mutex::new(VecDeque::new()),
        }
    }
    fn queue_reactive(&self, connections: Vec<CrossSpaceConnection>, tokens: u32) {
        self.reactive.lock().unwrap().push_back((connections, tokens));
    }
    fn queue_digest(&self, synthesis: DigestSynthesis, tokens: u32) {
        self.digest.lock().unwrap().push_back((synthesis, tokens));
    }
}

#[async_trait]
impl FarcasterAnalyzer for MockFarcasterAnalyzer {
    async fn analyze_cross_space(
        &self,
        _event: &MilestoneEvent,
        _snapshot: &CrossSpaceSnapshot,
    ) -> Result<(Vec<CrossSpaceConnection>, u32)> {
        self.reactive.lock().unwrap().pop_front()
            .ok_or_else(|| CharradissaError::Dispatch("no queued reactive response".into()))
    }
    async fn synthesize_digest(&self, _entries: &[DigestEntry]) -> Result<(DigestSynthesis, u32)> {
        self.digest.lock().unwrap().pop_front()
            .ok_or_else(|| CharradissaError::Dispatch("no queued digest response".into()))
    }
}

fn make_agent(projects: Vec<ProjectId>, agent_ids: HashMap<ProjectId, UserId>)
    -> (FarcasterAgent,
        Arc<tokio::sync::Mutex<Vec<(UserId, String)>>>,
        Arc<tokio::sync::Mutex<Vec<(RoomId, String)>>>,
        Arc<tokio::sync::Mutex<Vec<(ProjectId, Vec<Signal>)>>>,
        Arc<MockFarcasterAnalyzer>)
{
    let (backend, dms, messages) = MockChatBackend::new();
    let (farga, farga_calls) = MockFargaWriter::new();
    let analyzer = Arc::new(MockFarcasterAnalyzer::new());
    let agent = FarcasterAgent::new(
        Arc::new(backend),
        Arc::new(farga),
        Arc::clone(&analyzer) as Arc<dyn FarcasterAnalyzer>,
        projects,
        agent_ids,
    );
    (agent, dms, messages, farga_calls, analyzer)
}

// --- Task 6 test ---
#[tokio::test]
async fn farcaster_agent_new_initializes_with_defaults() {
    let (agent, _, _, _, _) = make_agent(
        vec![ProjectId::new("alpha")],
        HashMap::new(),
    );
    // If this compiles and runs, construction is correct.
    drop(agent);
}

fn artifact_event() -> MilestoneEvent {
    MilestoneEvent::ArtifactProduced {
        mission_id: "m1".into(),
        session_id: "s1".into(),
        project_id: ProjectId::new("alpha"),
        canvas_id: "c1".into(),
        artifact_summary: "auth schema finalized".into(),
        sub_objective_ids: vec!["obj-1".into()],
    }
}

#[test]
fn milestone_project_id_returns_correct_project() {
    let ev = artifact_event();
    assert_eq!(ev.project_id(), &ProjectId::new("alpha"));
}

#[test]
fn milestone_summary_contains_canvas_and_summary() {
    let ev = artifact_event();
    let s = ev.summary();
    assert!(s.contains("c1"), "expected canvas id in summary: {}", s);
    assert!(s.contains("auth schema"), "expected artifact summary: {}", s);
}

#[test]
fn replan_event_summary_contains_count() {
    let ev = MilestoneEvent::ReplanTriggered {
        mission_id: "m1".into(),
        project_id: ProjectId::new("beta"),
        sub_objective_id: "obj-2".into(),
        reason: "test failed".into(),
        replan_count: 3,
    };
    assert!(ev.summary().contains("3"));
}

#[test]
fn urgency_derives_eq_and_debug() {
    assert_eq!(Urgency::High, Urgency::High);
    assert_ne!(Urgency::Low, Urgency::High);
}

#[test]
fn agent_concurrence_round_trips_json() {
    let c = AgentConcurrence {
        project_id: "alpha".into(),
        agent_address: "alpha/dev+builder".into(),
        concurrence_type: ConcurrenceType::Whispered,
        note: Some("confirmed via DM".into()),
    };
    let json = serde_json::to_string(&c).unwrap();
    let back: AgentConcurrence = serde_json::from_str(&json).unwrap();
    assert_eq!(back.project_id, "alpha");
    assert_eq!(back.concurrence_type, ConcurrenceType::Whispered);
}
