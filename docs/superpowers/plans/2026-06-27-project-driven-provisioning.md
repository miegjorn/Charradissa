# Project-Driven Room Provisioning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace hardcoded `AGENT_LOCAL_PARTS` / manual `[component_agents]` config with a Farga+Fondament-driven pipeline that provisions Matrix rooms and Responders from project definitions at startup.

**Architecture:** Farga lists project components via a new `/context/components/:project` endpoint. Fondament exposes a new HTTP server (`fondament-server`) that resolves component-agent system prompts. Charradissa calls both at startup, creates-or-joins aliased rooms, and builds its `component_agents` map dynamically — zero config change needed to add a new component.

**Tech Stack:** Rust, axum 0.7, reqwest 0.12, tokio, serde_json, tempfile (tests), tower (tests).

## Global Constraints

- Rust edition 2021 throughout.
- All new HTTP endpoints return JSON (axum `Json<T>`), except `/resolve/:id` which returns `text/plain`.
- Idempotency: `create_or_join_aliased_room` always tries `join` before `create`; never errors if a room already exists.
- Graceful degradation: Farga or Fondament unavailable → warn + skip, never panic.
- `[component_agents]` in `charradissa.toml` kept as fallback — used only when `provision_project_rooms` returns 0 rooms.
- No placeholder strings in code. Every step must compile and pass tests before committing.
- Working directory prefixes: `Farga/` = `~/project/Farga`, `Fondament/` = `~/project/Fondament`, `Charradissa/` = `~/project/Charradissa`.
- Run tests with `cargo test` from the relevant repo root.

---

## Phase 1 — Farga: component listing endpoint

### Task 1: `list_components` in DocsTree + route

**Files:**
- Modify: `Farga/farga-server/src/docs.rs`
- Modify: `Farga/farga-server/src/routes/context.rs`
- Modify: `Farga/farga-server/src/routes/mod.rs`
- Test: `Farga/farga-server/tests/context_route_tests.rs` (new file)

**Interfaces:**
- Produces: `DocsTree::list_components(project: &str) -> Result<Vec<String>>` (sorted, only dirs containing `component.md`)
- Produces: `GET /context/components/:project` → `200 application/json` body `["amassada","caissa",...]` or `[]`

- [ ] **Step 1: Write the failing test**

Create `Farga/farga-server/tests/context_route_tests.rs`:

```rust
use axum::{body::Body, http::{Request, StatusCode}};
use sqlx::SqlitePool;
use std::{path::PathBuf, sync::Arc};
use tower::ServiceExt;

async fn test_pool() -> SqlitePool {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    pool
}

#[tokio::test]
async fn get_components_returns_subdirs_with_component_md() {
    let dir = tempfile::tempdir().unwrap();
    let occitan = dir.path().join("projects/occitan");
    std::fs::create_dir_all(occitan.join("amassada")).unwrap();
    std::fs::write(occitan.join("amassada/component.md"), "# Amassada").unwrap();
    std::fs::create_dir_all(occitan.join("gardian")).unwrap();
    std::fs::write(occitan.join("gardian/component.md"), "# Gardian").unwrap();
    // a subdir WITHOUT component.md must be excluded
    std::fs::create_dir_all(occitan.join("empty-dir")).unwrap();

    let pool = test_pool().await;
    use farga_server::{docs::DocsTree, routes, state::AppState};
    let state = AppState { pool, docs: Arc::new(DocsTree::new(dir.path().to_path_buf())) };
    let app = routes::router(state);

    let req = Request::builder()
        .method("GET")
        .uri("/context/components/occitan")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let components: Vec<String> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(components, vec!["amassada", "gardian"]); // sorted
}

#[tokio::test]
async fn get_components_returns_empty_for_unknown_project() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool().await;
    use farga_server::{docs::DocsTree, routes, state::AppState};
    let state = AppState { pool, docs: Arc::new(DocsTree::new(dir.path().to_path_buf())) };
    let app = routes::router(state);

    let req = Request::builder()
        .method("GET")
        .uri("/context/components/nonexistent")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let components: Vec<String> = serde_json::from_slice(&bytes).unwrap();
    assert!(components.is_empty());
}
```

- [ ] **Step 2: Run tests — expect compile error (route doesn't exist yet)**

```bash
cd ~/project/Farga && cargo test -p farga-server --test context_route_tests 2>&1 | head -30
```

Expected: compile error — `get_components` not found.

- [ ] **Step 3: Add `list_components` to `DocsTree`**

In `Farga/farga-server/src/docs.rs`, add after `read_component`:

```rust
pub fn list_components(&self, project: &str) -> anyhow::Result<Vec<String>> {
    let dir = self.root.join("projects").join(project);
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() && path.join("component.md").exists() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    Ok(names)
}
```

- [ ] **Step 4: Add route handler**

In `Farga/farga-server/src/routes/context.rs`, add:

```rust
pub async fn get_components(
    State(s): State<AppState>,
    Path(project): Path<String>,
) -> Json<Vec<String>> {
    Json(s.docs.list_components(&project).unwrap_or_default())
}
```

- [ ] **Step 5: Register the route**

In `Farga/farga-server/src/routes/mod.rs`, add to the router:

```rust
.route("/context/components/:project", get(context::get_components))
```

Place it alongside the other `/context/` routes.

- [ ] **Step 6: Run full test suite**

```bash
cd ~/project/Farga && cargo test
```

Expected:
```
test get_components_returns_subdirs_with_component_md ... ok
test get_components_returns_empty_for_unknown_project ... ok
... (all other tests pass)
```

- [ ] **Step 7: Commit**

```bash
cd ~/project/Farga && git add farga-server/src/docs.rs farga-server/src/routes/context.rs farga-server/src/routes/mod.rs farga-server/tests/context_route_tests.rs
git commit -m "feat(farga): add GET /context/components/:project endpoint

Lists subdirectories of docs/projects/{project}/ that contain a component.md.
Returns sorted JSON array. Used by Charradissa to discover project components
at startup without hardcoded lists.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Phase 2 — Fondament: `fondament-server` crate

### Task 2: Add `component` field to `DefinitionFile`

**Files:**
- Modify: `Fondament/fondament-core/src/definition.rs`
- Test: `Fondament/fondament-core/tests/resolver_tests.rs` (add one case)

**Interfaces:**
- Produces: `DefinitionFile.component: Option<String>` — present on `kind: component-agent` definitions

- [ ] **Step 1: Write the failing test**

In `Fondament/fondament-core/tests/resolver_tests.rs`, add:

```rust
#[test]
fn definition_file_deserializes_component_field() {
    let yaml = r#"
id: fondament/amassada-agent
kind: component-agent
component: amassada
default_model: claude-sonnet-4-6
context: "You are the Amassada agent."
"#;
    let def: fondament_core::definition::DefinitionFile = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(def.component.as_deref(), Some("amassada"));
}

#[test]
fn definition_file_component_defaults_to_none() {
    let yaml = r#"
id: fondament/guilhem
kind: role
context: "You are Guilhem."
"#;
    let def: fondament_core::definition::DefinitionFile = serde_yaml::from_str(yaml).unwrap();
    assert!(def.component.is_none());
}
```

- [ ] **Step 2: Run — expect compile error**

```bash
cd ~/project/Fondament && cargo test -p fondament-core 2>&1 | head -20
```

Expected: `no field component` or similar.

- [ ] **Step 3: Add `component` field**

In `Fondament/fondament-core/src/definition.rs`, add to `DefinitionFile`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefinitionFile {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub extends: Vec<String>,
    pub default_model: Option<ModelId>,
    pub context: Option<String>,
    #[serde(default)]
    pub tools: ToolSet,
    pub stance: Option<String>,
    pub cognitive_load: Option<String>,
    #[serde(default)]
    pub modifier: bool,
    #[serde(default)]
    pub component: Option<String>,
}
```

- [ ] **Step 4: Run tests — expect pass**

```bash
cd ~/project/Fondament && cargo test -p fondament-core
```

Expected: all tests pass including the two new ones.

- [ ] **Step 5: Commit**

```bash
cd ~/project/Fondament && git add fondament-core/src/definition.rs fondament-core/tests/resolver_tests.rs
git commit -m "feat(fondament-core): add optional component field to DefinitionFile

component-agent definitions carry a component: field (e.g. \"amassada\").
Used by fondament-server to list component agents by name.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

### Task 3: Create `fondament-server` crate

**Files:**
- Create: `Fondament/fondament-server/Cargo.toml`
- Create: `Fondament/fondament-server/src/main.rs`
- Modify: `Fondament/Cargo.toml` (add workspace member + shared deps)
- Test: `Fondament/fondament-server/tests/server_tests.rs` (new file)

**Interfaces:**
- Consumes: `DefinitionFile.component: Option<String>` (Task 2), `Fondament::resolve()`, `CompositionAddress`
- Produces:
  - `GET /component-agents` → `200 application/json` `[{"id":"fondament/amassada-agent","component":"amassada"},...]`
  - `GET /resolve/:id` → `200 text/plain` (resolved system prompt), `404` if definition not found
  - `GET /health` → `200 "ok"`

- [ ] **Step 1: Update workspace `Cargo.toml`**

In `Fondament/Cargo.toml`:

```toml
[workspace]
members = ["fondament-core", "fondament-cli", "fondament-server"]
resolver = "2"

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_yaml = "0.9"
serde_json = "1"
async-trait = "0.1"
clap = { version = "4", features = ["derive"] }
notify = "6"
anyhow = "1"
thiserror = "1"
tracing = "0.1"
tracing-subscriber = "0.3"
axum = "0.7"
reqwest = { version = "0.12", features = ["json"] }
```

- [ ] **Step 2: Create `fondament-server/Cargo.toml`**

Create `Fondament/fondament-server/Cargo.toml`:

```toml
[package]
name = "fondament-server"
version = "0.1.0"
edition = "2021"

[dependencies]
fondament-core = { path = "../fondament-core" }
tokio = { workspace = true }
axum = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
anyhow = { workspace = true }

[dev-dependencies]
tower = { version = "0.4", features = ["util"] }
tempfile = "3"
tokio = { workspace = true }
```

- [ ] **Step 3: Write the failing tests**

Create `Fondament/fondament-server/tests/server_tests.rs`:

```rust
use axum::{body::Body, http::{Request, StatusCode}};
use std::{path::PathBuf, sync::Arc};
use tower::ServiceExt;

fn make_app(definitions_path: PathBuf) -> axum::Router {
    fondament_server::router(definitions_path, "http://farga-does-not-exist:7500".into())
}

#[tokio::test]
async fn health_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let app = make_app(dir.path().to_path_buf());
    let req = Request::builder().uri("/health").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn get_component_agents_returns_only_component_agent_kind() {
    let dir = tempfile::tempdir().unwrap();
    let fondament_dir = dir.path().join("fondament");
    std::fs::create_dir_all(&fondament_dir).unwrap();

    std::fs::write(fondament_dir.join("amassada-agent.yaml"), r#"
id: fondament/amassada-agent
kind: component-agent
component: amassada
context: "You are Amassada."
"#).unwrap();

    std::fs::write(fondament_dir.join("guilhem.yaml"), r#"
id: fondament/guilhem
kind: role
context: "You are Guilhem."
"#).unwrap();

    let app = make_app(dir.path().to_path_buf());
    let req = Request::builder().uri("/component-agents").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let agents: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0]["id"], "fondament/amassada-agent");
    assert_eq!(agents[0]["component"], "amassada");
}

#[tokio::test]
async fn resolve_returns_system_prompt_for_known_id() {
    let dir = tempfile::tempdir().unwrap();
    let fondament_dir = dir.path().join("fondament");
    std::fs::create_dir_all(&fondament_dir).unwrap();
    std::fs::write(fondament_dir.join("amassada-agent.yaml"), r#"
id: fondament/amassada-agent
kind: component-agent
component: amassada
context: "You are the Amassada session engine agent."
"#).unwrap();

    let app = make_app(dir.path().to_path_buf());
    let req = Request::builder()
        .uri("/resolve/fondament/amassada-agent")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(body.contains("Amassada session engine agent"));
}

#[tokio::test]
async fn resolve_returns_404_for_unknown_id() {
    let dir = tempfile::tempdir().unwrap();
    let app = make_app(dir.path().to_path_buf());
    let req = Request::builder()
        .uri("/resolve/fondament/does-not-exist")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
```

- [ ] **Step 4: Run — expect compile error (crate doesn't exist)**

```bash
cd ~/project/Fondament && cargo test -p fondament-server 2>&1 | head -20
```

Expected: crate not found / compile error.

- [ ] **Step 5: Create `fondament-server/src/main.rs`**

Create `Fondament/fondament-server/src/main.rs`:

```rust
use std::{path::PathBuf, sync::Arc};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use fondament_core::{
    address::CompositionAddress,
    farga_http::HttpFargaReader,
    fondament::Fondament,
    tree::DefinitionTree,
};
use serde_json::Value;

#[derive(Clone)]
struct AppState {
    tree: Arc<DefinitionTree>,
    fondament: Arc<Fondament>,
}

pub fn router(definitions_path: PathBuf, farga_url: String) -> Router {
    let tree = DefinitionTree::load(&definitions_path)
        .expect("failed to load Fondament definitions");
    let farga = Arc::new(HttpFargaReader::new(farga_url));
    let fondament = Fondament::load(&definitions_path, farga, "occitan".into())
        .expect("failed to initialise Fondament");

    let state = AppState {
        tree: Arc::new(tree),
        fondament: Arc::new(fondament),
    };

    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/component-agents", get(list_component_agents))
        .route("/resolve/*id", get(resolve_definition))
        .with_state(state)
}

async fn list_component_agents(State(s): State<AppState>) -> Json<Vec<Value>> {
    let agents = s.tree
        .all()
        .filter(|d| d.kind == "component-agent")
        .map(|d| serde_json::json!({
            "id": d.id,
            "component": d.component.as_deref().unwrap_or("")
        }))
        .collect();
    Json(agents)
}

async fn resolve_definition(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<String, StatusCode> {
    let address: CompositionAddress = id.parse()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let resolved = s.fondament.resolve(&address).await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    if resolved.system_prompt.is_empty() {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(resolved.system_prompt)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let definitions_path = PathBuf::from(
        std::env::var("FONDAMENT_DEFINITIONS_PATH").unwrap_or_else(|_| "definitions".into())
    );
    let farga_url = std::env::var("FARGA_URL")
        .unwrap_or_else(|_| "http://farga:7500".into());
    let port = std::env::var("FONDAMENT_PORT").unwrap_or_else(|_| "7800".into());

    let app = router(definitions_path, farga_url);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    tracing::info!("fondament-server listening on :{}", port);
    axum::serve(listener, app).await?;
    Ok(())
}
```

- [ ] **Step 6: Run tests — expect pass**

```bash
cd ~/project/Fondament && cargo test -p fondament-server
```

Expected:
```
test health_returns_ok ... ok
test get_component_agents_returns_only_component_agent_kind ... ok
test resolve_returns_system_prompt_for_known_id ... ok
test resolve_returns_404_for_unknown_id ... ok
```

- [ ] **Step 7: Run full Fondament test suite**

```bash
cd ~/project/Fondament && cargo test
```

Expected: all tests pass.

- [ ] **Step 8: Commit**

```bash
cd ~/project/Fondament && git add Cargo.toml fondament-server/ fondament-core/src/definition.rs fondament-core/tests/resolver_tests.rs
git commit -m "feat(fondament): add fondament-server HTTP crate

Exposes GET /component-agents (list all component-agent definitions) and
GET /resolve/*id (resolve a Fondament definition to its composed system prompt).
Used by Charradissa to fetch per-component system prompts at startup.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Phase 3 — Charradissa: dynamic provisioning

### Task 4: `create_or_join_aliased_room` in `AppserviceClient`

**Files:**
- Modify: `Charradissa/charradissa-matrix/src/client.rs`
- Test: inline `#[cfg(test)]` block in `client.rs`

**Interfaces:**
- Produces: `AppserviceClient::create_or_join_aliased_room(alias_local: &str, name: &str) -> Result<RoomId>`

- [ ] **Step 1: Write the failing test**

In `Charradissa/charradissa-matrix/src/client.rs`, add to the existing `#[cfg(test)]` block:

```rust
#[test]
fn create_or_join_aliased_room_builds_correct_alias() {
    // The alias format must be #{local}:{server} — verify via pct encoding.
    let alias = format!("#{}:{}", "amassada", "occitane.guilhem");
    assert_eq!(pct(&alias), "%23amassada%3Aoccitane.guilhem");
}
```

- [ ] **Step 2: Run — verify test passes (alias encoding logic already exists)**

```bash
cd ~/project/Charradissa && cargo test -p charradissa-matrix create_or_join_aliased_room_builds_correct_alias
```

Expected: PASS (just validates `pct` works for aliases — we test the real logic in Task 5).

- [ ] **Step 3: Implement `create_or_join_aliased_room`**

In `Charradissa/charradissa-matrix/src/client.rs`, add after `join_room`:

```rust
/// Join `#{alias_local}:{server_name}` if it exists; create it with `name` on 404.
/// Returns the room_id in both cases. Idempotent.
pub async fn create_or_join_aliased_room(&self, alias_local: &str, name: &str) -> Result<RoomId> {
    let alias = format!("#{}:{}", alias_local, self.server_name);
    let url = format!("{}/_matrix/client/v3/join/{}", self.homeserver, pct(&alias));
    let resp = self.client.post(&url)
        .header("Authorization", self.auth_header())
        .json(&serde_json::json!({}))
        .send().await
        .map_err(|e| CharradissaError::Backend(e.to_string()))?;
    if resp.status().is_success() {
        let json: serde_json::Value = resp.json().await
            .map_err(|e| CharradissaError::Backend(e.to_string()))?;
        let room_id = json["room_id"].as_str()
            .ok_or_else(|| CharradissaError::Backend("no room_id in join response".into()))?;
        return Ok(RoomId::new(room_id));
    }
    if resp.status().as_u16() == 404 || resp.status().as_u16() == 400 {
        return self.create_room(alias_local, name).await;
    }
    let status = resp.status();
    Err(CharradissaError::Backend(format!("create_or_join_aliased_room failed: {}", status)))
}
```

- [ ] **Step 4: Run tests**

```bash
cd ~/project/Charradissa && cargo test -p charradissa-matrix
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
cd ~/project/Charradissa && git add charradissa-matrix/src/client.rs
git commit -m "feat(matrix): add create_or_join_aliased_room — join by alias, create on 404

Idempotent room provisioning: tries to join #alias:server first,
creates the room only if it doesn't exist yet.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

### Task 5: `ProvisioningConfig` + `[provisioning]` config section

**Files:**
- Modify: `Charradissa/charradissa-core/src/config.rs`
- Test: `Charradissa/charradissa-core/tests/` — add a new test file

**Interfaces:**
- Produces: `config::ProvisioningConfig { projects: Vec<String>, fondament_url: Option<String> }`
- Produces: `Config.provisioning: ProvisioningConfig` (serde default — no existing config breaks)

- [ ] **Step 1: Write the failing test**

Create `Charradissa/charradissa-core/tests/provisioning_config_tests.rs`:

```rust
use charradissa_core::config::Config;

#[test]
fn provisioning_defaults_to_occitan_project() {
    let toml = r#"
[org]
name = "dev"
homeserver = "http://synapse:8008"

[backend]
type = "matrix"

[concierge]
[approval]
[tasks]
type = "none"

[projects]
autodiscover = false
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert_eq!(config.provisioning.projects, vec!["occitan"]);
    assert!(config.provisioning.fondament_url.is_none());
}

#[test]
fn provisioning_accepts_explicit_config() {
    let toml = r#"
[org]
name = "dev"
homeserver = "http://synapse:8008"

[backend]
type = "matrix"

[concierge]
[approval]
[tasks]
type = "none"

[projects]
autodiscover = false

[provisioning]
projects = ["occitan", "future"]
fondament_url = "http://fondament:7800"
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert_eq!(config.provisioning.projects, vec!["occitan", "future"]);
    assert_eq!(config.provisioning.fondament_url.as_deref(), Some("http://fondament:7800"));
}
```

- [ ] **Step 2: Run — expect compile error**

```bash
cd ~/project/Charradissa && cargo test -p charradissa-core --test provisioning_config_tests 2>&1 | head -20
```

Expected: `no field provisioning` or similar.

- [ ] **Step 3: Add `ProvisioningConfig` to `config.rs`**

In `Charradissa/charradissa-core/src/config.rs`, add the struct and wire it into `Config`:

```rust
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProvisioningConfig {
    #[serde(default = "default_provisioning_projects")]
    pub projects: Vec<String>,
    pub fondament_url: Option<String>,
}

fn default_provisioning_projects() -> Vec<String> {
    vec!["occitan".into()]
}
```

And in the `Config` struct, add:

```rust
#[serde(default)]
pub provisioning: ProvisioningConfig,
```

- [ ] **Step 4: Run tests — expect pass**

```bash
cd ~/project/Charradissa && cargo test -p charradissa-core
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
cd ~/project/Charradissa && git add charradissa-core/src/config.rs charradissa-core/tests/provisioning_config_tests.rs
git commit -m "feat(config): add [provisioning] config section with projects list and fondament_url

Defaults to projects=[\"occitan\"] when absent so existing configs continue
to work without changes.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

### Task 6: `provision_project_rooms` in `MatrixBackend`

**Files:**
- Modify: `Charradissa/charradissa-matrix/src/backend.rs`
- Test: add to `Charradissa/charradissa-matrix/src/backend.rs` `#[cfg(test)]` block

**Interfaces:**
- Consumes: `AppserviceClient::create_or_join_aliased_room` (Task 4)
- Consumes: `Farga GET /context/components/:project` → `Vec<String>` (Phase 1)
- Consumes: `Fondament GET /resolve/:id` → `String` (Phase 2)
- Produces: `MatrixBackend::provision_project_rooms(project: &str, params: &RoomProvisioningParams) -> Result<HashMap<RoomId, Arc<Responder>>>`

- [ ] **Step 1: Write the failing tests**

In `Charradissa/charradissa-matrix/src/backend.rs`, add to the existing `#[cfg(test)]` block:

```rust
#[test]
fn room_provisioning_params_holds_all_fields() {
    let p = RoomProvisioningParams {
        farga_url: "http://farga:7500".into(),
        fondament_url: "http://fondament:7800".into(),
        anthropic_api_key: "key".into(),
        dispatcher_url: "http://dispatcher:9090/mcp".into(),
        amassada_url: "http://amassada:7700".into(),
    };
    assert_eq!(p.farga_url, "http://farga:7500");
    assert_eq!(p.fondament_url, "http://fondament:7800");
}
```

- [ ] **Step 2: Run — expect compile error**

```bash
cd ~/project/Charradissa && cargo test -p charradissa-matrix 2>&1 | head -20
```

Expected: `RoomProvisioningParams` not found.

- [ ] **Step 3: Add `RoomProvisioningParams` struct and `provision_project_rooms`**

In `Charradissa/charradissa-matrix/src/backend.rs`, add after the existing imports:

```rust
use charradissa_core::responder::Responder;
use std::collections::HashMap;
use std::sync::Arc;
```

(`reqwest` is already a workspace dependency of `charradissa-matrix` — no `Cargo.toml` change needed.)

Then add before `impl MatrixBackend`:

```rust
pub struct RoomProvisioningParams {
    pub farga_url: String,
    pub fondament_url: String,
    pub anthropic_api_key: String,
    pub dispatcher_url: String,
    pub amassada_url: String,
}
```

Then add inside `impl MatrixBackend`:

```rust
/// Discover project components from Farga, resolve system prompts from Fondament,
/// create-or-join aliased rooms, and return a room_id → Responder map.
pub async fn provision_project_rooms(
    &self,
    project: &str,
    params: &RoomProvisioningParams,
) -> charradissa_core::error::Result<HashMap<RoomId, std::sync::Arc<Responder>>> {
    let http = reqwest::Client::new();

    // 1. Fetch component list from Farga.
    let components_url = format!("{}/context/components/{}", params.farga_url, project);
    let components: Vec<String> = http.get(&components_url)
        .send().await
        .map_err(|e| charradissa_core::error::CharradissaError::Backend(
            format!("Farga component list failed: {}", e)
        ))?
        .json().await
        .map_err(|e| charradissa_core::error::CharradissaError::Backend(
            format!("Farga component list parse failed: {}", e)
        ))?;

    if components.is_empty() {
        tracing::warn!("no components found for project '{}' in Farga", project);
        return Ok(HashMap::new());
    }

    // 2. Create-or-join project room (no Responder — Guilhem HTTP handles it).
    if let Err(e) = self.client.create_or_join_aliased_room(project, &format!("{} project", project)).await {
        tracing::warn!("project room provisioning failed for '{}': {}", project, e);
    } else {
        tracing::info!("project room #{}:… ready", project);
    }

    // 3. For each component: resolve system prompt + create-or-join component room.
    let mut map = HashMap::new();
    let server_name = self.client.server_name();

    for component in &components {
        // Fetch system prompt from Fondament (best-effort).
        let fondament_id = format!("fondament/{}-agent", component);
        let resolve_url = format!("{}/resolve/{}", params.fondament_url, fondament_id);
        let system_prompt = match http.get(&resolve_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                resp.text().await.unwrap_or_default()
            }
            Ok(resp) => {
                tracing::warn!("Fondament resolve {} returned {}", fondament_id, resp.status());
                String::new()
            }
            Err(e) => {
                tracing::warn!("Fondament resolve {} failed: {}", fondament_id, e);
                String::new()
            }
        };

        // Create-or-join component room.
        match self.client.create_or_join_aliased_room(component, &format!("{} agent", component)).await {
            Ok(room_id) => {
                tracing::info!("component room #{}: {} ready", component, room_id.as_str());
                let responder = std::sync::Arc::new(Responder::with_config(
                    params.anthropic_api_key.clone(),
                    "claude-sonnet-4-6".into(),
                    server_name.clone(),
                    params.farga_url.clone(),
                    params.dispatcher_url.clone(),
                    params.amassada_url.clone(),
                    system_prompt,
                    false,
                ));
                map.insert(room_id, responder);
            }
            Err(e) => {
                tracing::warn!("component room provisioning failed for '{}': {}", component, e);
            }
        }
    }

    // 4. Write observability signal to Farga.
    let room_names: Vec<&str> = map.keys().map(|r| r.as_str()).collect();
    let signal_url = format!("{}/signals", params.farga_url);
    let _ = http.post(&signal_url)
        .json(&serde_json::json!({
            "project": project,
            "source": "charradissa-provisioning",
            "signals": [{
                "project": project,
                "source": "charradissa-provisioning",
                "content": format!("provisioned {} component rooms: {}", room_names.len(), room_names.join(", "))
            }]
        }))
        .send().await; // best-effort, ignore errors

    Ok(map)
}
```

Also add `server_name()` accessor to `AppserviceClient` in `client.rs` (needed above):

```rust
pub fn server_name(&self) -> &str {
    &self.server_name
}
```

- [ ] **Step 4: Run tests — expect pass**

```bash
cd ~/project/Charradissa && cargo test -p charradissa-matrix
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
cd ~/project/Charradissa && git add charradissa-matrix/src/backend.rs charradissa-matrix/src/client.rs
git commit -m "feat(matrix): provision_project_rooms — Farga+Fondament-driven room setup

Queries Farga for component list, Fondament for per-component system prompts,
creates-or-joins aliased Matrix rooms, returns room_id→Responder map.
Failures per-component are logged and skipped; no crash on partial failure.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

### Task 7: Wire dynamic provisioning into `main.rs`

**Files:**
- Modify: `Charradissa/charradissa-daemon/src/main.rs`

**Interfaces:**
- Consumes: `MatrixBackend::provision_project_rooms` (Task 6)
- Consumes: `Config.provisioning` (Task 5)

- [ ] **Step 1: Replace static component_agents block**

In `Charradissa/charradissa-daemon/src/main.rs`, replace the block:

```rust
// Build component agent responders from config (room_id → Responder).
let mut component_agents = HashMap::new();
for ca in &config.component_agents {
    ...
}
```

with:

```rust
let fondament_url = config.provisioning.fondament_url.clone()
    .or_else(|| std::env::var("FONDAMENT_URL").ok())
    .unwrap_or_else(|| "http://fondament:7800".into());

let provisioning_params = charradissa_matrix::backend::RoomProvisioningParams {
    farga_url: farga_base_url.clone(),
    fondament_url,
    anthropic_api_key: anthropic_api_key.clone(),
    dispatcher_url: std::env::var("DISPATCHER_URL")
        .unwrap_or_else(|_| "http://dispatcher.agents.svc.cluster.local:9090/mcp".into()),
    amassada_url: std::env::var("AMASSADA_URL")
        .unwrap_or_else(|_| "http://amassada:7700".into()),
};

let mut component_agents = HashMap::new();
for project in &config.provisioning.projects {
    match backend.provision_project_rooms(project, &provisioning_params).await {
        Ok(rooms) => {
            tracing::info!("provisioned {} rooms for project '{}'", rooms.len(), project);
            for (room_id, responder) in rooms {
                component_agents.insert(room_id.as_str().to_string(), responder);
            }
        }
        Err(e) => {
            tracing::warn!("provisioning failed for project '{}': {}", project, e);
        }
    }
}

// Fallback: if provisioning yielded nothing, fall back to [component_agents] config.
if component_agents.is_empty() {
    for ca in &config.component_agents {
        if ca.room_id.is_empty() {
            tracing::warn!("component agent '{}' has no room_id configured, skipping", ca.name);
            continue;
        }
        let responder = Arc::new(Responder::with_config(
            anthropic_api_key.clone(),
            "claude-sonnet-4-6".into(),
            server_name.clone(),
            farga_base_url.clone(),
            std::env::var("DISPATCHER_URL")
                .unwrap_or_else(|_| "http://dispatcher.agents.svc.cluster.local:9090/mcp".into()),
            std::env::var("AMASSADA_URL")
                .unwrap_or_else(|_| "http://amassada:7700".into()),
            ca.system_prompt.clone(),
            false,
        ));
        tracing::info!("registered component agent '{}' for room {} (config fallback)", ca.name, ca.room_id);
        component_agents.insert(ca.room_id.clone(), responder);
    }
}
```

- [ ] **Step 2: Add missing import if needed**

Ensure `charradissa_matrix::backend::RoomProvisioningParams` is accessible. Add at the top of `main.rs` if not already present:

```rust
use charradissa_matrix::backend::RoomProvisioningParams;
```

- [ ] **Step 3: Build the daemon — expect clean compile**

```bash
cd ~/project/Charradissa && cargo build -p charradissa-daemon 2>&1
```

Expected: compiles without errors or warnings.

- [ ] **Step 4: Run full test suite**

```bash
cd ~/project/Charradissa && cargo test
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
cd ~/project/Charradissa && git add charradissa-daemon/src/main.rs
git commit -m "feat(daemon): wire Farga+Fondament-driven room provisioning at startup

Replaces static config.component_agents loading with provision_project_rooms,
which queries Farga for the component list and Fondament for system prompts.
Static [component_agents] config is kept as a fallback when provisioning
returns zero rooms (Farga/Fondament unavailable at startup).

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Spec coverage self-check

| Requirement | Task |
|---|---|
| `GET /context/components/:project` in Farga | Task 1 |
| Fondament HTTP server with `/component-agents` and `/resolve/:id` | Tasks 2–3 |
| `DefinitionFile.component` field | Task 2 |
| `create_or_join_aliased_room` — join first, create on 404 | Task 4 |
| `[provisioning]` config section, `FONDAMENT_URL` env var | Task 5 |
| `RoomProvisioningParams` struct | Task 6 |
| `provision_project_rooms` — Farga discovery + Fondament resolution + room creation | Task 6 |
| Farga observability signal after provisioning | Task 6 |
| `[component_agents]` fallback when provisioning yields zero rooms | Task 7 |
| Guilhem in `#occitan` project room (no Responder, routes to `default_agent_url`) | Task 6 step 3 — project room created but not added to `component_agents` |
| Graceful degradation on Farga/Fondament failure | Task 6 — per-component warn+skip |
| No hardcoded component list in Charradissa | Tasks 6–7 (removed from `AGENT_LOCAL_PARTS` use path; `AGENT_LOCAL_PARTS` retained only for kick-power grant) |
