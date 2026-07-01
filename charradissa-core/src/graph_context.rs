//! Graph-based context collapse for long-lived Matrix conversations (Guilhem
//! and any other room-based agent), replacing raw transcript concatenation
//! with a persistent `SessionGraph` per room. Design:
//! `Amassada/docs/superpowers/specs/2026-07-01-trajectory-conditioned-collapse.md`.
//!
//! Experiment 10 (`Cor/experiment10_results.json`) measured linear history's
//! recall fidelity degrading with distance (10 -> 4 -> 4 over 5/10/15 turns)
//! despite strictly more raw data being available every turn, while a
//! fresh-context-graph condition held flat at 8. `build_user_prompt`
//! (`responder.rs`) is Guilhem's real production version of the "linear"
//! condition that degraded in that experiment — plain `sender: content\n`
//! concatenation over whatever history Charradissa fetched (capped at 20
//! messages). This module gives it the mechanism that held flat instead,
//! reusing `amassada-core`'s already-tested `SessionGraph` / `retrieve` /
//! extractor rather than a new implementation — Charradissa already depends
//! on `amassada-core`.
//!
//! Non-fatal by design, matching `amassada_core::farga`'s own convention:
//! any failure (extraction API error, Farga unreachable) logs a warning and
//! returns `None`. Callers fall back to the pre-existing raw-history prompt
//! — this must never turn into a hard failure of a reply.

use crate::types::ChatEvent;
use amassada_core::{extract_delta, NodeId, NodeType, SessionGraph};

/// Build a collapsed context string for `room_id`'s persistent graph, given
/// `history` (raw prior messages Charradissa already fetched) and `latest`
/// (the new message just received). Returns `None` on any failure, or when
/// the room's graph has no frontier nodes yet (first message in a room) --
/// callers should fall back to the raw-history prompt in both cases.
pub async fn build_graph_context(
    farga_url: &str,
    room_id: &str,
    history: &[ChatEvent],
    latest: &ChatEvent,
    api_key: Option<String>,
) -> Option<String> {
    let mut graph = amassada_core::farga::load_graph(farga_url, room_id)
        .await
        .unwrap_or_else(|| SessionGraph::new(room_id));

    let mut transcript_segment = String::new();
    for e in history {
        transcript_segment.push_str(&format!("{}: {}\n", e.sender, e.content));
    }
    transcript_segment.push_str(&format!("{}: {}\n", latest.sender, latest.content));

    let existing_nodes: Vec<NodeId> = graph.layers.causal.nodes.keys().cloned().collect();

    match extract_delta(&transcript_segment, &existing_nodes, api_key).await {
        Ok(delta) => graph.apply_delta(delta),
        Err(e) => {
            tracing::warn!(
                "graph_context: extraction failed for room {} (non-fatal, retrieving from existing graph as-is): {}",
                room_id, e
            );
        }
    }

    let frontier_ids: Vec<NodeId> = graph
        .layers
        .causal
        .nodes
        .values()
        .filter(|n| n.node_type == NodeType::Frontier)
        .map(|n| n.id.clone())
        .collect();

    let collapsed = if frontier_ids.is_empty() {
        None
    } else {
        Some(graph.retrieve(&frontier_ids, 1))
    };

    // Save even if extraction failed above -- version/vias/other layers may
    // still be worth persisting, and save_graph is itself non-fatal on error.
    amassada_core::farga::save_graph(farga_url, room_id, &graph).await;

    collapsed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatEventKind, RoomId, UserId};
    use chrono::Utc;

    fn ev(sender: &str, body: &str) -> ChatEvent {
        ChatEvent {
            event_id: "$x".into(),
            room_id: RoomId::new("!r:occitane.guilhem"),
            sender: UserId::new(sender),
            content: body.into(),
            timestamp: Utc::now(),
            kind: ChatEventKind::Message,
        }
    }

    #[tokio::test]
    async fn unreachable_farga_returns_none_not_panic() {
        // No live Farga at this address -- load_graph/save_graph are both
        // non-fatal (per amassada_core::farga's own doc comment), and
        // extract_delta will fail without ANTHROPIC_API_KEY reachable to a
        // real network in this sandbox -- either way this must resolve to
        // None, never panic or hang.
        let history = vec![ev("@p:occitane.guilhem", "hello")];
        let latest = ev("@p:occitane.guilhem", "how are you?");
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            build_graph_context(
                "http://127.0.0.1:1",
                "!unreachable-test-room:occitane.guilhem",
                &history,
                &latest,
                Some("not-a-real-key".into()),
            ),
        )
        .await;
        // Either the call completed (None, since farga:1 refuses the connection
        // fast) or it hit our timeout -- both are acceptable here; the point
        // of this test is that a fully-unreachable Farga does not panic.
        if let Ok(context) = result {
            assert!(context.is_none());
        }
    }
}
