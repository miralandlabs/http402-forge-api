use axum::{
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use bytes::Bytes;
use chrono::Utc;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::ListingFilterBinds;
use crate::db::ListingRow;
use crate::error::{AppError, AppResult};
use crate::models::{
    parse_price_usdc, parse_tags_field, tags_to_json, text_preview_snippet, validate_category,
    validate_license, validate_wallet, ListingPublic,
};
use crate::preview::{
    self, generate_media_clip, generate_pdf_first_page_jpeg, is_pdf_content_type,
};
use crate::state::SharedState;
use crate::storage::{object_key, ObjectStore};
use crate::x402::{PaymentContext, PaymentGate};

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub category: Option<String>,
    pub agent_friendly: Option<String>,
    pub q: Option<String>,
    pub seller_wallet: Option<String>,
    #[serde(default = "default_sort")]
    pub sort: String,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_sort() -> String {
    "trending".into()
}

fn default_limit() -> i64 {
    20
}

fn parse_optional_bool(raw: Option<&str>) -> Option<bool> {
    match raw?.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Some(true),
        "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub items: Vec<ListingPublic>,
    pub total: i64,
}

pub async fn list(
    State(state): State<SharedState>,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<ListResponse>> {
    if q.limit < 1 || q.limit > 100 {
        return Err(AppError::BadRequest("limit must be 1-100".into()));
    }
    let cat = q.category.as_deref();
    if let Some(c) = cat {
        validate_category(c).map_err(AppError::BadRequest)?;
    }
    let agent_friendly = parse_optional_bool(q.agent_friendly.as_deref());
    let seller_wallet_param = q
        .seller_wallet
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(w) = seller_wallet_param {
        validate_wallet(w).map_err(|m| AppError::validation("seller_wallet", m))?;
    }
    let filters =
        ListingFilterBinds::from_query(cat, agent_friendly, q.q.clone(), seller_wallet_param);
    if let Some(ref w) = filters.seller_wallet {
        validate_wallet(w).map_err(|m| AppError::validation("seller_wallet", m))?;
    }
    let total = state.db.count_listings(&filters).await?;
    let rows = state
        .db
        .list_listings(&filters, &q.sort, q.limit, q.offset)
        .await?;
    let base = state.config.seller_public_base_url.clone();
    let items = rows
        .into_iter()
        .map(|r| ListingPublic::from_row(r, &base))
        .collect();
    Ok(Json(ListResponse { items, total }))
}

pub async fn get_one(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<ListingPublic>> {
    let row = state.db.get_listing(id).await?;
    Ok(Json(ListingPublic::from_row(
        row,
        &state.config.seller_public_base_url,
    )))
}

pub async fn preview(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> AppResult<Response> {
    let row = state.db.get_listing(id).await?;
    let (stream_key, content_type) = resolve_preview_key_and_type(&state, &row).await?;

    if content_type.starts_with("text/") || content_type == "application/json" {
        let (bytes, _) = state.storage.get(&stream_key).await?;
        let text = String::from_utf8_lossy(&bytes);
        let snippet = text_preview_snippet(&text, 500);
        return Ok((
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            snippet,
        )
            .into_response());
    }

    let (stream, content_type) = state.storage.stream(&stream_key).await?;
    let body = Body::from_stream(stream.map(|result| result.map_err(axum::Error::new)));
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ACCEPT_RANGES, "bytes")
        .body(body)
        .map_err(|e| AppError::Internal(e.into()))?;
    Ok(response)
}

async fn resolve_preview_key_and_type(
    state: &SharedState,
    row: &ListingRow,
) -> AppResult<(String, String)> {
    let content_type = state.storage.head(&row.preview_key).await?;
    if content_type.starts_with("text/") || content_type == "application/json" {
        let (bytes, ct) = state.storage.get(&row.preview_key).await?;
        let is_legacy_placeholder = ct.starts_with("text/")
            && bytes.starts_with(b"Preview unavailable for")
            && (row.content_type.starts_with("video/") || row.content_type.starts_with("audio/"));
        if is_legacy_placeholder {
            let asset_ct = state.storage.head(&row.asset_key).await?;
            return Ok((row.asset_key.clone(), asset_ct));
        }
        return Ok((row.preview_key.clone(), ct));
    }

    let is_legacy_media_asset =
        row.preview_key == row.asset_key && preview::is_media_content_type(&row.content_type);
    if is_legacy_media_asset {
        tracing::warn!(
            listing_id = %row.id,
            "listing uses full asset as preview; re-upload to generate a clipped preview"
        );
    }
    Ok((row.preview_key.clone(), content_type))
}

pub async fn download(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let row = state.db.get_listing_any(id).await?;
    let path = format!("/api/v1/listings/{id}/download");
    let payment = PaymentGate::check_download_active_or_paid(&state, &headers, &row, &path).await?;

    if !payment.already_paid {
        record_sale(&state, &row, &payment).await?;
    }

    let (stream, content_type) = state.storage.stream(&row.asset_key).await?;
    let body = Body::from_stream(stream.map(|result| result.map_err(axum::Error::new)));
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", sanitize_filename(&row.title)),
        )
        .body(body)
        .map_err(|e| AppError::Internal(e.into()))?;

    if !payment.settle_proof.is_null() {
        response = PaymentGate::attach_payment_response(response, &payment.settle_proof);
    }
    Ok(response)
}

#[derive(Debug, Deserialize)]
pub struct DelistRequest {
    pub seller_wallet: String,
    pub seller_challenge: String,
    pub seller_signature: String,
}

pub async fn delist(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<DelistRequest>,
) -> AppResult<StatusCode> {
    validate_wallet(&body.seller_wallet).map_err(|m| AppError::validation("seller_wallet", m))?;

    if !state.config.skip_seller_auth {
        state.seller_auth.verify_and_consume(
            &body.seller_wallet,
            &body.seller_challenge,
            &body.seller_signature,
        )?;
        let challenge_listing = crate::auth::parse_delist_listing_id(&body.seller_challenge)
            .ok_or_else(|| AppError::Forbidden("invalid delist challenge".into()))?;
        if challenge_listing != id {
            return Err(AppError::Forbidden(
                "delist challenge listing mismatch".into(),
            ));
        }
    }

    let removed = state
        .db
        .soft_delist_listing(id, &body.seller_wallet)
        .await?;
    if !removed {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn record_sale(
    state: &SharedState,
    listing: &ListingRow,
    payment: &PaymentContext,
) -> AppResult<()> {
    let tx = payment
        .settle_proof
        .get("transaction")
        .and_then(|v| v.as_str())
        .unwrap_or(&payment.payment_signature)
        .to_string();
    let sale = state
        .db
        .insert_sale(
            listing.id,
            &listing.seller_wallet,
            &payment.payer_wallet,
            listing.price_micro_usdc,
            &tx,
        )
        .await?;
    let _ = state.sale_events.send(sale);
    Ok(())
}

const VAULT_REQUIRED_MSG: &str = "Activate your pr402 SplitVault before publishing.";

async fn require_seller_vault(state: &SharedState, seller_wallet: &str) -> AppResult<()> {
    let ok = state
        .facilitator
        .seller_has_vault(seller_wallet)
        .await
        .map_err(|e| AppError::PaymentConfig(format!("Seller vault check failed: {e}")))?;
    if !ok {
        return Err(AppError::Forbidden(VAULT_REQUIRED_MSG.into()));
    }
    Ok(())
}

fn sanitize_filename(title: &str) -> String {
    title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect()
}

pub async fn create(
    State(state): State<SharedState>,
    mut multipart: Multipart,
) -> AppResult<(StatusCode, Json<ListingPublic>)> {
    let mut seller_wallet = String::new();
    let mut display_name: Option<String> = None;
    let mut title = String::new();
    let mut description = String::new();
    let mut category = String::new();
    let mut price_usdc = String::new();
    let mut agent_friendly = false;
    let mut seller_challenge = String::new();
    let mut seller_signature = String::new();
    let mut tags_raw = String::new();
    let mut license: Option<String> = None;
    let mut content_hash: Option<String> = None;
    let mut asset_bytes: Option<(String, Bytes)> = None;
    let mut preview_bytes: Option<(String, Bytes)> = None;
    let mut vault_checked = state.config.skip_seller_vault_check;

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        let msg = e.to_string();
        if msg.contains("multipart") || msg.contains("limit") {
            AppError::BadRequest(format!(
                "upload too large or invalid multipart (max asset {} bytes, preview {} bytes)",
                state.config.max_asset_bytes, state.config.max_preview_bytes
            ))
        } else {
            AppError::BadRequest(msg)
        }
    })? {
        let name = field.name().unwrap_or("").to_string();
        if (name == "asset" || name == "preview")
            && !vault_checked
            && seller_wallet.trim().is_empty()
        {
            return Err(AppError::validation(
                "seller_wallet",
                "send seller_wallet before asset/preview",
            ));
        }
        if (name == "asset" || name == "preview") && !vault_checked {
            validate_wallet(&seller_wallet)
                .map_err(|m| AppError::validation("seller_wallet", m))?;
            require_seller_vault(&state, &seller_wallet).await?;
            vault_checked = true;
        }
        match name.as_str() {
            "seller_wallet" => seller_wallet = field.text().await.unwrap_or_default(),
            "display_name" => display_name = Some(field.text().await.unwrap_or_default()),
            "title" => title = field.text().await.unwrap_or_default(),
            "description" => description = field.text().await.unwrap_or_default(),
            "category" => category = field.text().await.unwrap_or_default(),
            "price_usdc" => price_usdc = field.text().await.unwrap_or_default(),
            "agent_friendly" => {
                let v = field.text().await.unwrap_or_default();
                agent_friendly = v == "1" || v.eq_ignore_ascii_case("true");
            }
            "seller_challenge" => seller_challenge = field.text().await.unwrap_or_default(),
            "seller_signature" => seller_signature = field.text().await.unwrap_or_default(),
            "tags" => tags_raw = field.text().await.unwrap_or_default(),
            "license" => {
                let v = field.text().await.unwrap_or_default();
                if !v.trim().is_empty() {
                    license = Some(v.trim().to_string());
                }
            }
            "content_hash" => {
                let v = field.text().await.unwrap_or_default();
                if !v.trim().is_empty() {
                    content_hash = Some(v.trim().to_string());
                }
            }
            "asset" => {
                let filename = field.file_name().unwrap_or("asset.bin").to_string();
                let ct = field.content_type().map(str::to_string).unwrap_or_else(|| {
                    mime_guess::from_path(&filename)
                        .first_or_octet_stream()
                        .to_string()
                });
                let data = field
                    .bytes()
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
                if data.len() as u64 > state.config.max_asset_bytes {
                    return Err(AppError::BadRequest(format!(
                        "asset exceeds max {} bytes",
                        state.config.max_asset_bytes
                    )));
                }
                asset_bytes = Some((ct, data));
            }
            "preview" => {
                let filename = field.file_name().unwrap_or("preview.bin").to_string();
                let ct = field.content_type().map(str::to_string).unwrap_or_else(|| {
                    mime_guess::from_path(&filename)
                        .first_or_octet_stream()
                        .to_string()
                });
                let data = field
                    .bytes()
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
                if data.len() as u64 > state.config.max_preview_bytes {
                    return Err(AppError::BadRequest(format!(
                        "preview exceeds max {} bytes",
                        state.config.max_preview_bytes
                    )));
                }
                preview_bytes = Some((ct, data));
            }
            _ => {}
        }
        if name == "seller_wallet" && !seller_wallet.trim().is_empty() && !vault_checked {
            validate_wallet(&seller_wallet)
                .map_err(|m| AppError::validation("seller_wallet", m))?;
            require_seller_vault(&state, &seller_wallet).await?;
            vault_checked = true;
        }
    }

    validate_wallet(&seller_wallet).map_err(|m| AppError::validation("seller_wallet", m))?;
    validate_category(&category).map_err(|m| AppError::validation("category", m))?;
    if title.trim().is_empty() || title.len() > 120 {
        return Err(AppError::validation("title", "required, max 120 chars"));
    }
    if description.len() > 2000 {
        return Err(AppError::validation("description", "max 2000 chars"));
    }
    let price_micro =
        parse_price_usdc(&price_usdc).map_err(|m| AppError::validation("price_usdc", m))?;
    validate_license(license.as_deref()).map_err(|m| AppError::validation("license", m))?;
    let tags = parse_tags_field(&tags_raw).map_err(|m| AppError::validation("tags", m))?;
    let (asset_ct, asset_data) =
        asset_bytes.ok_or_else(|| AppError::validation("asset", "file required"))?;

    let content_hash = content_hash.or_else(|| Some(sha256_hex(&asset_data)));

    if !state.config.skip_seller_auth {
        state.seller_auth.verify_and_consume(
            &seller_wallet,
            &seller_challenge,
            &seller_signature,
        )?;
    }

    if !vault_checked {
        require_seller_vault(&state, &seller_wallet).await?;
    }

    let id = Uuid::new_v4();
    let asset_key = object_key("assets", id, "asset");
    state
        .storage
        .put(&asset_key, &asset_ct, asset_data.clone())
        .await?;

    let (preview_key, _preview_ct) = if let Some((pct, pdata)) = preview_bytes {
        let key = object_key("previews", id, "preview");
        state.storage.put(&key, &pct, pdata).await?;
        (key, pct)
    } else if asset_ct.starts_with("image/") {
        let key = object_key("previews", id, "preview.jpg");
        let preview = generate_image_preview(&asset_data, &asset_ct)?;
        state
            .storage
            .put(&key, "image/jpeg", preview.clone())
            .await?;
        (key, "image/jpeg".to_string())
    } else if asset_ct.starts_with("text/") || asset_ct == "application/json" {
        let snippet = text_preview_snippet(&String::from_utf8_lossy(&asset_data), 500);
        let key = object_key("previews", id, "preview.txt");
        let bytes = Bytes::from(snippet);
        state
            .storage
            .put(&key, "text/plain; charset=utf-8", bytes)
            .await?;
        (key, "text/plain; charset=utf-8".to_string())
    } else if is_pdf_content_type(&asset_ct) {
        let key = object_key("previews", id, "preview.jpg");
        match generate_pdf_first_page_jpeg(&asset_data, &state.config).await {
            Ok(preview) => {
                state
                    .storage
                    .put(&key, "image/jpeg", preview.clone())
                    .await?;
                (key, "image/jpeg".to_string())
            }
            Err(e) => {
                tracing::warn!(error = %e, "PDF auto-preview failed; listing will use text placeholder");
                let placeholder_key = object_key("previews", id, "placeholder.txt");
                let bytes = Bytes::from(format!("Preview unavailable for {asset_ct}"));
                state
                    .storage
                    .put(&placeholder_key, "text/plain", bytes)
                    .await?;
                (placeholder_key, "text/plain".to_string())
            }
        }
    } else if asset_ct.starts_with("video/") || asset_ct.starts_with("audio/") {
        let (clip, clip_ct) = generate_media_clip(&asset_data, &asset_ct, &state.config).await?;
        let ext = clip_extension(&clip_ct);
        let key = object_key("previews", id, &format!("preview.{ext}"));
        state.storage.put(&key, &clip_ct, clip).await?;
        (key, clip_ct)
    } else {
        let key = object_key("previews", id, "placeholder.txt");
        let bytes = Bytes::from(format!("Preview unavailable for {asset_ct}"));
        state.storage.put(&key, "text/plain", bytes).await?;
        (key, "text/plain".to_string())
    };

    let delivery_scheme = if asset_data.len() as u64 >= state.config.escrow_size_threshold {
        "escrow"
    } else {
        "exact"
    };

    let row = ListingRow {
        id,
        seller_wallet: seller_wallet.clone(),
        display_name: display_name.filter(|s| !s.is_empty()),
        title: title.trim().to_string(),
        description,
        category,
        price_micro_usdc: price_micro,
        preview_key,
        asset_key,
        content_type: asset_ct,
        byte_size: asset_data.len() as i64,
        agent_friendly,
        delivery_scheme: delivery_scheme.into(),
        status: "active".into(),
        tags: tags_to_json(&tags),
        license,
        content_hash,
        created_at: Utc::now(),
    };

    state.db.insert_listing(&row).await?;

    Ok((
        StatusCode::CREATED,
        Json(ListingPublic::from_row(
            row,
            &state.config.seller_public_base_url,
        )),
    ))
}

fn generate_image_preview(data: &Bytes, content_type: &str) -> AppResult<Bytes> {
    if content_type == "image/svg+xml" {
        return Ok(data.clone());
    }
    let img = image::load_from_memory(data)
        .map_err(|e| AppError::BadRequest(format!("asset is not a valid image: {e}")))?;
    let thumb = img.thumbnail(400, 400);
    let mut buf = std::io::Cursor::new(Vec::new());
    thumb
        .write_to(&mut buf, image::ImageFormat::Jpeg)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("preview encode: {e}")))?;
    Ok(Bytes::from(buf.into_inner()))
}

fn clip_extension(content_type: &str) -> &'static str {
    if content_type.contains("webm") {
        "webm"
    } else if content_type.contains("mpeg") || content_type.contains("mp3") {
        "mp3"
    } else if content_type.contains("ogg") {
        "ogg"
    } else if content_type.starts_with("audio/") {
        "m4a"
    } else {
        "mp4"
    }
}

fn sha256_hex(data: &Bytes) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(data))
}
