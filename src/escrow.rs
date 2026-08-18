//! Escrow fund-bind helpers and oracle verdict-door signature checks.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde_json::Value;
use uuid::Uuid;

use crate::db::{EscrowFundBind, ListingRow};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub const ORACLE_MESSAGE_VERSION: &str = "forge-oracle-v1";
pub const ORACLE_REPLAY_WINDOW_SECS: i64 = 60;

pub fn public_host_from_base_url(base: &str) -> String {
    let s = base.trim();
    let rest = s.split("://").nth(1).unwrap_or(s);
    rest.split('/').next().unwrap_or(rest).to_string()
}

pub fn oracle_verdict_message(
    listing_id: Uuid,
    payment_uid_hex: &str,
    ts: i64,
    host: &str,
) -> String {
    format!("{ORACLE_MESSAGE_VERSION}|{listing_id}|{payment_uid_hex}|{ts}|{host}")
}

pub fn normalize_payment_uid_hex(raw: &str) -> Option<String> {
    let s = raw.trim().trim_start_matches("0x").to_ascii_lowercase();
    if s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(s)
    } else {
        None
    }
}

pub fn extract_escrow_fund_fields(verify: &Value) -> Option<(String, String)> {
    let payment_uid = json_payment_uid(verify)?;
    let oracle_authority = json_oracle_authority(verify)?;
    Some((payment_uid, oracle_authority))
}

fn json_payment_uid(v: &Value) -> Option<String> {
    for key in ["paymentUid", "payment_uid"] {
        if let Some(s) = v.get(key).and_then(Value::as_str) {
            if let Some(uid) = normalize_payment_uid_hex(s) {
                return Some(uid);
            }
        }
    }
    if let Some(extra) = v.get("extra") {
        if let Some(uid) = json_payment_uid(extra) {
            return Some(uid);
        }
    }
    if let Some(payload) = v.get("payload") {
        if let Some(uid) = json_payment_uid(payload) {
            return Some(uid);
        }
    }
    None
}

fn json_oracle_authority(v: &Value) -> Option<String> {
    for key in ["oracleAuthority", "oracle_authority"] {
        if let Some(s) = v.get(key).and_then(Value::as_str) {
            let s = s.trim();
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    if let Some(extra) = v.get("extra") {
        if let Some(auth) = json_oracle_authority(extra) {
            return Some(auth);
        }
    }
    if let Some(payload) = v.get("payload") {
        if let Some(auth) = json_oracle_authority(payload) {
            return Some(auth);
        }
    }
    None
}

pub fn verify_oracle_signature(authority_b58: &str, message: &str, signature_b58: &str) -> bool {
    let Ok(pubkey_bytes) = bs58::decode(authority_b58.trim()).into_vec() else {
        return false;
    };
    let Ok(pubkey_array): Result<[u8; 32], _> = pubkey_bytes.try_into() else {
        return false;
    };
    let Ok(verifying_key) = VerifyingKey::from_bytes(&pubkey_array) else {
        return false;
    };
    let Ok(sig_bytes) = bs58::decode(signature_b58.trim()).into_vec() else {
        return false;
    };
    let Ok(sig_array): Result<[u8; 64], _> = sig_bytes.try_into() else {
        return false;
    };
    let signature = Signature::from_bytes(&sig_array);
    verifying_key.verify(message.as_bytes(), &signature).is_ok()
}

pub async fn persist_escrow_fund_bind(
    state: &AppState,
    listing: &ListingRow,
    verify: &Value,
) -> AppResult<EscrowFundBind> {
    let (payment_uid, oracle_authority) = extract_escrow_fund_fields(verify).ok_or_else(|| {
        AppError::BadRequest("sla-escrow fund response missing payment_uid or oracle_authority".into())
    })?;
    let content_hash = listing.content_hash.clone().ok_or_else(|| {
        AppError::BadRequest("escrow listing is missing content_hash".into())
    })?;
    if !state
        .config
        .oracle_authorities
        .iter()
        .any(|a| a == &oracle_authority)
    {
        return Err(AppError::Forbidden(
            "oracle_authority is not in ORACLE_AUTHORITIES".into(),
        ));
    }
    state
        .db
        .upsert_escrow_fund_bind(
            listing.id,
            &payment_uid,
            &content_hash,
            &oracle_authority,
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn payment_uid_must_be_64_hex() {
        assert!(normalize_payment_uid_hex("ab".repeat(32).as_str()).is_some());
        assert!(normalize_payment_uid_hex("xyz").is_none());
        assert_eq!(
            normalize_payment_uid_hex(&format!("0x{}", "aa".repeat(32))),
            Some("aa".repeat(32))
        );
    }

    #[test]
    fn extracts_fund_fields_from_verify_json() {
        let uid = "ab".repeat(32);
        let v = json!({
            "isValid": true,
            "paymentUid": uid,
            "oracleAuthority": "Oracle1111111111111111111111111111111111111"
        });
        let (got_uid, got_auth) = extract_escrow_fund_fields(&v).expect("fields");
        assert_eq!(got_uid, uid);
        assert_eq!(got_auth, "Oracle1111111111111111111111111111111111111");
    }

    #[test]
    fn public_host_strips_scheme_and_path() {
        assert_eq!(
            public_host_from_base_url("https://preview.forge.http402.trade/"),
            "preview.forge.http402.trade"
        );
        assert_eq!(
            public_host_from_base_url("http://127.0.0.1:8092"),
            "127.0.0.1:8092"
        );
    }
}
