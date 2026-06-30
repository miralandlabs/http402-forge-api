use axum::extract::State;
use axum::Json;
use serde::Serialize;

use crate::db::{LeaderboardListingRow, LeaderboardWalletRow};
use crate::error::AppResult;
use crate::state::SharedState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaderboardsResponse {
    pub top_earners_24h: Vec<LeaderboardWalletRow>,
    pub top_payers_24h: Vec<LeaderboardWalletRow>,
    pub hottest_listings_24h: Vec<LeaderboardListingRow>,
}

pub async fn leaderboards(
    State(state): State<SharedState>,
) -> AppResult<Json<LeaderboardsResponse>> {
    Ok(Json(LeaderboardsResponse {
        top_earners_24h: state
            .db
            .top_earners_24h(state.config.leaderboard_limit)
            .await?,
        top_payers_24h: state
            .db
            .top_payers_24h(state.config.leaderboard_limit)
            .await?,
        hottest_listings_24h: state
            .db
            .hottest_listings_24h(state.config.leaderboard_limit)
            .await?,
    }))
}
