//! Routing helpers for the project-agent path (Option C, C-1).
//!
//! When a message lands in a project room, Charradissa forwards it to Amassada's
//! `POST /sessions/:session_id/message` endpoint. The `:session_id` path segment
//! is a *session handle*: Amassada keys its in-memory session state on it, so the
//! same handle continues the same conversation. The project is resolved separately,
//! from the `room_id` carried in the request body (Amassada's project registry,
//! Amassada#11).
//!
//! We do **not** put the raw Matrix room id into the URL path. A room id like
//! `!aBcD:occitane.guilhem` contains `!` and `:`, which are awkward (and, for `:`,
//! ambiguous) in a path segment, and it leaks the Matrix room id into Amassada's
//! session namespace. Instead we derive a stable, URL-safe handle of the form
//! `project-<hash>` — see [`project_session_id`].

/// 64-bit FNV-1a hash.
///
/// Chosen over [`std::hash::DefaultHasher`] because the result must be **stable
/// across process restarts and compiler versions**: the same room must map to the
/// same Amassada session forever, or a redeploy would silently fork every project
/// conversation into a fresh session. FNV-1a is a fixed, well-defined algorithm.
fn fnv1a_64(input: &str) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in input.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Derive the stable Amassada session id for a project room.
///
/// Returns `project-<16-hex>`, deterministic for a given `room_id` and safe to drop
/// straight into a URL path (only `[a-z0-9-]`). The same room always yields the same
/// handle, so the project's conversation stays in one Amassada session across turns
/// and across Charradissa restarts.
pub fn project_session_id(room_id: &str) -> String {
    format!("project-{:016x}", fnv1a_64(room_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_is_deterministic() {
        let room = "!proj-a:occitane.guilhem";
        assert_eq!(project_session_id(room), project_session_id(room));
    }

    #[test]
    fn session_id_has_project_prefix() {
        assert!(project_session_id("!r:occitane.guilhem").starts_with("project-"));
    }

    #[test]
    fn session_id_is_url_safe() {
        // No '!' or ':' from the room id may leak into the handle — only [a-z0-9-].
        let id = project_session_id("!aBcD:occitane.guilhem");
        assert!(
            id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "session id must be URL-safe, got {id:?}"
        );
        assert!(!id.contains('!'));
        assert!(!id.contains(':'));
    }

    #[test]
    fn distinct_rooms_get_distinct_session_ids() {
        assert_ne!(
            project_session_id("!proj-a:occitane.guilhem"),
            project_session_id("!proj-b:occitane.guilhem"),
        );
    }
}
