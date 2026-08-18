use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::routes::listings::build_asset_download_response;
use crate::state::SharedState;
use crate::storage::{DeliveryFormat, ObjectStore};

const ORACLE_REPLAY_WINDOW_SECS: i64 = 60;

pub async fn artifact(
    State(state): State<SharedState>,
    Path(listing_id): Path<Uuid>,
    headers: HeaderMap,
) -> AppResult<Response> {
    if listing_oracle_headers_present(&headers) {
        // Verdict door only; never accept PAYMENT-SIGNATURE here.
        if headers.contains_key("payment-signature") {
            return Err(AppError::Forbidden(
                "oracle verdict door does not accept payment signatures".into(),
            ));
        }
    } else {
        return Err(AppError::Forbidden("oracle verdict signature required".into()));
    }

    let payment_uid = header_value(&headers, "x-forge-payment-uid")?;
    let ts_raw = header_value(&headers, "x-forge-oracle-ts")?;
    let sig_b58 = header_value(&headers, "x-forge-oracle-sig")?;

    if payment_uid.len() != 64 || !payment_uid.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(AppError::Forbidden("invalid payment uid".into()));
    }
    let payment_uid = payment_uid.to_ascii_lowercase();

    let ts: i64 = ts_raw
        .parse()
        .map_err(|_| AppError::Forbidden("invalid oracle timestamp".into()))?;
    let now = chrono::Utc::now().timestamp();
    if (now - ts).abs() > ORACLE_REPLAY_WINDOW_SECS {
        return Err(AppError::Forbidden("oracle signature expired".into()));
    }

    let row = state.db.get_listing_any(listing_id).await?;
    if row.delivery_scheme != "escrow" {
        return Err(AppError::Forbidden(
            "verdict door is not available for exact-rail listings".into(),
        ));
    }

    let bind = state
        .db
        .get_escrow_fund_bind(listing_id, &payment_uid)
        .await?
        .ok_or_else(|| AppError::Forbidden("escrow fund bind not found".into()))?;

    if bind.content_hash != row.content_hash.clone().unwrap_or_default() {
        return Err(AppError::Forbidden("content hash bind mismatch".into()));
    }

    if !state
        .config
        .oracle_authorities
        .iter()
        .any(|a| a == &bind.oracle_authority)
    {
        return Err(AppError::Forbidden(
            "oracle authority is not configured on this host".into(),
        ));
    }

    let host = public_host(&state.config.seller_public_base_url)?;
    let message = format!(
        "forge-oracle-v1|{listing_id}|{payment_uid}|{ts}|{host}"
    );
    verify_oracle_signature(&bind.oracle_authority, message.as_bytes(), &sig_b58)?;

    state
        .storage
        .head(&bind.content_hash)
        .await
        .map_err(|_| AppError::NotFound)?;

    build_asset_download_response(
        &state,
        &row,
        None,
        None,
        DeliveryFormat::Proxy,
    )
    .await
}

fn listing_oracle_headers_present(headers: &HeaderMap) -> bool {
    headers.contains_key("x-forge-payment-uid")
        && headers.contains_key("x-forge-oracle-ts")
        && headers.contains_key("x-forge-oracle-sig")
}

fn header_value(headers: &HeaderMap, name: &str) -> AppResult<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| AppError::Forbidden(format!("missing {name} header")))
}

pub fn public_host(base_url: &str) -> AppResult<String> {
    let trimmed = base_url.trim().trim_end_matches('/');
    let without_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    Ok(without_scheme
        .split('/')
        .next()
        .unwrap_or(without_scheme)
        .split(':')
        .next()
        .unwrap_or(without_scheme)
        .to_string())
}

pub fn forge_oracle_message(
    listing_id: Uuid,
    payment_uid_hex: &str,
    ts: i64,
    host: &str,
) -> String {
    format!("forge-oracle-v1|{listing_id}|{payment_uid_hex}|{ts}|{host}")
}

pub fn verify_oracle_signature(
    authority_b58: &str,
    message: &[u8],
    sig_b58: &str,
) -> AppResult<()> {
    let pk_bytes = bs58::decode(authority_b58)
        .into_vec()
        .map_err(|_| AppError::Forbidden("invalid oracle authority".into()))?;
    if pk_bytes.len() != 32 {
        return Err(AppError::Forbidden("invalid oracle authority".into()));
    }
    let key_bytes: [u8; 32] = pk_bytes
        .try_into()
        .map_err(|_| AppError::Forbidden("invalid oracle authority".into()))?;
    let verifying_key = VerifyingKey::from_bytes(&key_bytes)
        .map_err(|_| AppError::Forbidden("invalid oracle authority".into()))?;
    let sig_bytes = bs58::decode(sig_b58)
        .into_vec()
        .map_err(|_| AppError::Forbidden("invalid oracle signature".into()))?;
    let sig_array: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| AppError::Forbidden("invalid oracle signature".into()))?;
    let signature = Signature::from_bytes(&sig_array);
    verifying_key
        .verify(message, &signature)
        .map_err(|_| AppError::Forbidden("invalid oracle signature".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Signer;
    use ed25519_dalek::SigningKey;

    #[test]
    fn public_host_strips_scheme_and_path() {
        assert_eq!(
            public_host("https://preview.forge.http402.trade/api").unwrap(),
            "preview.forge.http402.trade"
        );
    }

    #[test]
    fn verify_oracle_signature_accepts_valid_message() {
        let seed = [7u8; 32];
        let signing_key = SigningKey::from_bytes(&seed);
        let authority = bs58::encode(signing_key.verifying_key().as_bytes()).into_string();
        let message = forge_oracle_message(
            Uuid::new_v4(),
            &"ab".repeat(32),
            1_700_000_000,
            "preview.forge.http402.trade",
        );
        let sig = bs58::encode(signing_key.sign(message.as_bytes()).to_bytes()).into_string();
        verify_oracle_signature(&authority, message.as_bytes(), &sig).expect("valid sig");
    }

    #[test]
    fn verify_oracle_signature_rejects_bad_sig() {
        let seed = [9u8; 32];
        let signing_key = SigningKey::from_bytes(&seed);
        let authority = bs58::encode(signing_key.verifying_key().as_bytes()).into_string();
        let message = b"forge-oracle-v1|test";
        let err = verify_oracle_signature(&authority, message, "111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111").unwrap_err();
        assert!(matches!(err, AppError::Forbidden(_)));
    }
}
