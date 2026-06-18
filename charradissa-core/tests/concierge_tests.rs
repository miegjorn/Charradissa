use charradissa_core::concierge::extract_signals;

#[test]
fn extracts_signals_from_events() {
    use charradissa_core::types::{ChatEvent, ChatEventKind, RoomId, UserId};
    use chrono::Utc;

    let events = vec![
        ChatEvent {
            event_id: "1".into(),
            room_id: RoomId::new("!room:matrix.test"),
            sender: UserId::new("@agent:matrix.test"),
            content: "Decision: use PostgreSQL for the user service.".into(),
            timestamp: Utc::now(),
            kind: ChatEventKind::Message,
        },
        ChatEvent {
            event_id: "2".into(),
            room_id: RoomId::new("!room:matrix.test"),
            sender: UserId::new("@human:matrix.test"),
            content: "Blocker: we need a design review before proceeding.".into(),
            timestamp: Utc::now(),
            kind: ChatEventKind::Message,
        },
    ];

    let signals = extract_signals(&events);
    assert!(!signals.is_empty());
}

use std::sync::Arc;
use async_trait::async_trait;
use charradissa_core::concierge::ConciergeAgent;
use charradissa_core::farcaster::milestone::MilestoneEvent;
use charradissa_core::farcaster::system_agent::SystemAgent;
use charradissa_core::types::ProjectId;
use charradissa_core::error::Result;

struct RecordingAgent {
    name_str: String,
    calls: Arc<tokio::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl SystemAgent for RecordingAgent {
    fn name(&self) -> &str { &self.name_str }
    async fn on_milestone(&self, event: &MilestoneEvent) -> Result<()> {
        self.calls.lock().await.push(event.summary());
        Ok(())
    }
    async fn tick(&self) -> Result<()> { Ok(()) }
}

#[tokio::test]
async fn dispatch_milestone_fans_out_to_all_agents() {
    use charradissa_core::backend::ChatBackend;
    use charradissa_core::farga::FargaWriter;

    struct StubBackend;
    struct StubFarga;

    #[async_trait]
    impl ChatBackend for StubBackend {
        async fn send_message(&self, _: &charradissa_core::types::RoomId, _: &str) -> Result<()> { Ok(()) }
        async fn send_dm(&self, _: &charradissa_core::types::UserId, _: &str) -> Result<()> { Ok(()) }
        async fn create_room(&self, _: &charradissa_core::types::RoomOptions) -> Result<charradissa_core::types::RoomId> { Ok(charradissa_core::types::RoomId::new("!r:t")) }
        async fn create_space(&self, _: &str) -> Result<charradissa_core::types::SpaceId> { Ok(charradissa_core::types::SpaceId::new("!s:t")) }
        async fn add_to_space(&self, _: &charradissa_core::types::SpaceId, _: &charradissa_core::types::RoomId) -> Result<()> { Ok(()) }
        async fn invite(&self, _: &charradissa_core::types::RoomId, _: &charradissa_core::types::UserId) -> Result<()> { Ok(()) }
        async fn kick(&self, _: &charradissa_core::types::RoomId, _: &charradissa_core::types::UserId, _: &str) -> Result<()> { Ok(()) }
        async fn register_agent(&self, _: &charradissa_core::types::CompositionAddress) -> Result<charradissa_core::types::UserId> { Ok(charradissa_core::types::UserId::new("@x:t")) }
        async fn deregister_agent(&self, _: &charradissa_core::types::UserId) -> Result<()> { Ok(()) }
        async fn room_history(&self, _: &charradissa_core::types::RoomId, _: chrono::DateTime<chrono::Utc>) -> Result<Vec<charradissa_core::types::ChatEvent>> { Ok(vec![]) }
        async fn delete_room(&self, _: &charradissa_core::types::RoomId) -> Result<()> { Ok(()) }
    }

    #[async_trait]
    impl FargaWriter for StubFarga {
        async fn write_signals(&self, _: &charradissa_core::types::ProjectId, _: Vec<charradissa_core::farga::Signal>) -> Result<()> { Ok(()) }
        async fn recent_signals(&self, _: &charradissa_core::types::ProjectId, _: chrono::Duration) -> Result<Vec<charradissa_core::farga::Signal>> { Ok(vec![]) }
    }

    let calls_a = Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
    let calls_b = Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));

    let mut concierge = ConciergeAgent::new(
        Arc::new(StubBackend),
        Arc::new(StubFarga),
        vec![],
        std::collections::HashMap::new(),
        24, 6, 10_000,
    );

    concierge.register_system_agent(
        Box::new(RecordingAgent { name_str: "a".into(), calls: Arc::clone(&calls_a) }),
        std::time::Duration::from_secs(3600),
    );
    concierge.register_system_agent(
        Box::new(RecordingAgent { name_str: "b".into(), calls: Arc::clone(&calls_b) }),
        std::time::Duration::from_secs(3600),
    );

    let event = MilestoneEvent::MissionCompleted {
        mission_id: "m1".into(),
        project_id: ProjectId::new("alpha"),
        goal: "ship auth".into(),
        completed_sub_objectives: vec![],
        verdict: "submit".into(),
    };

    concierge.dispatch_milestone(&event).await;

    assert_eq!(calls_a.lock().await.len(), 1);
    assert_eq!(calls_b.lock().await.len(), 1);
    assert!(calls_a.lock().await[0].contains("ship auth"));
}
