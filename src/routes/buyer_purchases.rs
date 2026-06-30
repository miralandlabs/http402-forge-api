use axum::{
    extract::{Query, State},
    http::{header, HeaderMap},
    Json,
};
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::db::BuyerPurchaseRow;
use crate::error::{AppError, AppResult};
use crate::models::validate_wallet;
use crate::state::SharedState;

#[derive(Debug, Deserialize)]
pub struct PurchasesQuery {
    pub buyer_wallet: String,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    20
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PurchasesResponse {
    pub items: Vec<BuyerPurchaseRow>,
    pub total: i64,
}

#[derive(Debug, Deserialize)]
pub struct PurchasesChallengeQuery {
    pub buyer_wallet: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChallengeResponse {
    pub message: String,
    pub expires_at: DateTime<Utc>,
}

pub async fn purchases_challenge(
    State(state): State<SharedState>,
    Query(q): Query<PurchasesChallengeQuery>,
) -> AppResult<impl axum::response::IntoResponse> {
    validate_wallet(&q.buyer_wallet).map_err(|m| AppError::validation("buyer_wallet", m))?;
    let (message, expires_at) = state
        .seller_auth
        .issue_purchase_history_challenge(&q.buyer_wallet)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(ChallengeResponse {
            message,
            expires_at,
        }),
    ))
}

fn buyer_auth_header(headers: &HeaderMap, name: &str) -> AppResult<String> {
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

pub async fn list_purchases(
    State(state): State<SharedState>,
    Query(q): Query<PurchasesQuery>,
    headers: HeaderMap,
) -> AppResult<impl axum::response::IntoResponse> {
    validate_wallet(&q.buyer_wallet).map_err(|m| AppError::validation("buyer_wallet", m))?;
    if q.limit < 1 || q.limit > 100 {
        return Err(AppError::BadRequest("limit must be 1-100".into()));
    }
    if q.offset < 0 {
        return Err(AppError::BadRequest("offset must be >= 0".into()));
    }

    if !state.config.skip_buyer_auth {
        let header_wallet = buyer_auth_header(&headers, "x-forge-buyer-wallet")?;
        if header_wallet != q.buyer_wallet {
            return Err(AppError::Forbidden("buyer wallet header mismatch".into()));
        }
        let buyer_challenge = buyer_auth_header(&headers, "x-forge-buyer-challenge")?;
        let buyer_signature = headers
            .get("x-forge-buyer-signature")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AppError::validation("buyer_signature", "required"))?;
        state
            .seller_auth
            .verify_and_consume(&q.buyer_wallet, &buyer_challenge, buyer_signature)?;
    }

    let total = state.db.count_buyer_purchases(&q.buyer_wallet).await?;
    let items = state
        .db
        .list_buyer_purchases(&q.buyer_wallet, q.limit, q.offset)
        .await?;

    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(PurchasesResponse { items, total }),
    ))
}
