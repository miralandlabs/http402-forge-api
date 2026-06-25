use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::state::SharedState;

pub async fn sse(State(state): State<SharedState>) -> impl IntoResponse {
    let rx = state.sale_events.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|msg| {
        msg.ok().map(|sale| {
            let payload = serde_json::json!({
                "listing_id": sale.listing_id,
                "seller_wallet": sale.seller_wallet,
                "buyer_wallet": sale.buyer_wallet,
                "amount_micro_usdc": sale.amount_micro_usdc,
            });
            Ok::<Event, Infallible>(Event::default().event("sale").json_data(payload).unwrap())
        })
    });

    Sse::new(Box::pin(stream)).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
