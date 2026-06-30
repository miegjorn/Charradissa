//! Tests for appservice registration generation.
//!
//! The eight component agents (@guilhem, @gardian, @fondament, @farga, @amassada,
//! @cor, @caissa, @nervi) must each appear in the appservice user namespace so they
//! can respond as their own Matrix identity. @charradissa is the AS sender itself
//! and is already in the namespace via sender_localpart.

use charradissa_core::registration::{
    generate_registration, RegistrationParams, COMPONENT_AGENT_LOCALPARTS,
};

fn sample_params() -> RegistrationParams {
    RegistrationParams {
        id: "charradissa".into(),
        url: "http://charradissa:8448".into(),
        as_token: "as-secret".into(),
        hs_token: "hs-secret".into(),
        sender_localpart: "charradissa".into(),
        server_name: "occitane.guilhem".into(),
        component_localparts: COMPONENT_AGENT_LOCALPARTS
            .iter()
            .map(|s| s.to_string())
            .collect(),
    }
}

#[test]
fn there_are_eight_canonical_component_agents() {
    assert_eq!(COMPONENT_AGENT_LOCALPARTS.len(), 8);
    for lp in ["guilhem", "gardian", "fondament", "farga", "amassada", "cor", "caissa", "nervi"] {
        assert!(
            COMPONENT_AGENT_LOCALPARTS.contains(&lp),
            "canonical component agent {lp} is missing"
        );
    }
}

#[test]
fn embeds_identity_tokens_and_url() {
    let yaml = generate_registration(&sample_params());
    assert!(yaml.contains("id: charradissa"));
    assert!(yaml.contains("url: http://charradissa:8448"));
    assert!(yaml.contains("as_token: as-secret"));
    assert!(yaml.contains("hs_token: hs-secret"));
    assert!(yaml.contains("sender_localpart: charradissa"));
}

#[test]
fn keeps_sender_and_specialist_namespace() {
    let yaml = generate_registration(&sample_params());
    // The daemon's own identity.
    assert!(yaml.contains(r"@charradissa:occitane\.guilhem"));
    // Specialist virtual users (@charradissa-<project>).
    assert!(yaml.contains("@charradissa-.*"));
}

#[test]
fn includes_every_component_agent_localpart() {
    let yaml = generate_registration(&sample_params());
    for lp in COMPONENT_AGENT_LOCALPARTS {
        assert!(
            yaml.contains(&format!(r"@{lp}:occitane\.guilhem")),
            "user namespace is missing an entry for @{lp}"
        );
    }
}

#[test]
fn escapes_server_name_dots_in_regexes() {
    let yaml = generate_registration(&sample_params());
    // A literal dot in the regex would match any character; it must be escaped.
    assert!(yaml.contains(r"occitane\.guilhem"));
    assert!(!yaml.contains("occitane.guilhem"));
}
