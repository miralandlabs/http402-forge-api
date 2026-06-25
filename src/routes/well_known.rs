use axum::extract::State;
use axum::Json;
use serde_json::json;

use crate::state::SharedState;

pub async fn x402_resources(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let base = state.config.seller_public_base_url.trim_end_matches('/');
    Json(json!({
        "resources": [
            {
                "url": format!("{base}/api/v1/listings/{{id}}/download"),
                "method": "GET",
                "scheme": "exact",
                "description": "Forge marketplace paid download",
                "category": "marketplace",
                "tags": ["forge", "digital-goods", "download"]
            }
        ]
    }))
}
