use charradissa_core::config::AgentsConfig;

const AGENTS_WITH_PROJECT: &str = r#"
[agents]
default = "http://guilhem:8080"

[agents.routes]
"!abc:server" = "http://farga:7500"

[[agents.project]]
type = "amassada_backed"
rooms = ["!proj-a:occitane.guilhem", "!proj-b:occitane.guilhem"]
project_id = "alpha"
endpoint = "http://amassada:7700/sessions/{room_id}/message"

[[agents.project]]
type = "amassada_backed"
rooms = ["!proj-c:occitane.guilhem"]
project_id = "beta"
endpoint = "http://amassada:7700/sessions/{session_id}/message"
"#;

#[derive(serde::Deserialize)]
struct Wrapper {
    agents: AgentsConfig,
}

fn parse(s: &str) -> AgentsConfig {
    toml::from_str::<Wrapper>(s).expect("parse failed").agents
}

#[test]
fn project_configs_parse_from_toml() {
    let cfg = parse(AGENTS_WITH_PROJECT);
    assert_eq!(cfg.project.len(), 2);
    let alpha = &cfg.project[0];
    assert_eq!(alpha.project_id, "alpha");
    assert_eq!(alpha.agent_type, "amassada_backed");
    assert_eq!(alpha.rooms, vec!["!proj-a:occitane.guilhem", "!proj-b:occitane.guilhem"]);
    assert!(alpha.endpoint.contains("{room_id}"));
}

#[test]
fn project_configs_do_not_affect_default_or_routes() {
    let cfg = parse(AGENTS_WITH_PROJECT);
    assert_eq!(cfg.default.as_deref(), Some("http://guilhem:8080"));
    assert_eq!(cfg.routes.get("!abc:server").map(|s| s.as_str()), Some("http://farga:7500"));
}

#[test]
fn empty_project_config_is_valid() {
    let cfg: AgentsConfig = toml::from_str("").unwrap();
    assert!(cfg.project.is_empty());
    assert!(cfg.routes.is_empty());
    assert!(cfg.default.is_none());
}

#[test]
fn project_endpoint_template_contains_substitution_placeholder() {
    // Both the preferred {session_id} handle and the legacy {room_id} form must
    // parse and round-trip — Charradissa substitutes whichever the template uses.
    let cfg = parse(AGENTS_WITH_PROJECT);
    for entry in &cfg.project {
        assert!(
            entry.endpoint.contains("{session_id}") || entry.endpoint.contains("{room_id}"),
            "endpoint '{}' must contain a {{session_id}} or {{room_id}} placeholder",
            entry.endpoint
        );
    }
}
