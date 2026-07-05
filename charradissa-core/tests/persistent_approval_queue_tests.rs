use charradissa_core::approval::{ApprovalStatus, PersistentApprovalQueue};

fn temp_queue() -> PersistentApprovalQueue {
    PersistentApprovalQueue::new("http://127.0.0.1:1".to_string())
}

#[tokio::test]
async fn register_against_unreachable_farga_returns_err_not_panic() {
    let queue = temp_queue();
    let result = queue.register("!room:s", "code", "test PR", serde_json::json!({})).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn get_against_unreachable_farga_returns_none_not_panic() {
    let queue = temp_queue();
    let record = queue.get("nonexistent").await;
    assert!(record.is_none());
}

#[tokio::test]
async fn approve_against_unreachable_farga_returns_err_not_panic() {
    let queue = temp_queue();
    let result = queue.approve("nonexistent").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn reject_against_unreachable_farga_returns_err_not_panic() {
    let queue = temp_queue();
    let result = queue.reject("nonexistent", "reason".to_string()).await;
    assert!(result.is_err());
}

#[test]
fn approval_status_variants_are_distinguishable() {
    assert_ne!(ApprovalStatus::Pending, ApprovalStatus::Approved);
    assert_ne!(
        ApprovalStatus::Rejected("a".to_string()),
        ApprovalStatus::Rejected("b".to_string())
    );
}
