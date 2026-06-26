use axum::{
    extract::{Path, Query, State},
    http::header,
    response::Response,
    Json,
};
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::parse_redownload_listing_id;
use crate::error::{AppError, AppResult};
use crate::models::validate_wallet;
use crate::routes::listings::build_asset_download_response;
use crate::state::SharedState;

#[derive(Debug, Deserialize)]
pub struct RedownloadChallengeQuery {
    pub buyer_wallet: String,
    pub listing_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChallengeResponse {
    pub message: String,
    pub expires_at: DateTime<Utc>,
    pub sale_id: Uuid,
}

pub async fn redownload_challenge(
    State(state): State<SharedState>,
    Query(q): Query<RedownloadChallengeQuery>,
) -> AppResult<impl axum::response::IntoResponse> {
    validate_wallet(&q.buyer_wallet).map_err(|m| AppError::validation("buyer_wallet", m))?;
    let listing_id = Uuid::parse_str(q.listing_id.trim())
        .map_err(|_| AppError::validation("listing_id", "invalid uuid"))?;

    let sale = state
        .db
        .find_buyer_sale_for_listing(listing_id, &q.buyer_wallet)
        .await?
        .ok_or(AppError::NotFound)?;

    let (message, expires_at) = state
        .seller_auth
        .issue_redownload_challenge(&q.buyer_wallet, listing_id)?;

    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(ChallengeResponse {
            message,
            expires_at,
            sale_id: sale.id,
        }),
    ))
}

fn buyer_auth_header(headers: &axum::http::HeaderMap, name: &str) -> AppResult<String> {
    let raw = headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::validation(name, "required"))?;
    base64::engine::general_purpose::STANDARD
        .decode(raw)
        .map_err(|_| AppError::validation(name, "invalid base64"))
        .and_then(|bytes| {
            String::from_utf8(bytes).map_err(|_| AppError::validation(name, "invalid utf-8"))
        })
}

pub async fn redownload(
    State(state): State<SharedState>,
    Path(listing_id): Path<Uuid>,
    headers: axum::http::HeaderMap,
) -> AppResult<Response> {
    let buyer_wallet = buyer_auth_header(&headers, "x-forge-buyer-wallet")?;
    validate_wallet(&buyer_wallet).map_err(|m| AppError::validation("buyer_wallet", m))?;
    let buyer_challenge = buyer_auth_header(&headers, "x-forge-buyer-challenge")?;
    let buyer_signature = headers
        .get("x-forge-buyer-signature")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::validation("buyer_signature", "required"))?
        .to_string();

    if !state.config.skip_buyer_auth {
        state.seller_auth.verify_and_consume(
            &buyer_wallet,
            &buyer_challenge,
            &buyer_signature,
        )?;
        let challenge_listing = parse_redownload_listing_id(&buyer_challenge)
            .ok_or_else(|| AppError::Forbidden("invalid redownload challenge".into()))?;
        if challenge_listing != listing_id {
            return Err(AppError::Forbidden(
                "redownload challenge listing mismatch".into(),
            ));
        }
    }

    let sale = state
        .db
        .find_buyer_sale_for_listing(listing_id, &buyer_wallet)
        .await?
        .ok_or(AppError::NotFound)?;

    let row = state.db.get_listing_any(listing_id).await?;

    tracing::info!(
        listing_id = %listing_id,
        buyer = %buyer_wallet,
        sale_id = %sale.id,
        byte_size = row.byte_size,
        "wallet redownload authorized"
    );

    build_asset_download_response(&state, &row, Some(&sale), None).await
}
