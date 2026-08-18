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
            }));
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

        if listing.delivery_scheme == "escrow" {
            persist_escrow_fund_bind(state, listing, &settle, &proof).await?;
            return Err(AppError::PaymentRequired(payment_required_with_error(
                &pr,
                "escrow funded; download unlocks after oracle release",
            )?));
        }

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

pub(crate) fn extract_escrow_fund_fields(
    settle: &Value,
    proof: &Value,
) -> Result<(String, String), String> {
    let payment_uid = first_str(
        &[settle, proof],
        &[
            "/paymentUid",
            "/payment_uid",
            "/paymentPayload/payload/paymentUid",
            "/paymentPayload/payload/payment_uid",
            "/extra/paymentUid",
            "/paymentRequirements/extra/paymentUid",
        ],
    )
    .and_then(|raw| normalize_payment_uid(&raw))
    .ok_or_else(|| "missing payment_uid".to_string())?;
    let oracle_authority = first_str(
        &[settle, proof],
        &[
            "/oracleAuthority",
            "/oracle_authority",
            "/paymentPayload/payload/oracleAuthority",
            "/paymentPayload/payload/oracle_authority",
            "/extra/oracleAuthority",
            "/paymentRequirements/extra/oracleAuthority",
        ],
    )
    .ok_or_else(|| "missing oracle_authority".to_string())?;
    Ok((payment_uid, oracle_authority))
}

fn first_str(values: &[&Value], pointers: &[&str]) -> Option<String> {
    for value in values {
        for pointer in pointers {
            if let Some(s) = value
                .pointer(pointer)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn normalize_payment_uid(raw: &str) -> Option<String> {
    let hex = raw.trim().strip_prefix("0x").unwrap_or(raw.trim());
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(hex.to_ascii_lowercase())
}

async fn persist_escrow_fund_bind(
    state: &AppState,
    listing: &ListingRow,
    settle: &Value,
    proof: &Value,
) -> AppResult<crate::db::EscrowFundBind> {
    let (payment_uid, oracle_authority) =
        extract_escrow_fund_fields(settle, proof).map_err(|e| {
            AppError::PaymentRequired(json!({ "error": format!("payment verification failed: {e}") }))
        })?;
    if !state
        .config
        .oracle_authorities
        .iter()
        .any(|a| a == &oracle_authority)
    {
        return Err(AppError::PaymentRequired(
            json!({ "error": "payment verification failed: oracle_authority is not listed" }),
        ));
    }
    let content_hash = listing.content_hash.clone().ok_or_else(|| {
        AppError::PaymentRequired(json!({ "error": "payment verification failed: listing has no content_hash" }))
    })?;
    state
        .db
        .upsert_escrow_fund_bind(listing.id, &payment_uid, &content_hash, &oracle_authority)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, ListingRow};
    use chrono::Utc;
    use uuid::Uuid;

    fn escrow_listing(id: Uuid, hash: &str) -> ListingRow {
        ListingRow {
            id,
            seller_wallet: "buyA5hR1Z9KtHQRBTmLkjsFfjAabDwdZtrRC6edqxAJ".into(),
            display_name: None,
            title: "escrow".into(),
            description: String::new(),
            category: "art".into(),
            price_micro_usdc: 50_000,
            preview_key: "p".into(),
            preview_content_type: "text/plain".into(),
            asset_key: format!("assets/{hash}"),
            content_type: "application/octet-stream".into(),
            byte_size: 4,
            agent_friendly: false,
            delivery_scheme: "escrow".into(),
            status: "active".into(),
            tags: "[]".into(),
            license: None,
            content_hash: Some(hash.into()),
            moderation_status: "approved".into(),
            moderation_labels: "[]".into(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn extract_escrow_fund_fields_from_settle_json() {
        let uid = "ab".repeat(32);
        let settle = json!({
            "paymentUid": uid,
            "oracleAuthority": "Oracle11111111111111111111111111111111111"
        });
        let proof = json!({});
        let (got_uid, got_auth) = extract_escrow_fund_fields(&settle, &proof).unwrap();
        assert_eq!(got_uid, uid);
        assert_eq!(got_auth, "Oracle11111111111111111111111111111111111");
    }

    #[tokio::test]
    async fn sla_escrow_fund_persists_bind_from_on_chain_fields() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("forge.db");
        let db = Database::connect(&format!("sqlite:{}", db_path.display()))
            .await
            .unwrap();
        let listing_id = Uuid::new_v4();
        let hash = "cd".repeat(32);
        db.insert_listing(&escrow_listing(listing_id, &hash))
            .await
            .unwrap();
        let uid = "ef".repeat(32);
        let authority = "OracleAuthority11111111111111111111111111";
        let settle = json!({
            "paymentUid": uid,
            "oracleAuthority": authority
        });
        let (got_uid, got_auth) = extract_escrow_fund_fields(&settle, &json!({})).unwrap();
        let bind = db
            .upsert_escrow_fund_bind(listing_id, &got_uid, &hash, &got_auth)
            .await
            .unwrap();
        assert_eq!(bind.listing_id, listing_id);
        assert_eq!(bind.payment_uid, uid);
        assert_eq!(bind.content_hash, hash);
        assert_eq!(bind.oracle_authority, authority);
        assert_eq!(db.count_sales().await.unwrap(), 0);
    }
}
