use charradissa_core::farcaster::milestone::MilestoneEvent;
use charradissa_core::farcaster::concurrence::{AgentConcurrence, ConcurrenceType, Urgency};
use charradissa_core::farcaster::governance::{
    GovernanceContribution, FargaLayer, ReversibilityLevel, ImpactScope,
};
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
use charradissa_core::farcaster::{FarcasterAgent, SystemAgent};
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
    governance_calls: Arc<tokio::sync::Mutex<Vec<GovernanceContribution>>>,
    decision_calls: Arc<tokio::sync::Mutex<Vec<charradissa_core::farga::GovernanceDecision>>>,
}

impl MockFargaWriter {
    fn new() -> (
        Self,
        Arc<tokio::sync::Mutex<Vec<(ProjectId, Vec<Signal>)>>>,
        Arc<tokio::sync::Mutex<Vec<GovernanceContribution>>>,
        Arc<tokio::sync::Mutex<Vec<charradissa_core::farga::GovernanceDecision>>>,
    ) {
        let calls = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let governance_calls = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let decision_calls = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        (
            Self { calls: Arc::clone(&calls), governance_calls: Arc::clone(&governance_calls), decision_calls: Arc::clone(&decision_calls) },
            calls,
            governance_calls,
            decision_calls,
        )
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
    async fn submit_governance_contribution(
        &self,
        contribution: GovernanceContribution,
    ) -> charradissa_core::error::Result<String> {
        self.governance_calls.lock().await.push(contribution);
        Ok("mock-node-id".to_string())
    }
    async fn submit_governance_decision(
        &self,
        decision: charradissa_core::farga::GovernanceDecision,
    ) -> charradissa_core::error::Result<()> {
        self.decision_calls.lock().await.push(decision);
        Ok(())
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
        Arc<MockFarcasterAnalyzer>,
        Arc<tokio::sync::Mutex<Vec<GovernanceContribution>>>)
{
    let (backend, dms, messages) = MockChatBackend::new();
    let (farga, farga_calls, governance_calls, _decision_calls) = MockFargaWriter::new();
    let analyzer = Arc::new(MockFarcasterAnalyzer::new());
    let agent = FarcasterAgent::new(
        Arc::new(backend),
        Arc::new(farga),
        Arc::clone(&analyzer) as Arc<dyn FarcasterAnalyzer>,
        projects,
        agent_ids,
    );
    (agent, dms, messages, farga_calls, analyzer, governance_calls)
}

// --- Task 6 test ---
#[tokio::test]
async fn farcaster_agent_new_initializes_with_defaults() {
    let (agent, _, _, _, _, _) = make_agent(
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

// --- Task 7 tests ---

#[tokio::test]
async fn insignificant_events_skip_analyzer() {
    let (agent, _, _, _, _analyzer, _) = make_agent(
        vec![ProjectId::new("alpha"), ProjectId::new("beta")],
        HashMap::new(),
    );
    // EvaluationCompleted with satisfied=false is NOT significant
    let event = MilestoneEvent::EvaluationCompleted {
        mission_id: "m1".into(),
        project_id: ProjectId::new("alpha"),
        sub_objective_id: "obj-1".into(),
        satisfied: false,
        reason: "not done yet".into(),
    };
    agent.on_milestone(&event).await.unwrap();
    // digest_buffer should be empty (no deferred entry either)
    let digest = agent.digest_buffer.lock().await;
    assert!(digest.is_empty(), "insignificant event should not populate digest_buffer");
}

#[tokio::test]
async fn significant_event_triggers_analysis_and_dm() {
    let projects = vec![ProjectId::new("alpha"), ProjectId::new("beta")];
    let beta_agent_id = UserId::new("@beta-agent:matrix.test");
    let mut ids = HashMap::new();
    ids.insert(ProjectId::new("beta"), beta_agent_id.clone());

    let (agent, dms, _, _, analyzer, _) = make_agent(projects, ids);

    // Queue a High-urgency connection from alpha → beta
    analyzer.queue_reactive(vec![
        CrossSpaceConnection {
            from_project: ProjectId::new("alpha"),
            to_project: ProjectId::new("beta"),
            connection_type: "solved_problem".into(),
            summary: "auth schema resolution is applicable here".into(),
            urgency: Urgency::High,
        },
    ], 100);

    let event = MilestoneEvent::ArtifactProduced {
        mission_id: "m1".into(),
        session_id: "s1".into(),
        project_id: ProjectId::new("alpha"),
        canvas_id: "auth".into(),
        artifact_summary: "finalized auth schema".into(),
        sub_objective_ids: vec![],
    };

    agent.on_milestone(&event).await.unwrap();

    // DM should have been sent to beta-agent
    let sent_dms = dms.lock().await;
    assert_eq!(sent_dms.len(), 1, "expected one DM to beta agent");
    assert_eq!(sent_dms[0].0, beta_agent_id);
    assert!(sent_dms[0].1.contains("[farcaster]"), "DM should be tagged");
    assert!(sent_dms[0].1.contains("auth schema resolution"), "DM should include connection summary");

    // DigestEntry should be recorded
    let digest = agent.digest_buffer.lock().await;
    assert_eq!(digest.len(), 1);
    assert!(digest[0].whispered_at.is_some(), "High urgency should set whispered_at");
}

#[tokio::test]
async fn low_urgency_connection_skips_dm_but_records_digest_entry() {
    let projects = vec![ProjectId::new("alpha"), ProjectId::new("beta")];
    let mut ids = HashMap::new();
    ids.insert(ProjectId::new("beta"), UserId::new("@beta:t"));
    let (agent, dms, _, _, analyzer, _) = make_agent(projects, ids);

    analyzer.queue_reactive(vec![
        CrossSpaceConnection {
            from_project: ProjectId::new("alpha"),
            to_project: ProjectId::new("beta"),
            connection_type: "convergence_opportunity".into(),
            summary: "might share a library eventually".into(),
            urgency: Urgency::Low,
        },
    ], 50);

    let event = MilestoneEvent::ArtifactProduced {
        mission_id: "m1".into(),
        session_id: "s1".into(),
        project_id: ProjectId::new("alpha"),
        canvas_id: "c1".into(),
        artifact_summary: "drafted new API".into(),
        sub_objective_ids: vec![],
    };
    agent.on_milestone(&event).await.unwrap();

    assert!(dms.lock().await.is_empty(), "Low urgency should not DM");
    let digest = agent.digest_buffer.lock().await;
    assert_eq!(digest.len(), 1);
    assert!(digest[0].whispered_at.is_none(), "Low urgency should not set whispered_at");
}

// --- Task 8 tests ---

#[tokio::test]
async fn tick_with_empty_buffer_does_nothing() {
    let (agent, _, _, farga_calls, _, _) = make_agent(vec![], HashMap::new());
    agent.tick().await.unwrap();
    assert!(farga_calls.lock().await.is_empty());
}

#[tokio::test]
async fn tick_synthesizes_and_broadcasts_digest() {
    let projects = vec![ProjectId::new("alpha")];
    let (agent, _, messages, _, analyzer, _) = make_agent(projects, HashMap::new());

    // Pre-populate digest_buffer directly
    {
        let mut buf = agent.digest_buffer.lock().await;
        buf.push(DigestEntry {
            project_id: ProjectId::new("alpha"),
            connection_summary: "shared auth approach".into(),
            involved_projects: vec![ProjectId::new("alpha"), ProjectId::new("beta")],
            concurrence: vec![],
            urgency: Urgency::Medium,
            whispered_at: Some(chrono::Utc::now()),
            first_observed_at: chrono::Utc::now(),
        });
    }

    analyzer.queue_digest(DigestSynthesis {
        connections: vec!["alpha and beta share auth approach".into()],
        lessons: vec!["centralize auth early".into()],
        open_questions: vec!["which library?".into()],
        farga_verdict: "skip".into(),  // skip Farga submission in this test
        farga_title: None,
        farga_narrative: None,
    }, 200);

    agent.tick().await.unwrap();

    let sent = messages.lock().await;
    assert_eq!(sent.len(), 1, "digest should be broadcast to one room");
    assert!(sent[0].1.contains("Farcaster Digest"), "broadcast should be formatted digest");
    assert!(sent[0].1.contains("centralize auth early"), "digest should include lesson");
}

#[tokio::test]
async fn tick_submits_to_farga_on_submit_verdict() {
    let projects = vec![ProjectId::new("alpha")];
    let (agent, _, _, _, analyzer, governance_calls) = make_agent(projects, HashMap::new());

    {
        let mut buf = agent.digest_buffer.lock().await;
        buf.push(DigestEntry {
            project_id: ProjectId::new("alpha"),
            connection_summary: "important pattern".into(),
            involved_projects: vec![ProjectId::new("alpha")],
            concurrence: vec![],
            urgency: Urgency::High,
            whispered_at: None,
            first_observed_at: chrono::Utc::now(),
        });
    }

    analyzer.queue_digest(DigestSynthesis {
        connections: vec!["alpha discovered replan pattern".into()],
        lessons: vec!["aggressive replan budgets waste tokens".into()],
        open_questions: vec![],
        farga_verdict: "submit".into(),
        farga_title: Some("Replan Budget Lessons".into()),
        farga_narrative: Some("Projects replanning aggressively see 40% token waste.".into()),
    }, 500);

    agent.tick().await.unwrap();

    let gcalls = governance_calls.lock().await;
    assert_eq!(gcalls.len(), 1, "expected one governance contribution");
    assert_eq!(gcalls[0].title, "Replan Budget Lessons");
}

#[tokio::test]
async fn tick_requeues_entries_on_farga_failure() {
    use charradissa_core::farcaster::analyzer::FarcasterAnalyzer;

    struct FailingFarga;
    #[async_trait]
    impl FargaWriter for FailingFarga {
        async fn write_signals(&self, _: &ProjectId, _: Vec<Signal>) -> Result<()> {
            Err(CharradissaError::Backend("simulated failure".into()))
        }
        async fn recent_signals(&self, _: &ProjectId, _: chrono::Duration) -> Result<Vec<Signal>> { Ok(vec![]) }
        async fn submit_governance_decision(&self, _: charradissa_core::farga::GovernanceDecision) -> Result<()> {
            Ok(())
        }
    }

    let (backend, _, _) = MockChatBackend::new();
    let analyzer = Arc::new(MockFarcasterAnalyzer::new());
    let agent = FarcasterAgent::new(
        Arc::new(backend),
        Arc::new(FailingFarga),
        Arc::clone(&analyzer) as Arc<dyn FarcasterAnalyzer>,
        vec![],
        HashMap::new(),
    );

    {
        let mut buf = agent.digest_buffer.lock().await;
        buf.push(DigestEntry {
            project_id: ProjectId::new("alpha"),
            connection_summary: "test entry".into(),
            involved_projects: vec![],
            concurrence: vec![],
            urgency: Urgency::Low,
            whispered_at: None,
            first_observed_at: chrono::Utc::now(),
        });
    }

    analyzer.queue_digest(DigestSynthesis {
        connections: vec![],
        lessons: vec!["lesson".into()],
        open_questions: vec![],
        farga_verdict: "submit".into(),
        farga_title: Some("T".into()),
        farga_narrative: Some("N".into()),
    }, 100);

    agent.tick().await.unwrap();  // should not panic

    // Buffer should be re-queued due to failure
    let buf = agent.digest_buffer.lock().await;
    assert_eq!(buf.len(), 1, "entry should be re-queued after Farga failure");
}

#[tokio::test]
async fn tick_emits_governance_contribution_on_submit_verdict() {
    let projects = vec![ProjectId::new("alpha"), ProjectId::new("beta")];
    let (agent, _, _, _, analyzer, governance_calls) = make_agent(projects, HashMap::new());

    {
        let mut buf = agent.digest_buffer.lock().await;
        buf.push(DigestEntry {
            project_id: ProjectId::new("alpha"),
            connection_summary: "shared auth pattern".into(),
            involved_projects: vec![ProjectId::new("alpha"), ProjectId::new("beta")],
            concurrence: vec![],
            urgency: Urgency::High,
            whispered_at: None,
            first_observed_at: chrono::Utc::now(),
        });
    }

    analyzer.queue_digest(DigestSynthesis {
        connections: vec!["alpha and beta both chose RS256".into()],
        lessons: vec!["Use RS256 org-wide".into()],
        open_questions: vec![],
        farga_verdict: "submit".into(),
        farga_title: Some("JWT Signing Pattern".into()),
        farga_narrative: Some("Two projects independently converged on RS256.".into()),
    }, 300);

    agent.tick().await.unwrap();

    let gcalls = governance_calls.lock().await;
    assert_eq!(gcalls.len(), 1);
    let parsed = &gcalls[0];
    assert_eq!(parsed.title, "JWT Signing Pattern");
    assert_eq!(parsed.event_count, 1);
    assert_eq!(parsed.target_layer, FargaLayer::ProjectLevel);
    assert!(parsed.reversibility.is_none());
    assert!(parsed.impact.is_none());
}

// --- Task 9 tests ---

#[tokio::test]
async fn reactive_budget_exhausted_defers_to_digest_without_calling_analyzer() {
    let projects = vec![ProjectId::new("alpha"), ProjectId::new("beta")];
    let (agent, dms, _, _, _analyzer, _) = make_agent(projects, HashMap::new());

    // Exhaust the reactive budget
    agent.reactive_tokens_used.store(20_000, std::sync::atomic::Ordering::Relaxed);

    let event = MilestoneEvent::ArtifactProduced {
        mission_id: "m1".into(),
        session_id: "s1".into(),
        project_id: ProjectId::new("alpha"),
        canvas_id: "c1".into(),
        artifact_summary: "some artifact".into(),
        sub_objective_ids: vec![],
    };

    // No response queued in analyzer — would panic if called
    agent.on_milestone(&event).await.unwrap();

    // Should NOT have called analyzer (no queue pop) and NOT DM'd anyone
    assert!(dms.lock().await.is_empty(), "budget exhaustion should skip DMs");

    // But should have added a deferred entry to digest_buffer
    let digest = agent.digest_buffer.lock().await;
    assert_eq!(digest.len(), 1, "deferred entry should be in digest_buffer");
    assert!(digest[0].connection_summary.contains("deferred"),
        "deferred entry summary should indicate it was deferred");
}

#[tokio::test]
async fn digest_budget_exhausted_requeues_buffer_without_calling_analyzer() {
    let (agent, _, _, farga_calls, _analyzer, _) = make_agent(vec![], HashMap::new());

    // Exhaust the digest budget
    agent.digest_tokens_used.store(10_000, std::sync::atomic::Ordering::Relaxed);

    // Add an entry to the buffer
    {
        let mut buf = agent.digest_buffer.lock().await;
        buf.push(DigestEntry {
            project_id: ProjectId::new("alpha"),
            connection_summary: "pending entry".into(),
            involved_projects: vec![],
            concurrence: vec![],
            urgency: Urgency::Low,
            whispered_at: None,
            first_observed_at: chrono::Utc::now(),
        });
    }

    // No response queued in analyzer — would panic if called
    agent.tick().await.unwrap();

    // No Farga call
    assert!(farga_calls.lock().await.is_empty());

    // Buffer re-queued
    let buf = agent.digest_buffer.lock().await;
    assert_eq!(buf.len(), 1, "buffer should be re-queued when digest budget exhausted");
}

/// Integration test — requires ANTHROPIC_API_KEY env var. Run with:
/// cargo test -p charradissa-core --test farcaster_tests integration_claude_analyzer -- --ignored
#[tokio::test]
#[ignore]
async fn integration_claude_analyzer_produces_connections() {
    use charradissa_core::farcaster::claude_analyzer::ClaudeFarcasterAnalyzer;
    use charradissa_core::farcaster::analyzer::{CrossSpaceSnapshot, ProjectSnapshot};

    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .expect("ANTHROPIC_API_KEY required for integration test");

    let analyzer = ClaudeFarcasterAnalyzer::new(api_key);

    let event = MilestoneEvent::ArtifactProduced {
        mission_id: "m1".into(),
        session_id: "s1".into(),
        project_id: ProjectId::new("auth-service"),
        canvas_id: "design".into(),
        artifact_summary: "JWT token validation logic finalized using RS256".into(),
        sub_objective_ids: vec![],
    };

    let snapshot = CrossSpaceSnapshot {
        projects: vec![
            ProjectSnapshot {
                project_id: ProjectId::new("auth-service"),
                mission_goal: Some("implement authentication".into()),
                open_sub_objectives: vec!["write tests".into()],
                recent_events: vec!["finalized JWT logic".into()],
            },
            ProjectSnapshot {
                project_id: ProjectId::new("api-gateway"),
                mission_goal: Some("route and validate API requests".into()),
                open_sub_objectives: vec!["token verification middleware".into()],
                recent_events: vec!["started middleware spike".into()],
            },
        ],
    };

    let (connections, tokens) = analyzer.analyze_cross_space(&event, &snapshot).await.unwrap();

    println!("connections: {:?}", connections.iter().map(|c| &c.summary).collect::<Vec<_>>());
    println!("tokens used: {}", tokens);

    // The call should return without error and parse cleanly.
    // A real LLM should find at least one connection between auth-service and api-gateway.
    assert!(tokens > 0, "should report token usage");
}

#[test]
fn governance_contribution_serializes_round_trip() {
    use charradissa_core::farcaster::concurrence::{AgentConcurrence, ConcurrenceType};

    let contrib = GovernanceContribution {
        title: "Auth pattern discovered".into(),
        narrative: "Two projects independently chose RS256".into(),
        lessons: vec!["Use RS256 for JWT signing".into()],
        open_questions: vec!["Should we centralize key rotation?".into()],
        involved_projects: vec![ProjectId::new("auth"), ProjectId::new("gateway")],
        concurrence: vec![AgentConcurrence {
            project_id: "auth".into(),
            agent_address: "auth+adversarial".into(),
            concurrence_type: ConcurrenceType::Whispered,
            note: None,
        }],
        target_layer: FargaLayer::ProjectLevel,
        first_observed_at: chrono::Utc::now(),
        last_observed_at: chrono::Utc::now(),
        event_count: 3,
        reversibility: Some(ReversibilityLevel::FullyReversible),
        impact: None,
    };

    let json = serde_json::to_string(&contrib).unwrap();
    let decoded: GovernanceContribution = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.title, "Auth pattern discovered");
    assert_eq!(decoded.event_count, 3);
    assert!(decoded.impact.is_none());
    assert_eq!(decoded.reversibility, Some(ReversibilityLevel::FullyReversible));
}

// --- Task 10 test ---

#[tokio::test]
async fn run_tick_evaluates_governance_risk_on_submit() {
    // Verify that run_tick calls evaluate_governance and submits contribution to farga
    let projects = vec![ProjectId::new("proj-a")];
    let (agent, _dms, messages, _farga_calls, analyzer, governance_calls) =
        make_agent(projects, HashMap::new());

    let synthesis = DigestSynthesis {
        connections: vec!["conn".into()],
        lessons: vec!["lesson".into()],
        open_questions: vec![],
        farga_verdict: "submit".into(),
        farga_title: Some("Pattern detected".into()),
        farga_narrative: Some("Cross-project observation".into()),
    };
    analyzer.queue_digest(synthesis, 500);

    // Seed one digest entry
    {
        let mut buf = agent.digest_buffer.lock().await;
        buf.push(DigestEntry {
            project_id: ProjectId::new("proj-a"),
            connection_summary: "observed".into(),
            involved_projects: vec![ProjectId::new("proj-a")],
            concurrence: vec![],
            urgency: Urgency::Low,
            whispered_at: None,
            first_observed_at: chrono::Utc::now(),
        });
    }

    let result = agent.tick().await;
    assert!(result.is_ok());

    // Governance contribution was submitted
    let gov = governance_calls.lock().await;
    assert_eq!(gov.len(), 1, "should have submitted one governance contribution");
    assert_eq!(gov[0].title, "Pattern detected");

    // Digest broadcast happened (always sent, regardless of tier)
    let msgs = messages.lock().await;
    assert!(msgs.iter().any(|(room, _)| room.as_str() == "#farcaster"),
        "digest broadcast should go to #farcaster");
}

#[test]
fn farga_layer_variants_serialize_to_distinct_strings() {
    let variants = [FargaLayer::OrgLevel, FargaLayer::InitiativeLevel, FargaLayer::ProjectLevel];
    let serialized: Vec<String> = variants.iter()
        .map(|v| serde_json::to_string(v).unwrap())
        .collect();
    let unique: std::collections::HashSet<_> = serialized.iter().collect();
    assert_eq!(unique.len(), 3);
    assert_eq!(serde_json::to_string(&FargaLayer::OrgLevel).unwrap(), r#""OrgLevel""#);
}

#[test]
fn http_farga_writer_is_send_sync() {
    fn _assert_send_sync<T: Send + Sync>() {}
    _assert_send_sync::<charradissa_core::farga::HttpFargaWriter>();
}

#[test]
fn all_reversibility_and_impact_variants_serialize() {
    let rev = [
        ReversibilityLevel::FullyReversible,
        ReversibilityLevel::EffectsLinger,
        ReversibilityLevel::CostlyReversible,
        ReversibilityLevel::Irreversible,
    ];
    let unique_rev: std::collections::HashSet<_> = rev.iter()
        .map(|r| serde_json::to_string(r).unwrap())
        .collect();
    assert_eq!(unique_rev.len(), 4);

    let imp = [
        ImpactScope::Contained,
        ImpactScope::CrossProject,
        ImpactScope::DomainWide,
        ImpactScope::OrgWide,
    ];
    let unique_imp: std::collections::HashSet<_> = imp.iter()
        .map(|i| serde_json::to_string(i).unwrap())
        .collect();
    assert_eq!(unique_imp.len(), 4);
}

// --- derive_risk_factors / evaluate_governance tests ---

use charradissa_core::farcaster::governance::{derive_risk_factors, evaluate_governance};
use amassada_core::governance::{GovernanceConfig, RiskTier};

fn make_minimal_contribution() -> GovernanceContribution {
    GovernanceContribution {
        title: "test".into(),
        narrative: "test narrative".into(),
        lessons: vec![],
        open_questions: vec![],
        involved_projects: vec![ProjectId::new("proj-a")],
        concurrence: vec![],
        target_layer: FargaLayer::ProjectLevel,
        first_observed_at: chrono::Utc::now(),
        last_observed_at: chrono::Utc::now(),
        event_count: 1,
        reversibility: None,
        impact: None,
    }
}

#[test]
fn derive_risk_factors_project_level_has_low_proximity() {
    let contrib = make_minimal_contribution();
    let factors = derive_risk_factors(&contrib);
    assert!(factors.primitive_proximity < 0.5);
}

#[test]
fn derive_risk_factors_org_level_has_high_proximity() {
    let mut contrib = make_minimal_contribution();
    contrib.target_layer = FargaLayer::OrgLevel;
    let factors = derive_risk_factors(&contrib);
    assert!(factors.primitive_proximity > 0.9);
}

#[test]
fn derive_risk_factors_irreversible_sets_flag() {
    let mut contrib = make_minimal_contribution();
    contrib.reversibility = Some(ReversibilityLevel::Irreversible);
    let factors = derive_risk_factors(&contrib);
    assert!(factors.is_irreversible);
    assert_eq!(factors.reversibility, 1.0);
}

#[test]
fn derive_risk_factors_org_wide_sets_flag() {
    let mut contrib = make_minimal_contribution();
    contrib.impact = Some(ImpactScope::OrgWide);
    let factors = derive_risk_factors(&contrib);
    assert!(factors.is_org_wide);
    assert_eq!(factors.impact, 1.0);
}

#[test]
fn derive_risk_factors_none_reversibility_is_zero() {
    let contrib = make_minimal_contribution();
    let factors = derive_risk_factors(&contrib);
    assert_eq!(factors.reversibility, 0.0);
    assert!(!factors.is_irreversible);
}

#[test]
fn derive_risk_factors_concurrence_caps_at_one() {
    let mut contrib = make_minimal_contribution();
    for i in 0..5 {
        contrib.concurrence.push(AgentConcurrence {
            project_id: format!("p{}", i),
            agent_address: format!("a{}", i),
            concurrence_type: ConcurrenceType::Whispered,
            note: None,
        });
    }
    let factors = derive_risk_factors(&contrib);
    assert!(factors.signal_concurrence <= 1.0);
}

#[test]
fn evaluate_governance_returns_session_composition() {
    let contrib = make_minimal_contribution();
    let config = GovernanceConfig::default_weights();
    let composition = evaluate_governance(&contrib, &config);
    assert!(matches!(composition.tier, RiskTier::Low | RiskTier::Medium | RiskTier::High | RiskTier::Critical));
    assert!(!composition.primary_session.is_empty());
}

#[test]
fn evaluate_governance_org_wide_irreversible_is_critical() {
    let mut contrib = make_minimal_contribution();
    contrib.target_layer = FargaLayer::OrgLevel;
    contrib.impact = Some(ImpactScope::OrgWide);
    contrib.reversibility = Some(ReversibilityLevel::Irreversible);
    contrib.event_count = 10;
    for i in 0..2 {
        contrib.concurrence.push(AgentConcurrence {
            project_id: format!("p{}", i),
            agent_address: format!("a{}", i),
            concurrence_type: ConcurrenceType::Whispered,
            note: None,
        });
    }
    let config = GovernanceConfig::default_weights();
    let composition = evaluate_governance(&contrib, &config);
    assert_eq!(composition.tier, RiskTier::Critical);
}
