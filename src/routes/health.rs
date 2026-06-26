use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;

use crate::state::SharedState;

pub async fn health(State(state): State<SharedState>) -> impl IntoResponse {
    let database_ok = state.db.health_check().await;
    let status_code = if database_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status_code,
        Json(json!({
            "status": if database_ok { "healthy" } else { "degraded" },
            "service": "http402-forge-api",
            "version": state.config.version,
            "cluster": state.cluster.label,
            "database": state.db.kind().label(),
            "databaseOk": database_ok,
        })),
    )
}
