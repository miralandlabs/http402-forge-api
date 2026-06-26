use axum::{
    extract::{Query, State},
    http::header,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::models::validate_wallet;
use crate::state::SharedState;
use crate::x402::vault_activated_from_preview;

#[derive(Debug, Deserialize)]
pub struct SellerWalletQuery {
    pub seller_wallet: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChallengeResponse {
    pub message: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SellerStatusResponse {
    pub vault_activated: bool,
    pub can_sell: bool,
    pub vault_pda: Option<String>,
    pub fee_bps: Option<u16>,
    pub protocol_fee_percent: Option<String>,
    pub seller_dashboard_url: String,
    pub vault_check_enforced: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionRequest {
    pub seller_wallet: String,
    #[serde(default = "default_provision_asset")]
    pub asset: String,
}

fn default_provision_asset() -> String {
    "USDC".into()
}

pub async fn challenge(
    State(state): State<SharedState>,
    Query(q): Query<SellerWalletQuery>,
) -> AppResult<impl axum::response::IntoResponse> {
    let (message, expires_at) = state.seller_auth.issue_challenge(&q.seller_wallet)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(ChallengeResponse {
            message,
            expires_at,
        }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct DelistChallengeQuery {
    pub seller_wallet: String,
    pub listing_id: String,
}

pub async fn delist_challenge(
    State(state): State<SharedState>,
    Query(q): Query<DelistChallengeQuery>,
) -> AppResult<impl axum::response::IntoResponse> {
    validate_wallet(&q.seller_wallet).map_err(|m| AppError::validation("seller_wallet", m))?;
    let listing_id = Uuid::parse_str(q.listing_id.trim())
        .map_err(|_| AppError::validation("listing_id", "invalid uuid"))?;
    let row = state.db.get_listing(listing_id).await?;
    if row.seller_wallet != q.seller_wallet {
        return Err(AppError::Forbidden("not listing owner".into()));
    }
    let (message, expires_at) = state
        .seller_auth
        .issue_delist_challenge(&q.seller_wallet, listing_id)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(ChallengeResponse {
            message,
            expires_at,
        }),
    ))
}

pub async fn status(
    State(state): State<SharedState>,
    Query(q): Query<SellerWalletQuery>,
) -> AppResult<impl axum::response::IntoResponse> {
    validate_wallet(&q.seller_wallet).map_err(|m| AppError::validation("seller_wallet", m))?;

    let vault_check_enforced = !state.config.skip_seller_vault_check;
    if !vault_check_enforced {
        return Ok((
            [(header::CACHE_CONTROL, "no-store")],
            Json(SellerStatusResponse {
                vault_activated: false,
                can_sell: true,
                vault_pda: None,
                fee_bps: None,
                protocol_fee_percent: None,
                seller_dashboard_url: state.facilitator.seller_dashboard_url(),
                vault_check_enforced: false,
            }),
        ));
    }

    let preview = state
        .facilitator
        .fetch_seller_preview(&q.seller_wallet)
        .await
        .map_err(|e| AppError::PaymentConfig(format!("seller status: {e}")))?;

    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(parse_seller_status(
            &preview,
            state.facilitator.seller_dashboard_url(),
            vault_check_enforced,
        )),
    ))
}

pub async fn provision_tx(
    State(state): State<SharedState>,
    Json(body): Json<ProvisionRequest>,
) -> AppResult<Json<Value>> {
    validate_wallet(&body.seller_wallet).map_err(|m| AppError::validation("seller_wallet", m))?;
    let asset = body.asset.trim();
    if asset.is_empty() {
        return Err(AppError::validation("asset", "required (e.g. USDC)"));
    }

    let response = state
        .facilitator
        .build_provision_tx(&body.seller_wallet, asset)
        .await
        .map_err(|e| AppError::PaymentConfig(format!("provision-tx: {e}")))?;

    Ok(Json(response))
}

fn parse_seller_status(
    preview: &Value,
    seller_dashboard_url: String,
    vault_check_enforced: bool,
) -> SellerStatusResponse {
    let exact = preview
        .pointer("/schemes/exact")
        .or_else(|| preview.pointer("/schemes/\"exact\""));

    let vault_pda = exact
        .and_then(|e| e.get("vaultPda"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let fee_bps = exact
        .and_then(|e| e.get("feeBps"))
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
        .map(|n| n as u16);

    let protocol_fee_percent = fee_bps.map(|bps| format!("{:.2}", bps as f64 / 100.0));

    let vault_activated = vault_activated_from_preview(preview);

    SellerStatusResponse {
        vault_activated,
        can_sell: !vault_check_enforced || vault_activated,
        vault_pda,
        fee_bps,
        protocol_fee_percent,
        seller_dashboard_url,
        vault_check_enforced,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_preview_activated() {
        let preview = json!({
            "lifecycle": { "activated": true },
            "schemes": {
                "exact": { "vaultPda": "Vault111", "feeBps": 90 }
            }
        });
        let status = parse_seller_status(&preview, "https://preview.ipay.sh".into(), true);
        assert!(status.vault_activated);
        assert!(status.can_sell);
        assert_eq!(status.fee_bps, Some(90));
    }

    #[test]
    fn parse_preview_not_activated() {
        let preview = json!({
            "lifecycle": { "activated": false },
            "schemes": {}
        });
        let status = parse_seller_status(&preview, "https://preview.ipay.sh".into(), true);
        assert!(!status.vault_activated);
        assert!(!status.can_sell);
    }

    #[test]
    fn parse_preview_derived_pda_is_not_activated() {
        let preview = json!({
            "lifecycle": { "previewed": true, "activated": false },
            "schemes": {
                "exact": {
                    "vaultPda": "9AKmHTQNd1jakQ9XhNtoCMXtc8RCBfATVqNyA8qy64yd",
                    "status": "NotProvisioned",
                    "feeBps": "100"
                }
            }
        });
        let status = parse_seller_status(&preview, "https://preview.ipay.sh".into(), true);
        assert!(!status.vault_activated);
        assert!(!status.can_sell);
        assert_eq!(
            status.vault_pda.as_deref(),
            Some("9AKmHTQNd1jakQ9XhNtoCMXtc8RCBfATVqNyA8qy64yd")
        );
    }
}
