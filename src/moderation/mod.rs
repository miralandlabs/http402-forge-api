use bytes::Bytes;
use serde::Deserialize;

use crate::config::{ModerationConfig, ModerationProvider};
use crate::error::{AppError, AppResult};
use crate::models::tags_to_json;

#[derive(Debug, Clone)]
pub struct ModerationScanResult {
    pub flagged: bool,
    pub labels: Vec<String>,
}

pub struct ListingModerationInput<'a> {
    pub title: &'a str,
    pub description: &'a str,
    pub tags: &'a [String],
    pub asset_content_type: &'a str,
    pub asset_data: &'a Bytes,
    pub preview_data: Option<&'a Bytes>,
    pub preview_content_type: Option<&'a str>,
}

pub async fn scan_listing_upload(
    config: &ModerationConfig,
    input: ListingModerationInput<'_>,
) -> AppResult<ModerationScanResult> {
    match config.provider {
        ModerationProvider::None => Ok(ModerationScanResult {
            flagged: false,
            labels: Vec::new(),
        }),
        ModerationProvider::OpenAi => scan_openai(config, input).await,
    }
}

async fn scan_openai(
    config: &ModerationConfig,
    input: ListingModerationInput<'_>,
) -> AppResult<ModerationScanResult> {
    let api_key = config.openai_api_key.as_deref().ok_or_else(|| {
        AppError::Internal(anyhow::anyhow!(
            "MODERATION_PROVIDER=openai requires OPENAI_API_KEY"
        ))
    })?;

    let mut labels = Vec::new();
    let mut flagged = false;

    let text_blob = format!(
        "{}\n{}\n{}",
        input.title,
        input.description,
        input.tags.join(", ")
    );
    if let Some(result) = moderate_text(api_key, &text_blob).await? {
        if result.flagged {
            flagged = true;
        }
        labels.extend(result.labels);
    }

    if input.asset_content_type.starts_with("image/") && input.asset_content_type != "image/svg+xml"
    {
        if let Some(result) =
            moderate_image(api_key, input.asset_data, input.asset_content_type).await?
        {
            if result.flagged {
                flagged = true;
            }
            labels.extend(result.labels);
        }
    }

    if let (Some(data), Some(ct)) = (input.preview_data, input.preview_content_type) {
        if ct.starts_with("image/") && ct != "image/svg+xml" {
            if let Some(result) = moderate_image(api_key, data, ct).await? {
                if result.flagged {
                    flagged = true;
                }
                labels.extend(result.labels);
            }
        }
    }

    labels.sort_unstable();
    labels.dedup();

    Ok(ModerationScanResult { flagged, labels })
}

#[derive(Debug, Deserialize)]
struct OpenAiModerationResponse {
    results: Vec<OpenAiModerationResult>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModerationResult {
    flagged: bool,
    categories: serde_json::Map<String, serde_json::Value>,
}

async fn moderate_text(api_key: &str, text: &str) -> AppResult<Option<ModerationScanResult>> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let body = serde_json::json!({
        "model": "omni-moderation-latest",
        "input": text,
    });
    post_moderation(api_key, body).await
}

async fn moderate_image(
    api_key: &str,
    data: &Bytes,
    content_type: &str,
) -> AppResult<Option<ModerationScanResult>> {
    if data.is_empty() {
        return Ok(None);
    }
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data.as_ref());
    let data_url = format!("data:{content_type};base64,{b64}");
    let body = serde_json::json!({
        "model": "omni-moderation-latest",
        "input": [{
            "type": "image_url",
            "image_url": { "url": data_url },
        }],
    });
    post_moderation(api_key, body).await
}

async fn post_moderation(
    api_key: &str,
    body: serde_json::Value,
) -> AppResult<Option<ModerationScanResult>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("moderation client: {e}")))?;

    let res = client
        .post("https://api.openai.com/v1/moderations")
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("moderation request: {e}")))?;

    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        tracing::warn!(status = %status, body = %text, "OpenAI moderation HTTP error");
        return Err(AppError::Internal(anyhow::anyhow!(
            "moderation provider error ({status})"
        )));
    }

    let parsed: OpenAiModerationResponse = res
        .json()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("moderation response parse: {e}")))?;

    let Some(first) = parsed.results.first() else {
        return Ok(None);
    };

    let labels = flagged_category_labels(&first.categories);
    Ok(Some(ModerationScanResult {
        flagged: first.flagged,
        labels,
    }))
}

fn flagged_category_labels(categories: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
    categories
        .iter()
        .filter_map(|(name, value)| {
            if value.as_bool().unwrap_or(false) {
                Some(name.clone())
            } else {
                None
            }
        })
        .collect()
}

pub fn moderation_labels_json(labels: &[String]) -> String {
    tags_to_json(labels)
}
