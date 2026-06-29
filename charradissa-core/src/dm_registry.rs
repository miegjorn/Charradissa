//! DM room registry — resolves the DM room between `@guilhem` and a component agent.
//!
//! ## Storage contract (Charradissa#22 ↔ #23)
//!
//! The registry is a flat JSON object mapping a component agent's *localpart* to the
//! Matrix room ID of its DM with `@guilhem`:
//!
//! ```json
//! {
//!   "farga":    "!abc:occitane.guilhem",
//!   "amassada": "!def:occitane.guilhem"
//! }
//! ```
//!
//! Charradissa#22 provisions these DM rooms at startup and persists this file (or a
//! ConfigMap mounted at this path). Charradissa#23 (this module) *reads* it to back the
//! `matrix_get_dm` MCP tool.
//!
//! **Dependency note:** #22 was not merged when #23 was implemented. This module is the
//! agreed-upon contract: a JSON object keyed by agent localpart, located at the path named
//! by the `CHARRADISSA_DM_REGISTRY` env var (defaulting to `charradissa-dm-registry.json`).
//! #22 only has to start writing this same shape at this same location. A missing file
//! resolves to an empty registry so a not-yet-provisioned stack degrades gracefully rather
//! than crashing.

use crate::error::{CharradissaError, Result};
use std::collections::HashMap;

/// Env var naming the DM registry file path.
pub const DM_REGISTRY_ENV: &str = "CHARRADISSA_DM_REGISTRY";

/// Default registry file path used when [`DM_REGISTRY_ENV`] is unset.
pub const DEFAULT_DM_REGISTRY_PATH: &str = "charradissa-dm-registry.json";

/// Registry of DM room IDs between `@guilhem` and each component agent, keyed by localpart.
#[derive(Debug, Clone, Default)]
pub struct DmRegistry {
    rooms: HashMap<String, String>,
}

impl DmRegistry {
    /// Construct directly from a localpart → room_id map (used in tests and by #22 in-process).
    pub fn from_map(rooms: HashMap<String, String>) -> Self {
        Self { rooms }
    }

    /// Load from a JSON file at `path`.
    ///
    /// A missing file yields an empty registry (graceful degradation before #22 provisions).
    /// A present-but-malformed file is a hard error so misconfiguration is visible.
    pub fn from_file(path: &str) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(content) => {
                let rooms: HashMap<String, String> = serde_json::from_str(&content)
                    .map_err(|e| CharradissaError::Config(format!("DM registry parse error: {e}")))?;
                Ok(Self { rooms })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(CharradissaError::Io(e)),
        }
    }

    /// Load from the path named by [`DM_REGISTRY_ENV`], defaulting to
    /// [`DEFAULT_DM_REGISTRY_PATH`].
    pub fn from_env() -> Result<Self> {
        let path = std::env::var(DM_REGISTRY_ENV)
            .unwrap_or_else(|_| DEFAULT_DM_REGISTRY_PATH.to_string());
        Self::from_file(&path)
    }

    /// Resolve the DM room ID for an agent identity.
    ///
    /// Accepts either a bare localpart (`farga`) or a full MXID (`@farga:occitane.guilhem`);
    /// both resolve via the localpart key.
    pub fn resolve(&self, agent: &str) -> Option<&str> {
        self.rooms.get(&normalize_agent(agent)).map(|s| s.as_str())
    }

    pub fn len(&self) -> usize {
        self.rooms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rooms.is_empty()
    }
}

/// Normalize an agent identity to its localpart key: strip a leading `@` and any `:server`
/// suffix. `@farga:occitane.guilhem` and `farga` both normalize to `farga`.
pub fn normalize_agent(agent: &str) -> String {
    let a = agent.strip_prefix('@').unwrap_or(agent);
    a.split(':').next().unwrap_or(a).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> DmRegistry {
        let mut m = HashMap::new();
        m.insert("farga".to_string(), "!abc:occitane.guilhem".to_string());
        m.insert("amassada".to_string(), "!def:occitane.guilhem".to_string());
        DmRegistry::from_map(m)
    }

    #[test]
    fn resolves_by_bare_localpart() {
        assert_eq!(sample().resolve("farga"), Some("!abc:occitane.guilhem"));
    }

    #[test]
    fn resolves_by_full_mxid() {
        // matrix_get_dm callers may pass either form.
        assert_eq!(sample().resolve("@amassada:occitane.guilhem"), Some("!def:occitane.guilhem"));
    }

    #[test]
    fn unknown_agent_resolves_none() {
        assert_eq!(sample().resolve("nonexistent"), None);
    }

    #[test]
    fn normalize_strips_sigil_and_server() {
        assert_eq!(normalize_agent("@farga:occitane.guilhem"), "farga");
        assert_eq!(normalize_agent("farga"), "farga");
    }

    #[test]
    fn missing_file_yields_empty_registry() {
        let reg = DmRegistry::from_file("/no/such/dm-registry-xyz.json").expect("missing file is ok");
        assert!(reg.is_empty());
        assert_eq!(reg.resolve("farga"), None);
    }

    #[test]
    fn loads_and_resolves_from_json_file() {
        // Write to a unique temp path (uuid is already a workspace dep).
        let path = std::env::temp_dir().join(format!("dm-registry-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(&path, r#"{"farga":"!room:occitane.guilhem"}"#).unwrap();
        let reg = DmRegistry::from_file(path.to_str().unwrap()).unwrap();
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.resolve("farga"), Some("!room:occitane.guilhem"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn malformed_file_is_hard_error() {
        let path = std::env::temp_dir().join(format!("dm-registry-bad-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(&path, "not json at all").unwrap();
        assert!(DmRegistry::from_file(path.to_str().unwrap()).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
