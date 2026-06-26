mod events;
mod health;
mod leaderboards;
mod listings;
mod seller;
mod well_known;

use axum::{
    extract::DefaultBodyLimit,
    http::HeaderValue,
    routing::{get, post},
    Router,
};
use tower_http::cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::state::SharedState;

fn cors_layer(origins: &[String]) -> CorsLayer {
    let allowed: Vec<HeaderValue> = origins.iter().filter_map(|o| o.parse().ok()).collect();
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(allowed))
        .allow_methods(AllowMethods::list([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::DELETE,
            axum::http::Method::OPTIONS,
        ]))
        .allow_headers(AllowHeaders::list([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderName::from_static("payment-signature"),
        ]))
}

pub fn router(state: SharedState) -> Router {
    let max_body = state.config.max_asset_bytes + state.config.max_preview_bytes + 1_048_576;
    let cors = cors_layer(&state.config.cors_allowed_origins);

    Router::new()
        .route("/health", get(health::health))
        .route("/api/v1/seller/challenge", get(seller::challenge))
        .route(
            "/api/v1/seller/delist-challenge",
            get(seller::delist_challenge),
        )
        .route("/api/v1/seller/status", get(seller::status))
        .route("/api/v1/seller/provision-tx", post(seller::provision_tx))
        .route(
            "/api/v1/listings",
            get(listings::list).post(listings::create),
        )
        .route(
            "/api/v1/listings/{id}",
            get(listings::get_one).delete(listings::delist),
        )
        .route("/api/v1/listings/{id}/preview", get(listings::preview))
        .route("/api/v1/listings/{id}/download", get(listings::download))
        .route("/api/v1/leaderboards", get(leaderboards::leaderboards))
        .route("/api/v1/events", get(events::sse))
        .route(
            "/.well-known/x402-resources.json",
            get(well_known::x402_resources),
        )
        .layer(DefaultBodyLimit::max(max_body as usize))
        .layer(RequestBodyLimitLayer::new(max_body as usize))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
