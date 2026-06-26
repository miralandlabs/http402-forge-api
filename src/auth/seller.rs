use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine;
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

const CHALLENGE_PREFIX: &str = "http402-forge:create-listing:v1";
const DELIST_CHALLENGE_PREFIX: &str = "http402-forge:delist-listing:v1";
const FEEDBACK_CHALLENGE_PREFIX: &str = "http402-forge:sale-feedback:v1";
const REDOWNLOAD_CHALLENGE_PREFIX: &str = "http402-forge:redownload:v1";
const CHALLENGE_TTL: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub struct StoredChallenge {
    pub wallet: String,
    pub message: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Default)]
pub struct SellerAuth {
    pending: Mutex<HashMap<String, StoredChallenge>>,
}

impl SellerAuth {
    pub fn issue_challenge(&self, wallet: &str) -> AppResult<(String, DateTime<Utc>)> {
        validate_wallet_pubkey(wallet).map_err(|m| AppError::validation("seller_wallet", m))?;
        let id = Uuid::new_v4().to_string();
        let expires_at = Utc::now() + chrono::Duration::seconds(CHALLENGE_TTL.as_secs() as i64);
        let message = format!(
            "{CHALLENGE_PREFIX}\nwallet:{wallet}\nchallenge:{id}\nexpires:{}",
            expires_at.to_rfc3339()
        );
        let stored = StoredChallenge {
            wallet: wallet.to_string(),
            message: message.clone(),
            expires_at,
        };
        self.pending
            .lock()
            .expect("seller auth lock")
            .insert(id, stored);
        Ok((message, expires_at))
    }

    pub fn issue_delist_challenge(
        &self,
        wallet: &str,
        listing_id: uuid::Uuid,
    ) -> AppResult<(String, DateTime<Utc>)> {
        validate_wallet_pubkey(wallet).map_err(|m| AppError::validation("seller_wallet", m))?;
        let id = Uuid::new_v4().to_string();
        let expires_at = Utc::now() + chrono::Duration::seconds(CHALLENGE_TTL.as_secs() as i64);
        let message = format!(
            "{DELIST_CHALLENGE_PREFIX}\nwallet:{wallet}\nlisting:{listing_id}\nchallenge:{id}\nexpires:{}",
            expires_at.to_rfc3339()
        );
        let stored = StoredChallenge {
            wallet: wallet.to_string(),
            message: message.clone(),
            expires_at,
        };
        self.pending
            .lock()
            .expect("seller auth lock")
            .insert(id, stored);
        Ok((message, expires_at))
    }

    pub fn issue_feedback_challenge(
        &self,
        wallet: &str,
        sale_id: uuid::Uuid,
    ) -> AppResult<(String, DateTime<Utc>)> {
        validate_wallet_pubkey(wallet).map_err(|m| AppError::validation("buyer_wallet", m))?;
        let id = Uuid::new_v4().to_string();
        let expires_at = Utc::now() + chrono::Duration::seconds(CHALLENGE_TTL.as_secs() as i64);
        let message = format!(
            "{FEEDBACK_CHALLENGE_PREFIX}\nwallet:{wallet}\nsale:{sale_id}\nchallenge:{id}\nexpires:{}",
            expires_at.to_rfc3339()
        );
        let stored = StoredChallenge {
            wallet: wallet.to_string(),
            message: message.clone(),
            expires_at,
        };
        self.pending
            .lock()
            .expect("seller auth lock")
            .insert(id, stored);
        Ok((message, expires_at))
    }

    pub fn issue_redownload_challenge(
        &self,
        wallet: &str,
        listing_id: uuid::Uuid,
    ) -> AppResult<(String, DateTime<Utc>)> {
        validate_wallet_pubkey(wallet).map_err(|m| AppError::validation("buyer_wallet", m))?;
        let id = Uuid::new_v4().to_string();
        let expires_at = Utc::now() + chrono::Duration::seconds(CHALLENGE_TTL.as_secs() as i64);
        let message = format!(
            "{REDOWNLOAD_CHALLENGE_PREFIX}\nwallet:{wallet}\nlisting:{listing_id}\nchallenge:{id}\nexpires:{}",
            expires_at.to_rfc3339()
        );
        let stored = StoredChallenge {
            wallet: wallet.to_string(),
            message: message.clone(),
            expires_at,
        };
        self.pending
            .lock()
            .expect("seller auth lock")
            .insert(id, stored);
        Ok((message, expires_at))
    }

    pub fn verify_and_consume(
        &self,
        wallet: &str,
        challenge_message: &str,
        signature_b64: &str,
    ) -> AppResult<()> {
        validate_wallet_pubkey(wallet).map_err(|m| AppError::validation("seller_wallet", m))?;
        if challenge_message.trim().is_empty() {
            return Err(AppError::validation(
                "seller_challenge",
                "signed challenge message required",
            ));
        }
        if signature_b64.trim().is_empty() {
            return Err(AppError::validation(
                "seller_signature",
                "base64 ed25519 signature required",
            ));
        }

        let challenge_id = parse_challenge_id(challenge_message).ok_or_else(|| {
            AppError::validation("seller_challenge", "invalid challenge message format")
        })?;

        let stored = {
            let mut pending = self.pending.lock().expect("seller auth lock");
            pending.remove(&challenge_id).ok_or_else(|| {
                AppError::Forbidden("challenge expired or already used — request a new one".into())
            })?
        };

        if stored.wallet != wallet {
            return Err(AppError::Forbidden(
                "challenge wallet does not match seller_wallet".into(),
            ));
        }
        if normalize_challenge_message(&stored.message)
            != normalize_challenge_message(challenge_message)
        {
            return Err(AppError::Forbidden("challenge message mismatch".into()));
        }
        if Utc::now() > stored.expires_at {
            return Err(AppError::Forbidden("challenge expired".into()));
        }

        verify_ed25519_signature(wallet, challenge_message.as_bytes(), signature_b64)?;
        Ok(())
    }
}

fn normalize_challenge_message(message: &str) -> String {
    message.replace("\r\n", "\n").trim().to_string()
}

fn solana_off_chain_message(message: &[u8]) -> Vec<u8> {
    const PREFIX: &[u8] = b"\xffSolana Off-chain Message";
    let mut out = Vec::with_capacity(PREFIX.len() + message.len() + 8);
    out.extend_from_slice(PREFIX);
    let len = message.len();
    if len <= 0x7f {
        out.push(len as u8);
    } else if len <= 0x3fff {
        out.push(((len & 0x7f) as u8) | 0x80);
        out.push((len >> 7) as u8);
    } else {
        out.push(((len & 0x7f) as u8) | 0x80);
        out.push((((len >> 7) & 0x7f) as u8) | 0x80);
        out.push((len >> 14) as u8);
    }
    out.extend_from_slice(message);
    out
}

fn parse_challenge_id(message: &str) -> Option<String> {
    message.lines().find_map(|line| {
        line.strip_prefix("challenge:")
            .map(str::trim)
            .map(str::to_string)
    })
}

pub fn parse_delist_listing_id(message: &str) -> Option<uuid::Uuid> {
    message.lines().find_map(|line| {
        line.strip_prefix("listing:")
            .map(str::trim)
            .and_then(|s| uuid::Uuid::parse_str(s).ok())
    })
}

pub fn parse_feedback_sale_id(message: &str) -> Option<uuid::Uuid> {
    message.lines().find_map(|line| {
        line.strip_prefix("sale:")
            .map(str::trim)
            .and_then(|s| uuid::Uuid::parse_str(s).ok())
    })
}

pub fn parse_redownload_listing_id(message: &str) -> Option<uuid::Uuid> {
    parse_delist_listing_id(message)
}

fn validate_wallet_pubkey(wallet: &str) -> Result<(), String> {
    if wallet.len() < 32 || wallet.len() > 44 {
        return Err("invalid seller_wallet".into());
    }
    let bytes = bs58::decode(wallet)
        .into_vec()
        .map_err(|_| "invalid seller_wallet base58".to_string())?;
    if bytes.len() != 32 {
        return Err("seller_wallet must decode to 32 bytes".into());
    }
    Ok(())
}

fn verify_ed25519_signature(wallet: &str, message: &[u8], signature_b64: &str) -> AppResult<()> {
    let pubkey_bytes = bs58::decode(wallet)
        .into_vec()
        .map_err(|_| AppError::Forbidden("invalid seller_wallet".into()))?;
    let pubkey_array: [u8; 32] = pubkey_bytes
        .try_into()
        .map_err(|_| AppError::Forbidden("invalid seller_wallet length".into()))?;
    let verifying_key = VerifyingKey::from_bytes(&pubkey_array)
        .map_err(|_| AppError::Forbidden("invalid seller_wallet key".into()))?;

    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(signature_b64.trim())
        .map_err(|_| AppError::Forbidden("invalid seller_signature base64".into()))?;
    let sig_array: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| AppError::Forbidden("invalid seller_signature length".into()))?;
    let signature = Signature::from_bytes(&sig_array);
    let normalized = normalize_challenge_message(
        std::str::from_utf8(message)
            .map_err(|_| AppError::Forbidden("invalid challenge utf-8".into()))?,
    );

    verifying_key
        .verify(normalized.as_bytes(), &signature)
        .or_else(|_| {
            let prefixed = solana_off_chain_message(normalized.as_bytes());
            verifying_key.verify(prefixed.as_slice(), &signature)
        })
        .map_err(|_| AppError::Forbidden("seller_signature verification failed".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Signer;

    #[test]
    fn challenge_round_trip() {
        let auth = SellerAuth::default();
        let seed = [7u8; 32];
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let wallet = bs58::encode(signing_key.verifying_key().to_bytes()).into_string();
        let (message, _expires) = auth.issue_challenge(&wallet).unwrap();
        let signature = signing_key.sign(message.as_bytes());
        let sig_b64 = base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());
        auth.verify_and_consume(&wallet, &message, &sig_b64)
            .expect("verify");
        assert!(auth
            .verify_and_consume(&wallet, &message, &sig_b64)
            .is_err());
    }

    #[test]
    fn challenge_round_trip_crlf() {
        let auth = SellerAuth::default();
        let seed = [9u8; 32];
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let wallet = bs58::encode(signing_key.verifying_key().to_bytes()).into_string();
        let (message, _expires) = auth.issue_challenge(&wallet).unwrap();
        let crlf = message.replace('\n', "\r\n");
        let signature = signing_key.sign(normalize_challenge_message(&crlf).as_bytes());
        let sig_b64 = base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());
        auth.verify_and_consume(&wallet, &crlf, &sig_b64)
            .expect("verify crlf");
    }

    #[test]
    fn delist_challenge_round_trip() {
        let auth = SellerAuth::default();
        let seed = [11u8; 32];
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let wallet = bs58::encode(signing_key.verifying_key().to_bytes()).into_string();
        let listing_id = uuid::Uuid::new_v4();
        let (message, _expires) = auth.issue_delist_challenge(&wallet, listing_id).unwrap();
        assert_eq!(parse_delist_listing_id(&message), Some(listing_id));
        let signature = signing_key.sign(message.as_bytes());
        let sig_b64 = base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());
        auth.verify_and_consume(&wallet, &message, &sig_b64)
            .expect("verify delist");
    }

    #[test]
    fn redownload_challenge_round_trip() {
        let auth = SellerAuth::default();
        let seed = [13u8; 32];
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let wallet = bs58::encode(signing_key.verifying_key().to_bytes()).into_string();
        let listing_id = uuid::Uuid::new_v4();
        let (message, _expires) = auth.issue_redownload_challenge(&wallet, listing_id).unwrap();
        assert_eq!(parse_redownload_listing_id(&message), Some(listing_id));
        let signature = signing_key.sign(message.as_bytes());
        let sig_b64 = base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());
        auth.verify_and_consume(&wallet, &message, &sig_b64)
            .expect("verify redownload");
    }
}
