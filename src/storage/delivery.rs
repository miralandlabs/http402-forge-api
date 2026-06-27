use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};

use super::ObjectStore;
use crate::config::{AppConfig, ObjectDelivery};
use crate::error::{AppError, AppResult};
use crate::state::SharedState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryFormat {
    Redirect,
    Json,
    Proxy,
}

#[derive(Debug, Deserialize)]
pub struct DeliveryQuery {
    /// `json` returns `{ url, ... }`; `proxy` forces streaming via API.
    pub delivery: Option<String>,
}

impl DeliveryQuery {
    pub fn format(&self, config: &AppConfig) -> AppResult<DeliveryFormat> {
        Ok(match self.delivery.as_deref().map(str::trim) {
            Some("json") => DeliveryFormat::Json,
            Some("proxy") => DeliveryFormat::Proxy,
            Some("redirect") => DeliveryFormat::Redirect,
            Some("") | None => match config.object_delivery {
                ObjectDelivery::Redirect
                    if config.storage_backend == crate::config::StorageBackend::R2 =>
                {
                    DeliveryFormat::Redirect
                }
                _ => DeliveryFormat::Proxy,
            },
            Some(other) => {
                return Err(AppError::BadRequest(format!(
                    "invalid delivery={other:?}; use json, redirect, or proxy"
                )))
            }
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedirectDeliveryBody {
    pub delivery: &'static str,
    pub url: String,
    pub expires_in_secs: u32,
    pub content_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_disposition: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sale_id: Option<String>,
}

pub struct ObjectServeOptions<'a> {
    pub key: &'a str,
    pub content_type: &'a str,
    pub content_disposition: Option<String>,
    pub extra_headers: HeaderMap,
    pub format: DeliveryFormat,
    pub sale_id: Option<&'a str>,
}

pub async fn serve_object(
    state: &SharedState,
    opts: ObjectServeOptions<'_>,
) -> AppResult<Response> {
    let ttl = state.config.presign_ttl_secs;
    match opts.format {
        DeliveryFormat::Proxy => serve_proxied(state, &opts).await,
        DeliveryFormat::Redirect | DeliveryFormat::Json => {
            let url = state
                .storage
                .presign_get(opts.key, ttl)
                .await
                .map_err(|_| {
                    AppError::BadRequest(
                        "direct delivery requires STORAGE_BACKEND=r2 and presigned URLs".into(),
                    )
                })?;
            if opts.format == DeliveryFormat::Json {
                let mut response = Json(RedirectDeliveryBody {
                    delivery: "redirect",
                    url,
                    expires_in_secs: ttl,
                    content_type: opts.content_type.to_string(),
                    content_disposition: opts.content_disposition,
                    sale_id: opts.sale_id.map(str::to_string),
                })
                .into_response();
                for (name, value) in opts.extra_headers.iter() {
                    response.headers_mut().insert(name.clone(), value.clone());
                }
                Ok(response)
            } else {
                let mut builder = Response::builder()
                    .status(StatusCode::TEMPORARY_REDIRECT)
                    .header(header::LOCATION, url)
                    .header(header::CONTENT_TYPE, opts.content_type)
                    .header("X-Forge-Delivery", "redirect")
                    .header(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
                for (name, value) in opts.extra_headers.iter() {
                    builder = builder.header(name, value);
                }
                if let Some(cd) = opts.content_disposition {
                    builder = builder.header(header::CONTENT_DISPOSITION, cd);
                }
                builder
                    .body(Body::empty())
                    .map_err(|e| AppError::Internal(e.into()))
            }
        }
    }
}

async fn serve_proxied(state: &SharedState, opts: &ObjectServeOptions<'_>) -> AppResult<Response> {
    let (stream, content_type) = state.storage.stream(opts.key).await?;
    let body = Body::from_stream(stream.map(|result| result.map_err(axum::Error::new)));
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header("X-Forge-Delivery", "proxy")
        .header(header::ACCEPT_RANGES, "bytes");
    if let Some(cd) = &opts.content_disposition {
        builder = builder.header(header::CONTENT_DISPOSITION, cd);
    }
    for (name, value) in opts.extra_headers.iter() {
        builder = builder.header(name, value);
    }
    builder.body(body).map_err(|e| AppError::Internal(e.into()))
}

pub fn supports_presigned_upload(config: &AppConfig) -> bool {
    config.storage_backend == crate::config::StorageBackend::R2
}
