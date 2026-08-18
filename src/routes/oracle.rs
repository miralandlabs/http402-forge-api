use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::Response,
};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::oracle::{
    forge_public_host, oracle_verdict_message, ts_in_replay_window, unix_now_secs,
    verify_oracle_signature,
};
use crate::state::SharedState;
use crate::storage::{
    content_addressed_asset_key, serve_object, DeliveryFormat, DeliveryQuery, ObjectServeOptions,
    ObjectStore,
};
use crate::x402::normalize_payment_uid;

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim)
}

fn forbidden() -> AppError {
    AppError::Forbidden("oracle verdict denied".into())
}

pub async fn artifact(
    State(state): State<SharedState>,
    Path(listing_id): Path<Uuid>,
    Query(_delivery_q): Query<DeliveryQuery>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let payment_uid = header_str(&headers, "x-forge-payment-uid")
        .and_then(|raw| normalize_payment_uid(raw))
        .ok_or_else(forbidden)?;
    let ts = header_str(&headers, "x-forge-oracle-ts")
        .and_then(|s| s.parse::<i64>().ok())
        .ok_or_else(forbidden)?;
    let sig = header_str(&headers, "x-forge-oracle-sig").ok_or_else(forbidden)?;
    if !ts_in_replay_window(ts, unix_now_secs()) {
        return Err(forbidden());
    }

    let listing = state.db.get_listing_any(listing_id).await.map_err(|_| forbidden())?;
    if listing.delivery_scheme != "escrow" {
        return Err(forbidden());
    }

    let bind = state
        .db
        .get_escrow_fund_bind(listing_id, &payment_uid)
        .await?
        .ok_or_else(forbidden)?;
    let listing_hash = listing
        .content_hash
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(forbidden)?;
    if bind.content_hash != listing_hash {
        return Err(forbidden());
    }

    let host = forge_public_host(&state.config.seller_public_base_url);
    let message = oracle_verdict_message(&listing_id.to_string(), &payment_uid, ts, &host);
    verify_oracle_signature(&bind.oracle_authority, &message, sig).map_err(|_| forbidden())?;

    let key = if listing.asset_key.is_empty() {
        content_addressed_asset_key(listing_hash)
    } else {
        listing.asset_key.clone()
    };
    let content_type = state.storage.head(&key).await.map_err(|_| forbidden())?;

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
    use std::net::SocketAddr;
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::Json;
    use bytes::Bytes;
    use chrono::Utc;
    use ed25519_dalek::Signer;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use tokio::net::TcpListener;
    use uuid::Uuid;

    use super::*;
    use crate::db::ListingRow;
    use crate::oracle::{forge_public_host, oracle_verdict_message};
    use crate::routes::listings::{publish_listing, PublishListingInput};
    use crate::state::test_support::test_state;
    use crate::storage::{content_addressed_asset_key, ObjectStore};

    const HOST: &str = "preview.forge.http402.trade";
    const SELLER_BASE: &str = "https://preview.forge.http402.trade";
    const PAYER: &str = "BuyerWallet1111111111111111111111111111111";

    fn now_ts() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    fn oracle_keypair() -> (ed25519_dalek::SigningKey, String) {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[17u8; 32]);
        let authority = bs58::encode(signing_key.verifying_key().to_bytes()).into_string();
        (signing_key, authority)
    }

    fn payment_uid() -> String {
        "ab".repeat(32)
    }

    fn listing_row(
        id: Uuid,
        scheme: &str,
        asset_key: &str,
        content_hash: &str,
        byte_size: i64,
    ) -> ListingRow {
        ListingRow {
            id,
            seller_wallet: "SellerWallet111111111111111111111111111111".into(),
            display_name: None,
            title: "escrow asset".into(),
            description: String::new(),
            category: "text".into(),
            price_micro_usdc: 50_000,
            preview_key: "previews/x/preview.txt".into(),
            preview_content_type: "text/plain".into(),
            asset_key: asset_key.into(),
            content_type: "text/plain".into(),
            byte_size,
            agent_friendly: false,
            delivery_scheme: scheme.into(),
            status: "active".into(),
            tags: "[]".into(),
            license: None,
            content_hash: Some(content_hash.into()),
            moderation_status: "approved".into(),
            moderation_labels: "[]".into(),
            created_at: Utc::now(),
        }
    }

    async fn spawn_app(state: crate::state::SharedState) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let app = crate::routes::router(state);
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .ok();
        });
        format!("http://{addr}")
    }

    async fn wait_ready(base: &str) {
        let client = reqwest::Client::new();
        for _ in 0..50 {
            if client
                .get(format!("{base}/openapi.yaml"))
                .send()
                .await
                .is_ok()
            {
                return;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
        }
        panic!("server did not become ready");
    }

    fn sign_verdict(
        signing_key: &ed25519_dalek::SigningKey,
        listing_id: Uuid,
        payment_uid: &str,
        ts: i64,
    ) -> String {
        let message = oracle_verdict_message(&listing_id.to_string(), payment_uid, ts, HOST);
        bs58::encode(signing_key.sign(message.as_bytes()).to_bytes()).into_string()
    }

    #[test]
    fn public_host_matches_preview() {
        assert_eq!(forge_public_host(SELLER_BASE), HOST);
    }

    #[tokio::test]
    async fn publish_uses_sha256_key_and_rejects_overwrite() {
        let (state, _dir) = test_state("http://127.0.0.1:9".into(), SELLER_BASE.into(), vec![]).await;
        let asset = Bytes::from_static(b"immutable-bytes");
        let hash = format!("{:x}", Sha256::digest(&asset));
        let expected_key = content_addressed_asset_key(&hash);

        let row = publish_listing(
            &state,
            PublishListingInput {
                seller_wallet: "SellerWallet111111111111111111111111111111".into(),
                display_name: None,
                title: "first".into(),
                description: String::new(),
                category: "text".into(),
                price_usdc: "0.05".into(),
                agent_friendly: false,
                tags_raw: String::new(),
                license: None,
                content_hash: None,
                asset_ct: "text/plain".into(),
                asset_data: asset.clone(),
                preview_bytes: None,
            },
            Uuid::new_v4(),
            String::new(),
            false,
        )
        .await
        .expect("first publish");
        assert_eq!(row.content_hash.as_deref(), Some(hash.as_str()));
        assert_eq!(row.asset_key, expected_key);

        let err = publish_listing(
            &state,
            PublishListingInput {
                seller_wallet: "SellerWallet111111111111111111111111111111".into(),
                display_name: None,
                title: "second".into(),
                description: String::new(),
                category: "text".into(),
                price_usdc: "0.05".into(),
                agent_friendly: false,
                tags_raw: String::new(),
                license: None,
                content_hash: None,
                asset_ct: "text/plain".into(),
                asset_data: asset,
                preview_bytes: None,
            },
            Uuid::new_v4(),
            String::new(),
            false,
        )
        .await
        .expect_err("overwrite");
        assert!(matches!(err, AppError::Conflict(_)));
    }

    #[tokio::test]
    async fn verdict_streams_bound_escrow_object_without_sale() {
        let (signing_key, authority) = oracle_keypair();
        let (state, _dir) = test_state(
            "http://127.0.0.1:9".into(),
            SELLER_BASE.into(),
            vec![authority.clone()],
        )
        .await;
        let bytes = Bytes::from_static(b"verdict-object");
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let key = content_addressed_asset_key(&hash);
        state
            .storage
            .put(&key, "text/plain", bytes.clone())
            .await
            .expect("put");
        let listing_id = Uuid::new_v4();
        state
            .db
            .insert_listing(&listing_row(listing_id, "escrow", &key, &hash, bytes.len() as i64))
            .await
            .expect("listing");
        let uid = payment_uid();
        state
            .db
            .insert_escrow_fund_bind(listing_id, &uid, &hash, &authority)
            .await
            .expect("bind");

        let base = spawn_app(state.clone()).await;
        wait_ready(&base).await;
        let ts = now_ts();
        let sig = sign_verdict(&signing_key, listing_id, &uid, ts);
        let res = reqwest::Client::new()
            .get(format!("{base}/api/v1/oracle/listings/{listing_id}/artifact"))
            .header("X-Forge-Payment-Uid", &uid)
            .header("X-Forge-Oracle-Ts", ts.to_string())
            .header("X-Forge-Oracle-Sig", sig)
            .send()
            .await
            .expect("request");
        assert_eq!(res.status(), 200);
        assert!(res.headers().get("x-forge-sale-id").is_none());
        let body = res.bytes().await.expect("body");
        assert_eq!(&body[..], &bytes[..]);
        assert!(state
            .db
            .find_buyer_sale_for_listing(listing_id, PAYER)
            .await
            .expect("sales lookup")
            .is_none());
    }

    #[tokio::test]
    async fn verdict_forbidden_without_signature_and_for_exact_listings() {
        let (signing_key, authority) = oracle_keypair();
        let (state, _dir) = test_state(
            "http://127.0.0.1:9".into(),
            SELLER_BASE.into(),
            vec![authority.clone()],
        )
        .await;
        let bytes = Bytes::from_static(b"exact-object");
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let key = content_addressed_asset_key(&hash);
        state
            .storage
            .put(&key, "text/plain", bytes.clone())
            .await
            .expect("put");
        let escrow_id = Uuid::new_v4();
        let exact_id = Uuid::new_v4();
        state
            .db
            .insert_listing(&listing_row(escrow_id, "escrow", &key, &hash, bytes.len() as i64))
            .await
            .expect("escrow listing");
        state
            .db
            .insert_listing(&listing_row(exact_id, "exact", &key, &hash, bytes.len() as i64))
            .await
            .expect("exact listing");
        let uid = payment_uid();
        state
            .db
            .insert_escrow_fund_bind(escrow_id, &uid, &hash, &authority)
            .await
            .expect("bind");
        state
            .db
            .insert_escrow_fund_bind(exact_id, &uid, &hash, &authority)
            .await
            .ok();

        let base = spawn_app(state).await;
        wait_ready(&base).await;
        let client = reqwest::Client::new();
        let unsigned = client
            .get(format!("{base}/api/v1/oracle/listings/{escrow_id}/artifact"))
            .send()
            .await
            .expect("unsigned");
        assert_eq!(unsigned.status(), 403);

        let ts = now_ts();
        let sig = sign_verdict(&signing_key, exact_id, &uid, ts);
        let exact = client
            .get(format!("{base}/api/v1/oracle/listings/{exact_id}/artifact"))
            .header("X-Forge-Payment-Uid", &uid)
            .header("X-Forge-Oracle-Ts", ts.to_string())
            .header("X-Forge-Oracle-Sig", sig)
            .send()
            .await
            .expect("exact");
        assert_eq!(exact.status(), 403);
    }

    #[tokio::test]
    async fn payment_door_rejects_oracle_signature_as_payment() {
        let (signing_key, authority) = oracle_keypair();
        let uid = payment_uid();
        let facilitator = spawn_facilitator(uid.clone(), authority.clone()).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(30)).await;
        let (state, _dir) = test_state(facilitator, SELLER_BASE.into(), vec![authority.clone()]).await;
        let bytes = Bytes::from_static(b"paid-object");
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let key = content_addressed_asset_key(&hash);
        state
            .storage
            .put(&key, "text/plain", bytes)
            .await
            .expect("put");
        let listing_id = Uuid::new_v4();
        state
            .db
            .insert_listing(&listing_row(listing_id, "exact", &key, &hash, 12))
            .await
            .expect("listing");
        let uid = payment_uid();
        let ts = now_ts();
        let sig = sign_verdict(&signing_key, listing_id, &uid, ts);

        let base = spawn_app(state).await;
        wait_ready(&base).await;
        let res = reqwest::Client::new()
            .get(format!("{base}/api/v1/listings/{listing_id}/download"))
            .header("X-Forge-Payment-Uid", &uid)
            .header("X-Forge-Oracle-Ts", ts.to_string())
            .header("X-Forge-Oracle-Sig", sig)
            .send()
            .await
            .expect("request");
        assert_eq!(res.status(), 402);
        assert!(res.headers().get("x-forge-sale-id").is_none());
    }

    async fn spawn_facilitator(uid: String, oracle_authority: String) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let kinds = json!({
            "kinds": [
                {
                    "scheme": "exact",
                    "network": crate::config::SOLANA_DEVNET_NETWORK,
                    "extra": {}
                },
                {
                    "scheme": "sla-escrow",
                    "network": crate::config::SOLANA_DEVNET_NETWORK,
                    "extra": {}
                }
            ]
        });
        let app = axum::Router::new()
            .route(
                "/api/v1/facilitator/supported",
                axum::routing::get({
                    let kinds = kinds.clone();
                    move || {
                        let kinds = kinds.clone();
                        async move { Json(kinds) }
                    }
                }),
            )
            .route(
                "/api/v1/facilitator/sellers/{wallet}/rails/exact",
                axum::routing::get(|| async {
                    Json(json!({ "vaultPda": "VaultPda111111111111111111111111111111111" }))
                }),
            )
            .route(
                "/api/v1/facilitator/verify",
                axum::routing::post({
                    let uid = uid.clone();
                    let oracle_authority = oracle_authority.clone();
                    move || {
                        let uid = uid.clone();
                        let oracle_authority = oracle_authority.clone();
                        async move {
                            Json(json!({
                                "isValid": true,
                                "payer": PAYER,
                                "paymentUid": uid,
                                "oracleAuthority": oracle_authority
                            }))
                        }
                    }
                }),
            )
            .route(
                "/api/v1/facilitator/settle",
                axum::routing::post({
                    let uid = uid.clone();
                    let oracle_authority = oracle_authority.clone();
                    move || {
                        let uid = uid.clone();
                        let oracle_authority = oracle_authority.clone();
                        async move {
                            Json(json!({
                                "success": true,
                                "payer": PAYER,
                                "transaction": "fundtx",
                                "paymentUid": uid,
                                "oracleAuthority": oracle_authority
                            }))
                        }
                    }
                }),
            );
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service())
                .await
                .ok();
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn sla_escrow_fund_persists_bind_row() {
        let (_signing_key, authority) = oracle_keypair();
        let uid = payment_uid();
        let facilitator = spawn_facilitator(uid.clone(), authority.clone()).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(30)).await;
        let (state, _dir) = test_state(facilitator, SELLER_BASE.into(), vec![authority.clone()]).await;
        let bytes = Bytes::from_static(b"fund-object");
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let key = content_addressed_asset_key(&hash);
        state
            .storage
            .put(&key, "text/plain", bytes.clone())
            .await
            .expect("put");
        let listing_id = Uuid::new_v4();
        state
            .db
            .insert_listing(&listing_row(
                listing_id,
                "escrow",
                &key,
                &hash,
                bytes.len() as i64,
            ))
            .await
            .expect("listing");

        let base = spawn_app(state.clone()).await;
        wait_ready(&base).await;
        let proof = json!({
            "x402Version": 2,
            "paymentPayload": { "payload": { "transaction": "funded-tx", "paymentUid": uid } },
            "paymentRequirements": { "scheme": "sla-escrow" }
        });
        let res = reqwest::Client::new()
            .get(format!("{base}/api/v1/listings/{listing_id}/download"))
            .header("PAYMENT-SIGNATURE", proof.to_string())
            .send()
            .await
            .expect("fund request");
        assert_eq!(res.status(), 402);

        let bind = state
            .db
            .get_escrow_fund_bind(listing_id, &uid)
            .await
            .expect("get bind")
            .expect("bind row");
        assert_eq!(bind.listing_id, listing_id);
        assert_eq!(bind.payment_uid, uid);
        assert_eq!(bind.content_hash, hash);
        assert_eq!(bind.oracle_authority, authority);
        assert!(state
            .db
            .find_buyer_sale_for_listing(listing_id, PAYER)
            .await
            .expect("sales")
            .is_none());
    }
}
