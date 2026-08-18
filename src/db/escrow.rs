use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct EscrowFundBindRow {
    pub listing_id: Uuid,
    pub payment_uid: String,
    pub content_hash: String,
    pub oracle_authority: String,
    pub created_at: DateTime<Utc>,
}
