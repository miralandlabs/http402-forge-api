use std::sync::Arc;

use tokio::sync::broadcast;

use crate::auth::SellerAuth;
use crate::config::{AppConfig, ClusterConfig};
use crate::db::Database;
use crate::db::SaleRow;
use crate::rate_limit::RateLimiter;
use crate::storage::Storage;
use crate::x402::Facilitator;

pub struct AppState {
    pub config: AppConfig,
    pub cluster: ClusterConfig,
    pub db: Database,
    pub storage: Storage,
    pub facilitator: Facilitator,
    pub seller_auth: SellerAuth,
    pub sale_events: broadcast::Sender<SaleRow>,
    pub rate_limiter: RateLimiter,
}

impl AppState {
    pub async fn build(
        config: AppConfig,
        cluster: ClusterConfig,
        db: Database,
    ) -> crate::error::AppResult<Self> {
        let storage = Storage::from_config(&config).await?;
        let facilitator = Facilitator::new(&config)
            .map_err(|e| crate::error::AppError::Internal(anyhow::anyhow!("facilitator: {e}")))?;
        let (sale_events, _) = broadcast::channel(256);
        Ok(Self {
            config,
            cluster,
            db,
            storage,
            facilitator,
            seller_auth: SellerAuth::default(),
            sale_events,
            rate_limiter: RateLimiter::from_env(),
        })
    }
}

pub type SharedState = Arc<AppState>;
