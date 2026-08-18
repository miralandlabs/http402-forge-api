use std::sync::Arc;

use crate::config::{
    AppConfig, ClusterConfig, ModerationConfig, ModerationProvider, ObjectDelivery, SolanaCluster,
    StorageBackend, DEFAULT_ESCROW_SIZE_THRESHOLD_BYTES, DEFAULT_MAX_ASSET_BYTES,
    DEFAULT_MAX_PREVIEW_BYTES,
};
use crate::db::Database;
use crate::rate_limit::RateLimiter;
use crate::state::{AppState, SharedState};
use crate::storage::Storage;
use crate::x402::Facilitator;

pub struct TestEnv {
    pub state: SharedState,
    _tmp: tempfile::TempDir,
}

impl TestEnv {
    pub async fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("forge.db");
        let objects = tmp.path().join("objects");
        let config = test_config(
            &format!("sqlite:{}", db_path.display()),
            objects,
            "https://preview.forge.http402.trade",
        );
        let cluster = ClusterConfig::for_cluster(config.cluster);
        let db = Database::connect(&config.database_url)
            .await
            .expect("test db");
        let storage = Storage::from_config(&config).await.expect("test storage");
        let facilitator = Facilitator::new(&config).expect("test facilitator");
        let (sale_events, _) = tokio::sync::broadcast::channel(8);
        let state = Arc::new(AppState {
            config,
            cluster,
            db,
            storage,
            facilitator,
            seller_auth: crate::auth::SellerAuth::default(),
            sale_events,
            rate_limiter: RateLimiter::from_env(),
        });
        Self { state, _tmp: tmp }
    }
}

fn test_config(
    database_url: &str,
    local_storage_path: std::path::PathBuf,
    seller_public_base_url: &str,
) -> AppConfig {
    AppConfig {
        cluster: SolanaCluster::Devnet,
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        seller_public_base_url: seller_public_base_url.into(),
        database_url: database_url.into(),
        facilitator_base_url: "http://127.0.0.1:1".into(),
        facilitator_timeout_secs: 1,
        payment_timeout_secs: 300,
        storage_backend: StorageBackend::Local,
        local_storage_path,
        r2_account_id: None,
        r2_bucket: None,
        r2_access_key_id: None,
        r2_secret_access_key: None,
        max_asset_bytes: DEFAULT_MAX_ASSET_BYTES,
        max_preview_bytes: DEFAULT_MAX_PREVIEW_BYTES,
        preview_media_seconds: 30,
        ffmpeg_bin: "ffmpeg".into(),
        pdftoppm_bin: "pdftoppm".into(),
        gs_bin: "gs".into(),
        mutool_bin: "mutool".into(),
        escrow_size_threshold: DEFAULT_ESCROW_SIZE_THRESHOLD_BYTES,
        platform_fee_bps: 0,
        platform_fee_wallet: None,
        oracle_authorities: Vec::new(),
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
        version: "test".into(),
        leaderboard_limit: 5,
    }
}
