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

#[allow(dead_code)]
pub fn outcome_quality_points(outcome: &str) -> i32 {
    match outcome {
        "as_described" => 100,
        "hash_mismatch" => 0,
        "corrupt" => 25,
        "misleading" => 35,
        "other" => 50,
        _ => 50,
    }
}

#[allow(dead_code)]
pub fn compute_quality_score(outcomes: &[(String, i64)]) -> ListingQualityStats {
    if outcomes.is_empty() {
        return ListingQualityStats::default();
    }
    let mut total_weight = 0i64;
    let mut count = 0i64;
    for (outcome, n) in outcomes {
        let points = outcome_quality_points(outcome) as i64;
        total_weight += points * n;
        count += n;
    }
    if count == 0 {
        return ListingQualityStats::default();
    }
    ListingQualityStats {
        quality_score: (total_weight / count) as i32,
        verified_feedback_count: count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_score_averages_outcomes() {
        let stats =
            compute_quality_score(&[("as_described".into(), 1), ("hash_mismatch".into(), 1)]);
        assert_eq!(stats.quality_score, 50);
        assert_eq!(stats.verified_feedback_count, 2);
    }
}
