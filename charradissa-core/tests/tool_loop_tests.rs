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
fn parses_mission_with_budget() {
    let cmd = parse_slash_command("/mission 50000 design our auth system");
    assert!(matches!(cmd, Some(SlashCommand::Mission { budget_tokens: 50000, .. })));
    if let Some(SlashCommand::Mission { goal, budget_tokens }) = cmd {
        assert_eq!(budget_tokens, 50000);
        assert_eq!(goal, "design our auth system");
    }
}

#[test]
fn parses_mission_without_budget_uses_default() {
    let cmd = parse_slash_command("/mission improve the onboarding flow");
    assert!(matches!(cmd, Some(SlashCommand::Mission { .. })));
    if let Some(SlashCommand::Mission { budget_tokens, .. }) = cmd {
        assert_eq!(budget_tokens, 100_000);
    }
}

#[test]
fn returns_none_for_unknown() {
    let cmd = parse_slash_command("/unknown foo");
    assert!(cmd.is_none());
}
