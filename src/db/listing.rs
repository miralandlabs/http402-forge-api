use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ListingRow {
    pub id: Uuid,
    pub seller_wallet: String,
    pub display_name: Option<String>,
    pub title: String,
    pub description: String,
    pub category: String,
    pub price_micro_usdc: i64,
    pub preview_key: String,
    pub asset_key: String,
    pub content_type: String,
    pub byte_size: i64,
    pub agent_friendly: bool,
    pub delivery_scheme: String,
    pub status: String,
    pub tags: String,
    pub license: Option<String>,
    pub content_hash: Option<String>,
    pub created_at: DateTime<Utc>,
}
