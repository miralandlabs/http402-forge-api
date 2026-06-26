use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::state::SharedState;

pub async fn rate_limit_middleware(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    let ip = addr.ip().to_string();
    if state.rate_limiter.check(&ip) {
        next.run(request).await
    } else {
        (
            StatusCode::TOO_MANY_REQUESTS,
            [(axum::http::header::RETRY_AFTER, "1")],
            "rate limit exceeded",
        )
            .into_response()
    }
}
