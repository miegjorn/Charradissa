mod registry;

use std::sync::Arc;
use charradissa_core::config::Config;
use charradissa_matrix::backend::MatrixBackend;
use charradissa_matrix::appservice::AppserviceState;
use axum::{routing::put, Router};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config_path = std::env::var("CHARRADISSA_CONFIG")
        .unwrap_or("charradissa.toml".into());
    let config = Config::from_file(&config_path)
        .map_err(|e| anyhow::anyhow!("config error: {}", e))?;

    let as_token = std::env::var("MATRIX_AS_TOKEN")
        .unwrap_or_else(|_| "dev-token".into());
    let bot_user_id = format!("@charradissa:{}",
        config.org.homeserver.trim_start_matches("https://").trim_start_matches("http://"));

    let backend = Arc::new(MatrixBackend::new(
        config.org.homeserver.clone(),
        as_token.clone(),
        bot_user_id,
    ));

    let mut registry = registry::AgentRegistry::new();
    tracing::info!("charradissa-daemon starting for org: {}", config.org.name);

    let appservice_port = std::env::var("CHARRADISSA_PORT").unwrap_or("8448".into());
    let appservice_state = AppserviceState { hs_token: as_token };

    let app = Router::new()
        .route("/_matrix/app/v1/transactions/:txnId",
            put(charradissa_matrix::appservice::handle_transaction))
        .with_state(appservice_state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", appservice_port)).await?;
    tracing::info!("charradissa-daemon webhook listening on :{}", appservice_port);
    axum::serve(listener, app).await?;
    Ok(())
}
