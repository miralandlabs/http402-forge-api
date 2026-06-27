use axum::{
    extract::{Path, Query, State},
    http::header,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::parse_feedback_sale_id;
use crate::db::validate_feedback_outcome;
use crate::error::{AppError, AppResult};
use crate::models::validate_wallet;
use crate::state::SharedState;

#[derive(Debug, Deserialize)]
pub struct FeedbackChallengeQuery {
    pub buyer_wallet: String,
    pub sale_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChallengeResponse {
    pub message: String,
    pub expires_at: DateTime<Utc>,
}

pub async fn feedback_challenge(
    State(state): State<SharedState>,
    Query(q): Query<FeedbackChallengeQuery>,
) -> AppResult<impl axum::response::IntoResponse> {
    validate_wallet(&q.buyer_wallet).map_err(|m| AppError::validation("buyer_wallet", m))?;
    let sale_id = Uuid::parse_str(q.sale_id.trim())
        .map_err(|_| AppError::validation("sale_id", "invalid uuid"))?;
    let sale = state.db.get_sale(sale_id).await?;
    if sale.buyer_wallet != q.buyer_wallet {
        return Err(AppError::Forbidden("not buyer on this sale".into()));
    }
    let (message, expires_at) = state
        .seller_auth
        .issue_feedback_challenge(&q.buyer_wallet, sale_id)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(ChallengeResponse {
            message,
            expires_at,
        }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct SaleFeedbackRequest {
    pub buyer_wallet: String,
    pub buyer_challenge: String,
    pub buyer_signature: String,
    pub outcome: String,
    #[serde(default)]
    pub score: Option<i16>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaleFeedbackResponse {
    pub sale_id: Uuid,
    pub listing_id: Uuid,
    pub outcome: String,
    pub score: Option<i16>,
    pub created_at: DateTime<Utc>,
}

pub async fn submit_feedback(
    State(state): State<SharedState>,
    Path(sale_id): Path<Uuid>,
    Json(body): Json<SaleFeedbackRequest>,
) -> AppResult<Json<SaleFeedbackResponse>> {
    validate_wallet(&body.buyer_wallet).map_err(|m| AppError::validation("buyer_wallet", m))?;
    validate_feedback_outcome(body.outcome.trim())
        .map_err(|m| AppError::validation("outcome", m))?;

    if let Some(score) = body.score {
        if !(1..=5).contains(&score) {
            return Err(AppError::validation("score", "must be between 1 and 5"));
        }
    }

    let note = body
        .note
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if let Some(ref n) = note {
        if n.len() > 500 {
            return Err(AppError::validation("note", "max 500 chars"));
        }
    }

    if !state.config.skip_buyer_auth {
        state.seller_auth.verify_and_consume(
            &body.buyer_wallet,
            &body.buyer_challenge,
            &body.buyer_signature,
        )?;
        let challenge_sale = parse_feedback_sale_id(&body.buyer_challenge)
            .ok_or_else(|| AppError::Forbidden("invalid feedback challenge".into()))?;
        if challenge_sale != sale_id {
            return Err(AppError::Forbidden(
                "feedback challenge sale mismatch".into(),
            ));
        }
    }

    let sale = state.db.get_sale(sale_id).await?;
    if sale.buyer_wallet != body.buyer_wallet {
        return Err(AppError::Forbidden("not buyer on this sale".into()));
    }

    let row = state
        .db
        .insert_sale_feedback(
            sale_id,
            sale.listing_id,
            &body.buyer_wallet,
            body.outcome.trim(),
            body.score,
            note.as_deref(),
        )
        .await?;

    Ok(Json(SaleFeedbackResponse {
        sale_id: row.sale_id,
        listing_id: row.listing_id,
        outcome: row.outcome,
        score: row.score,
        created_at: row.created_at,
    }))
}
