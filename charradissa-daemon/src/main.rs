mod registry;
mod queue_api;
mod mcp_api;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use charradissa_core::config::Config;
use charradissa_core::concierge::ConciergeAgent;
use charradissa_core::farcaster::MilestoneEvent;
use charradissa_core::farcaster::FarcasterAgent;
use charradissa_core::farcaster::ClaudeFarcasterAnalyzer;
use charradissa_core::farga::HttpFargaWriter;
use charradissa_matrix::backend::{MatrixBackend, RoomProvisioningParams};
use charradissa_matrix::appservice::AppserviceState;
use charradissa_core::responder::Responder;
use axum::{routing::put, Router};
use charradissa_core::approval::PersistentApprovalQueue;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config_path = std::env::var("CHARRADISSA_CONFIG")
        .unwrap_or("charradissa.toml".into());
    let config = Config::from_file(&config_path)
        .map_err(|e| anyhow::anyhow!("config error: {}", e))?;

    // `charradissa-daemon --generate-registration` prints the Synapse appservice
    // registration YAML to stdout and exits. This is the source of truth for the
    // file Synapse loads from /data/charradissa-registration.yaml.
    if std::env::args().any(|a| a == "--generate-registration") {
        print!("{}", build_registration(&config));
        return Ok(());
    }

    let as_token = std::env::var("MATRIX_AS_TOKEN")
        .unwrap_or_else(|_| "dev-token".into());
    let hs_token = charradissa_core::config::hs_token(&as_token);
    let server_name = config.org.server_name.clone();
    let bot_user_id = format!("@charradissa:{}", server_name);

    let backend = Arc::new(MatrixBackend::new(
        config.org.homeserver.clone(),
        as_token.clone(),
        bot_user_id.clone(),
        server_name.clone(),
    ));

    // Milestone broadcast channel — sender is used by appservice handlers (future task),
    // receiver is consumed by the dispatch task below.
    let (milestone_tx, mut milestone_rx) =
        tokio::sync::broadcast::channel::<MilestoneEvent>(256);
    let _ = milestone_tx; // suppress unused warning until appservice wiring is added

    let anthropic_api_key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();
    let farga_base_url = std::env::var("FARGA_URL")
        .unwrap_or_else(|_| "http://farga:7500".into());

    let mut registry = registry::AgentRegistry::new();
    let _ = &mut registry; // suppress unused warning — populated in future tasks

    // ConciergeAgent owns the FarcasterAgent — no Arc needed for the agent itself.
    let mut concierge = ConciergeAgent::new(
        Arc::clone(&backend) as Arc<dyn charradissa_core::backend::ChatBackend>,
        Arc::new(HttpFargaWriter::new(farga_base_url.clone())),
        vec![],
        HashMap::new(),
        24, 6, 50_000,
    );

    concierge.register_system_agent(
        Box::new(FarcasterAgent::new(
            Arc::clone(&backend) as Arc<dyn charradissa_core::backend::ChatBackend>,
            Arc::new(HttpFargaWriter::new(farga_base_url.clone())),
            Arc::new(ClaudeFarcasterAnalyzer::new(anthropic_api_key.clone())),
            vec![], // projects populated from config in a future task
            HashMap::new(),
        )),
        Duration::from_secs(6 * 3600),
    );

    let concierge = Arc::new(concierge);

    // Dispatch milestones from the broadcast channel to all registered system agents.
    let concierge_dispatch = Arc::clone(&concierge);
    tokio::spawn(async move {
        loop {
            match milestone_rx.recv().await {
                Ok(event) => concierge_dispatch.dispatch_milestone(&event).await,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("farcaster: milestone receiver lagged, dropped {} events", n);
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    // Run system agent tick loop (polls every 60s, calls tick() when interval elapses).
    let concierge_ticks = Arc::clone(&concierge);
    tokio::spawn(async move {
        concierge_ticks.run_system_agent_ticks().await;
    });

    let concierge_archival = Arc::clone(&concierge);
    tokio::spawn(async move { concierge_archival.run_archival_loop().await; });

    tracing::info!("charradissa-daemon starting for org: {}", config.org.name);

    let queue_file = std::env::var("CHARRADISSA_QUEUE_FILE")
        .unwrap_or_else(|_| "charradissa-queue.json".into());
    let persistent_queue = Arc::new(PersistentApprovalQueue::new(queue_file.into()));
    let queue_state = queue_api::QueueState { queue: Arc::clone(&persistent_queue) };

    let appservice_port = charradissa_core::config::listen_port();

    // Default agent URL: prefer [agents].default in config, fall back to GUILHEM_URL env var.
    let default_agent_url = config.agents.default.clone()
        .or_else(|| std::env::var("GUILHEM_URL").ok())
        .unwrap_or_else(|| "http://guilhem.agents.svc.cluster.local:8080".into());

    // Startup: ensure the appservice is registered with Synapse and set display name.
    if let Err(e) = backend.ensure_registered().await {
        tracing::warn!("self-registration failed: {}", e);
    }
    if let Err(e) = backend.set_self_display_name("Guilhem").await {
        tracing::warn!("set display name failed: {}", e);
    }

    let fondament_url = config.provisioning.fondament_url.clone()
        .or_else(|| std::env::var("FONDAMENT_URL").ok())
        .unwrap_or_else(|| "http://fondament:7800".into());

    let provisioning_params = RoomProvisioningParams {
        farga_url: farga_base_url.clone(),
        fondament_url,
        anthropic_api_key: anthropic_api_key.clone(),
        dispatcher_url: std::env::var("DISPATCHER_URL")
            .unwrap_or_else(|_| "http://dispatcher.agents.svc.cluster.local:9090/mcp".into()),
        amassada_url: std::env::var("AMASSADA_URL")
            .unwrap_or_else(|_| "http://amassada:7700".into()),
    };

    // Dynamic provisioning: query Farga for each project's components, resolve system
    // prompts via Fondament, and create/join the corresponding Matrix rooms.
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

    // Fallback: if provisioning yielded nothing (Farga/Fondament unavailable at startup),
    // fall back to static [component_agents] config entries.
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
                false, // component agents do not get org-level tools
            ));
            tracing::info!("registered component agent '{}' for room {} (config fallback)", ca.name, ca.room_id);
            component_agents.insert(ca.room_id.clone(), responder);
        }
    }

    // Build project_routes: expand each ProjectAgentConfig's room list into
    // a flat room_id → config map (cloning the config per room so the lookup
    // is O(1) and doesn't require an extra indirection at dispatch time).
    let mut project_routes = HashMap::new();
    for proj in &config.agents.project {
        for room in &proj.rooms {
            project_routes.insert(room.clone(), proj.clone());
        }
    }

    // Provision the DM fabric: ensure a direct room exists between Guilhem (the
    // appservice sender) and each component agent. Runs after identity setup and
    // room provisioning, when Matrix auth is ready. Idempotent — DMs already
    // recorded in the sender's m.direct account_data are reused. (Charradissa #22)
    match backend.provision_dm_rooms(&farga_base_url).await {
        Ok(dms) => tracing::info!("DM fabric ready: {} rooms", dms.len()),
        Err(e) => tracing::warn!("DM room provisioning failed: {}", e),
    }

    // Grant kick power (PL 50) to all component agents in every room Charradissa is now
    // in. Running after provisioning ensures newly-created rooms receive the grant in
    // this startup cycle. Idempotent — rooms already at PL ≥ 50 are left untouched.
    if let Err(e) = backend.provision_agent_kick_power().await {
        tracing::warn!("kick power provisioning failed: {}", e);
    }

    let appservice_state = AppserviceState {
        hs_token,
        default_agent_url,
        agent_routes: config.agents.routes.clone(),
        backend: Arc::clone(&backend) as Arc<dyn charradissa_core::backend::ChatBackend>,
        self_user_id: bot_user_id.clone(),
        component_agents,
        project_routes,
    };

    // Matrix MCP tool server (Charradissa#23): lets agents act in Matrix (send/invite/kick)
    // and resolve DM rooms. It shares the appservice's Matrix token, so MCP actions speak as
    // the same identity as inbound handling. The DM registry is read from the path named by
    // CHARRADISSA_DM_REGISTRY (provisioned by Charradissa#22); a missing file degrades to an
    // empty registry so matrix_get_dm reports "not provisioned" rather than crashing.
    let dm_registry = charradissa_core::dm_registry::DmRegistry::from_env()
        .unwrap_or_else(|e| {
            tracing::warn!("DM registry load failed ({}); matrix_get_dm will be empty", e);
            Default::default()
        });
    tracing::info!("Matrix MCP: DM registry loaded with {} entries", dm_registry.len());
    let matrix_mcp = std::sync::Arc::new(
        charradissa_matrix::mcp::MatrixMcp::new(backend.appservice_client(), dm_registry),
    );

    let app = Router::new()
        .route("/health", axum::routing::get(|| async { "ok" }))
        .route("/_matrix/app/v1/transactions/:txnId",
            put(charradissa_matrix::appservice::handle_transaction))
        .with_state(appservice_state)
        .merge(queue_api::router(queue_state))
        .merge(mcp_api::router(mcp_api::McpState { mcp: matrix_mcp }));

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", appservice_port)).await?;
    tracing::info!("charradissa-daemon webhook listening on :{}", appservice_port);
    axum::serve(listener, app).await?;
    Ok(())
}

/// Build the Synapse appservice registration YAML from config and environment.
///
/// Tokens come from `MATRIX_AS_TOKEN` / `MATRIX_HS_TOKEN`; the callback URL from
/// `CHARRADISSA_URL` (defaulting to the in-cluster Service URL). The user
/// namespace grants each of the seven component agents its own Matrix identity.
fn build_registration(config: &Config) -> String {
    use charradissa_core::registration::{
        generate_registration, RegistrationParams, COMPONENT_AGENT_LOCALPARTS,
    };

    let listen_port = charradissa_core::config::listen_port();
    let params = RegistrationParams {
        id: "charradissa".into(),
        url: std::env::var("CHARRADISSA_URL")
            .unwrap_or_else(|_| format!("http://charradissa:{listen_port}")),
        as_token: std::env::var("MATRIX_AS_TOKEN").unwrap_or_else(|_| "dev-token".into()),
        hs_token: std::env::var("MATRIX_HS_TOKEN").unwrap_or_else(|_| "dev-hs-token".into()),
        sender_localpart: "charradissa".into(),
        server_name: config.org.server_name.clone(),
        component_localparts: COMPONENT_AGENT_LOCALPARTS
            .iter()
            .map(|s| s.to_string())
            .collect(),
    };
    generate_registration(&params)
}
