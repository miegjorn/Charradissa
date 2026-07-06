//! Live Nervi consumers replacing Charradissa's direct-Matrix approval flow.
//! Two durable subscriptions, both with stable (non-per-pod) consumer
//! names so this scales to N replicas of charradissa-daemon with correct
//! competing-consumer distribution from NATS JetStream, no new code needed:
//!
//! - `occitan.approval.request` (fixed subject, every requester publishes
//!   here): register in Farga, post into the approval room via Corrièr's
//!   existing outbound chat relay.
//! - `approval`'s own inbound chat subject (Corrièr relays a human's Matrix
//!   reply here the instant it arrives -- event-driven, not polled): parse
//!   `/approve <id>` / `/reject <id> <reason>`, resolve in Farga, publish
//!   the outcome directly onto the request's own status subject.

use charradissa_core::approval::PersistentApprovalQueue;
use corrier_core::agent_subjects::{mint_approval_status_subject, APPROVAL_REQUEST_SUBJECT};
use corrier_core::chat_subjects::publish_outbound;
use corrier_core::ChatReply;
use futures::StreamExt;
use std::sync::Arc;

#[derive(serde::Deserialize)]
struct ApprovalRequest {
    id: String,
    component: String,
    category: String,
    description: String,
    #[serde(default)]
    params: serde_json::Value,
}

#[derive(serde::Serialize)]
struct ApprovalOutcome {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

pub async fn run_approval_relay(nats_url: &str, farga_url: &str, room_id: String) -> anyhow::Result<()> {
    let nervi = nervi_core::NerviClient::connect(nats_url).await?;
    let queue = Arc::new(PersistentApprovalQueue::new(farga_url.to_string()));

    let nervi_a = nervi.clone();
    let queue_a = Arc::clone(&queue);
    let room_a = room_id.clone();
    tokio::spawn(async move {
        run_request_consumer(nervi_a, queue_a, room_a).await;
    });

    run_resolution_consumer(nervi, queue).await;
    Ok(())
}

async fn run_request_consumer(nervi: nervi_core::NerviClient, queue: Arc<PersistentApprovalQueue>, room_id: String) {
    let durable_name = "charradissa-approval-request";
    let mut stream = match nervi.consume_durable(APPROVAL_REQUEST_SUBJECT, durable_name).await {
        Ok(s) => Box::pin(s),
        Err(e) => {
            tracing::error!("approval_relay: failed to open request consumer: {}", e);
            return;
        }
    };

    while let Some(result) = stream.next().await {
        let raw = match result {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("approval_relay: request delivery error (non-fatal): {}", e);
                continue;
            }
        };
        let req: ApprovalRequest = match serde_json::from_str(&raw.payload) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("approval_relay: failed to decode request (non-fatal): {}", e);
                continue;
            }
        };

        if let Err(e) = queue.register(&req.id, &room_id, &req.category, &req.description, req.params).await {
            tracing::error!("approval_relay: register failed: {}", e);
            continue;
        }

        let content = format!(
            "⏳ **[{}/{}]** {}\n   ID: `{}`\n   Reply: `/approve {}` or `/reject {} <reason>`",
            req.component, req.category, req.description, req.id, req.id, req.id
        );
        let reply = ChatReply {
            conversation_id: room_id.clone(),
            content,
            adapter: "matrix".to_string(),
        };
        if let Err(e) = publish_outbound(&nervi, "approval", &reply).await {
            tracing::error!("approval_relay: publish for {} failed: {}", req.id, e);
        }
    }
}

/// Format the ACK posted back into the approval room after a resolution,
/// so a human watching the room can follow the flow without checking Farga
/// or the requester's own status subject separately.
fn format_ack(id: &str, reason: &Option<String>) -> String {
    match reason {
        None => format!("✅ `{}` approved.", id),
        Some(r) => format!("❌ `{}` rejected: {}", id, r),
    }
}

/// Parse a `/approve <id>` or `/reject <id> [reason]` chat message into
/// (id, reason) -- `None` reason means approve, `Some` means reject. A
/// missing or blank reason after `/reject <id>` defaults to "rejected by
/// operator" rather than an empty string.
fn parse_resolution_command(content: &str) -> Option<(String, Option<String>)> {
    let content = content.trim();
    if let Some(rest) = content.strip_prefix("/approve ") {
        Some((rest.trim().to_string(), None))
    } else if let Some(rest) = content.strip_prefix("/reject ") {
        let mut parts = rest.splitn(2, ' ');
        let id = parts.next().unwrap_or("").trim().to_string();
        let reason = parts.next().unwrap_or("").trim().to_string();
        let reason = if reason.is_empty() { "rejected by operator".to_string() } else { reason };
        Some((id, Some(reason)))
    } else {
        None
    }
}

async fn run_resolution_consumer(nervi: nervi_core::NerviClient, queue: Arc<PersistentApprovalQueue>) {
    let mut stream = match corrier_core::chat_subjects::consume_inbound(&nervi, "approval").await {
        Ok(s) => Box::pin(s),
        Err(e) => {
            tracing::error!("approval_relay: failed to open resolution consumer: {}", e);
            return;
        }
    };

    while let Some(result) = stream.next().await {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("approval_relay: resolution delivery error (non-fatal): {}", e);
                continue;
            }
        };

        let Some((id, reason)) = parse_resolution_command(&msg.content) else { continue };
        if id.is_empty() {
            continue;
        }

        let result = match &reason {
            None => queue.approve(&id).await,
            Some(r) => queue.reject(&id, r.clone()).await,
        };
        if let Err(e) = result {
            tracing::warn!("approval_relay: resolve {} failed: {}", id, e);
            continue;
        }

        let status_subject = mint_approval_status_subject(&id);
        let outcome_payload = ApprovalOutcome {
            status: if reason.is_none() { "approved".to_string() } else { "rejected".to_string() },
            reason: reason.clone(),
        };
        let payload = match serde_json::to_string(&outcome_payload) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("approval_relay: failed to encode outcome for {}: {}", id, e);
                continue;
            }
        };
        if let Err(e) = nervi
            .publish(nervi_core::client::PublishOptions {
                subject: status_subject,
                payload,
                qualifier: Some("info".to_string()),
                timestamp: Some(chrono::Utc::now().to_rfc3339()),
            })
            .await
        {
            tracing::error!("approval_relay: status publish for {} failed: {}", id, e);
        }

        let ack = ChatReply {
            conversation_id: msg.conversation_id.clone(),
            content: format_ack(&id, &reason),
            adapter: "matrix".to_string(),
        };
        if let Err(e) = publish_outbound(&nervi, "approval", &ack).await {
            tracing::error!("approval_relay: ACK post for {} failed: {}", id, e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_request_deserializes_with_default_params() {
        let json = r#"{"id":"appr-test123","component":"gardian","category":"code","description":"test"}"#;
        let req: ApprovalRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.id, "appr-test123");
        assert_eq!(req.component, "gardian");
        assert_eq!(req.params, serde_json::Value::Null);
    }

    #[test]
    fn approval_outcome_omits_reason_when_approved() {
        let outcome = ApprovalOutcome { status: "approved".to_string(), reason: None };
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(!json.contains("reason"));
    }

    #[test]
    fn approval_outcome_includes_reason_when_rejected() {
        let outcome = ApprovalOutcome { status: "rejected".to_string(), reason: Some("too risky".to_string()) };
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(json.contains("too risky"));
    }

    #[test]
    fn parse_approve_command() {
        assert_eq!(
            parse_resolution_command("/approve abc123"),
            Some(("abc123".to_string(), None))
        );
    }

    #[test]
    fn parse_reject_command_with_reason() {
        assert_eq!(
            parse_resolution_command("/reject abc123 too risky"),
            Some(("abc123".to_string(), Some("too risky".to_string())))
        );
    }

    #[test]
    fn parse_reject_command_without_reason_defaults() {
        assert_eq!(
            parse_resolution_command("/reject abc123"),
            Some(("abc123".to_string(), Some("rejected by operator".to_string())))
        );
    }

    #[test]
    fn parse_reject_command_with_trailing_whitespace_and_no_reason_defaults() {
        // Regression: splitn(2, ' ') on "abc123 " yields Some("") for the
        // second part, not None -- unwrap_or's default never fired here
        // before the empty-string check was added.
        assert_eq!(
            parse_resolution_command("/reject abc123 "),
            Some(("abc123".to_string(), Some("rejected by operator".to_string())))
        );
    }

    #[test]
    fn parse_unrecognized_command_returns_none() {
        assert_eq!(parse_resolution_command("hello there"), None);
    }

    #[test]
    fn format_ack_for_approval() {
        assert_eq!(format_ack("abc123", &None), "✅ `abc123` approved.");
    }

    #[test]
    fn format_ack_for_rejection_includes_reason() {
        assert_eq!(
            format_ack("abc123", &Some("too risky".to_string())),
            "❌ `abc123` rejected: too risky"
        );
    }
}
