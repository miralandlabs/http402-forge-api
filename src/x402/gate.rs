use crate::db::ListingRow;
use crate::x402::accepts::{build_accepts_for_listing, idempotency_key, listing_uses_escrow};
use crate::x402::escrow_fund::extract_escrow_fund_details;
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
    pub escrow_funded: bool,
}

pub struct PaymentGate;

impl PaymentGate {
    /// Returns `Ok(None)` when the listing is free (`price_micro_usdc == 0`):
    /// no vault resolve, 402, verify, or settle — same pattern as
    /// spl-token-balance-serverless `payment_skipped`.
    pub async fn check_download(
        state: &AppState,
        headers: &HeaderMap,
        listing: &ListingRow,
        canonical_path: &str,
    ) -> AppResult<Option<PaymentContext>> {
        if listing.price_micro_usdc == 0 {
            return Ok(None);
        }

        reject_oracle_verdict_headers(headers)?;

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
            return Ok(Some(PaymentContext {
                payer_wallet: existing.buyer_wallet,
                payment_signature: existing.tx_signature,
                settle_proof: json!({}),
                already_paid: true,
                escrow_funded: false,
            }));
        }

        if use_escrow {
            return Self::check_escrow_fund(state, listing, &proof, &pr, &sig, &raw).await;
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
            .record_payment_and_sale(
                &idem,
                listing.id,
                &listing.seller_wallet,
                &payer,
                listing.price_micro_usdc,
                &tx_sig,
            )
            .await?;

        Ok(Some(PaymentContext {
            payer_wallet: payer,
            payment_signature: sig,
            settle_proof: settle,
            already_paid: false,
            escrow_funded: false,
        }))
    }

    async fn check_escrow_fund(
        state: &AppState,
        listing: &ListingRow,
        proof: &Value,
        pr: &PaymentRequired,
        sig: &str,
        raw: &str,
    ) -> AppResult<Option<PaymentContext>> {
        let _ = raw;
        let fund = state
            .facilitator
            .verify_and_fund(proof)
            .await
            .map_err(|e| {
                AppError::PaymentRequired(
                    payment_required_with_error(pr, &format!("escrow fund failed: {e}"))
                        .unwrap_or(json!({ "error": "payment failed" })),
                )
            })?;

        let details = extract_escrow_fund_details(&fund, proof).ok_or_else(|| {
            AppError::PaymentRequired(
                payment_required_with_error(pr, "escrow fund missing paymentUid or oracleAuthority")
                    .unwrap_or(json!({ "error": "payment failed" })),
            )
        })?;

        if !state
            .config
            .oracle_authorities
            .iter()
            .any(|a| a == &details.oracle_authority)
        {
            return Err(AppError::PaymentRequired(
                payment_required_with_error(pr, "oracle authority is not allowed for this host")
                    .unwrap_or(json!({ "error": "payment failed" })),
            ));
        }

        let content_hash = listing.content_hash.clone().ok_or_else(|| {
            AppError::PaymentRequired(
                payment_required_with_error(pr, "listing has no content hash")
                    .unwrap_or(json!({ "error": "payment failed" })),
            )
        })?;

        state
            .db
            .insert_escrow_fund_bind(
                listing.id,
                &details.payment_uid,
                &content_hash,
                &details.oracle_authority,
            )
            .await?;

        Ok(Some(PaymentContext {
            payer_wallet: details.payer_wallet,
            payment_signature: sig.to_string(),
            settle_proof: fund,
            already_paid: false,
            escrow_funded: true,
        }))
    }

    pub async fn check_download_active_or_paid(
        state: &AppState,
        headers: &HeaderMap,
        listing: &ListingRow,
        canonical_path: &str,
    ) -> AppResult<Option<PaymentContext>> {
        if listing.status == "active" {
            return Self::check_download(state, headers, listing, canonical_path).await;
        }
        // Free listings never record a sale; delisted free assets are not recoverable via payment proof.
        if listing.price_micro_usdc == 0 {
            return Err(AppError::NotFound);
        }
        Ok(Some(
            Self::check_delisted_redownload(state, headers, canonical_path).await?,
        ))
    }

    async fn check_delisted_redownload(
        state: &AppState,
        headers: &HeaderMap,
        canonical_path: &str,
    ) -> AppResult<PaymentContext> {
        reject_oracle_verdict_headers(headers)?;
        let raw = extract_payment_header_value(|name| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        });
        let Some(raw) = raw else {
            return Err(AppError::NotFound);
        };
        let proof = parse_payment_header(&raw).map_err(|_| AppError::NotFound)?;
        let sig = proof
            .get("paymentPayload")
            .and_then(|p| p.pointer("/payload/transaction"))
            .and_then(|v| v.as_str())
            .unwrap_or(&raw)
            .to_string();
        let idem = idempotency_key(&sig, canonical_path);
        let Some(existing) = state.db.find_by_idempotency(&idem).await? else {
            return Err(AppError::NotFound);
        };
        Ok(PaymentContext {
            payer_wallet: existing.buyer_wallet,
            payment_signature: existing.tx_signature,
            settle_proof: json!({}),
            already_paid: true,
            escrow_funded: false,
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

fn reject_oracle_verdict_headers(headers: &HeaderMap) -> AppResult<()> {
    if headers.contains_key("x-forge-oracle-sig")
        || headers.contains_key("x-forge-oracle-ts")
        || headers.contains_key("x-forge-payment-uid")
    {
        return Err(AppError::Forbidden(
            "payment door does not accept oracle verdict signatures".into(),
        ));
    }
    Ok(())
}

fn payment_required_with_error(pr: &PaymentRequired, msg: &str) -> AppResult<Value> {
    let mut copy = pr.clone();
    copy.error = Some(msg.to_string());
    payment_required_json(&copy).map_err(|e| AppError::Internal(anyhow::anyhow!("402: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn payment_door_rejects_oracle_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forge-oracle-sig", HeaderValue::from_static("sig"));
        let err = reject_oracle_verdict_headers(&headers).unwrap_err();
        assert!(matches!(err, AppError::Forbidden(_)));
    }
}
