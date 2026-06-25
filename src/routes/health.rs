use axum::extract::State;
use axum::Json;
use serde_json::json;

use crate::state::SharedState;

pub async fn health(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let database_ok = state.db.health_check().await;
    Json(json!({
        "status": if database_ok { "healthy" } else { "degraded" },
        "service": "http402-forge-api",
        "version": state.config.version,
        "cluster": state.cluster.label,
        "database": state.db.kind().label(),
        "databaseOk": database_ok,
    }))
}
