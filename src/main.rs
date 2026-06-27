mod auth;
mod config;
mod db;
mod error;
mod logging;
mod models;
mod moderation;
mod preview;
mod rate_limit;
mod routes;
mod state;
mod storage;
mod x402;

use std::sync::Arc;

use std::net::SocketAddr;
use tracing::info;

use crate::config::{AppConfig, ClusterConfig};
use crate::db::Database;
use crate::state::AppState;

#[tokio::main]
async fn main() {
    let env_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".env");
    let _ = dotenvy::from_path(env_path);
    let _log_guard = logging::init();

    if let Err(e) = run().await {
        tracing::error!(error = %e, "fatal");
        eprintln!("http402-forge-api failed: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = AppConfig::from_env().map_err(|e| {
        tracing::error!("{e}");
        e
    })?;
    let cluster = ClusterConfig::for_cluster(config.cluster);
    let db = Database::connect(&config.database_url).await.map_err(|e| {
        tracing::error!("{e}");
        e
    })?;

    let state = Arc::new(
        AppState::build(config.clone(), cluster, db)
            .await
            .map_err(|e| {
                tracing::error!("{e}");
                e
            })?,
    );

    let bind = state.config.bind_addr;
    let app = routes::router(state.clone());
    let listener = tokio::net::TcpListener::bind(bind).await?;
    info!(
        cluster = state.cluster.label,
        database = state.db.kind().label(),
        version = %state.config.version,
        storage = ?state.config.storage_backend,
        max_asset_bytes = state.config.max_asset_bytes,
        max_preview_bytes = state.config.max_preview_bytes,
        "http402-forge-api listening on {bind}"
    );
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
