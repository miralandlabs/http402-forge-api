use axum::extract::State;
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::state::SharedState;

fn portal_base_url(seller_public_base_url: &str) -> &'static str {
    if seller_public_base_url.contains("preview.forge")
        || seller_public_base_url.contains("preview.http402")
    {
        "https://preview.http402.trade"
    } else if seller_public_base_url.contains("127.0.0.1") || seller_public_base_url.contains("localhost")
    {
        "http://127.0.0.1:5175"
    } else {
        "https://http402.trade"
    }
}

pub async fn x402_resources(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let base = state.config.seller_public_base_url.trim_end_matches('/');
    let portal = portal_base_url(base);
    Json(json!({
        "x402Version": 2,
        "hubUrl": portal,
        "portalManifestUrl": format!("{portal}/.well-known/x402-portal.json"),
        "openApiUrl": format!("{base}/openapi.yaml"),
        "agentDiscovery": {
            "catalog": format!("{base}/api/v1/listings"),
            "capabilities": format!("{base}/api/v1/capabilities"),
            "events": format!("{base}/api/v1/events"),
            "leaderboards": format!("{base}/api/v1/leaderboards")
        },
        "resources": [
            {
                "url": format!("{base}/api/v1/listings/{{id}}/download"),
                "method": "GET",
                "scheme": "exact",
                "description": "Forge marketplace paid download (Digital Bazaar channel)",
                "category": "marketplace",
                "tags": ["forge", "digital-goods", "download"],
                "inventory": format!("{base}/api/v1/listings"),
                "agentFriendlyFilter": "agent_friendly=true"
            }
        ]
    }))
}

pub async fn openapi_spec() -> Response {
    (
        [(header::CONTENT_TYPE, HeaderValue::from_static("application/yaml"))],
        include_str!("../../docs/openapi.yaml"),
    )
        .into_response()
}
