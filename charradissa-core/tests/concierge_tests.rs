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
