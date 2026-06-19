use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use charradissa_core::approval::PersistentApprovalQueue;

#[derive(Clone)]
pub struct QueueState {
    pub queue: Arc<PersistentApprovalQueue>,
}

pub fn router(state: QueueState) -> Router {
    Router::new()
        .route("/api/queue", get(list_queue))
        .route("/api/queue/:id/approve", post(approve))
        .route("/api/queue/:id/reject", post(reject))
        .with_state(state)
}

async fn list_queue(State(s): State<QueueState>) -> Json<serde_json::Value> {
    let pending = s.queue.list_pending();
    let count = pending.len();
    Json(serde_json::json!({ "pending": pending, "count": count }))
}

async fn approve(
    State(s): State<QueueState>,
    Path(id): Path<String>,
) -> StatusCode {
    match s.queue.approve(&id) {
        Ok(_) => StatusCode::OK,
        Err(_) => StatusCode::NOT_FOUND,
    }
}

#[derive(serde::Deserialize)]
struct RejectBody { reason: Option<String> }

async fn reject(
    State(s): State<QueueState>,
    Path(id): Path<String>,
    Json(body): Json<RejectBody>,
) -> StatusCode {
    let reason = body.reason.unwrap_or_else(|| "rejected via API".to_string());
    match s.queue.reject(&id, reason) {
        Ok(_) => StatusCode::OK,
        Err(_) => StatusCode::NOT_FOUND,
    }
}
