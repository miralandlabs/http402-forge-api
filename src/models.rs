use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;

use crate::db::ListingRow;

pub const CATEGORIES: &[&str] = &["art", "text", "audio", "video", "prompt_pack"];

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
    pub byte_size: i64,
    pub agent_friendly: bool,
    pub delivery_scheme: String,
    pub preview_url: String,
    pub created_at: chrono::DateTime<Utc>,
}

impl ListingPublic {
    pub fn from_row(row: ListingRow, base_url: &str) -> Self {
        let base = base_url.trim_end_matches('/');
        Self {
            id: row.id,
            seller_wallet: row.seller_wallet,
            display_name: row.display_name,
            title: row.title,
            description: row.description,
            category: row.category,
            price_micro_usdc: row.price_micro_usdc,
            content_type: row.content_type,
            byte_size: row.byte_size,
            agent_friendly: row.agent_friendly,
            delivery_scheme: row.delivery_scheme,
            preview_url: format!("{base}/api/v1/listings/{}/preview", row.id),
            created_at: row.created_at,
        }
    }
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

    #[test]
    fn text_preview_snippet_respects_char_boundary() {
        let text = "中文测试内容".repeat(50);
        let snippet = text_preview_snippet(&text, 10);
        assert!(snippet.ends_with('…'));
        assert_eq!(snippet.chars().count(), 11);
    }
}
