//! Oracle verdict door: signature-verified stream of the same listing object.
//! Not a sale and not a 402 payment door.

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
};
use chrono::Utc;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use uuid::Uuid;

use crate::db::{EscrowFundBind, ListingRow};
use crate::error::{AppError, AppResult};
use crate::state::SharedState;
use crate::storage::{asset_content_key, serve_object, ObjectServeOptions, ObjectStore};

const ORACLE_MESSAGE_PREFIX: &str = "forge-oracle-v1";
const REPLAY_WINDOW_SECS: i64 = 60;

fn verdict_denied() -> AppError {
    AppError::Forbidden("oracle verdict denied".into())
}

pub(crate) fn oracle_verdict_message(
    listing_id: Uuid,
    payment_uid_hex: &str,
    ts: i64,
    host: &str,
) -> String {
    format!("{ORACLE_MESSAGE_PREFIX}|{listing_id}|{payment_uid_hex}|{ts}|{host}")
}

pub(crate) fn public_api_host(base_url: &str) -> Option<String> {
    let url = reqwest::Url::parse(base_url).ok()?;
    let host = url.host_str()?;
    match url.port() {
        Some(port) => Some(format!("{host}:{port}")),
        None => Some(host.to_string()),
    }
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim)
}

fn normalize_payment_uid(raw: &str) -> Option<String> {
    let hex = raw.trim().strip_prefix("0x").unwrap_or(raw.trim());
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(hex.to_ascii_lowercase())
}

fn listing_is_escrow(listing: &ListingRow) -> bool {
    listing.delivery_scheme == "escrow"
}

fn verify_oracle_ed25519(authority: &str, message: &str, sig_b58: &str) -> bool {
    let Ok(pubkey_bytes) = bs58::decode(authority).into_vec() else {
        return false;
    };
    let Ok(pubkey_array) = <[u8; 32]>::try_from(pubkey_bytes) else {
        return false;
    };
    let Ok(verifying_key) = VerifyingKey::from_bytes(&pubkey_array) else {
        return false;
    };
    let Ok(sig_bytes) = bs58::decode(sig_b58.trim()).into_vec() else {
        return false;
    };
    let Ok(sig_array) = <[u8; 64]>::try_from(sig_bytes) else {
        return false;
    };
    let signature = Signature::from_bytes(&sig_array);
    verifying_key.verify(message.as_bytes(), &signature).is_ok()
}

pub(crate) fn authorize_oracle_verdict(
    listing: &ListingRow,
    bind: Option<&EscrowFundBind>,
    payment_uid: &str,
    ts: i64,
    sig_b58: &str,
    host: &str,
    now_unix: i64,
) -> Result<EscrowFundBind, AppError> {
    if !listing_is_escrow(listing) {
        return Err(AppError::Forbidden(
            "exact-rail listings never open the verdict door".into(),
        ));
    }
    let Some(bind) = bind else {
        return Err(verdict_denied());
    };
    if bind.listing_id != listing.id || bind.payment_uid != payment_uid {
        return Err(verdict_denied());
    }
    if (now_unix - ts).abs() > REPLAY_WINDOW_SECS {
        return Err(AppError::Forbidden("oracle timestamp outside replay window".into()));
    }
    let message = oracle_verdict_message(listing.id, payment_uid, ts, host);
    if !verify_oracle_ed25519(&bind.oracle_authority, &message, sig_b58) {
        return Err(AppError::Forbidden("invalid oracle signature".into()));
    }
    Ok(bind.clone())
}

pub async fn artifact(
    State(state): State<SharedState>,
    Path(listing_id): Path<Uuid>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let Some(payment_uid) =
        header_str(&headers, "x-forge-payment-uid").and_then(normalize_payment_uid)
    else {
        return Err(verdict_denied());
    };
    let Some(ts) = header_str(&headers, "x-forge-oracle-ts")
        .and_then(|s| s.parse::<i64>().ok())
    else {
        return Err(verdict_denied());
    };
    let Some(sig) = header_str(&headers, "x-forge-oracle-sig").filter(|s| !s.is_empty()) else {
        return Err(verdict_denied());
    };
    let Some(host) = public_api_host(&state.config.seller_public_base_url) else {
        return Err(verdict_denied());
    };

    let listing = match state.db.get_listing_any(listing_id).await {
        Ok(row) => row,
        Err(AppError::NotFound) => return Err(verdict_denied()),
        Err(e) => return Err(e),
    };
    let bind = state
        .db
        .get_escrow_fund_bind(listing_id, &payment_uid)
        .await?;
    let bind = authorize_oracle_verdict(
        &listing,
        bind.as_ref(),
        &payment_uid,
        ts,
        sig,
        &host,
        Utc::now().timestamp(),
    )?;

    let key = if !listing.asset_key.is_empty() {
        listing.asset_key.clone()
    } else {
        asset_content_key(&bind.content_hash)
    };
    let content_type = match state.storage.head(&key).await {
        Ok(ct) => ct,
        Err(AppError::NotFound) => return Err(verdict_denied()),
        Err(e) => return Err(e),
    };

    serve_object(
        &state,
        ObjectServeOptions {
            key: &key,
            content_type: &content_type,
            content_disposition: None,
            extra_headers: HeaderMap::new(),
            format: crate::storage::DeliveryFormat::Proxy,
            sale_id: None,
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, ListingRow};
    use crate::routes::router;
    use crate::state::test_support::test_state;
    use axum::body::{to_bytes, Body};
    use axum::extract::ConnectInfo;
    use axum::http::{Request, StatusCode};
    use bytes::Bytes;
    use chrono::Utc;
    use ed25519_dalek::{Signer, SigningKey};
    use std::net::SocketAddr;
    use tower::ServiceExt;
    use uuid::Uuid;

    fn escrow_listing(id: Uuid, hash: &str, scheme: &str, asset_key: &str) -> ListingRow {
        ListingRow {
            id,
            seller_wallet: "SellerWallet111111111111111111111111111111".into(),
            display_name: None,
            title: "escrow file".into(),
            description: String::new(),
            category: "art".into(),
            price_micro_usdc: 50_000,
            preview_key: "previews/x".into(),
            preview_content_type: "text/plain".into(),
            asset_key: asset_key.into(),
            content_type: "application/octet-stream".into(),
            byte_size: 4,
            agent_friendly: false,
            delivery_scheme: scheme.into(),
            status: "active".into(),
            tags: "[]".into(),
            license: None,
            content_hash: Some(hash.into()),
            moderation_status: "approved".into(),
            moderation_labels: "[]".into(),
            created_at: Utc::now(),
        }
    }

    fn oracle_keypair() -> (SigningKey, String) {
        let signing_key = SigningKey::from_bytes(&[9u8; 32]);
        let wallet = bs58::encode(signing_key.verifying_key().to_bytes()).into_string();
        (signing_key, wallet)
    }

    #[test]
    fn authorize_rejects_exact_rail_even_with_bind() {
        let id = Uuid::new_v4();
        let uid = "ab".repeat(32);
        let listing = escrow_listing(id, "deadbeef", "exact", "assets/deadbeef");
        let bind = EscrowFundBind {
            listing_id: id,
            payment_uid: uid.clone(),
            content_hash: "deadbeef".into(),
            oracle_authority: "oracle".into(),
        };
        let err = authorize_oracle_verdict(&listing, Some(&bind), &uid, 1, "sig", "host", 1)
            .unwrap_err();
        match err {
            AppError::Forbidden(msg) => {
                assert!(msg.contains("exact-rail"));
            }
            other => panic!("expected forbidden, got {other:?}"),
        }
    }

    #[test]
    fn authorize_rejects_missing_signature_bind_mismatch_and_stale_ts() {
        let id = Uuid::new_v4();
        let uid = "cd".repeat(32);
        let listing = escrow_listing(id, "aa", "escrow", "assets/aa");
        assert!(authorize_oracle_verdict(&listing, None, &uid, 1, "sig", "host", 1).is_err());

        let bind = EscrowFundBind {
            listing_id: id,
            payment_uid: uid.clone(),
            content_hash: "aa".into(),
            oracle_authority: "oracle".into(),
        };
        assert!(authorize_oracle_verdict(
            &listing,
            Some(&bind),
            &uid,
            1,
            "not-a-sig",
            "host",
            1
        )
        .is_err());
        assert!(authorize_oracle_verdict(
            &listing,
            Some(&bind),
            &uid,
            1,
            "sig",
            "host",
            1 + REPLAY_WINDOW_SECS + 1
        )
        .is_err());
    }

    #[tokio::test]
    async fn verdict_streams_bound_escrow_object_without_sale_or_402() {
        let (signing_key, oracle) = oracle_keypair();
        let (state, _tmp) = test_state(Some(&oracle)).await;
        let asset = Bytes::from_static(b"abcd");
        let hash = crate::storage::content_sha256_hex(&asset);
        let key = asset_content_key(&hash);
        state
            .storage
            .put_if_absent(&key, "application/octet-stream", asset.clone())
            .await
            .unwrap();

        let listing_id = Uuid::new_v4();
        state
            .db
            .insert_listing(&escrow_listing(listing_id, &hash, "escrow", &key))
            .await
            .unwrap();
        let uid = "11".repeat(32);
        state
            .db
            .upsert_escrow_fund_bind(listing_id, &uid, &hash, &oracle)
            .await
            .unwrap();

        let host = public_api_host(&state.config.seller_public_base_url).unwrap();
        let ts = Utc::now().timestamp();
        let message = oracle_verdict_message(listing_id, &uid, ts, &host);
        let sig = bs58::encode(signing_key.sign(message.as_bytes()).to_bytes()).into_string();

        let app = router(state.clone());
        let response = app
            .oneshot(oracle_request(
                listing_id,
                &uid,
                ts,
                &sig,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("x-forge-sale-id").is_none());
        assert!(response.headers().get("payment-response").is_none());
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        assert_eq!(body.as_ref(), asset.as_ref());
        assert_eq!(count_sales(&state.db).await, 0);
    }

    #[tokio::test]
    async fn verdict_returns_403_without_valid_signature() {
        let (_signing_key, oracle) = oracle_keypair();
        let (state, _tmp) = test_state(Some(&oracle)).await;
        let listing_id = Uuid::new_v4();
        let hash = "aa".repeat(32);
        state
            .db
            .insert_listing(&escrow_listing(
                listing_id,
                &hash,
                "escrow",
                &asset_content_key(&hash),
            ))
            .await
            .unwrap();

        let app = router(state);
        let response = app
            .oneshot(
                with_connect_info(
                    Request::builder()
                        .uri(format!("/api/v1/oracle/listings/{listing_id}/artifact"))
                        .body(Body::empty())
                        .unwrap(),
                ),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn exact_rail_listing_never_opens_verdict_route() {
        let (signing_key, oracle) = oracle_keypair();
        let (state, _tmp) = test_state(Some(&oracle)).await;
        let listing_id = Uuid::new_v4();
        let hash = "bb".repeat(32);
        let key = asset_content_key(&hash);
        state
            .db
            .insert_listing(&escrow_listing(listing_id, &hash, "exact", &key))
            .await
            .unwrap();
        let uid = "22".repeat(32);
        state
            .db
            .upsert_escrow_fund_bind(listing_id, &uid, &hash, &oracle)
            .await
            .unwrap();
        let host = public_api_host(&state.config.seller_public_base_url).unwrap();
        let ts = Utc::now().timestamp();
        let message = oracle_verdict_message(listing_id, &uid, ts, &host);
        let sig = bs58::encode(signing_key.sign(message.as_bytes()).to_bytes()).into_string();

        let app = router(state);
        let response = app
            .oneshot(oracle_request(listing_id, &uid, ts, &sig))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("exact-rail"));
    }

    #[tokio::test]
    async fn payment_door_does_not_accept_oracle_signatures() {
        let (signing_key, oracle) = oracle_keypair();
        let (state, _tmp) = test_state(Some(&oracle)).await;
        let listing_id = Uuid::new_v4();
        let hash = "cc".repeat(32);
        state
            .db
            .insert_listing(&escrow_listing(
                listing_id,
                &hash,
                "exact",
                &asset_content_key(&hash),
            ))
            .await
            .unwrap();
        let uid = "33".repeat(32);
        let host = public_api_host(&state.config.seller_public_base_url).unwrap();
        let ts = Utc::now().timestamp();
        let message = oracle_verdict_message(listing_id, &uid, ts, &host);
        let sig = bs58::encode(signing_key.sign(message.as_bytes()).to_bytes()).into_string();

        let app = router(state);
        let response = app
            .oneshot(
                with_connect_info(
                    Request::builder()
                        .uri(format!("/api/v1/listings/{listing_id}/download"))
                        .header("x-forge-payment-uid", &uid)
                        .header("x-forge-oracle-ts", ts.to_string())
                        .header("x-forge-oracle-sig", sig)
                        .body(Body::empty())
                        .unwrap(),
                ),
            )
            .await
            .unwrap();
        assert_ne!(
            response.status(),
            StatusCode::OK,
            "oracle headers must not unlock the payment door"
        );
        assert!(
            response.status() == StatusCode::PAYMENT_REQUIRED
                || response.status() == StatusCode::SERVICE_UNAVAILABLE,
            "payment door must reject oracle signatures as payment, got {}",
            response.status()
        );
        assert!(response.headers().get("x-forge-sale-id").is_none());
    }

    async fn count_sales(db: &Database) -> i64 {
        db.count_sales().await.expect("count sales")
    }

    fn oracle_request(
        listing_id: Uuid,
        uid: &str,
        ts: i64,
        sig: &str,
    ) -> Request<Body> {
        with_connect_info(
            Request::builder()
                .uri(format!("/api/v1/oracle/listings/{listing_id}/artifact"))
                .header("x-forge-payment-uid", uid)
                .header("x-forge-oracle-ts", ts.to_string())
                .header("x-forge-oracle-sig", sig)
                .body(Body::empty())
                .unwrap(),
        )
    }

    fn with_connect_info(mut req: Request<Body>) -> Request<Body> {
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
        req
    }
}
