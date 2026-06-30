use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;

use crate::error::AppError;
use crate::state::SharedState;
use crate::storage::ObjectStore;

const STORAGE_PROBE_KEY: &str = "__forge_health_probe__";

async fn storage_ok(state: &SharedState) -> bool {
    match state.storage.head(STORAGE_PROBE_KEY).await {
        Ok(_) => true,
        Err(AppError::NotFound) => true,
        Err(_) => false,
    }
}

pub async fn health(State(state): State<SharedState>) -> impl IntoResponse {
    let database_ok = state.db.health_check().await;
    let storage_ok = storage_ok(&state).await;
    let facilitator_ok = state.facilitator.ping_supported().await;
    let all_ok = database_ok && storage_ok && facilitator_ok;
    let status_code = if all_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status_code,
        Json(json!({
            "status": if all_ok { "healthy" } else { "degraded" },
            "service": "http402-forge-api",
            "version": state.config.version,
            "cluster": state.cluster.label,
            "database": state.db.kind().label(),
            "databaseOk": database_ok,
            "storageOk": storage_ok,
            "facilitatorOk": facilitator_ok,
        })),
    )
}
