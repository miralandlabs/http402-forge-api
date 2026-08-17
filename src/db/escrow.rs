use uuid::Uuid;

/// A funded sla-escrow payment bound to a listing.
///
/// The row is written once, at successful `FundPayment`, and pins the
/// `oracle_authority` that may stream the listing's artifact through the
/// verdict door. `content_hash` is copied from the immutable listing row.
#[derive(Debug, Clone)]
pub struct EscrowFundBindRow {
    pub listing_id: Uuid,
    pub payment_uid: String,
    pub content_hash: String,
    pub oracle_authority: String,
}
