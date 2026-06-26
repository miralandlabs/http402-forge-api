use std::sync::Arc;

use tokio::sync::broadcast;

use crate::auth::SellerAuth;
use crate::config::{AppConfig, ClusterConfig};
use crate::db::Database;
use crate::db::SaleRow;
use crate::rate_limit::RateLimiter;
use crate::storage::{ObjectStore, Storage};
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
        let state = Self {
            config,
            cluster,
            db,
            storage,
            facilitator,
            seller_auth: SellerAuth::default(),
            sale_events,
            rate_limiter: RateLimiter::from_env(),
        };
        state.backfill_preview_content_types().await?;
        Ok(state)
    }

    async fn backfill_preview_content_types(&self) -> crate::error::AppResult<()> {
        let rows = self.db.listings_missing_preview_content_type().await?;
        if rows.is_empty() {
            return Ok(());
        }
        tracing::info!(
            count = rows.len(),
            "backfilling preview_content_type for legacy listings"
        );
        for (id, preview_key) in rows {
            match self.storage.head(&preview_key).await {
                Ok(content_type) if !content_type.trim().is_empty() => {
                    if let Err(e) = self.db.set_preview_content_type(id, &content_type).await {
                        tracing::warn!(listing_id = %id, error = %e, "preview content type backfill failed");
                    }
                }
                Ok(_) => tracing::warn!(listing_id = %id, "preview object missing content-type"),
                Err(e) => tracing::warn!(listing_id = %id, error = %e, "preview head failed during backfill"),
            }
        }
        Ok(())
    }
}

pub type SharedState = Arc<AppState>;
