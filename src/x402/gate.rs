use crate::db::ListingRow;
use crate::x402::accepts::{build_accepts_for_listing, idempotency_key, listing_uses_escrow};
use crate::x402::wire::{
    encode_payment_response, extract_payment_header_value, parse_payment_header,
    payment_required_json, PaymentRequired, ResourceInfo,
};
use axum::http::HeaderMap;
use axum::response::Response;
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Debug, Clone)]
pub struct PaymentContext {
    pub payer_wallet: String,
    pub payment_signature: String,
    pub settle_proof: Value,
    pub already_paid: bool,
}

pub struct PaymentGate;

impl PaymentGate {
    pub async fn check_download(
        state: &AppState,
        headers: &HeaderMap,
        listing: &ListingRow,
        canonical_path: &str,
    ) -> AppResult<PaymentContext> {
        let use_escrow = listing_uses_escrow(
            &listing.delivery_scheme,
            listing.byte_size,
            state.config.escrow_size_threshold,
        );

        if use_escrow && state.config.oracle_authorities.is_empty() {
            return Err(AppError::PaymentConfig(
                "Escrow listings require ORACLE_AUTHORITIES on the API host".into(),
            ));
        }

        let pay_to = if use_escrow {
            listing.seller_wallet.clone()
        } else {
            state
                .facilitator
                .resolve_vault_pda(&listing.seller_wallet)
                .await
                .map_err(|e| {
                    AppError::PaymentConfig(format!(
                        "seller {} has no pr402 vault: {e}",
                        listing.seller_wallet
                    ))
                })?
        };

        let mut accepts = build_accepts_for_listing(
            &state.cluster,
            &pay_to,
            listing.price_micro_usdc,
            state.config.payment_timeout_secs,
            use_escrow,
            state.config.platform_fee_bps,
            state.config.platform_fee_wallet.as_deref(),
        );

        accepts = state
            .facilitator
            .enrich_accepts(
                accepts,
                &listing.seller_wallet,
                &state.cluster.network,
                use_escrow,
                &state.config.oracle_authorities,
                &state.config.oracle_profile_id,
            )
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!("enrich accepts: {e}")))?;

        let description = format!("Download: {}", listing.title);
        let pr = PaymentRequired {
            x402_version: 2,
            error: None,
            resource: ResourceInfo {
                url: format!(
                    "{}{}",
                    state.config.seller_public_base_url.trim_end_matches('/'),
                    canonical_path
                ),
                description: description.clone(),
                mime_type: listing.content_type.clone(),
            },
            accepts,
            extensions: json!({
                "pr402FacilitatorUrl": state.config.facilitator_base_url,
                "forge": {
                    "listingId": listing.id,
                    "deliveryScheme": if use_escrow { "escrow" } else { "exact" },
                },
            }),
        };

        let raw = extract_payment_header_value(|name| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        });

        let Some(raw) = raw else {
            let body = payment_required_json(&pr)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("402 json: {e}")))?;
            return Err(AppError::PaymentRequired(body));
        };

        let proof = match parse_payment_header(&raw) {
            Ok(p) => p,
            Err(e) => {
                return Err(AppError::PaymentRequired(payment_required_with_error(
                    &pr,
                    &e.to_string(),
                )?));
            }
        };

        let sig = proof
            .get("paymentPayload")
            .and_then(|p| p.pointer("/payload/transaction"))
            .and_then(|v| v.as_str())
            .unwrap_or(&raw)
            .to_string();

        let idem = idempotency_key(&sig, canonical_path);
        if let Some(existing) = state.db.find_by_idempotency(&idem).await? {
            return Ok(PaymentContext {
                payer_wallet: existing.buyer_wallet,
                payment_signature: existing.tx_signature,
                settle_proof: json!({}),
                already_paid: true,
            });
        }

        let settle = state
            .facilitator
            .verify_and_settle(&proof)
            .await
            .map_err(|e| {
                AppError::PaymentRequired(
                    payment_required_with_error(&pr, &format!("payment verification failed: {e}"))
                        .unwrap_or(json!({ "error": "payment failed" })),
                )
            })?;

        let payer = settle
            .get("payer")
            .and_then(|v| v.as_str())
            .unwrap_or("anonymous")
            .to_string();

        let tx_sig = settle
            .get("transaction")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        state
            .db
            .insert_payment(&idem, listing.id, &payer, &tx_sig)
            .await?;

        Ok(PaymentContext {
            payer_wallet: payer,
            payment_signature: sig,
            settle_proof: settle,
            already_paid: false,
        })
    }

    pub fn attach_payment_response(mut response: Response, settle: &Value) -> Response {
        let encoded = encode_payment_response(settle);
        if let Ok(header) = encoded.parse() {
            response.headers_mut().insert("PAYMENT-RESPONSE", header);
        }
        response
    }
}

fn payment_required_with_error(pr: &PaymentRequired, msg: &str) -> AppResult<Value> {
    let mut copy = pr.clone();
    copy.error = Some(msg.to_string());
    payment_required_json(&copy).map_err(|e| AppError::Internal(anyhow::anyhow!("402: {e}")))
}
