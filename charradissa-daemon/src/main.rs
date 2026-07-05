mod registry;
mod queue_api;
mod mcp_api;

use std::collections::HashMap;
use std::sync::Arc;
use charradissa_core::config::Config;
use charradissa_core::concierge::ConciergeAgent;
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
    // Renamed off @charradissa: that Matrix username is now the independent
    // charradissa *component agent*'s own identity (it chronicles the
    // Charradissa GitHub repo) — see Occitan#per-agent-matrix-independence.
    // This relay identity keeps the DM fabric, #occitan project room, and
    // concierge archival responsibilities, none of which this rename affects.
    let bot_user_id = format!("@charradissa-relay:{}", server_name);

    let backend = Arc::new(MatrixBackend::new(
        config.org.homeserver.clone(),
        as_token.clone(),
        bot_user_id.clone(),
        server_name.clone(),
    ));

    let anthropic_api_key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();
    let xai_api_key = std::env::var("XAI_API_KEY").ok();
    let farga_base_url = std::env::var("FARGA_URL")
        .unwrap_or_else(|_| "http://farga:7500".into());

    let mut registry = registry::AgentRegistry::new();
    let _ = &mut registry; // suppress unused warning — populated in future tasks

    let concierge = ConciergeAgent::new(
        Arc::clone(&backend) as Arc<dyn charradissa_core::backend::ChatBackend>,
        Arc::new(HttpFargaWriter::new(farga_base_url.clone())),
        vec![],
        HashMap::new(),
        24, 6, 50_000,
    );

    let concierge = Arc::new(concierge);

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
    if let Err(e) = backend.set_self_display_name("Charradissa").await {
        tracing::warn!("set display name failed: {}", e);
    }

    let fondament_url = config.provisioning.fondament_url.clone()
        .or_else(|| std::env::var("FONDAMENT_URL").ok())
        .unwrap_or_else(|| "http://fondament:7800".into());

    let provisioning_params = RoomProvisioningParams {
        farga_url: farga_base_url.clone(),
        fondament_url,
        anthropic_api_key: anthropic_api_key.clone(),
        xai_api_key: xai_api_key.clone(),
        dispatcher_url: std::env::var("DISPATCHER_URL")
            .unwrap_or_else(|_| "http://dispatcher.agents.svc.cluster.local:9090/mcp".into()),
        amassada_url: std::env::var("AMASSADA_URL")
            .unwrap_or_else(|_| "http://amassada:7700".into()),
    };

    // Component agents (gardian, fondament, farga, amassada, cor, caissa,
    // charradissa, nervi) and guilhem now run their own independent Matrix
    // sessions (see Caissa/caissa-cli/src/commands/listen.rs's
    // run_matrix_client_loop) — Charradissa no longer provisions or relays
    // for their rooms. `provisioning_params` and `RoomProvisioningParams`
    // are kept (used by nothing now, harmless) rather than threading a
    // larger removal through this file; a future cleanup can drop them.
    let component_agents: HashMap<String, (String, Arc<Responder>)> = HashMap::new();

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

    let kroki_url = std::env::var("KROKI_URL").ok();
    if let Some(ref url) = kroki_url {
        tracing::info!("Mermaid rendering enabled via Kroki at {}", url);
    }

    let appservice_state = AppserviceState {
        hs_token,
        default_agent_url,
        agent_routes: config.agents.routes.clone(),
        backend: Arc::clone(&backend) as Arc<dyn charradissa_core::backend::ChatBackend>,
        self_user_id: bot_user_id.clone(),
        component_agents,
        project_routes,
        approval_queue: Arc::clone(&persistent_queue),
        kroki_url,
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

    let approval_room_id = std::env::var("APPROVAL_ROOM_ID").unwrap_or_else(|_| {
        tracing::warn!("APPROVAL_ROOM_ID not set — matrix_request_approval will fail to post notifications");
        String::new()
    });

    // Ensure the appservice bot is a member of the shared approval room before it
    // needs to post there. That room is created externally (its ID arrives via
    // APPROVAL_ROOM_ID) and never invites the bot, so without this every
    // matrix_request_approval 403s regardless of which component called it (#46).
    // Best-effort — a failure here only degrades approval posting, which logs its
    // own error, so it must not block startup.
    if !approval_room_id.is_empty() {
        let admin_token = std::env::var("SYNAPSE_ADMIN_TOKEN").ok();
        match backend
            .provision_approval_room(
                &charradissa_core::types::RoomId::new(&approval_room_id),
                admin_token.as_deref(),
            )
            .await
        {
            Ok(()) => tracing::info!("approval room {}: bot membership ensured", approval_room_id),
            Err(e) => tracing::warn!(
                "approval room {} membership provisioning failed ({}); \
                 matrix_request_approval may 403 until the bot is invited manually",
                approval_room_id, e
            ),
        }
    }

    let matrix_mcp = std::sync::Arc::new(
        charradissa_matrix::mcp::MatrixMcp::new(
            backend.appservice_client(),
            dm_registry,
            Arc::clone(&persistent_queue),
            approval_room_id,
            config.approval.timeout_minutes,
        ),
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
/// `CHARRADISSA_URL` (defaulting to the in-cluster Service URL).
///
/// `component_localparts` is intentionally empty: guilhem and the 8 component
/// agents are now real, independently-registered Matrix users (see Occitan's
/// per-agent-matrix-independence work), not appservice-exclusive ghosts —
/// claiming their usernames here would make Synapse reject the bootstrap
/// job's registration of those same usernames with M_EXCLUSIVE. This appservice
/// only still owns its own sender identity and any `@charradissa-relay-*`
/// specialist virtual users.
fn build_registration(config: &Config) -> String {
    use charradissa_core::registration::{generate_registration, RegistrationParams};

    let listen_port = charradissa_core::config::listen_port();
    let params = RegistrationParams {
        id: "charradissa".into(),
        url: std::env::var("CHARRADISSA_URL")
            .unwrap_or_else(|_| format!("http://charradissa:{listen_port}")),
        as_token: std::env::var("MATRIX_AS_TOKEN").unwrap_or_else(|_| "dev-token".into()),
        hs_token: std::env::var("MATRIX_HS_TOKEN").unwrap_or_else(|_| "dev-hs-token".into()),
        sender_localpart: "charradissa-relay".into(),
        server_name: config.org.server_name.clone(),
        component_localparts: vec![],
    };
    generate_registration(&params)
}
