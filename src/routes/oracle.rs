use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::oracle::{
    forge_public_host, has_oracle_artifact_headers, normalize_payment_uid, parse_oracle_ts,
    ts_in_replay_window, verdict_message, verify_oracle_signature, HEADER_ORACLE_SIG,
    HEADER_ORACLE_TS, HEADER_PAYMENT_UID, REPLAY_WINDOW_SECS,
};
use crate::state::SharedState;
use crate::storage::{serve_object, DeliveryFormat, ObjectServeOptions, ObjectStore};

fn verdict_denied() -> AppError {
    AppError::Forbidden("oracle verdict denied".into())
}

fn header_str<'a>(headers: &'a HeaderMap, name: &'static str) -> AppResult<&'a str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(verdict_denied)
}

/// GET /api/v1/oracle/listings/{id}/artifact — verdict door (no 402, no sales row).
pub async fn artifact(
    State(state): State<SharedState>,
    Path(listing_id): Path<Uuid>,
    headers: HeaderMap,
) -> AppResult<Response> {
    if !has_oracle_artifact_headers(&headers) {
        return Err(verdict_denied());
    }
    let payment_uid = normalize_payment_uid(header_str(&headers, HEADER_PAYMENT_UID)?)?;
    let ts = parse_oracle_ts(header_str(&headers, HEADER_ORACLE_TS)?)?;
    let sig = header_str(&headers, HEADER_ORACLE_SIG)?;

    let now = chrono::Utc::now().timestamp();
    if !ts_in_replay_window(ts, now) {
        tracing::info!(
            listing_id = %listing_id,
            ts,
            now,
            window = REPLAY_WINDOW_SECS,
            "oracle verdict replay window missed"
        );
        return Err(verdict_denied());
    }

    let listing = state
        .db
        .get_listing_any(listing_id)
        .await
        .map_err(|_| verdict_denied())?;
    if listing.delivery_scheme != "escrow" {
        return Err(verdict_denied());
    }

    let bind = state
        .db
        .get_escrow_fund_bind(listing_id, &payment_uid)
        .await?
        .ok_or_else(verdict_denied)?;
    if bind.listing_id != listing_id {
        return Err(verdict_denied());
    }

    let host = forge_public_host(&state.config.seller_public_base_url);
    let message = verdict_message(listing_id, &payment_uid, ts, &host);
    verify_oracle_signature(&bind.oracle_authority, &message, sig)?;

    let Some(content_hash) = listing.content_hash.clone() else {
        return Err(verdict_denied());
    };
    if bind.content_hash != content_hash {
        return Err(verdict_denied());
    }

    let key = listing.asset_key.clone();
    let content_type = state.storage.head(&key).await.map_err(|_| verdict_denied())?;

    serve_object(
        &state,
        ObjectServeOptions {
            key: &key,
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
    use axum::extract::{Path, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use bytes::Bytes;
    use chrono::Utc;
    use ed25519_dalek::Signer;
    use uuid::Uuid;

    use crate::db::ListingRow;
    use crate::error::AppError;
    use crate::oracle::{
        forge_public_host, verdict_message, HEADER_ORACLE_SIG, HEADER_ORACLE_TS, HEADER_PAYMENT_UID,
    };
    use crate::storage::{content_hash_hex, content_object_key, ObjectStore};
    use crate::test_harness::TestEnv;

    fn escrow_listing(id: Uuid, content_hash: &str, asset_key: &str, byte_size: i64) -> ListingRow {
        ListingRow {
            id,
            seller_wallet: bs58::encode([5u8; 32]).into_string(),
            display_name: None,
            title: "escrow item".into(),
            description: String::new(),
            category: "text".into(),
            price_micro_usdc: 50_000,
            preview_key: "previews/x".into(),
            preview_content_type: "text/plain".into(),
            asset_key: asset_key.into(),
            content_type: "text/plain".into(),
            byte_size,
            agent_friendly: false,
            delivery_scheme: "escrow".into(),
            status: "active".into(),
            tags: "[]".into(),
            license: None,
            content_hash: Some(content_hash.into()),
            moderation_status: "approved".into(),
            moderation_labels: "[]".into(),
            created_at: Utc::now(),
        }
    }

    fn signed_headers(
        listing_id: Uuid,
        payment_uid: &str,
        host: &str,
        signing_key: &ed25519_dalek::SigningKey,
        ts: i64,
    ) -> HeaderMap {
        let message = verdict_message(listing_id, payment_uid, ts, host);
        let sig = signing_key.sign(message.as_bytes());
        let sig_b58 = bs58::encode(sig.to_bytes()).into_string();
        let mut headers = HeaderMap::new();
        headers.insert(HEADER_PAYMENT_UID, payment_uid.parse().unwrap());
        headers.insert(HEADER_ORACLE_TS, ts.to_string().parse().unwrap());
        headers.insert(HEADER_ORACLE_SIG, sig_b58.parse().unwrap());
        headers
    }

    #[tokio::test]
    async fn oracle_verdict_streams_without_402_or_sale() {
        let env = TestEnv::new().await;
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[21u8; 32]);
        let oracle_authority = bs58::encode(signing_key.verifying_key().to_bytes()).into_string();
        let bytes = Bytes::from_static(b"oracle-verdict-bytes");
        let hash = content_hash_hex(&bytes);
        let key = content_object_key(&hash);
        env.state
            .storage
            .put(&key, "text/plain", bytes.clone())
            .await
            .unwrap();

        let listing_id = Uuid::new_v4();
        let listing = escrow_listing(listing_id, &hash, &key, bytes.len() as i64);
        env.state.db.insert_listing(&listing).await.unwrap();
        let payment_uid = "ab".repeat(32);
        env.state
            .db
            .persist_escrow_fund_bind(&listing, &payment_uid, &oracle_authority)
            .await
            .unwrap();

        let host = forge_public_host(&env.state.config.seller_public_base_url);
        let ts = Utc::now().timestamp();
        let headers = signed_headers(listing_id, &payment_uid, &host, &signing_key, ts);
        let response = artifact(State(env.state.clone()), Path(listing_id), headers)
            .await
            .expect("verdict stream")
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("x-forge-sale-id").is_none());
        assert!(response.headers().get("payment-response").is_none());
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], &bytes[..]);
        assert_eq!(env.state.db.sales_count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn oracle_verdict_forbidden_without_signature() {
        let env = TestEnv::new().await;
        let listing_id = Uuid::new_v4();
        let listing = escrow_listing(listing_id, &"cd".repeat(32), "assets/x", 1);
        env.state.db.insert_listing(&listing).await.unwrap();
        let err = artifact(State(env.state), Path(listing_id), HeaderMap::new())
            .await
            .unwrap_err();
        match err {
            AppError::Forbidden(_) => {}
            other => panic!("expected 403 forbidden, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn exact_rail_listing_never_opens_verdict() {
        let env = TestEnv::new().await;
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[23u8; 32]);
        let oracle_authority = bs58::encode(signing_key.verifying_key().to_bytes()).into_string();
        let bytes = Bytes::from_static(b"exact-rail");
        let hash = content_hash_hex(&bytes);
        let key = content_object_key(&hash);
        env.state
            .storage
            .put(&key, "text/plain", bytes.clone())
            .await
            .unwrap();

        let listing_id = Uuid::new_v4();
        let mut listing = escrow_listing(listing_id, &hash, &key, bytes.len() as i64);
        listing.delivery_scheme = "exact".into();
        env.state.db.insert_listing(&listing).await.unwrap();
        let payment_uid = "11".repeat(32);
        assert!(env
            .state
            .db
            .persist_escrow_fund_bind(&listing, &payment_uid, &oracle_authority)
            .await
            .is_err());

        let host = forge_public_host(&env.state.config.seller_public_base_url);
        let ts = Utc::now().timestamp();
        let headers = signed_headers(listing_id, &payment_uid, &host, &signing_key, ts);
        let err = artifact(State(env.state), Path(listing_id), headers)
            .await
            .unwrap_err();
        match err {
            AppError::Forbidden(_) => {}
            other => panic!("exact-rail must not open verdict, got {other:?}"),
        }
    }
}
