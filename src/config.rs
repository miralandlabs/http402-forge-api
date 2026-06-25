pub const SOLANA_MAINNET_NETWORK: &str = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp";
pub const SOLANA_DEVNET_NETWORK: &str = "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1";
pub const DEFAULT_SCHEME_EXACT: &str = "v2:solana:exact";
pub const DEFAULT_SCHEME_ESCROW: &str = "sla-escrow";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SolanaCluster {
    Mainnet,
    Devnet,
}

impl SolanaCluster {
    pub fn from_env() -> Result<Self, String> {
        match std::env::var("SOLANA_CLUSTER").as_deref() {
            Ok("mainnet") => Ok(Self::Mainnet),
            Ok("devnet") => Ok(Self::Devnet),
            Ok(v) => Err(format!(
                "SOLANA_CLUSTER must be 'mainnet' or 'devnet'; got '{v}'"
            )),
            Err(_) => Err("SOLANA_CLUSTER must be 'mainnet' or 'devnet'".into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClusterConfig {
    pub usdc_mint: String,
    pub network: String,
    pub label: &'static str,
}

impl ClusterConfig {
    pub fn for_cluster(cluster: SolanaCluster) -> Self {
        match cluster {
            SolanaCluster::Mainnet => Self {
                usdc_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".into(),
                network: SOLANA_MAINNET_NETWORK.into(),
                label: "mainnet",
            },
            SolanaCluster::Devnet => Self {
                usdc_mint: "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU".into(),
                network: SOLANA_DEVNET_NETWORK.into(),
                label: "devnet",
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageBackend {
    Local,
    R2,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub cluster: SolanaCluster,
    pub bind_addr: std::net::SocketAddr,
    pub seller_public_base_url: String,
    pub database_url: String,
    pub facilitator_base_url: String,
    pub facilitator_timeout_secs: u64,
    pub payment_timeout_secs: u64,
    pub storage_backend: StorageBackend,
    pub local_storage_path: std::path::PathBuf,
    pub r2_account_id: Option<String>,
    pub r2_bucket: Option<String>,
    pub r2_access_key_id: Option<String>,
    pub r2_secret_access_key: Option<String>,
    pub max_asset_bytes: u64,
    pub max_preview_bytes: u64,
    pub preview_media_seconds: u32,
    pub ffmpeg_bin: String,
    pub pdftoppm_bin: String,
    pub gs_bin: String,
    pub mutool_bin: String,
    pub escrow_size_threshold: u64,
    pub platform_fee_bps: u16,
    pub platform_fee_wallet: Option<String>,
    pub oracle_authorities: Vec<String>,
    pub oracle_profile_id: String,
    pub skip_seller_vault_check: bool,
    pub skip_seller_auth: bool,
    pub cors_allowed_origins: Vec<String>,
    pub version: String,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, String> {
        let cluster = SolanaCluster::from_env()?;
        let default_bind = match cluster {
            SolanaCluster::Devnet => "127.0.0.1:8092",
            SolanaCluster::Mainnet => "127.0.0.1:8093",
        };
        let bind_addr: std::net::SocketAddr = std::env::var("BIND_ADDR")
            .unwrap_or_else(|_| default_bind.into())
            .parse()
            .map_err(|e| format!("invalid BIND_ADDR: {e}"))?;

        let storage_backend = match std::env::var("STORAGE_BACKEND")
            .unwrap_or_else(|_| "local".into())
            .to_ascii_lowercase()
            .as_str()
        {
            "local" => StorageBackend::Local,
            "r2" => StorageBackend::R2,
            v => {
                return Err(format!(
                    "STORAGE_BACKEND must be 'local' or 'r2'; got '{v}'"
                ))
            }
        };

        let oracle_authorities = std::env::var("ORACLE_AUTHORITIES")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();

        let database_url = resolve_database_url(cluster);

        Ok(Self {
            cluster,
            bind_addr,
            seller_public_base_url: std::env::var("SELLER_PUBLIC_BASE_URL")
                .map_err(|_| "SELLER_PUBLIC_BASE_URL is required".to_string())?,
            database_url,
            facilitator_base_url: std::env::var("FACILITATOR_BASE_URL")
                .map_err(|_| "FACILITATOR_BASE_URL is required".to_string())?,
            facilitator_timeout_secs: env_u64("FACILITATOR_TIMEOUT_SECS", 15),
            payment_timeout_secs: env_u64("PAYMENT_TIMEOUT_SECS", 300),
            storage_backend,
            local_storage_path: std::path::PathBuf::from(
                std::env::var("LOCAL_STORAGE_PATH").unwrap_or_else(|_| "./data/objects".into()),
            ),
            r2_account_id: std::env::var("R2_ACCOUNT_ID").ok(),
            r2_bucket: std::env::var("R2_BUCKET").ok(),
            r2_access_key_id: std::env::var("R2_ACCESS_KEY_ID").ok(),
            r2_secret_access_key: std::env::var("R2_SECRET_ACCESS_KEY").ok(),
            max_asset_bytes: env_u64("MAX_ASSET_BYTES", 52_428_800),
            max_preview_bytes: env_u64("MAX_PREVIEW_BYTES", 5_242_880),
            preview_media_seconds: env_u32("PREVIEW_MEDIA_SECONDS", 30),
            ffmpeg_bin: std::env::var("FFMPEG_BIN").unwrap_or_else(|_| "ffmpeg".into()),
            pdftoppm_bin: std::env::var("PDFTOPPM_BIN").unwrap_or_else(|_| "pdftoppm".into()),
            gs_bin: std::env::var("GS_BIN").unwrap_or_else(|_| "gs".into()),
            mutool_bin: std::env::var("MUTOOL_BIN").unwrap_or_else(|_| "mutool".into()),
            escrow_size_threshold: env_u64("ESCROW_SIZE_THRESHOLD_BYTES", 10_485_760),
            platform_fee_bps: env_u16("PLATFORM_FEE_BPS", 0),
            platform_fee_wallet: std::env::var("PLATFORM_FEE_WALLET")
                .ok()
                .filter(|s| !s.trim().is_empty()),
            oracle_authorities,
            oracle_profile_id: std::env::var("ORACLE_PROFILE_ID")
                .unwrap_or_else(|_| "x402/oracles/file-delivery/attestation/v1".into()),
            skip_seller_vault_check: match std::env::var("SKIP_SELLER_VAULT_CHECK") {
                Ok(v) if v == "1" || v.eq_ignore_ascii_case("true") => true,
                Ok(v) if v == "0" || v.eq_ignore_ascii_case("false") => false,
                Ok(v) => return Err(format!("SKIP_SELLER_VAULT_CHECK must be 0 or 1; got '{v}'")),
                Err(_) => false,
            },
            skip_seller_auth: match std::env::var("SKIP_SELLER_AUTH") {
                Ok(v) if v == "1" || v.eq_ignore_ascii_case("true") => true,
                Ok(v) if v == "0" || v.eq_ignore_ascii_case("false") => false,
                Ok(v) => return Err(format!("SKIP_SELLER_AUTH must be 0 or 1; got '{v}'")),
                Err(_) => false,
            },
            cors_allowed_origins: parse_cors_origins(cluster),
            version: std::env::var("FORGE_VERSION").unwrap_or_else(|_| "0.1.0".into()),
        })
    }
}

fn resolve_database_url(cluster: SolanaCluster) -> String {
    if let Ok(url) = std::env::var("DATABASE_URL") {
        return url;
    }
    match cluster {
        SolanaCluster::Devnet => "sqlite:./data/forge.db".into(),
        SolanaCluster::Mainnet => "postgres://forge:forge@127.0.0.1:5432/forge".into(),
    }
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_u16(key: &str, default: u16) -> u16 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn parse_cors_origins(cluster: SolanaCluster) -> Vec<String> {
    if let Ok(raw) = std::env::var("CORS_ALLOWED_ORIGINS") {
        return raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
    }
    match cluster {
        SolanaCluster::Devnet => vec![
            "http://localhost:5175".into(),
            "http://127.0.0.1:5175".into(),
            "https://preview.http402.trade".into(),
        ],
        SolanaCluster::Mainnet => vec![
            "https://http402.trade".into(),
            "https://www.http402.trade".into(),
        ],
    }
}
