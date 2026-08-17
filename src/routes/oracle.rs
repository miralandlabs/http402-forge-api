use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    Json,
};
use chrono::Utc;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::SharedState;
use crate::storage::{serve_object, DeliveryFormat, ObjectServeOptions, ObjectStore};

const ORACLE_SIG_PREFIX: &str = "forge-oracle-v1";
const ORACLE_REPLAY_WINDOW_SECS: i64 = 60;

fn required_header(headers: &HeaderMap, name: &str) -> AppResult<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| AppError::Forbidden(format!("missing required header: {name}")))
}

/// Canonical lowercase hex-64 payment uid (32 bytes). Hex is case-insensitive,
/// but the signature is over this normalized form, so both doors normalize.
pub fn normalize_payment_uid(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.len() != 64 || !trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

/// The exact UTF-8 message an oracle signs for the verdict door.
pub fn oracle_verdict_message(listing_id: Uuid, payment_uid: &str, ts: i64, host: &str) -> String {
    format!("{ORACLE_SIG_PREFIX}|{listing_id}|{payment_uid}|{ts}|{host}")
}

/// Verifies a base58 Ed25519 signature over `message` against a base58 pubkey.
pub fn verify_oracle_signature(message: &str, sig_b58: &str, pubkey_b58: &str) -> Result<(), String> {
    let sig_bytes = bs58::decode(sig_b58.trim())
        .into_vec()
        .map_err(|e| e.to_string())?;
    let sig_array: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| "oracle signature must be 64 bytes".to_string())?;
    let signature = Signature::from_bytes(&sig_array);

    let pk_bytes = bs58::decode(pubkey_b58.trim())
        .into_vec()
        .map_err(|e| e.to_string())?;
    let pk_array: [u8; 32] = pk_bytes
        .try_into()
        .map_err(|_| "oracle authority must be 32 bytes".to_string())?;
    let verifying_key = VerifyingKey::from_bytes(&pk_array).map_err(|e| e.to_string())?;

    verifying_key
        .verify(message.as_bytes(), &signature)
        .map_err(|e| e.to_string())
}

fn verdict_host(state: &SharedState) -> String {
    let base = state.config.seller_public_base_url.trim_end_matches('/');
    base.split("://").nth(1).unwrap_or(base).to_string()
}

/// GET /api/v1/oracle/listings/{listing_id}/artifact
///
/// Oracle verdict door: streams the immutable stored object for a funded
/// sla-escrow payment after verifying the oracle's Ed25519 signature over
/// `forge-oracle-v1|{listing_id}|{payment_uid}|{ts}|{host}`. No 402 and no
/// sale row. Exact-rail listings never open this door.
pub async fn artifact(
    State(state): State<SharedState>,
    Path(listing_id): Path<Uuid>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let payment_uid_raw = required_header(&headers, "x-forge-payment-uid")?;
    let ts_raw = required_header(&headers, "x-forge-oracle-ts")?;
    let sig = required_header(&headers, "x-forge-oracle-sig")?;

    let payment_uid = normalize_payment_uid(&payment_uid_raw)
        .ok_or_else(|| AppError::Forbidden("invalid payment uid (expected 64 hex chars)".into()))?;
    let ts: i64 = ts_raw
        .parse()
        .map_err(|_| AppError::Forbidden("invalid oracle timestamp".into()))?;
    let now = Utc::now().timestamp();
    if (now - ts).abs() > ORACLE_REPLAY_WINDOW_SECS {
        return Err(AppError::Forbidden(
            "oracle signature outside replay window".into(),
        ));
    }

    let row = state.db.get_listing_any(listing_id).await?;
    if row.delivery_scheme != "escrow" {
        return Err(AppError::Forbidden(
            "verdict door is only available for escrow listings".into(),
        ));
    }

    let bind = state
        .db
        .find_escrow_fund_bind(listing_id, &payment_uid)
        .await?
        .ok_or_else(|| AppError::Forbidden("no escrow fund bind for this payment".into()))?;

    let message = oracle_verdict_message(listing_id, &payment_uid, ts, &verdict_host(&state));
    verify_oracle_signature(&message, &sig, &bind.oracle_authority)
        .map_err(|_| AppError::Forbidden("oracle signature verification failed".into()))?;

    let content_hash = row
        .content_hash
        .as_deref()
        .ok_or_else(|| AppError::Forbidden("listing has no content hash".into()))?;
    if content_hash != bind.content_hash {
        return Err(AppError::Forbidden(
            "escrow bind content hash mismatch".into(),
        ));
    }

    let content_type = state.storage.head(&row.asset_key).await?;
    serve_object(
        &state,
        ObjectServeOptions {
            key: &row.asset_key,
            content_type: &content_type,
            content_disposition: None,
            extra_headers: HeaderMap::new(),
            format: DeliveryFormat::Proxy,
            sale_id: None,
        },
    )
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EscrowFundBindRequest {
    pub listing_id: Uuid,
    pub payment_uid: String,
    #[serde(default)]
    pub content_hash: Option<String>,
    pub oracle_authority: String,
}

/// POST /api/v1/oracle/escrow-binds
///
/// Facilitator-facing door: records the bind row when a sla-escrow payment is
/// successfully funded. The stored `oracle_authority` is the on-chain
/// authority pinned by FundPayment and must be a listed `ORACLE_AUTHORITIES`
/// operator. `content_hash` is taken from the immutable listing row.
pub async fn record_fund_bind(
    State(state): State<SharedState>,
    Json(body): Json<EscrowFundBindRequest>,
) -> AppResult<(StatusCode, Json<serde_json::Value>)> {
    let payment_uid = normalize_payment_uid(&body.payment_uid)
        .ok_or_else(|| AppError::validation("payment_uid", "must be 64 hex chars"))?;

    let row = state.db.get_listing_any(body.listing_id).await?;
    if row.delivery_scheme != "escrow" {
        return Err(AppError::Forbidden(
            "escrow fund bind requires an escrow listing".into(),
        ));
    }
    let content_hash = row
        .content_hash
        .as_deref()
        .ok_or_else(|| AppError::Forbidden("listing has no content hash".into()))?;
    if let Some(ref provided) = body.content_hash {
        if provided.trim().to_ascii_lowercase() != content_hash {
            return Err(AppError::Forbidden(
                "content_hash does not match listing".into(),
            ));
        }
    }

    if state.config.oracle_authorities.is_empty() {
        return Err(AppError::PaymentConfig(
            "ORACLE_AUTHORITIES not configured".into(),
        ));
    }
    if !state
        .config
        .oracle_authorities
        .iter()
        .any(|a| a == &body.oracle_authority)
    {
        return Err(AppError::Forbidden(
            "oracle_authority is not a listed operator".into(),
        ));
    }

    state
        .db
        .record_escrow_fund_bind(
            body.listing_id,
            &payment_uid,
            content_hash,
            &body.oracle_authority,
        )
        .await?;

    Ok((StatusCode::CREATED, Json(serde_json::json!({ "ok": true }))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::header::HeaderValue;
    use bytes::Bytes;
    use ed25519_dalek::Signer;
    use std::sync::Arc;

    use crate::config::{
        AppConfig, ClusterConfig, ModerationConfig, ModerationProvider, ObjectDelivery,
        SolanaCluster, StorageBackend,
    };
    use crate::db::{Database, ListingRow};
    use crate::state::AppState;
    use crate::storage::{asset_object_key, ObjectStore};

    fn sha256_hex(data: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(data))
    }

    fn test_config(tmp: &std::path::Path, oracle_authorities: Vec<String>) -> AppConfig {
        AppConfig {
            cluster: SolanaCluster::Devnet,
            bind_addr: "127.0.0.1:8092".parse().unwrap(),
            seller_public_base_url: "http://127.0.0.1:8092".into(),
            database_url: format!("sqlite:{}", tmp.join("forge.db").display()),
            facilitator_base_url: "https://preview.ipay.sh".into(),
            facilitator_timeout_secs: 15,
            payment_timeout_secs: 300,
            storage_backend: StorageBackend::Local,
            local_storage_path: tmp.join("objects"),
            r2_account_id: None,
            r2_bucket: None,
            r2_access_key_id: None,
            r2_secret_access_key: None,
            max_asset_bytes: crate::config::DEFAULT_MAX_ASSET_BYTES,
            max_preview_bytes: crate::config::DEFAULT_MAX_PREVIEW_BYTES,
            preview_media_seconds: 30,
            ffmpeg_bin: "ffmpeg".into(),
            pdftoppm_bin: "pdftoppm".into(),
            gs_bin: "gs".into(),
            mutool_bin: "mutool".into(),
            escrow_size_threshold: crate::config::DEFAULT_ESCROW_SIZE_THRESHOLD_BYTES,
            platform_fee_bps: 0,
            platform_fee_wallet: None,
            oracle_authorities,
            oracle_profile_id: "x402/oracles/file-delivery/attestation/v1".into(),
            skip_seller_vault_check: true,
            skip_seller_auth: true,
            skip_buyer_auth: true,
            moderation: ModerationConfig {
                provider: ModerationProvider::None,
                openai_api_key: None,
                fail_closed: false,
            },
            cors_allowed_origins: vec![],
            object_delivery: ObjectDelivery::Proxy,
            presign_ttl_secs: 300,
            version: "0.1.0".into(),
            leaderboard_limit: 5,
        }
    }

    async fn build_state(tmp: &std::path::Path, oracle_authorities: Vec<String>) -> SharedState {
        let config = test_config(tmp, oracle_authorities);
        let cluster = ClusterConfig::for_cluster(config.cluster);
        let db = Database::connect(&config.database_url)
            .await
            .expect("connect db");
        Arc::new(
            AppState::build(config, cluster, db)
                .await
                .expect("build state"),
        )
    }

    async fn insert_listing(
        state: &SharedState,
        delivery_scheme: &str,
        asset_bytes: &[u8],
    ) -> (Uuid, String) {
        let id = Uuid::new_v4();
        let content_hash = sha256_hex(asset_bytes);
        let asset_key = asset_object_key(&content_hash);
        state
            .storage
            .put_if_absent(
                &asset_key,
                "application/octet-stream",
                Bytes::copy_from_slice(asset_bytes),
            )
            .await
            .expect("store asset");
        let row = ListingRow {
            id,
            seller_wallet: "SellerWallet111111111111111111111111111111".into(),
            display_name: None,
            title: "escrow test".into(),
            description: String::new(),
            category: "art".into(),
            price_micro_usdc: 50_000,
            preview_key: "previews/x/preview".into(),
            preview_content_type: "text/plain".into(),
            asset_key,
            content_type: "application/octet-stream".into(),
            byte_size: asset_bytes.len() as i64,
            agent_friendly: false,
            delivery_scheme: delivery_scheme.into(),
            status: "active".into(),
            tags: "[]".into(),
            license: None,
            content_hash: Some(content_hash.clone()),
            moderation_status: "approved".into(),
            moderation_labels: "[]".into(),
            created_at: Utc::now(),
        };
        state.db.insert_listing(&row).await.expect("insert listing");
        (id, content_hash)
    }

    fn signed_headers(
        signing_key: &ed25519_dalek::SigningKey,
        listing_id: Uuid,
        payment_uid: &str,
        ts: i64,
    ) -> HeaderMap {
        let message = oracle_verdict_message(listing_id, payment_uid, ts, "127.0.0.1:8092");
        let sig = bs58::encode(signing_key.sign(message.as_bytes()).to_bytes()).into_string();
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forge-payment-uid",
            HeaderValue::from_str(payment_uid).unwrap(),
        );
        headers.insert(
            "x-forge-oracle-ts",
            HeaderValue::from_str(&ts.to_string()).unwrap(),
        );
        headers.insert(
            "x-forge-oracle-sig",
            HeaderValue::from_str(&sig).unwrap(),
        );
        headers
    }

    #[tokio::test]
    async fn verdict_streams_object_for_valid_oracle_signature() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let seed = [42u8; 32];
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let oracle_authority = bs58::encode(signing_key.verifying_key().to_bytes()).into_string();

        let state = build_state(tmp.path(), vec![oracle_authority.clone()]).await;
        let asset = b"oracle artifact bytes";
        let (listing_id, content_hash) = insert_listing(&state, "escrow", asset).await;

        let payment_uid = "ab".repeat(32);
        state
            .db
            .record_escrow_fund_bind(listing_id, &payment_uid, &content_hash, &oracle_authority)
            .await
            .expect("bind");

        let ts = Utc::now().timestamp();
        let headers = signed_headers(&signing_key, listing_id, &payment_uid, ts);

        let response = artifact(State(state.clone()), Path(listing_id), headers)
            .await
            .expect("artifact");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            !response.headers().contains_key("x-forge-sale-id"),
            "verdict door must not set a sale id"
        );
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        assert_eq!(body.as_ref(), asset);
    }

    #[tokio::test]
    async fn verdict_returns_403_without_signature() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let seed = [42u8; 32];
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let oracle_authority = bs58::encode(signing_key.verifying_key().to_bytes()).into_string();
        let state = build_state(tmp.path(), vec![oracle_authority.clone()]).await;
        let asset = b"oracle artifact bytes";
        let (listing_id, content_hash) = insert_listing(&state, "escrow", asset).await;
        let payment_uid = "ab".repeat(32);
        state
            .db
            .record_escrow_fund_bind(listing_id, &payment_uid, &content_hash, &oracle_authority)
            .await
            .expect("bind");

        let err = artifact(State(state.clone()), Path(listing_id), HeaderMap::new())
            .await
            .expect_err("missing headers must be rejected");
        assert!(matches!(err, AppError::Forbidden(_)));
    }

    #[tokio::test]
    async fn exact_rail_listing_never_opens_verdict_door() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let seed = [42u8; 32];
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let oracle_authority = bs58::encode(signing_key.verifying_key().to_bytes()).into_string();
        let state = build_state(tmp.path(), vec![oracle_authority.clone()]).await;
        let asset = b"exact rail asset";
        let (listing_id, content_hash) = insert_listing(&state, "exact", asset).await;
        let payment_uid = "ab".repeat(32);
        state
            .db
            .record_escrow_fund_bind(listing_id, &payment_uid, &content_hash, &oracle_authority)
            .await
            .expect("bind");

        let ts = Utc::now().timestamp();
        let headers = signed_headers(&signing_key, listing_id, &payment_uid, ts);
        let err = artifact(State(state.clone()), Path(listing_id), headers)
            .await
            .expect_err("exact rail must not open verdict door");
        assert!(matches!(err, AppError::Forbidden(_)));
    }

    #[tokio::test]
    async fn verdict_rejects_stale_timestamp() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let seed = [42u8; 32];
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let oracle_authority = bs58::encode(signing_key.verifying_key().to_bytes()).into_string();
        let state = build_state(tmp.path(), vec![oracle_authority.clone()]).await;
        let asset = b"oracle artifact bytes";
        let (listing_id, content_hash) = insert_listing(&state, "escrow", asset).await;
        let payment_uid = "ab".repeat(32);
        state
            .db
            .record_escrow_fund_bind(listing_id, &payment_uid, &content_hash, &oracle_authority)
            .await
            .expect("bind");

        let stale_ts = Utc::now().timestamp() - 120;
        let headers = signed_headers(&signing_key, listing_id, &payment_uid, stale_ts);
        let err = artifact(State(state.clone()), Path(listing_id), headers)
            .await
            .expect_err("stale ts must be rejected");
        assert!(matches!(err, AppError::Forbidden(_)));
    }

    #[tokio::test]
    async fn verdict_rejects_wrong_signer_and_missing_bind() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let seed = [42u8; 32];
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let oracle_authority = bs58::encode(signing_key.verifying_key().to_bytes()).into_string();
        let state = build_state(tmp.path(), vec![oracle_authority.clone()]).await;
        let asset = b"oracle artifact bytes";
        let (listing_id, content_hash) = insert_listing(&state, "escrow", asset).await;
        let payment_uid = "ab".repeat(32);
        state
            .db
            .record_escrow_fund_bind(listing_id, &payment_uid, &content_hash, &oracle_authority)
            .await
            .expect("bind");

        // A different oracle key (not the bound authority) must fail.
        let impostor = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let ts = Utc::now().timestamp();
        let headers = signed_headers(&impostor, listing_id, &payment_uid, ts);
        let err = artifact(State(state.clone()), Path(listing_id), headers)
            .await
            .expect_err("impostor signature must be rejected");
        assert!(matches!(err, AppError::Forbidden(_)));

        // A payment uid with no bind row must fail even with a valid signature.
        let unbound_uid = "cd".repeat(32);
        let headers =
            signed_headers(&signing_key, listing_id, &unbound_uid, Utc::now().timestamp());
        let err = artifact(State(state.clone()), Path(listing_id), headers)
            .await
            .expect_err("unbound payment must be rejected");
        assert!(matches!(err, AppError::Forbidden(_)));
    }

    #[tokio::test]
    async fn fund_bind_records_row_and_validates_operator() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let seed = [42u8; 32];
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let oracle_authority = bs58::encode(signing_key.verifying_key().to_bytes()).into_string();
        let state = build_state(tmp.path(), vec![oracle_authority.clone()]).await;
        let asset = b"escrow asset";
        let (listing_id, content_hash) = insert_listing(&state, "escrow", asset).await;
        let payment_uid = "ab".repeat(32);

        let req = EscrowFundBindRequest {
            listing_id,
            payment_uid: payment_uid.clone(),
            content_hash: None,
            oracle_authority: oracle_authority.clone(),
        };
        let (status, _) = record_fund_bind(State(state.clone()), Json(req))
            .await
            .expect("record bind");
        assert_eq!(status, StatusCode::CREATED);

        let bind = state
            .db
            .find_escrow_fund_bind(listing_id, &payment_uid)
            .await
            .expect("find")
            .expect("bind row");
        assert_eq!(bind.content_hash, content_hash);
        assert_eq!(bind.oracle_authority, oracle_authority);

        // A non-listed operator must be rejected.
        let bad = EscrowFundBindRequest {
            listing_id,
            payment_uid: "cd".repeat(32),
            content_hash: None,
            oracle_authority: "NotAListedOperator111111111111111111111111111".into(),
        };
        let err = record_fund_bind(State(state.clone()), Json(bad))
            .await
            .expect_err("non-listed operator must be rejected");
        assert!(matches!(err, AppError::Forbidden(_)));
    }

    #[test]
    fn oracle_verdict_message_is_deterministic() {
        let listing_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let payment_uid = "ab".repeat(32);
        let message = oracle_verdict_message(
            listing_id,
            &payment_uid,
            1723843200,
            "preview.forge.http402.trade",
        );
        assert_eq!(
            message,
            format!(
                "forge-oracle-v1|550e8400-e29b-41d4-a716-446655440000|{}|1723843200|preview.forge.http402.trade",
                payment_uid
            )
        );
    }
}
