use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;

use crate::db::ListingQualityStats;
use crate::db::ListingRow;

pub const CATEGORIES: &[&str] = &["art", "text", "audio", "video", "prompt_pack"];

pub const LICENSES: &[&str] = &["personal", "commercial"];

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListingPublic {
    pub id: Uuid,
    pub seller_wallet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub title: String,
    pub description: String,
    pub category: String,
    pub price_micro_usdc: i64,
    pub content_type: String,
    pub preview_content_type: String,
    pub byte_size: i64,
    pub agent_friendly: bool,
    pub delivery_scheme: String,
    pub preview_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_pdf_url: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality_score: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_feedback_count: Option<i64>,
    pub created_at: chrono::DateTime<Utc>,
}

impl ListingPublic {
    pub fn from_row(row: ListingRow, base_url: &str) -> Self {
        Self::from_row_with_quality(row, base_url, None)
    }

    pub fn from_row_with_quality(
        row: ListingRow,
        base_url: &str,
        quality: Option<ListingQualityStats>,
    ) -> Self {
        let base = base_url.trim_end_matches('/');
        let preview_pdf_url = offers_pdf_sample_preview(&row)
            .then(|| format!("{base}/api/v1/listings/{}/preview-pdf", row.id));
        Self {
            id: row.id,
            seller_wallet: row.seller_wallet,
            display_name: row.display_name,
            title: row.title,
            description: row.description,
            category: row.category,
            price_micro_usdc: row.price_micro_usdc,
            content_type: row.content_type,
            preview_content_type: row.preview_content_type,
            byte_size: row.byte_size,
            agent_friendly: row.agent_friendly,
            delivery_scheme: row.delivery_scheme,
            preview_url: format!("{base}/api/v1/listings/{}/preview", row.id),
            preview_pdf_url,
            tags: parse_tags_json(&row.tags),
            license: row.license,
            content_hash: row.content_hash,
            quality_score: quality
                .as_ref()
                .filter(|q| q.verified_feedback_count > 0)
                .map(|q| q.quality_score),
            verified_feedback_count: quality
                .as_ref()
                .filter(|q| q.verified_feedback_count > 0)
                .map(|q| q.verified_feedback_count),
            created_at: row.created_at,
        }
    }
}

pub fn parse_tags_json(raw: &str) -> Vec<String> {
    if let Ok(v) = serde_json::from_str::<Vec<String>>(raw) {
        return v;
    }
    Vec::new()
}

pub fn parse_tags_field(raw: &str) -> Result<Vec<String>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if trimmed.starts_with('[') {
        serde_json::from_str(trimmed).map_err(|e| format!("invalid tags JSON: {e}"))
    } else {
        Ok(trimmed
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect())
    }
}

pub fn validate_license(license: Option<&str>) -> Result<(), String> {
    match license {
        None => Ok(()),
        Some(l) if LICENSES.contains(&l) => Ok(()),
        Some(l) => Err(format!(
            "license must be one of: {} (got '{l}')",
            LICENSES.join(", ")
        )),
    }
}

pub fn tags_to_json(tags: &[String]) -> String {
    serde_json::to_string(tags).unwrap_or_else(|_| "[]".into())
}

pub fn parse_price_usdc(raw: &str) -> Result<i64, String> {
    let v: f64 = raw
        .trim()
        .parse()
        .map_err(|_| "price_usdc must be a decimal number".to_string())?;
    if v <= 0.0 || v > 1000.0 {
        return Err("price_usdc must be between 0 and 1000 USDC".into());
    }
    let micro = (v * 1_000_000.0).round() as i64;
    if micro < 10_000 {
        return Err("minimum price is 0.01 USDC".into());
    }
    Ok(micro)
}

pub fn validate_category(cat: &str) -> Result<(), String> {
    if CATEGORIES.contains(&cat) {
        Ok(())
    } else {
        Err(format!(
            "category must be one of: {}",
            CATEGORIES.join(", ")
        ))
    }
}

pub fn validate_wallet(wallet: &str) -> Result<(), String> {
    if wallet.len() < 32 || wallet.len() > 44 {
        return Err("invalid seller_wallet".into());
    }
    Ok(())
}

pub fn offers_pdf_sample_preview(row: &ListingRow) -> bool {
    let asset_ct = row.content_type.trim().to_ascii_lowercase();
    if asset_ct != "application/pdf" && asset_ct != "application/x-pdf" {
        return false;
    }
    let preview_ct = row.preview_content_type.trim().to_ascii_lowercase();
    // Seller-uploaded non-PDF teaser (image/audio/video/text) — render via /preview, not sample PDF.
    if row.preview_key.ends_with("/preview")
        && preview_ct != "application/pdf"
        && preview_ct != "application/x-pdf"
    {
        return false;
    }
    if preview_ct.starts_with("video/")
        || preview_ct.starts_with("audio/")
        || preview_ct.starts_with("text/")
    {
        return false;
    }
    preview_ct == "application/pdf"
        || preview_ct == "application/x-pdf"
        || preview_ct.starts_with("image/")
}

pub fn text_preview_snippet(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    let char_count = trimmed.chars().count();
    if char_count <= max {
        trimmed.to_string()
    } else {
        let snippet: String = trimmed.chars().take(max).collect();
        format!("{snippet}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn pdf_row(preview_key: &str, preview_content_type: &str) -> ListingRow {
        ListingRow {
            id: Uuid::new_v4(),
            seller_wallet: "seller".into(),
            display_name: None,
            title: "t".into(),
            description: "d".into(),
            category: "text".into(),
            price_micro_usdc: 50_000,
            preview_key: preview_key.into(),
            preview_content_type: preview_content_type.into(),
            asset_key: "assets/x/asset".into(),
            content_type: "application/pdf".into(),
            byte_size: 100,
            agent_friendly: false,
            delivery_scheme: "exact".into(),
            status: "active".into(),
            tags: "[]".into(),
            license: None,
            content_hash: None,
            moderation_status: "approved".into(),
            moderation_labels: "[]".into(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn offers_pdf_sample_for_auto_jpeg_thumbnail() {
        let row = pdf_row("previews/id/preview.jpg", "image/jpeg");
        assert!(offers_pdf_sample_preview(&row));
    }

    #[test]
    fn offers_pdf_sample_for_uploaded_pdf_preview() {
        let row = pdf_row("previews/id/preview.pdf", "application/pdf");
        assert!(offers_pdf_sample_preview(&row));
    }

    #[test]
    fn skips_pdf_sample_for_custom_image_preview() {
        let row = pdf_row("previews/id/preview", "image/jpeg");
        assert!(!offers_pdf_sample_preview(&row));
    }

    #[test]
    fn skips_pdf_sample_for_custom_video_preview() {
        let row = pdf_row("previews/id/preview", "video/mp4");
        assert!(!offers_pdf_sample_preview(&row));
    }

    #[test]
    fn text_preview_snippet_respects_char_boundary() {
        let text = "中文测试内容".repeat(50);
        let snippet = text_preview_snippet(&text, 10);
        assert!(snippet.ends_with('…'));
        assert_eq!(snippet.chars().count(), 11);
    }
}
