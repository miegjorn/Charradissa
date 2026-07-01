use charradissa_core::approval::{ApprovalStatus, PersistentApprovalQueue};
use serde_json::json;
use uuid::Uuid;

fn temp_queue() -> PersistentApprovalQueue {
    let path = std::env::temp_dir().join(format!("test-queue-{}.json", Uuid::new_v4()));
    PersistentApprovalQueue::new(path)
}

#[test]
fn get_returns_pending_record_after_register() {
    let queue = temp_queue();
    let id = queue
        .register("!room:s", "code", "test PR", json!({}))
        .expect("register should succeed");
    let record = queue.get(&id).expect("record should exist");
    assert_eq!(record.status, ApprovalStatus::Pending);
}

#[test]
fn get_returns_approved_after_approve() {
    let queue = temp_queue();
    let id = queue
        .register("!room:s", "code", "test PR", json!({}))
        .expect("register should succeed");
    queue.approve(&id).expect("approve should succeed");
    let record = queue.get(&id).expect("record should exist");
    assert_eq!(record.status, ApprovalStatus::Approved);
}

#[test]
fn get_returns_rejected_with_reason_after_reject() {
    let queue = temp_queue();
    let id = queue
        .register("!room:s", "code", "test PR", json!({}))
        .expect("register should succeed");
    queue
        .reject(&id, "not safe".to_string())
        .expect("reject should succeed");
    let record = queue.get(&id).expect("record should exist");
    assert_eq!(record.status, ApprovalStatus::Rejected("not safe".to_string()));
}

#[test]
fn get_returns_none_for_unknown_id() {
    let queue = temp_queue();
    assert!(queue.get("nonexistent").is_none());
}
