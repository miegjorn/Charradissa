use charradissa_core::config::Config;

#[test]
fn provisioning_defaults_to_occitan_project() {
    let toml = r#"
[org]
name = "dev"
homeserver = "http://synapse:8008"

[backend]
type = "matrix"

[concierge]
[approval]
[tasks]
type = "none"

[projects]
autodiscover = false
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert_eq!(config.provisioning.projects, vec!["occitan"]);
    assert!(config.provisioning.fondament_url.is_none());
}

#[test]
fn provisioning_accepts_explicit_config() {
    let toml = r#"
[org]
name = "dev"
homeserver = "http://synapse:8008"

[backend]
type = "matrix"

[concierge]
[approval]
[tasks]
type = "none"

[projects]
autodiscover = false

[provisioning]
projects = ["occitan", "future"]
fondament_url = "http://fondament:7800"
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert_eq!(config.provisioning.projects, vec!["occitan", "future"]);
    assert_eq!(config.provisioning.fondament_url.as_deref(), Some("http://fondament:7800"));
}
