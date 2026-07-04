use chrono::{DateTime, Utc};
use uuid::Uuid;

pub const FEEDBACK_OUTCOMES: &[&str] = &[
    "as_described",
    "hash_mismatch",
    "corrupt",
    "misleading",
    "other",
];

#[derive(Debug, Clone)]
pub struct SaleFeedbackRow {
    pub sale_id: Uuid,
    pub listing_id: Uuid,
    #[allow(dead_code)]
    pub buyer_wallet: String,
    pub outcome: String,
    pub score: Option<i16>,
    #[allow(dead_code)]
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
pub struct ListingQualityStats {
    pub quality_score: i32,
    pub verified_feedback_count: i64,
}

pub fn validate_feedback_outcome(outcome: &str) -> Result<(), String> {
    if FEEDBACK_OUTCOMES.contains(&outcome) {
        Ok(())
    } else {
        Err(format!(
            "outcome must be one of: {}",
            FEEDBACK_OUTCOMES.join(", ")
        ))
    }
}

// Quality scoring (outcome → points, averaged per listing) lives in SQL:
// see the CASE expressions in sqlite.rs / postgres.rs listing quality stats queries.
