use charradissa_core::farcaster::milestone::MilestoneEvent;
use charradissa_core::farcaster::concurrence::{AgentConcurrence, ConcurrenceType, Urgency};
use charradissa_core::types::ProjectId;

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
