//! Matrix appservice registration generation.
//!
//! Synapse loads the appservice registration from
//! `/data/charradissa-registration.yaml` (placed out-of-band on its data PVC).
//! This module is the single source of truth for that file's contents;
//! `charradissa-daemon --generate-registration` prints it to stdout.
//!
//! The user namespace covers three kinds of identity:
//!   1. the daemon's own sender (`@charradissa`);
//!   2. specialist virtual users (`@charradissa-<project>`);
//!   3. the eight component agents, each of which responds as its own Matrix
//!      identity rather than as `@charradissa`.

/// The eight Occitan-stack component agents. Each needs its own exclusive entry
/// in the appservice user namespace so it can speak as itself in its room.
pub const COMPONENT_AGENT_LOCALPARTS: [&str; 8] = [
    "guilhem",
    "gardian",
    "fondament",
    "farga",
    "amassada",
    "cor",
    "caissa",
    "nervi",
];

/// Inputs for [`generate_registration`].
pub struct RegistrationParams {
    /// Appservice id (`id:` field). Conventionally `charradissa`.
    pub id: String,
    /// URL Synapse posts transactions to (`url:` field).
    pub url: String,
    /// Appservice token; must match `MATRIX_AS_TOKEN` in the daemon environment.
    pub as_token: String,
    /// Homeserver token Synapse sends back on inbound requests.
    pub hs_token: String,
    /// Localpart of the appservice's own sender user.
    pub sender_localpart: String,
    /// Matrix server name (e.g. `occitane.guilhem`), used to anchor regexes.
    pub server_name: String,
    /// Component agent localparts to grant their own identity.
    pub component_localparts: Vec<String>,
}

/// Render the appservice registration YAML.
pub fn generate_registration(p: &RegistrationParams) -> String {
    // A literal `.` in a regex matches any character; escape it so the server
    // name is matched literally.
    let server = p.server_name.replace('.', r"\.");

    // Build the user-namespace regexes in a stable order.
    let mut regexes = vec![
        // The daemon's own sender identity.
        format!("@{}:{}", p.sender_localpart, server),
        // Specialist virtual users (@charradissa-<project>).
        format!("@{}-.*", p.sender_localpart),
    ];
    for lp in &p.component_localparts {
        regexes.push(format!("@{lp}:{server}"));
    }

    let mut out = String::new();
    out.push_str(&format!("id: {}\n", p.id));
    out.push_str(&format!("url: {}\n", p.url));
    out.push_str(&format!("as_token: {}\n", p.as_token));
    out.push_str(&format!("hs_token: {}\n", p.hs_token));
    out.push_str(&format!("sender_localpart: {}\n", p.sender_localpart));
    out.push_str("namespaces:\n");
    out.push_str("  users:\n");
    for regex in &regexes {
        out.push_str("    - exclusive: true\n");
        out.push_str(&format!("      regex: '{regex}'\n"));
    }
    out.push_str("  rooms: []\n");
    out.push_str("  aliases: []\n");
    out
}
