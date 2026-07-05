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
use corrier_core::chat_subjects::{inbound_subject, outbound_subject};
use futures::StreamExt;
use std::sync::Arc;

#[derive(serde::Deserialize)]
struct ApprovalRequest {
    component: String,
    category: String,
    description: String,
    #[serde(default)]
    params: serde_json::Value,
}

#[derive(serde::Serialize)]
struct ApprovalPost {
    component: String,
    category: String,
    description: String,
    id: String,
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

    run_resolution_consumer(nervi, queue, room_id).await;
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

        let id = match queue.register(&room_id, &req.category, &req.description, req.params.clone()).await {
            Ok(id) => id,
            Err(e) => {
                tracing::error!("approval_relay: register failed: {}", e);
                continue;
            }
        };

        let post = ApprovalPost {
            component: req.component,
            category: req.category,
            description: req.description,
            id: id.clone(),
        };
        let subject = outbound_subject("approval", &room_id);
        let payload = match serde_json::to_string(&post) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("approval_relay: failed to encode post for {}: {}", id, e);
                continue;
            }
        };
        if let Err(e) = nervi
            .publish(nervi_core::client::PublishOptions {
                subject,
                payload,
                qualifier: Some("info".to_string()),
                timestamp: Some(chrono::Utc::now().to_rfc3339()),
            })
            .await
        {
            tracing::error!("approval_relay: publish for {} failed: {}", id, e);
        }
    }
}

async fn run_resolution_consumer(nervi: nervi_core::NerviClient, queue: Arc<PersistentApprovalQueue>, room_id: String) {
    let subject = inbound_subject("approval", &room_id);
    let durable_name = "charradissa-approval-resolution";
    let mut stream = match nervi.consume_durable(&subject, durable_name).await {
        Ok(s) => Box::pin(s),
        Err(e) => {
            tracing::error!("approval_relay: failed to open resolution consumer: {}", e);
            return;
        }
    };

    while let Some(result) = stream.next().await {
        let raw = match result {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("approval_relay: resolution delivery error (non-fatal): {}", e);
                continue;
            }
        };
        let msg: corrier_core::ChatMessage = match serde_json::from_str(&raw.payload) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("approval_relay: failed to decode inbound chat message (non-fatal): {}", e);
                continue;
            }
        };

        let content = msg.content.trim();
        let outcome = if let Some(rest) = content.strip_prefix("/approve ") {
            Some((rest.trim().to_string(), None))
        } else if let Some(rest) = content.strip_prefix("/reject ") {
            let mut parts = rest.splitn(2, ' ');
            let id = parts.next().unwrap_or("").trim().to_string();
            let reason = parts.next().unwrap_or("rejected by operator").trim().to_string();
            Some((id, Some(reason)))
        } else {
            None
        };

        let Some((id, reason)) = outcome else { continue };
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
            reason,
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_request_deserializes_with_default_params() {
        let json = r#"{"component":"gardian","category":"code","description":"test"}"#;
        let req: ApprovalRequest = serde_json::from_str(json).unwrap();
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
}
