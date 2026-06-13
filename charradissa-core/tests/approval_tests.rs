use charradissa_core::approval::{ApprovalQueue, ApprovalOutcome};

#[tokio::test]
async fn approval_resolves_on_approve() {
    let mut queue = ApprovalQueue::new(60);
    let (id, rx) = queue.create_pending("infra".into(), "delete bucket".into(), serde_json::json!({}));
    queue.resolve(&id, ApprovalOutcome::Approved).unwrap();
    let result = rx.await.unwrap();
    assert_eq!(result, ApprovalOutcome::Approved);
}

#[tokio::test]
async fn approval_resolves_on_reject() {
    let mut queue = ApprovalQueue::new(60);
    let (id, rx) = queue.create_pending("code".into(), "merge PR".into(), serde_json::json!({}));
    queue.resolve(&id, ApprovalOutcome::Rejected("too risky".into())).unwrap();
    let result = rx.await.unwrap();
    assert!(matches!(result, ApprovalOutcome::Rejected(_)));
}
