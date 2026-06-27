use axum::{extract::State, Json};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::models::validate_wallet;
use crate::routes::listings::{publish_listing, require_seller_vault, PublishListingInput};
use crate::state::SharedState;
use crate::storage::{object_key, supports_presigned_upload, ObjectStore, PresignedPut};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadSessionRequest {
    pub seller_wallet: String,
    pub seller_challenge: String,
    pub seller_signature: String,
    pub asset_content_type: String,
    pub asset_byte_size: u64,
    #[serde(default)]
    pub preview_content_type: Option<String>,
    #[serde(default)]
    pub preview_byte_size: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresignedUploadTarget {
    pub object_key: String,
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadSessionResponse {
    pub listing_id: Uuid,
    pub expires_at: chrono::DateTime<Utc>,
    pub asset: PresignedUploadTarget,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<PresignedUploadTarget>,
}

fn presigned_target(put: PresignedPut) -> PresignedUploadTarget {
    PresignedUploadTarget {
        object_key: put.object_key,
        method: put.method,
        url: put.url,
        headers: put.headers,
    }
}

pub async fn upload_session(
    State(state): State<SharedState>,
    Json(body): Json<UploadSessionRequest>,
) -> AppResult<(axum::http::StatusCode, Json<UploadSessionResponse>)> {
    if !supports_presigned_upload(&state.config) {
        return Err(AppError::BadRequest(
            "presigned upload requires STORAGE_BACKEND=r2".into(),
        ));
    }
    validate_wallet(&body.seller_wallet).map_err(|m| AppError::validation("seller_wallet", m))?;
    if body.asset_byte_size == 0 {
        return Err(AppError::validation("asset_byte_size", "required"));
    }
    if body.asset_byte_size > state.config.max_asset_bytes {
        return Err(AppError::BadRequest(format!(
            "asset exceeds max {} bytes",
            state.config.max_asset_bytes
        )));
    }
    if let Some(preview_size) = body.preview_byte_size {
        if preview_size > state.config.max_preview_bytes {
            return Err(AppError::BadRequest(format!(
                "preview exceeds max {} bytes",
                state.config.max_preview_bytes
            )));
        }
    }

    if !state.config.skip_seller_auth {
        state.seller_auth.verify_without_consume(
            &body.seller_wallet,
            &body.seller_challenge,
            &body.seller_signature,
        )?;
    }
    require_seller_vault(&state, &body.seller_wallet).await?;

    let listing_id = Uuid::new_v4();
    let ttl = state.config.presign_ttl_secs;
    let asset_key = object_key("assets", listing_id, "asset");
    let asset_put = state
        .storage
        .presign_put(&asset_key, body.asset_content_type.trim(), ttl)
        .await?;

    let preview = if let Some(preview_ct) = body
        .preview_content_type
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        let preview_size = body.preview_byte_size.unwrap_or(0);
        if preview_size == 0 {
            return Err(AppError::validation(
                "preview_byte_size",
                "required when preview_content_type is set",
            ));
        }
        let preview_key = object_key("previews", listing_id, "preview");
        let preview_put = state
            .storage
            .presign_put(&preview_key, preview_ct.trim(), ttl)
            .await?;
        Some(presigned_target(preview_put))
    } else {
        None
    };

    let expires_at = Utc::now() + Duration::seconds(i64::from(ttl));
    state
        .seller_auth
        .register_upload_session(listing_id, &body.seller_wallet, expires_at);
    Ok((
        axum::http::StatusCode::OK,
        Json(UploadSessionResponse {
            listing_id,
            expires_at,
            asset: presigned_target(asset_put),
            preview,
        }),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompleteUploadRequest {
    pub listing_id: Uuid,
    pub seller_wallet: String,
    pub seller_challenge: String,
    pub seller_signature: String,
    pub title: String,
    pub description: String,
    pub category: String,
    pub price_usdc: String,
    #[serde(default)]
    pub agent_friendly: bool,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub tags: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub content_hash: Option<String>,
    #[serde(default)]
    pub preview_uploaded: bool,
}

pub async fn complete_upload(
    State(state): State<SharedState>,
    Json(body): Json<CompleteUploadRequest>,
) -> AppResult<(axum::http::StatusCode, Json<crate::models::ListingPublic>)> {
    if !supports_presigned_upload(&state.config) {
        return Err(AppError::BadRequest(
            "presigned upload requires STORAGE_BACKEND=r2".into(),
        ));
    }

    if !state.config.skip_seller_auth {
        state.seller_auth.verify_and_consume(
            &body.seller_wallet,
            &body.seller_challenge,
            &body.seller_signature,
        )?;
    }
    state
        .seller_auth
        .consume_upload_session(body.listing_id, &body.seller_wallet)?;

    let asset_key = object_key("assets", body.listing_id, "asset");
    let asset_size = state.storage.object_size(&asset_key).await?;
    let asset_ct = state.storage.head(&asset_key).await?;

    let preview_bytes = if body.preview_uploaded {
        let preview_key = object_key("previews", body.listing_id, "preview");
        let preview_ct = state.storage.head(&preview_key).await?;
        let (data, _) = state.storage.get(&preview_key).await?;
        Some((preview_ct, data))
    } else {
        None
    };

    let (asset_data, _) = state.storage.get(&asset_key).await?;
    if asset_data.len() as u64 != asset_size {
        return Err(AppError::BadRequest("asset upload incomplete".into()));
    }

    let row = publish_listing(
        &state,
        PublishListingInput {
            seller_wallet: body.seller_wallet,
            display_name: body.display_name,
            title: body.title,
            description: body.description,
            category: body.category,
            price_usdc: body.price_usdc,
            agent_friendly: body.agent_friendly,
            tags_raw: body.tags.unwrap_or_default(),
            license: body.license,
            content_hash: body.content_hash,
            asset_ct,
            asset_data,
            preview_bytes,
        },
        body.listing_id,
        asset_key,
        true,
    )
    .await?;

    Ok((
        axum::http::StatusCode::CREATED,
        Json(crate::models::ListingPublic::from_row(
            row,
            &state.config.seller_public_base_url,
        )),
    ))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitiesResponse {
    pub presigned_upload: bool,
    pub presigned_download: bool,
    pub object_delivery: &'static str,
}

pub async fn capabilities(State(state): State<SharedState>) -> Json<CapabilitiesResponse> {
    Json(CapabilitiesResponse {
        presigned_upload: supports_presigned_upload(&state.config),
        presigned_download: state.config.storage_backend == crate::config::StorageBackend::R2,
        object_delivery: match state.config.object_delivery {
            crate::config::ObjectDelivery::Redirect => "redirect",
            crate::config::ObjectDelivery::Proxy => "proxy",
        },
    })
}
