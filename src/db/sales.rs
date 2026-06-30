use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize)]
pub struct SaleRow {
    pub id: Uuid,
    pub listing_id: Uuid,
    pub seller_wallet: String,
    pub buyer_wallet: String,
    pub amount_micro_usdc: i64,
    pub tx_signature: String,
    pub settled_at: DateTime<Utc>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaderboardWalletRow {
    pub wallet: String,
    pub amount_micro_usdc: i64,
    pub sales_count: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaderboardListingRow {
    pub listing_id: Uuid,
    pub title: String,
    pub sales_count: i64,
    pub volume_micro_usdc: i64,
}

/// Buyer-facing purchase row (sale joined with listing summary).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuyerPurchaseRow {
    pub sale_id: Uuid,
    pub listing_id: Uuid,
    pub listing_title: String,
    pub listing_status: String,
    pub seller_wallet: String,
    pub amount_micro_usdc: i64,
    pub tx_signature: String,
    pub settled_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback_outcome: Option<String>,
}
