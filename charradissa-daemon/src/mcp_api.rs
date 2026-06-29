//! HTTP transport for the Matrix MCP tool server (Charradissa#23).
//!
//! Mounts `POST /mcp` on the daemon's axum app, mirroring the stack convention used by the
//! dispatcher (`:9090/mcp`) and read by `Responder::mcp_call`. The request/response bodies
//! are JSON-RPC 2.0 envelopes handled by [`charradissa_matrix::mcp::MatrixMcp`].

use axum::{extract::State, routing::post, Json, Router};
use charradissa_matrix::mcp::MatrixMcp;
use std::sync::Arc;

#[derive(Clone)]
pub struct McpState {
    pub mcp: Arc<MatrixMcp>,
}

pub fn router(state: McpState) -> Router {
    Router::new().route("/mcp", post(handle_mcp)).with_state(state)
}

/// Handle a JSON-RPC request (single object or batch array).
async fn handle_mcp(
    State(s): State<McpState>,
    Json(request): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let response = if let Some(batch) = request.as_array() {
        // Batch: process each, dropping notification (null) responses per JSON-RPC.
        let mut out = Vec::with_capacity(batch.len());
        for req in batch {
            let r = s.mcp.handle(req.clone()).await;
            if !r.is_null() {
                out.push(r);
            }
        }
        serde_json::Value::Array(out)
    } else {
        s.mcp.handle(request).await
    };
    Json(response)
}
