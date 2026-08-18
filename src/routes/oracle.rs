//! Oracle verdict door: signature-verified stream of the same stored object (not a sale).

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::escrow::{
    normalize_payment_uid_hex, oracle_verdict_message, public_host_from_base_url,
    verify_oracle_signature, ORACLE_REPLAY_WINDOW_SECS,
};
use crate::state::SharedState;
use crate::storage::{serve_object, DeliveryFormat, ObjectServeOptions, ObjectStore};

fn header_str<'a>(headers: &'a HeaderMap, name: &'static str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim)
}

pub async fn artifact(
    State(state): State<SharedState>,
    Path(listing_id): Path<Uuid>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let payment_uid = header_str(&headers, "x-forge-payment-uid").ok_or_else(|| {
        AppError::Forbidden("missing X-Forge-Payment-Uid".into())
    })?;
    let ts_raw = header_str(&headers, "x-forge-oracle-ts")
        .ok_or_else(|| AppError::Forbidden("missing X-Forge-Oracle-Ts".into()))?;
    let sig = header_str(&headers, "x-forge-oracle-sig")
        .ok_or_else(|| AppError::Forbidden("missing X-Forge-Oracle-Sig".into()))?;

    let payment_uid = normalize_payment_uid_hex(payment_uid)
        .ok_or_else(|| AppError::Forbidden("invalid X-Forge-Payment-Uid".into()))?;
    let ts: i64 = ts_raw
        .parse()
        .map_err(|_| AppError::Forbidden("invalid X-Forge-Oracle-Ts".into()))?;
    let now = chrono::Utc::now().timestamp();
    if (now - ts).abs() > ORACLE_REPLAY_WINDOW_SECS {
        return Err(AppError::Forbidden("oracle timestamp outside replay window".into()));
    }

    let listing = state
        .db
        .get_listing(listing_id)
        .await
        .map_err(|_| AppError::Forbidden("verdict denied".into()))?;
    if listing.delivery_scheme != "escrow" {
        return Err(AppError::Forbidden("exact-rail listings never open the verdict door".into()));
    }

    let bind = state
        .db
        .get_escrow_fund_bind(listing_id, &payment_uid)
        .await?
        .ok_or_else(|| AppError::Forbidden("no escrow fund bind for this payment".into()))?;
    if listing.content_hash.as_deref() != Some(bind.content_hash.as_str()) {
        return Err(AppError::Forbidden("bind content_hash mismatch".into()));
    }

    let host = public_host_from_base_url(&state.config.seller_public_base_url);
    let message = oracle_verdict_message(listing_id, &payment_uid, ts, &host);
    if !verify_oracle_signature(&bind.oracle_authority, &message, sig) {
        return Err(AppError::Forbidden("invalid oracle signature".into()));
    }

    let content_type = state
        .storage
        .head(&listing.asset_key)
        .await
        .map_err(|_| AppError::Forbidden("stored object not found".into()))?;
    serve_object(
        &state,
        ObjectServeOptions {
            key: &listing.asset_key,
            content_type: &content_type,
            content_disposition: None,
            extra_headers: HeaderMap::new(),
            format: DeliveryFormat::Proxy,
            sale_id: None,
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AppConfig, ModerationConfig, ModerationProvider, ObjectDelivery, SolanaCluster,
        StorageBackend, DEFAULT_ESCROW_SIZE_THRESHOLD_BYTES, DEFAULT_MAX_ASSET_BYTES,
        DEFAULT_MAX_PREVIEW_BYTES,
    };
    use crate::db::{Database, ListingRow};
    use crate::error::AppError;
    use crate::escrow::{persist_escrow_fund_bind, public_host_from_base_url};
    use crate::routes::listings::{publish_listing, PublishListingInput};
    use crate::state::AppState;
    use crate::storage::ObjectStore;
    use crate::x402::PaymentGate;
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use axum::response::IntoResponse;
    use bytes::Bytes;
    use chrono::Utc;
    use ed25519_dalek::Signer;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::sync::Arc;
    use uuid::Uuid;

    fn sha256_hex(data: &[u8]) -> String {
        format!("{:x}", Sha256::digest(data))
    }

    fn test_config(
        objects: std::path::PathBuf,
        facilitator: String,
        oracles: Vec<String>,
    ) -> AppConfig {
        AppConfig {
            cluster: SolanaCluster::Devnet,
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            seller_public_base_url: "https://preview.forge.http402.trade".into(),
            database_url: "sqlite::memory:".into(),
            facilitator_base_url: facilitator,
            facilitator_timeout_secs: 5,
            payment_timeout_secs: 300,
            storage_backend: StorageBackend::Local,
            local_storage_path: objects,
            r2_account_id: None,
            r2_bucket: None,
            r2_access_key_id: None,
            r2_secret_access_key: None,
            max_asset_bytes: DEFAULT_MAX_ASSET_BYTES,
            max_preview_bytes: DEFAULT_MAX_PREVIEW_BYTES,
            preview_media_seconds: 30,
            ffmpeg_bin: "ffmpeg".into(),
            pdftoppm_bin: "pdftoppm".into(),
            gs_bin: "gs".into(),
            mutool_bin: "mutool".into(),
            escrow_size_threshold: DEFAULT_ESCROW_SIZE_THRESHOLD_BYTES,
            platform_fee_bps: 0,
            platform_fee_wallet: None,
            oracle_authorities: oracles,
            oracle_profile_id: "x402/oracles/file-delivery/attestation/v1".into(),
            skip_seller_vault_check: true,
            skip_seller_auth: true,
            skip_buyer_auth: true,
            moderation: ModerationConfig {
                provider: ModerationProvider::None,
                openai_api_key: None,
                fail_closed: false,
            },
            cors_allowed_origins: vec!["http://localhost:5175".into()],
            object_delivery: ObjectDelivery::Proxy,
            presign_ttl_secs: 300,
            version: "0.1.0".into(),
            leaderboard_limit: 5,
        }
    }

    async fn test_state(oracles: Vec<String>) -> (SharedState, tempfile::TempDir) {
        test_state_with_facilitator(oracles, "http://127.0.0.1:9".into()).await
    }

    async fn test_state_with_facilitator(
        oracles: Vec<String>,
        facilitator: String,
    ) -> (SharedState, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tmp");
        let db_path = tmp.path().join(format!("forge-{}.db", Uuid::new_v4()));
        let objects = tmp.path().join("objects");
        let db = Database::connect(&format!("sqlite:{}", db_path.display()))
            .await
            .expect("db");
        let config = test_config(objects, facilitator, oracles);
        let state = Arc::new(
            AppState::build(
                config,
                crate::config::ClusterConfig::for_cluster(SolanaCluster::Devnet),
                db,
            )
            .await
            .expect("state"),
        );
        (state, tmp)
    }

    async fn spawn_facilitator_mock() -> String {
        use axum::routing::{get, post};
        use axum::{Json, Router};
        let app = Router::new()
            .route(
                "/api/v1/facilitator/supported",
                get(|| async {
                    Json(json!({
                        "kinds": [
                            {"scheme": "exact", "network": "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1", "extra": {}},
                            {"scheme": "sla-escrow", "network": "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1", "extra": {}}
                        ]
                    }))
                }),
            )
            .route(
                "/api/v1/facilitator/sellers/{wallet}/rails/exact",
                get(|| async {
                    Json(json!({"vaultPda": "VaultPda11111111111111111111111111111111111"}))
                }),
            )
            .route(
                "/api/v1/facilitator/verify",
                post(|| async { Json(json!({"isValid": true, "payer": "Buyer"})) }),
            )
            .route(
                "/api/v1/facilitator/settle",
                post(|| async {
                    Json(json!({"success": true, "payer": "Buyer", "transaction": "sig"}))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    fn oracle_keypair() -> (ed25519_dalek::SigningKey, String) {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
        let wallet = bs58::encode(signing_key.verifying_key().to_bytes()).into_string();
        (signing_key, wallet)
    }

    fn escrow_listing(id: Uuid, hash: &str, _oracle: &str) -> ListingRow {
        ListingRow {
            id,
            seller_wallet: "SellerWallet111111111111111111111111111111".into(),
            display_name: None,
            title: "escrow file".into(),
            description: String::new(),
            category: "text".into(),
            price_micro_usdc: 50_000,
            preview_key: hash.to_string(),
            preview_content_type: "text/plain".into(),
            asset_key: hash.to_string(),
            content_type: "text/plain".into(),
            byte_size: 12,
            agent_friendly: false,
            delivery_scheme: "escrow".into(),
            status: "active".into(),
            tags: "[]".into(),
            license: None,
            content_hash: Some(hash.to_string()),
            moderation_status: "approved".into(),
            moderation_labels: "[]".into(),
            created_at: Utc::now(),
        }
    }

    fn oracle_headers(listing_id: Uuid, uid: &str, ts: i64, host: &str, key: &ed25519_dalek::SigningKey) -> HeaderMap {
        let msg = oracle_verdict_message(listing_id, uid, ts, host);
        let sig = bs58::encode(key.sign(msg.as_bytes()).to_bytes()).into_string();
        let mut headers = HeaderMap::new();
        headers.insert("x-forge-payment-uid", HeaderValue::from_str(uid).unwrap());
        headers.insert("x-forge-oracle-ts", HeaderValue::from_str(&ts.to_string()).unwrap());
        headers.insert("x-forge-oracle-sig", HeaderValue::from_str(&sig).unwrap());
        headers
    }

    #[tokio::test]
    async fn publish_uses_sha256_key_and_rejects_overwrite() {
        let (state, _tmp) = test_state(Vec::new()).await;
        let asset = Bytes::from_static(b"immutable-bytes");
        let hash = sha256_hex(&asset);
        let input = || PublishListingInput {
            seller_wallet: "SellerWallet111111111111111111111111111111".into(),
            display_name: None,
            title: "t".into(),
            description: String::new(),
            category: "text".into(),
            price_usdc: "0".into(),
            agent_friendly: false,
            tags_raw: String::new(),
            license: None,
            content_hash: None,
            asset_ct: "text/plain".into(),
            asset_data: asset.clone(),
            preview_bytes: None,
        };
        let row = publish_listing(&state, input(), Uuid::new_v4(), String::new(), false)
            .await
            .expect("first publish");
        assert_eq!(row.asset_key, hash);
        assert_eq!(row.content_hash.as_deref(), Some(hash.as_str()));
        let err = publish_listing(&state, input(), Uuid::new_v4(), String::new(), false)
            .await
            .expect_err("overwrite");
        assert!(matches!(err, AppError::Conflict(_)));
        state
            .storage
            .put_if_absent(&hash, "text/plain", asset.clone())
            .await
            .expect_err("put_if_absent existing key");
    }

    #[tokio::test]
    async fn sla_escrow_fund_persists_bind_row() {
        let (_, oracle) = oracle_keypair();
        let (state, _tmp) = test_state(vec![oracle.clone()]).await;
        let asset = Bytes::from_static(b"escrow-asset");
        let hash = sha256_hex(&asset);
        let id = Uuid::new_v4();
        state
            .storage
            .put_if_absent(&hash, "text/plain", asset)
            .await
            .unwrap();
        let listing = escrow_listing(id, &hash, &oracle);
        state.db.insert_listing(&listing).await.unwrap();
        let uid = "ab".repeat(32);
        let verify = json!({
            "isValid": true,
            "paymentUid": uid,
            "oracleAuthority": oracle,
        });
        let bind = persist_escrow_fund_bind(&state, &listing, &verify)
            .await
            .expect("bind");
        assert_eq!(bind.listing_id, id);
        assert_eq!(bind.payment_uid, uid);
        assert_eq!(bind.content_hash, hash);
        assert_eq!(bind.oracle_authority, oracle);
        let stored = state
            .db
            .get_escrow_fund_bind(id, &uid)
            .await
            .unwrap()
            .expect("row");
        assert_eq!(stored.oracle_authority, oracle);
    }

    #[tokio::test]
    async fn verdict_streams_object_without_402_or_sale() {
        let (key, oracle) = oracle_keypair();
        let (state, _tmp) = test_state(vec![oracle.clone()]).await;
        let asset = Bytes::from_static(b"verdict-bytes");
        let hash = sha256_hex(&asset);
        let id = Uuid::new_v4();
        state
            .storage
            .put_if_absent(&hash, "text/plain", asset.clone())
            .await
            .unwrap();
        let listing = escrow_listing(id, &hash, &oracle);
        state.db.insert_listing(&listing).await.unwrap();
        let uid = "cd".repeat(32);
        persist_escrow_fund_bind(
            &state,
            &listing,
            &json!({"paymentUid": uid, "oracleAuthority": oracle}),
        )
        .await
        .unwrap();
        let host = public_host_from_base_url(&state.config.seller_public_base_url);
        let ts = Utc::now().timestamp();
        let headers = oracle_headers(id, &uid, ts, &host, &key);
        let response = artifact(State(state.clone()), Path(id), headers)
            .await
            .expect("200");
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), b"verdict-bytes");
        assert!(state
            .db
            .find_buyer_sale_for_listing(id, "anyone")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn verdict_forbidden_without_signature_and_for_exact_listings() {
        let (key, oracle) = oracle_keypair();
        let (state, _tmp) = test_state(vec![oracle.clone()]).await;
        let asset = Bytes::from_static(b"exact-or-escrow");
        let hash = sha256_hex(&asset);
        let escrow_id = Uuid::new_v4();
        let exact_id = Uuid::new_v4();
        state
            .storage
            .put_if_absent(&hash, "text/plain", asset)
            .await
            .unwrap();
        let escrow = escrow_listing(escrow_id, &hash, &oracle);
        let mut exact = escrow_listing(exact_id, &hash, &oracle);
        exact.delivery_scheme = "exact".into();
        state.db.insert_listing(&escrow).await.unwrap();
        state.db.insert_listing(&exact).await.unwrap();
        let uid = "ee".repeat(32);
        persist_escrow_fund_bind(
            &state,
            &escrow,
            &json!({"paymentUid": uid, "oracleAuthority": oracle}),
        )
        .await
        .unwrap();
        persist_escrow_fund_bind(
            &state,
            &exact,
            &json!({"paymentUid": uid, "oracleAuthority": oracle}),
        )
        .await
        .unwrap();

        let missing = artifact(State(state.clone()), Path(escrow_id), HeaderMap::new())
            .await
            .expect_err("missing sig");
        assert_eq!(missing.into_response().status(), StatusCode::FORBIDDEN);

        let host = public_host_from_base_url(&state.config.seller_public_base_url);
        let ts = Utc::now().timestamp();
        let headers = oracle_headers(exact_id, &uid, ts, &host, &key);
        let exact_err = artifact(State(state), Path(exact_id), headers)
            .await
            .expect_err("exact");
        assert_eq!(exact_err.into_response().status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn payment_door_ignores_oracle_signatures() {
        let (key, oracle) = oracle_keypair();
        let facilitator = spawn_facilitator_mock().await;
        let (state, _tmp) = test_state_with_facilitator(vec![oracle.clone()], facilitator).await;
        let asset = Bytes::from_static(b"paid-asset");
        let hash = sha256_hex(&asset);
        let id = Uuid::new_v4();
        state
            .storage
            .put_if_absent(&hash, "text/plain", asset)
            .await
            .unwrap();
        let mut listing = escrow_listing(id, &hash, &oracle);
        listing.delivery_scheme = "exact".into();
        state.db.insert_listing(&listing).await.unwrap();
        let uid = "ff".repeat(32);
        let host = public_host_from_base_url(&state.config.seller_public_base_url);
        let ts = Utc::now().timestamp();
        let headers = oracle_headers(id, &uid, ts, &host, &key);
        let err = PaymentGate::check_download(
            &state,
            &headers,
            &listing,
            &format!("/api/v1/listings/{id}/download"),
        )
        .await
        .expect_err("oracle headers are not payment");
        assert!(matches!(err, AppError::PaymentRequired(_)));
    }

    #[tokio::test]
    async fn sla_escrow_fund_via_payment_gate_persists_bind() {
        let (_, oracle) = oracle_keypair();
        let uid = "11".repeat(32);
        let verify = json!({
            "isValid": true,
            "paymentUid": uid,
            "oracleAuthority": oracle,
            "payer": "BuyerWallet1111111111111111111111111111111"
        });
        use axum::routing::{get, post};
        use axum::{Json, Router};
        let verify_body = verify.clone();
        let app = Router::new()
            .route(
                "/api/v1/facilitator/supported",
                get(|| async {
                    Json(json!({
                        "kinds": [
                            {"scheme": "sla-escrow", "network": "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1", "extra": {}}
                        ]
                    }))
                }),
            )
            .route(
                "/api/v1/facilitator/verify",
                post(move |_b: Json<serde_json::Value>| {
                    let body = verify_body.clone();
                    async move { Json(body) }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let (state, _tmp) =
            test_state_with_facilitator(vec![oracle.clone()], format!("http://{addr}")).await;
        let asset = Bytes::from_static(b"funded-asset");
        let hash = sha256_hex(&asset);
        let id = Uuid::new_v4();
        state
            .storage
            .put_if_absent(&hash, "text/plain", asset)
            .await
            .unwrap();
        let listing = escrow_listing(id, &hash, &oracle);
        state.db.insert_listing(&listing).await.unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "payment-signature",
            HeaderValue::from_str(r#"{"paymentPayload":{"payload":{"transaction":"fund"}}}"#)
                .unwrap(),
        );
        let err = PaymentGate::check_download(
            &state,
            &headers,
            &listing,
            &format!("/api/v1/listings/{id}/download"),
        )
        .await
        .expect_err("fund does not unlock download");
        assert!(matches!(err, AppError::PaymentRequired(_)));
        let bind = state
            .db
            .get_escrow_fund_bind(id, &uid)
            .await
            .unwrap()
            .expect("bind row");
        assert_eq!(bind.listing_id, id);
        assert_eq!(bind.payment_uid, uid);
        assert_eq!(bind.content_hash, hash);
        assert_eq!(bind.oracle_authority, oracle);
        assert!(state
            .db
            .find_buyer_sale_for_listing(id, "anyone")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn ensure_exact_lane_upload_still_rejects_threshold() {
        let (state, _tmp) = test_state(Vec::new()).await;
        let err = crate::routes::listings::ensure_exact_lane_upload(
            &state,
            state.config.escrow_size_threshold,
        )
        .expect_err("threshold");
        assert!(matches!(err, AppError::BadRequest(_)));
    }
}

