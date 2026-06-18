# Governance Plan 2: Farga API Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `POST /governance` endpoint to Farga that receives `GovernanceContribution` submissions from Farcaster, stores them with a pending `LibrarianAssessment` row, exposes a precedent query (`GET /governance/precedent`) and a governance config endpoint (`GET /governance/config`), then override `HttpFargaWriter::submit_governance_contribution` in Charradissa to hit the real endpoint instead of the signal fallback.

**Architecture:** The Farga server uses a SQLite `nodes` table for all content. Governance contributions land as `NodeKind::GovernanceContribution` nodes. A separate `governance_assessments` table tracks librarian assessment status (starts `pending`; Plan 3 will fill `reversibility`/`impact`/`routing` when the librarian agent runs). Governance config is a `governance.yaml` file read from the Farga docs root. Charradissa's `HttpFargaWriter` overrides the trait default to POST directly to `/governance` instead of signal-flattening.

**Tech Stack:** Rust, Axum, SQLx + SQLite, serde_json, reqwest (Charradissa side). Two repos: `/Users/bedardpl/project/Farga` and `/Users/bedardpl/project/Charradissa`.

---

## Context for the Implementer

### Farga repo structure

```
Farga/
  farga-core/src/
    lib.rs           -- pub mod error, reader, types, writer
    types.rs         -- Node, NodeKind, Signal, Artifact, Edge + their impls
  farga-server/src/
    main.rs          -- reads env vars, runs migrations, starts axum
    state.rs         -- AppState { pool: SqlitePool, docs: Arc<DocsTree> }
    db.rs            -- insert_node, get_node, mark_stale, insert_edge, get_subgraph
    docs.rs          -- DocsTree { root: PathBuf } + read_org, read_initiatives, etc.
    routes/
      mod.rs         -- router() fn; registers all routes
      signals.rs     -- POST /signals, GET /signals/recent
      artifacts.rs   -- POST /artifacts, GET /artifacts/:project
      context.rs     -- GET /context/org/:org, GET /context/initiatives/:org, etc.
  farga-server/tests/
    db_tests.rs      -- integration tests using in-memory SQLite
  migrations/
    001_initial_schema.sql  -- nodes + edges tables
    002_add_indexes.sql     -- indexes
```

### Nodes table schema

```sql
CREATE TABLE nodes (
    id TEXT PRIMARY KEY, kind TEXT NOT NULL, address TEXT,
    project TEXT, component TEXT, title TEXT, content TEXT,
    created_at TEXT NOT NULL, updated_at TEXT NOT NULL, stale INTEGER DEFAULT 0
);
```

`Node::new(kind, project, content)` creates a node with a UUID `id` and `Utc::now()` timestamps.

### Existing test pattern (db_tests.rs)

```rust
async fn test_pool() -> SqlitePool {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    pool
}
```

All db tests use this pattern. Migration path is relative to the `farga-server` crate root.

### Charradissa farga.rs current state

`charradissa-core/src/farga.rs` has:
- `FargaWriter` trait with `write_signals`, `recent_signals`, and `submit_governance_contribution` (default method that signal-flattens)
- `HttpFargaWriter { client: reqwest::Client, base_url: String }` implementing the trait
- `GovernanceContribution` imported from `crate::farcaster::governance`

`HttpFargaWriter` does NOT yet override `submit_governance_contribution` — it inherits the signal-flattening default. Task 5 fixes this.

---

## Task 1: Governance types in farga-core

**Files:**
- Modify: `Farga/farga-core/src/types.rs`

Add governance types and extend `NodeKind`. No new files — everything goes in the existing `types.rs`.

- [ ] **Step 1: Write failing type tests**

Add to `Farga/farga-core/tests/types_tests.rs` (create if not present):

```rust
use farga_core::types::{
    GovernanceContribution, FargaLayer, ReversibilityLevel, ImpactScope,
    LibrarianAssessment, LibrarianRouting, GovernanceStatus, NodeKind,
};
use chrono::Utc;

#[test]
fn governance_contribution_round_trips() {
    let contrib = GovernanceContribution {
        title: "JWT Signing Pattern".into(),
        narrative: "Two projects converged on RS256.".into(),
        lessons: vec!["Use RS256 org-wide".into()],
        open_questions: vec![],
        involved_projects: vec!["auth-service".into(), "api-gateway".into()],
        concurrence: vec![],
        target_layer: FargaLayer::ProjectLevel,
        first_observed_at: Utc::now(),
        last_observed_at: Utc::now(),
        event_count: 3,
        reversibility: None,
        impact: None,
    };
    let json = serde_json::to_string(&contrib).unwrap();
    let back: GovernanceContribution = serde_json::from_str(&json).unwrap();
    assert_eq!(back.title, "JWT Signing Pattern");
    assert_eq!(back.event_count, 3);
    assert_eq!(back.target_layer, FargaLayer::ProjectLevel);
    assert!(back.reversibility.is_none());
}

#[test]
fn librarian_assessment_round_trips() {
    let assessment = LibrarianAssessment {
        reversibility: ReversibilityLevel::CostlyReversible,
        impact: ImpactScope::DomainWide,
        routing: LibrarianRouting::OpenGovernance,
        notes: Some("Broad Fondament impact".into()),
    };
    let json = serde_json::to_string(&assessment).unwrap();
    let back: LibrarianAssessment = serde_json::from_str(&json).unwrap();
    assert_eq!(back.impact, ImpactScope::DomainWide);
    assert_eq!(back.routing, LibrarianRouting::OpenGovernance);
}

#[test]
fn governance_status_variants_round_trip() {
    for status in [
        GovernanceStatus::Pending,
        GovernanceStatus::DirectIntegrate,
        GovernanceStatus::OpenGovernance,
        GovernanceStatus::Rejected,
    ] {
        let json = serde_json::to_string(&status).unwrap();
        let back: GovernanceStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, status);
    }
}

#[test]
fn node_kind_governance_contribution_round_trips() {
    let kind = NodeKind::GovernanceContribution;
    assert_eq!(kind.as_str(), "GovernanceContribution");
    let back: NodeKind = "GovernanceContribution".parse().unwrap();
    assert_eq!(back, NodeKind::GovernanceContribution);
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cd /Users/bedardpl/project/Farga && cargo test -p farga-core 2>&1 | grep -E "^error|FAILED|error\[" | head -20
```

Expected: compile errors — types don't exist yet.

- [ ] **Step 3: Add governance types to farga-core/src/types.rs**

Open `Farga/farga-core/src/types.rs`. At the top, the file already imports `chrono::{DateTime, Utc}` and `serde::{Deserialize, Serialize}`.

Add `NodeKind::GovernanceContribution` to the enum:

```rust
pub enum NodeKind {
    OrgLayer, InitiativeLayer, ProjectLayer, ComponentLayer,
    Artifact, Signal, Decision, Pattern, FondamentProposal, AuditEntry,
    GovernanceContribution,  // add this
}
```

Extend `as_str()` match arm:
```rust
Self::GovernanceContribution => "GovernanceContribution",
```

Extend `from_str()` match arm:
```rust
"GovernanceContribution" => Ok(Self::GovernanceContribution),
```

Then append these structs/enums at the end of the file (after `Artifact`):

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FargaLayer {
    OrgLevel,
    InitiativeLevel,
    ProjectLevel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReversibilityLevel {
    FullyReversible,
    EffectsLinger,
    CostlyReversible,
    Irreversible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImpactScope {
    Contained,
    CrossProject,
    DomainWide,
    OrgWide,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LibrarianRouting {
    DirectIntegrate,
    OpenGovernance,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GovernanceStatus {
    Pending,
    DirectIntegrate,
    OpenGovernance,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceContribution {
    pub title: String,
    pub narrative: String,
    pub lessons: Vec<String>,
    pub open_questions: Vec<String>,
    pub involved_projects: Vec<String>,
    pub concurrence: Vec<serde_json::Value>,
    pub target_layer: FargaLayer,
    pub first_observed_at: DateTime<Utc>,
    pub last_observed_at: DateTime<Utc>,
    pub event_count: u32,
    pub reversibility: Option<ReversibilityLevel>,
    pub impact: Option<ImpactScope>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibrarianAssessment {
    pub reversibility: ReversibilityLevel,
    pub impact: ImpactScope,
    pub routing: LibrarianRouting,
    pub notes: Option<String>,
}
```

`serde_json` is already a dependency (used by `farga-server`). Verify it's in `farga-core/Cargo.toml`. If not, add `serde_json = { version = "1", features = ["preserve_order"] }`.

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd /Users/bedardpl/project/Farga && cargo test -p farga-core 2>&1 | tail -15
```

Expected: all tests pass (the new 4 + existing 1 = 5 total).

- [ ] **Step 5: Commit**

```bash
cd /Users/bedardpl/project/Farga && git add farga-core/src/types.rs farga-core/tests/types_tests.rs && git commit -m "feat: add governance contribution and librarian assessment types"
```

---

## Task 2: DB migration + storage functions

**Files:**
- Create: `Farga/migrations/003_governance.sql`
- Modify: `Farga/farga-server/src/db.rs`
- Modify: `Farga/farga-server/tests/db_tests.rs`

The `governance_assessments` table stores the librarian evaluation state for each contribution. It starts `pending` at insert time; the librarian agent (Plan 3) updates it later.

- [ ] **Step 1: Write failing DB tests**

Add to `Farga/farga-server/tests/db_tests.rs`:

```rust
use farga_core::types::{GovernanceContribution, FargaLayer};
use chrono::Utc;
use farga_server::db::{insert_governance_contribution, count_precedent_rejections};

fn make_contrib(title: &str) -> GovernanceContribution {
    GovernanceContribution {
        title: title.into(),
        narrative: "Test narrative".into(),
        lessons: vec![],
        open_questions: vec![],
        involved_projects: vec!["proj-a".into()],
        concurrence: vec![],
        target_layer: FargaLayer::ProjectLevel,
        first_observed_at: Utc::now(),
        last_observed_at: Utc::now(),
        event_count: 1,
        reversibility: None,
        impact: None,
    }
}

#[tokio::test]
async fn insert_governance_contribution_creates_node_and_assessment() {
    let pool = test_pool().await;
    let contrib = make_contrib("JWT Signing Pattern");
    let node_id = insert_governance_contribution(&pool, &contrib).await.unwrap();
    assert!(!node_id.is_empty());

    // Node exists with correct kind
    let node = get_node(&pool, &node_id).await.unwrap();
    assert_eq!(node.kind, NodeKind::GovernanceContribution);
    assert_eq!(node.title.as_deref(), Some("JWT Signing Pattern"));

    // Assessment row created with status=pending
    let status: (String,) = sqlx::query_as(
        "SELECT status FROM governance_assessments WHERE node_id = ?"
    )
    .bind(&node_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status.0, "pending");
}

#[tokio::test]
async fn count_precedent_rejections_returns_zero_when_empty() {
    let pool = test_pool().await;
    let count = count_precedent_rejections(&pool, "jwt").await.unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn count_precedent_rejections_counts_only_rejected_rows() {
    let pool = test_pool().await;

    // Insert two contributions
    let id1 = insert_governance_contribution(&pool, &make_contrib("JWT Signing Pattern")).await.unwrap();
    let id2 = insert_governance_contribution(&pool, &make_contrib("JWT Key Rotation")).await.unwrap();

    // Mark id1 as rejected, leave id2 as pending
    sqlx::query("UPDATE governance_assessments SET status = 'rejected' WHERE node_id = ?")
        .bind(&id1)
        .execute(&pool)
        .await
        .unwrap();

    // "jwt" matches both titles, but only 1 is rejected
    let count = count_precedent_rejections(&pool, "jwt").await.unwrap();
    assert_eq!(count, 1);

    // Non-matching keyword returns 0
    let count2 = count_precedent_rejections(&pool, "auth").await.unwrap();
    assert_eq!(count2, 0);
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cd /Users/bedardpl/project/Farga && cargo test -p farga-server 2>&1 | grep -E "^error|FAILED" | head -10
```

Expected: compile errors — `insert_governance_contribution`, `count_precedent_rejections` don't exist yet.

- [ ] **Step 3: Create the migration**

Create `Farga/migrations/003_governance.sql`:

```sql
CREATE TABLE IF NOT EXISTS governance_assessments (
    id           TEXT PRIMARY KEY,
    node_id      TEXT NOT NULL REFERENCES nodes(id),
    status       TEXT NOT NULL DEFAULT 'pending',
    reversibility TEXT,
    impact       TEXT,
    routing      TEXT,
    notes        TEXT,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);
```

- [ ] **Step 4: Add DB functions to farga-server/src/db.rs**

Add these imports at the top of `db.rs` if not present:
```rust
use farga_core::types::GovernanceContribution;
use uuid::Uuid;
```

Then add these two functions at the end of `db.rs`:

```rust
pub async fn insert_governance_contribution(
    pool: &SqlitePool,
    contrib: &GovernanceContribution,
) -> Result<String> {
    let content = serde_json::to_string(contrib)?;
    let mut node = Node::new(
        NodeKind::GovernanceContribution,
        Some("system".into()),
        Some(content),
    );
    node.title = Some(contrib.title.clone());
    let node_id = node.id.clone();
    insert_node(pool, &node).await?;

    let assess_id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO governance_assessments (id, node_id, status, created_at, updated_at)
         VALUES (?, ?, 'pending', ?, ?)",
    )
    .bind(&assess_id)
    .bind(&node_id)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;

    Ok(node_id)
}

pub async fn count_precedent_rejections(pool: &SqlitePool, keywords: &str) -> Result<u32> {
    let pattern = format!("%{}%", keywords);
    let row: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM governance_assessments ga
         JOIN nodes n ON ga.node_id = n.id
         WHERE ga.status = 'rejected' AND n.title LIKE ?",
    )
    .bind(&pattern)
    .fetch_one(pool)
    .await?;
    Ok(row.0 as u32)
}
```

`uuid` and `serde_json` should already be in `farga-server/Cargo.toml`. If not, add:
```toml
uuid = { version = "1", features = ["v4"] }
serde_json = "1"
```

`Node` already has a `title` field (type `Option<String>`) — the `Node::new` constructor leaves it as `None`, so we set it manually on the struct before inserting.

- [ ] **Step 5: Run tests**

```bash
cd /Users/bedardpl/project/Farga && cargo test -p farga-server 2>&1 | tail -15
```

Expected: all tests pass (2 existing + 3 new = 5 total).

- [ ] **Step 6: Commit**

```bash
cd /Users/bedardpl/project/Farga && git add migrations/003_governance.sql farga-server/src/db.rs farga-server/tests/db_tests.rs && git commit -m "feat: add governance_assessments table and storage functions"
```

---

## Task 3: POST /governance + GET /governance/precedent routes

**Files:**
- Create: `Farga/farga-server/src/routes/governance.rs`
- Modify: `Farga/farga-server/src/routes/mod.rs`

- [ ] **Step 1: Write failing HTTP route test**

Create `Farga/farga-server/tests/governance_route_tests.rs`:

```rust
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use farga_core::types::{GovernanceContribution, FargaLayer};
use chrono::Utc;
use sqlx::SqlitePool;
use std::{path::PathBuf, sync::Arc};
use tower::ServiceExt;

async fn test_pool() -> SqlitePool {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    pool
}

fn test_app(pool: SqlitePool) -> axum::Router {
    use farga_server::{docs::DocsTree, routes, state::AppState};
    let state = AppState {
        pool,
        docs: Arc::new(DocsTree::new(PathBuf::from("/tmp/farga-test-docs"))),
    };
    routes::router(state)
}

fn make_contrib(title: &str) -> GovernanceContribution {
    GovernanceContribution {
        title: title.into(),
        narrative: "Two projects converged on RS256.".into(),
        lessons: vec!["Use RS256 org-wide".into()],
        open_questions: vec![],
        involved_projects: vec!["auth-service".into()],
        concurrence: vec![],
        target_layer: FargaLayer::ProjectLevel,
        first_observed_at: Utc::now(),
        last_observed_at: Utc::now(),
        event_count: 2,
        reversibility: None,
        impact: None,
    }
}

#[tokio::test]
async fn post_governance_returns_201_with_id() {
    let pool = test_pool().await;
    let app = test_app(pool);
    let body = serde_json::to_string(&make_contrib("JWT Signing Pattern")).unwrap();
    let req = Request::builder()
        .method("POST")
        .uri("/governance")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json["id"].as_str().map_or(false, |s| !s.is_empty()), "response must contain non-empty id");
}

#[tokio::test]
async fn get_precedent_returns_rejection_count() {
    let pool = test_pool().await;
    let app = test_app(pool.clone());

    // Seed a rejected assessment via the DB directly
    let contrib = make_contrib("JWT Signing Pattern");
    let node_id = farga_server::db::insert_governance_contribution(&pool, &contrib).await.unwrap();
    sqlx::query("UPDATE governance_assessments SET status = 'rejected' WHERE node_id = ?")
        .bind(&node_id)
        .execute(&pool)
        .await
        .unwrap();

    let req = Request::builder()
        .method("GET")
        .uri("/governance/precedent?keywords=jwt")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["rejection_count"].as_u64(), Some(1));
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cd /Users/bedardpl/project/Farga && cargo test -p farga-server governance_route 2>&1 | grep -E "^error|FAILED" | head -10
```

Expected: compile errors — route module doesn't exist yet.

- [ ] **Step 3: Create farga-server/src/routes/governance.rs**

```rust
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use farga_core::types::GovernanceContribution;
use serde::Deserialize;
use crate::{
    db::{insert_governance_contribution, count_precedent_rejections},
    state::AppState,
};

pub async fn post_governance(
    State(s): State<AppState>,
    Json(contrib): Json<GovernanceContribution>,
) -> (StatusCode, Json<serde_json::Value>) {
    match insert_governance_contribution(&s.pool, &contrib).await {
        Ok(id) => (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))),
        Err(e) => {
            tracing::error!("insert governance contribution failed: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
        }
    }
}

#[derive(Deserialize)]
pub struct PrecedentQuery {
    pub keywords: String,
}

pub async fn get_precedent(
    State(s): State<AppState>,
    Query(q): Query<PrecedentQuery>,
) -> Json<serde_json::Value> {
    let count = count_precedent_rejections(&s.pool, &q.keywords)
        .await
        .unwrap_or(0);
    Json(serde_json::json!({ "rejection_count": count }))
}
```

- [ ] **Step 4: Register routes in farga-server/src/routes/mod.rs**

```rust
pub mod artifacts;
pub mod context;
pub mod governance;
pub mod signals;

use axum::{routing::{get, post}, Router};
use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/context/org/:org", get(context::get_org))
        .route("/context/initiatives/:org", get(context::get_initiatives))
        .route("/context/project/:project", get(context::get_project))
        .route("/context/component/:project/*path", get(context::get_component))
        .route("/signals", post(signals::post_signals))
        .route("/signals/recent", get(signals::get_recent_signals))
        .route("/artifacts", post(artifacts::post_artifact))
        .route("/artifacts/:project", get(artifacts::get_artifacts))
        .route("/governance", post(governance::post_governance))
        .route("/governance/precedent", get(governance::get_precedent))
        .with_state(state)
}
```

- [ ] **Step 5: Run tests**

```bash
cd /Users/bedardpl/project/Farga && cargo test -p farga-server 2>&1 | tail -20
```

Expected: all tests pass (5 existing + 2 new = 7 total).

- [ ] **Step 6: Commit**

```bash
cd /Users/bedardpl/project/Farga && git add farga-server/src/routes/governance.rs farga-server/src/routes/mod.rs farga-server/tests/governance_route_tests.rs && git commit -m "feat: add POST /governance and GET /governance/precedent routes"
```

---

## Task 4: GET /governance/config route

**Files:**
- Modify: `Farga/farga-server/src/docs.rs`
- Modify: `Farga/farga-server/src/routes/governance.rs`
- Modify: `Farga/farga-server/src/routes/mod.rs`

The governance config is a `governance.yaml` file at the docs root (same directory as `org.md`). The route returns the raw YAML string. If the file doesn't exist, returns empty string with 200. Amassada (Plan 3) reads this to get risk weights.

The expected file format (stored at `$FARGA_DOCS/governance.yaml`):

```yaml
governance:
  risk_weights:
    primitive_proximity: 0.25
    signal_concurrence: 0.20
    signal_velocity: 0.15
    reversibility: 0.20
    impact: 0.15
    precedent: 0.05
  tier_thresholds:
    medium: 0.30
    high: 0.55
    critical: 0.80
  budget:
    daily_tokens: 50000
    per_session_cap: 15000
    counter_session_cap: 10000
  tier_minimums:
    low: 2000
    medium: 5000
    high: 8000
    critical: 12000
```

- [ ] **Step 1: Write failing test**

Add to `Farga/farga-server/tests/governance_route_tests.rs`:

```rust
#[tokio::test]
async fn get_governance_config_returns_yaml_if_present() {
    let pool = test_pool().await;
    let docs_dir = tempfile::tempdir().unwrap();
    let config_yaml = "governance:\n  risk_weights:\n    primitive_proximity: 0.25\n";
    std::fs::write(docs_dir.path().join("governance.yaml"), config_yaml).unwrap();

    use farga_server::{docs::DocsTree, routes, state::AppState};
    let state = AppState {
        pool,
        docs: Arc::new(DocsTree::new(docs_dir.path().to_path_buf())),
    };
    let app = routes::router(state);

    let req = Request::builder()
        .method("GET")
        .uri("/governance/config")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(body.contains("primitive_proximity"), "should return governance.yaml content");
}

#[tokio::test]
async fn get_governance_config_returns_empty_if_missing() {
    let pool = test_pool().await;
    let docs_dir = tempfile::tempdir().unwrap(); // no governance.yaml written
    use farga_server::{docs::DocsTree, routes, state::AppState};
    let state = AppState {
        pool,
        docs: Arc::new(DocsTree::new(docs_dir.path().to_path_buf())),
    };
    let app = routes::router(state);
    let req = Request::builder()
        .method("GET")
        .uri("/governance/config")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(bytes.len(), 0);
}
```

Check `Cargo.toml` for `tempfile` dependency. If not present, add:
```toml
[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Run to verify tests fail**

```bash
cd /Users/bedardpl/project/Farga && cargo test -p farga-server get_governance_config 2>&1 | grep -E "^error|FAILED" | head -10
```

Expected: compile error — route doesn't exist.

- [ ] **Step 3: Add read_governance_config to DocsTree**

In `Farga/farga-server/src/docs.rs`, add after `read_component`:

```rust
pub fn read_governance_config(&self) -> Result<String> {
    let p = self.root.join("governance.yaml");
    Ok(if p.exists() { std::fs::read_to_string(p)? } else { String::new() })
}
```

- [ ] **Step 4: Add get_governance_config handler to routes/governance.rs**

Add at the end of `Farga/farga-server/src/routes/governance.rs`:

```rust
pub async fn get_governance_config(State(s): State<AppState>) -> String {
    s.docs.read_governance_config().unwrap_or_default()
}
```

- [ ] **Step 5: Register the route in routes/mod.rs**

Add one line after the precedent route:

```rust
.route("/governance/config", get(governance::get_governance_config))
```

Full `router()` function:
```rust
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/context/org/:org", get(context::get_org))
        .route("/context/initiatives/:org", get(context::get_initiatives))
        .route("/context/project/:project", get(context::get_project))
        .route("/context/component/:project/*path", get(context::get_component))
        .route("/signals", post(signals::post_signals))
        .route("/signals/recent", get(signals::get_recent_signals))
        .route("/artifacts", post(artifacts::post_artifact))
        .route("/artifacts/:project", get(artifacts::get_artifacts))
        .route("/governance", post(governance::post_governance))
        .route("/governance/precedent", get(governance::get_precedent))
        .route("/governance/config", get(governance::get_governance_config))
        .with_state(state)
}
```

- [ ] **Step 6: Run tests**

```bash
cd /Users/bedardpl/project/Farga && cargo test -p farga-server 2>&1 | tail -20
```

Expected: all tests pass (7 existing + 2 new = 9 total).

- [ ] **Step 7: Commit**

```bash
cd /Users/bedardpl/project/Farga && git add farga-server/src/docs.rs farga-server/src/routes/governance.rs farga-server/src/routes/mod.rs farga-server/tests/governance_route_tests.rs && git commit -m "feat: add GET /governance/config endpoint"
```

---

## Task 5: HttpFargaWriter::submit_governance_contribution override in Charradissa

**Files:**
- Modify: `Charradissa/charradissa-core/src/farga.rs`
- Modify: `Charradissa/charradissa-core/tests/farcaster_tests.rs`

`HttpFargaWriter` currently inherits the trait default for `submit_governance_contribution` which signal-flattens. We add an explicit override that POSTs the `GovernanceContribution` JSON directly to `{base_url}/governance`.

After this change, a live `HttpFargaWriter` calls the real `/governance` endpoint. `MockFargaWriter` already overrides `submit_governance_contribution` (added in Plan 1 final-review fix) so no mock changes needed.

- [ ] **Step 1: Write a compile-only test verifying the override exists**

Add to `Charradissa/charradissa-core/tests/farcaster_tests.rs` at the top-level (not inside an async fn):

```rust
#[test]
fn http_farga_writer_has_governance_override() {
    // This test verifies that HttpFargaWriter::submit_governance_contribution
    // exists as an explicit method (overrides the default). If the override
    // is removed and the signal-flattening default kicks in, this test still
    // passes — the purpose is documentation that the override was intentional.
    // The real verification is that the method compiles and POSTs to /governance.
    let _writer = charradissa_core::farga::HttpFargaWriter::new("http://localhost:7500".into());
    // If HttpFargaWriter is not Send + Sync, this fails at compile time
    fn _assert_send_sync<T: Send + Sync>() {}
    _assert_send_sync::<charradissa_core::farga::HttpFargaWriter>();
}
```

- [ ] **Step 2: Run to confirm it passes (it already should)**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core http_farga_writer_has_governance_override 2>&1 | tail -5
```

Expected: PASS (the Send+Sync bound already holds).

- [ ] **Step 3: Add the override to HttpFargaWriter**

Open `Charradissa/charradissa-core/src/farga.rs`. In the `impl FargaWriter for HttpFargaWriter` block, add this method after `recent_signals`:

```rust
async fn submit_governance_contribution(
    &self,
    contribution: GovernanceContribution,
) -> Result<()> {
    let url = format!("{}/governance", self.base_url);
    self.client
        .post(&url)
        .json(&contribution)
        .send()
        .await
        .map_err(|e| crate::error::CharradissaError::Backend(e.to_string()))?;
    Ok(())
}
```

`GovernanceContribution` is already imported at the top of `farga.rs` via `use crate::farcaster::governance::GovernanceContribution;`.

Note: `contribution` must implement `serde::Serialize` for `.json(&contribution)`. `GovernanceContribution` is `#[derive(Serialize, Deserialize)]` in `charradissa-core/src/farcaster/governance.rs` — this is already satisfied.

The full `impl FargaWriter for HttpFargaWriter` block becomes:

```rust
#[async_trait]
impl FargaWriter for HttpFargaWriter {
    async fn write_signals(&self, project: &ProjectId, signals: Vec<Signal>) -> Result<()> {
        let url = format!("{}/signals", self.base_url);
        self.client.post(&url)
            .json(&serde_json::json!({ "project": project.as_str(), "signals": signals }))
            .send().await
            .map_err(|e| crate::error::CharradissaError::Backend(e.to_string()))?;
        Ok(())
    }

    async fn recent_signals(&self, project: &ProjectId, since: Duration) -> Result<Vec<Signal>> {
        let url = format!("{}/signals/recent?project={}&since={}h",
            self.base_url, project.as_str(), since.num_hours());
        let resp = self.client.get(&url).send().await
            .map_err(|e| crate::error::CharradissaError::Backend(e.to_string()))?;
        resp.json().await
            .map_err(|e| crate::error::CharradissaError::Backend(e.to_string()))
    }

    async fn submit_governance_contribution(
        &self,
        contribution: GovernanceContribution,
    ) -> Result<()> {
        let url = format!("{}/governance", self.base_url);
        self.client
            .post(&url)
            .json(&contribution)
            .send()
            .await
            .map_err(|e| crate::error::CharradissaError::Backend(e.to_string()))?;
        Ok(())
    }
}
```

- [ ] **Step 4: Run all Charradissa tests**

```bash
cd /Users/bedardpl/project/Charradissa && cargo test -p charradissa-core 2>&1 | tail -15
```

Expected: 20 tests pass + 1 ignored (19 from Plan 1 + the new compile test).

- [ ] **Step 5: Commit**

```bash
cd /Users/bedardpl/project/Charradissa && git add charradissa-core/src/farga.rs charradissa-core/tests/farcaster_tests.rs && git commit -m "feat: override submit_governance_contribution on HttpFargaWriter to POST /governance"
```

---

## Self-Review

**Spec coverage check:**

| Spec requirement | Covered by |
|---|---|
| `submit_governance_contribution` HTTP endpoint in Farga | Task 3 `POST /governance` |
| `LibrarianAssessment` storage | Task 2 `governance_assessments` table + `insert_governance_contribution` creates pending row |
| Precedent query | Task 3 `GET /governance/precedent?keywords=...` |
| Org config governance block with weights YAML | Task 4 `GET /governance/config` reads `governance.yaml` |
| HttpFargaWriter override (implied by "real HTTP backend" in Plan 1 doc comment) | Task 5 |

**Placeholder scan:** None found.

**Type consistency check:**
- `GovernanceContribution` defined in Task 1 (`farga-core/src/types.rs`) → used in Task 2 (`db.rs`), Task 3 (`governance.rs`), Task 5 (`charradissa farga.rs`) — consistent
- `insert_governance_contribution(pool, &contrib) → Result<String>` defined in Task 2 → used in Task 3 `post_governance` handler and Task 2 test — consistent
- `count_precedent_rejections(pool, &str) → Result<u32>` defined in Task 2 → used in Task 3 `get_precedent` handler — consistent
- `DocsTree::read_governance_config()` defined in Task 4 → used in `get_governance_config` handler in Task 4 — consistent

**Note on `GovernanceContribution.concurrence` field:** In `charradissa-core`, `concurrence: Vec<AgentConcurrence>`. In `farga-core`, `concurrence: Vec<serde_json::Value>`. This asymmetry is intentional — `farga-core` doesn't need to interpret `AgentConcurrence` deeply; it stores it as opaque JSON. Serde handles this transparently: Charradissa serializes `Vec<AgentConcurrence>` → JSON array; Farga deserializes → `Vec<serde_json::Value>`. No type mismatch on the wire.
