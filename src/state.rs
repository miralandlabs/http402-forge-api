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
                Err(e) => {
                    tracing::warn!(listing_id = %id, error = %e, "preview head failed during backfill")
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Arc;

    use crate::auth::SellerAuth;
    use crate::config::{
        AppConfig, ClusterConfig, ModerationConfig, ModerationProvider, ObjectDelivery,
        SolanaCluster, StorageBackend,
    };
    use crate::db::Database;
    use crate::rate_limit::RateLimiter;
    use crate::storage::Storage;
    use crate::x402::Facilitator;

    use super::{AppState, SharedState};

    pub async fn test_state(oracle_authority: Option<&str>) -> (SharedState, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("forge.db");
        let storage_path = tmp.path().join("objects");
        let config = AppConfig {
            cluster: SolanaCluster::Devnet,
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            seller_public_base_url: "http://127.0.0.1:8092".into(),
            database_url: format!("sqlite:{}", db_path.display()),
            facilitator_base_url: "http://127.0.0.1:1".into(),
            facilitator_timeout_secs: 1,
            payment_timeout_secs: 30,
            storage_backend: StorageBackend::Local,
            local_storage_path: storage_path,
            r2_account_id: None,
            r2_bucket: None,
            r2_access_key_id: None,
            r2_secret_access_key: None,
            max_asset_bytes: crate::config::DEFAULT_MAX_ASSET_BYTES,
            max_preview_bytes: crate::config::DEFAULT_MAX_PREVIEW_BYTES,
            preview_media_seconds: 30,
            ffmpeg_bin: "ffmpeg".into(),
            pdftoppm_bin: "pdftoppm".into(),
            gs_bin: "gs".into(),
            mutool_bin: "mutool".into(),
            escrow_size_threshold: crate::config::DEFAULT_ESCROW_SIZE_THRESHOLD_BYTES,
            platform_fee_bps: 0,
            platform_fee_wallet: None,
            oracle_authorities: oracle_authority
                .map(|s| vec![s.to_string()])
                .unwrap_or_default(),
            oracle_profile_id: "x402/oracles/file-delivery/attestation/v1".into(),
            skip_seller_vault_check: true,
            skip_seller_auth: true,
            skip_buyer_auth: true,
            moderation: ModerationConfig {
                provider: ModerationProvider::None,
                openai_api_key: None,
                fail_closed: false,
            },
            cors_allowed_origins: vec!["http://127.0.0.1:5175".into()],
            object_delivery: ObjectDelivery::Proxy,
            presign_ttl_secs: 300,
            version: "0.1.0".into(),
            leaderboard_limit: 5,
        };
        let db = Database::connect(&config.database_url)
            .await
            .expect("test db");
        let storage = Storage::from_config(&config).await.expect("test storage");
        let facilitator = Facilitator::new(&config).expect("test facilitator");
        let (sale_events, _) = tokio::sync::broadcast::channel(8);
        let state = AppState {
            cluster: ClusterConfig::for_cluster(config.cluster),
            config,
            db,
            storage,
            facilitator,
            seller_auth: SellerAuth::default(),
            sale_events,
            rate_limiter: RateLimiter::from_env(),
        };
        (Arc::new(state), tmp)
    }
}

pub type SharedState = Arc<AppState>;
