//! Oracle verdict door: signature message, replay window, Ed25519 verify.

use std::time::{SystemTime, UNIX_EPOCH};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};

use crate::error::{AppError, AppResult};

pub const ORACLE_MESSAGE_PREFIX: &str = "forge-oracle-v1";
pub const ORACLE_REPLAY_WINDOW_SECS: i64 = 60;

pub fn forge_public_host(seller_public_base_url: &str) -> String {
    let trimmed = seller_public_base_url.trim().trim_end_matches('/');
    if let Ok(url) = reqwest::Url::parse(trimmed) {
        if let Some(host) = url.host_str() {
            return match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host.to_string(),
            };
        }
    }
    trimmed
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .to_string()
}

pub fn oracle_verdict_message(
    listing_id: &str,
    payment_uid_hex: &str,
    ts: i64,
    host: &str,
) -> String {
    format!("{ORACLE_MESSAGE_PREFIX}|{listing_id}|{payment_uid_hex}|{ts}|{host}")
}

pub fn unix_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn ts_in_replay_window(ts: i64, now: i64) -> bool {
    (now - ts).abs() <= ORACLE_REPLAY_WINDOW_SECS
}

pub fn verify_oracle_signature(
    oracle_authority: &str,
    message: &str,
    signature_b58: &str,
) -> AppResult<()> {
    let pubkey_bytes = bs58::decode(oracle_authority)
        .into_vec()
        .map_err(|_| AppError::Forbidden("invalid oracle_authority".into()))?;
    let pubkey_array: [u8; 32] = pubkey_bytes
        .try_into()
        .map_err(|_| AppError::Forbidden("invalid oracle_authority length".into()))?;
    let verifying_key = VerifyingKey::from_bytes(&pubkey_array)
        .map_err(|_| AppError::Forbidden("invalid oracle_authority key".into()))?;

    let sig_bytes = bs58::decode(signature_b58.trim())
        .into_vec()
        .map_err(|_| AppError::Forbidden("invalid oracle signature".into()))?;
    let sig_array: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| AppError::Forbidden("invalid oracle signature length".into()))?;
    let signature = Signature::from_bytes(&sig_array);

    verifying_key
        .verify(message.as_bytes(), &signature)
        .map_err(|_| AppError::Forbidden("oracle signature verification failed".into()))
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
        assert_eq!(
            forge_public_host("http://127.0.0.1:8092"),
            "127.0.0.1:8092"
        );
    }

    #[test]
    fn replay_window_accepts_plus_minus_60s() {
        assert!(ts_in_replay_window(1000, 1000));
        assert!(ts_in_replay_window(940, 1000));
        assert!(ts_in_replay_window(1060, 1000));
        assert!(!ts_in_replay_window(939, 1000));
        assert!(!ts_in_replay_window(1061, 1000));
    }

    #[test]
    fn oracle_signature_round_trip() {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[3u8; 32]);
        let authority = bs58::encode(signing_key.verifying_key().to_bytes()).into_string();
        let listing_id = "550e8400-e29b-41d4-a716-446655440000";
        let uid = "ab".repeat(32);
        let ts = 1_700_000_000;
        let host = "preview.forge.http402.trade";
        let message = oracle_verdict_message(listing_id, &uid, ts, host);
        let sig = bs58::encode(signing_key.sign(message.as_bytes()).to_bytes()).into_string();
        verify_oracle_signature(&authority, &message, &sig).expect("valid sig");
        assert!(verify_oracle_signature(&authority, &message, "not-a-sig").is_err());
    }
}
