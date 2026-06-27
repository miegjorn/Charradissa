mod registry;
mod queue_api;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use charradissa_core::config::Config;
use charradissa_core::concierge::ConciergeAgent;
use charradissa_core::farcaster::MilestoneEvent;
use charradissa_core::farcaster::FarcasterAgent;
use charradissa_core::farcaster::ClaudeFarcasterAnalyzer;
use charradissa_core::farga::HttpFargaWriter;
use charradissa_matrix::backend::MatrixBackend;
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

    let as_token = std::env::var("MATRIX_AS_TOKEN")
        .unwrap_or_else(|_| "dev-token".into());
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

    let appservice_port = std::env::var("CHARRADISSA_PORT").unwrap_or("8448".into());

    // Default agent URL: prefer [agents].default in config, fall back to GUILHEM_URL env var.
    let default_agent_url = config.agents.default.clone()
        .or_else(|| std::env::var("GUILHEM_URL").ok())
        .unwrap_or_else(|| "http://guilhem.agents.svc.cluster.local:8080".into());

    // Build component agent responders from config (room_id → Responder).
    let mut component_agents = HashMap::new();
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
            std::env::var("DISPATCHER_URL").unwrap_or_else(|_| "http://dispatcher.agents.svc.cluster.local:9090/mcp".into()),
            std::env::var("AMASSADA_URL").unwrap_or_else(|_| "http://amassada:7700".into()),
            ca.system_prompt.clone(),
            false, // component agents do not get org-level tools
        ));
        tracing::info!("registered component agent '{}' for room {}", ca.name, ca.room_id);
        component_agents.insert(ca.room_id.clone(), responder);
    }

    let appservice_state = AppserviceState {
        hs_token: as_token.clone(),
        default_agent_url,
        agent_routes: config.agents.routes.clone(),
        backend: Arc::clone(&backend) as Arc<dyn charradissa_core::backend::ChatBackend>,
        self_user_id: bot_user_id.clone(),
        component_agents,
    };

    let app = Router::new()
        .route("/health", axum::routing::get(|| async { "ok" }))
        .route("/_matrix/app/v1/transactions/:txnId",
            put(charradissa_matrix::appservice::handle_transaction))
        .with_state(appservice_state)
        .merge(queue_api::router(queue_state));

    if let Err(e) = backend.ensure_registered().await {
        tracing::warn!("self-registration failed: {}", e);
    }
    if let Err(e) = backend.set_self_display_name("Guilhem").await {
        tracing::warn!("set display name failed: {}", e);
    }

    // Grant kick power (PL 50) to all component agents in every room Charradissa is
    // currently in. This backfills existing rooms and is idempotent — rooms that
    // already have PL ≥ 50 for agents are left untouched.
    if let Err(e) = backend.provision_agent_kick_power().await {
        tracing::warn!("kick power provisioning failed: {}", e);
    }

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", appservice_port)).await?;
    tracing::info!("charradissa-daemon webhook listening on :{}", appservice_port);
    axum::serve(listener, app).await?;
    Ok(())
}
