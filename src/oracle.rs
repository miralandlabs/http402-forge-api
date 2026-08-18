use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

pub const ORACLE_MESSAGE_PREFIX: &str = "forge-oracle-v1";
pub const REPLAY_WINDOW_SECS: i64 = 60;

pub const HEADER_PAYMENT_UID: &str = "x-forge-payment-uid";
pub const HEADER_ORACLE_TS: &str = "x-forge-oracle-ts";
pub const HEADER_ORACLE_SIG: &str = "x-forge-oracle-sig";

pub fn has_oracle_artifact_headers(headers: &axum::http::HeaderMap) -> bool {
    headers.contains_key(HEADER_PAYMENT_UID)
        || headers.contains_key(HEADER_ORACLE_TS)
        || headers.contains_key(HEADER_ORACLE_SIG)
}

pub fn forge_public_host(seller_public_base_url: &str) -> String {
    let rest = seller_public_base_url
        .strip_prefix("https://")
        .or_else(|| seller_public_base_url.strip_prefix("http://"))
        .unwrap_or(seller_public_base_url);
    rest.split('/')
        .next()
        .unwrap_or(rest)
        .trim()
        .trim_end_matches('/')
        .to_string()
}

pub fn verdict_message(listing_id: Uuid, payment_uid_hex: &str, ts: i64, host: &str) -> String {
    format!("{ORACLE_MESSAGE_PREFIX}|{listing_id}|{payment_uid_hex}|{ts}|{host}")
}

pub fn normalize_payment_uid(raw: &str) -> AppResult<String> {
    let trimmed = raw.trim().strip_prefix("0x").unwrap_or(raw.trim());
    let lower = trimmed.to_ascii_lowercase();
    if lower.len() != 64 || !lower.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(AppError::Forbidden("oracle verdict denied".into()));
    }
    Ok(lower)
}

pub fn parse_oracle_ts(raw: &str) -> AppResult<i64> {
    raw.trim()
        .parse::<i64>()
        .map_err(|_| AppError::Forbidden("oracle verdict denied".into()))
}

pub fn ts_in_replay_window(ts: i64, now: i64) -> bool {
    ts.abs_diff(now) <= REPLAY_WINDOW_SECS as u64
}

pub fn verify_oracle_signature(
    oracle_authority: &str,
    message: &str,
    signature_b58: &str,
) -> AppResult<()> {
    let pubkey_bytes = bs58::decode(oracle_authority.trim())
        .into_vec()
        .map_err(|_| AppError::Forbidden("oracle verdict denied".into()))?;
    let pubkey_array: [u8; 32] = pubkey_bytes
        .try_into()
        .map_err(|_| AppError::Forbidden("oracle verdict denied".into()))?;
    let verifying_key = VerifyingKey::from_bytes(&pubkey_array)
        .map_err(|_| AppError::Forbidden("oracle verdict denied".into()))?;

    let sig_bytes = bs58::decode(signature_b58.trim())
        .into_vec()
        .map_err(|_| AppError::Forbidden("oracle verdict denied".into()))?;
    let sig_array: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| AppError::Forbidden("oracle verdict denied".into()))?;
    let signature = Signature::from_bytes(&sig_array);

    verifying_key
        .verify(message.as_bytes(), &signature)
        .map_err(|_| AppError::Forbidden("oracle verdict denied".into()))
}

/// Pull payment_uid + oracle_authority from a facilitator verify result and/or the PAYMENT-SIGNATURE proof.
pub fn extract_escrow_fund_fields(proof: &serde_json::Value, verify: &serde_json::Value) -> Option<(String, String)> {
    let uid_raw = first_str(
        verify,
        &[
            "/paymentUid",
            "/payment_uid",
            "/extra/paymentUid",
            "/extra/payment_uid",
        ],
    )
    .or_else(|| {
        first_str(
            proof,
            &[
                "/paymentUid",
                "/payment_uid",
                "/paymentPayload/payload/paymentUid",
                "/paymentRequirements/extra/paymentUid",
                "/accepted/extra/paymentUid",
            ],
        )
    })?;
    let oracle = first_str(
        verify,
        &[
            "/oracleAuthority",
            "/oracle_authority",
            "/extra/oracleAuthority",
            "/extra/oracle_authority",
        ],
    )
    .or_else(|| {
        first_str(
            proof,
            &[
                "/oracleAuthority",
                "/oracle_authority",
                "/paymentPayload/payload/oracleAuthority",
                "/paymentRequirements/extra/oracleAuthority",
                "/accepted/extra/oracleAuthority",
            ],
        )
    })?;
    let uid = normalize_payment_uid(&uid_raw).ok()?;
    Some((uid, oracle))
}

fn first_str(v: &serde_json::Value, pointers: &[&str]) -> Option<String> {
    for pointer in pointers {
        if let Some(s) = v
            .pointer(pointer)
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(s.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Signer;

    #[test]
    fn forge_public_host_strips_scheme_and_path() {
        assert_eq!(
            forge_public_host("https://preview.forge.http402.trade/"),
            "preview.forge.http402.trade"
        );
        assert_eq!(forge_public_host("http://127.0.0.1:8092"), "127.0.0.1:8092");
    }

    #[test]
    fn verdict_message_is_pipe_delimited() {
        let id = Uuid::nil();
        let uid = "ab".repeat(32);
        let msg = verdict_message(id, &uid, 1_700_000_000, "preview.forge.http402.trade");
        assert_eq!(
            msg,
            format!("forge-oracle-v1|{id}|{uid}|1700000000|preview.forge.http402.trade")
        );
    }

    #[test]
    fn replay_window_is_plus_minus_60s() {
        assert!(ts_in_replay_window(100, 100));
        assert!(ts_in_replay_window(160, 100));
        assert!(ts_in_replay_window(40, 100));
        assert!(!ts_in_replay_window(161, 100));
        assert!(!ts_in_replay_window(39, 100));
    }

    #[test]
    fn oracle_signature_round_trip() {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[17u8; 32]);
        let authority = bs58::encode(signing_key.verifying_key().to_bytes()).into_string();
        let message = "forge-oracle-v1|listing|uid|1|host";
        let sig = signing_key.sign(message.as_bytes());
        let sig_b58 = bs58::encode(sig.to_bytes()).into_string();
        verify_oracle_signature(&authority, message, &sig_b58).expect("valid sig");
        assert!(verify_oracle_signature(&authority, "tampered", &sig_b58).is_err());
    }

    #[test]
    fn extract_escrow_fund_fields_from_verify_json() {
        let uid = "ab".repeat(32);
        let verify = serde_json::json!({
            "paymentUid": uid,
            "oracleAuthority": "Oracle1111111111111111111111111111111111111"
        });
        let proof = serde_json::json!({});
        let (got_uid, got_oracle) = extract_escrow_fund_fields(&proof, &verify).unwrap();
        assert_eq!(got_uid, uid);
        assert_eq!(got_oracle, "Oracle1111111111111111111111111111111111111");
    }
}
