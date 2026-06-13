use charradissa_core::tool_loop::{parse_slash_command, SlashCommand};

#[test]
fn parses_approve_command() {
    let cmd = parse_slash_command("/approve abc-123");
    assert_eq!(cmd, Some(SlashCommand::Approve { id: "abc-123".into() }));
}

#[test]
fn parses_reject_command() {
    let cmd = parse_slash_command("/reject abc-123 too risky");
    assert_eq!(cmd, Some(SlashCommand::Reject { id: "abc-123".into(), reason: "too risky".into() }));
}

#[test]
fn parses_session_command() {
    let cmd = parse_slash_command("/session debate \"should we use microservices?\"");
    assert!(matches!(cmd, Some(SlashCommand::Session { .. })));
}

#[test]
fn returns_none_for_unknown() {
    let cmd = parse_slash_command("/unknown foo");
    assert!(cmd.is_none());
}
