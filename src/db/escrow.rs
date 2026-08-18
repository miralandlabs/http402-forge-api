use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EscrowFundBind {
    pub listing_id: Uuid,
    pub payment_uid: String,
    pub content_hash: String,
    pub oracle_authority: String,
}
