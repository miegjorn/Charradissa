use charradissa_core::types::*;

#[test]
fn room_id_roundtrip() {
    let id = RoomId::new("!abc123:matrix.acme.internal");
    assert_eq!(id.as_str(), "!abc123:matrix.acme.internal");
}

#[test]
fn composition_address_display() {
    let addr = CompositionAddress::Role { role: "tech-moderator".into(), stance_override: None };
    assert!(addr.to_string().contains("tech-moderator"));
}
