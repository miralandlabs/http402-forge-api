use std::io::BufReader;
use std::path::Path;
use std::time::Duration;

use deadpool_postgres::{Config, Pool, PoolConfig, Runtime};
use rustls::{ClientConfig, RootCertStore};
use tokio_postgres::{NoTls, Row};
use tokio_postgres_rustls::MakeRustlsConnect;
use uuid::Uuid;
use webpki_roots::TLS_SERVER_ROOTS;

use super::trust::{ListingQualityStats, SaleFeedbackRow};
use super::{LeaderboardListingRow, LeaderboardWalletRow, ListingRow, PaymentRow, SaleRow};
use crate::db::listing_filters::{listing_filter_suffix, ListingFilterBinds};
use crate::error::{AppError, AppResult};
use tokio_postgres::types::ToSql;

const SCHEMA: &str = include_str!("../../migrations/postgres/001_init.sql");
const SCHEMA_002: &str = include_str!("../../migrations/postgres/002_agent_metadata.sql");
const SCHEMA_003: &str = include_str!("../../migrations/postgres/003_preview_content_type.sql");
const SCHEMA_004: &str = include_str!("../../migrations/postgres/004_trust_moderation.sql");

fn is_supabase_host(database_url: &str) -> bool {
    let lower = database_url.to_ascii_lowercase();
    lower.contains("supabase.co") || lower.contains("supabase.com")
}

fn uses_tls(database_url: &str) -> bool {
    let lower = database_url.to_ascii_lowercase();
    lower.contains("sslmode=require")
        || lower.contains("sslmode=verify-full")
        || lower.contains("sslmode=verify-ca")
}

fn validate_database_url(database_url: &str) -> AppResult<()> {
    if is_supabase_host(database_url) && !uses_tls(database_url) {
        return Err(AppError::Internal(anyhow::anyhow!(
            "Supabase DATABASE_URL must include ?sslmode=require (or verify-full)"
        )));
    }
    if is_supabase_host(database_url) && supabase_ca_path().is_none() {
        return Err(AppError::Internal(anyhow::anyhow!(
            "Supabase requires DATABASE_SSL_ROOT_CERT in /etc/forge/*.env \
             (preview: /etc/forge/ssl/supabase-preview-ca.crt, \
             production: /etc/forge/ssl/supabase-prod-ca.crt). \
             Download CA from Supabase Dashboard → Database → SSL Configuration"
        )));
    }
    Ok(())
}

fn supabase_ca_path() -> Option<String> {
    let path = match std::env::var("DATABASE_SSL_ROOT_CERT") {
        Ok(val) => {
            let trimmed = val.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        }
        Err(_) => None,
    };

    let path = path.unwrap_or_else(|| {
        let solana_cluster = std::env::var("SOLANA_CLUSTER").unwrap_or_default();
        let fallback = if solana_cluster == "mainnet" {
            "/etc/forge/ssl/supabase-prod-ca.crt".to_string()
        } else {
            "/etc/forge/ssl/supabase-preview-ca.crt".to_string()
        };
        tracing::info!(
            "DATABASE_SSL_ROOT_CERT not set. Falling back to default CA path for cluster '{}': {}",
            solana_cluster,
            fallback
        );
        fallback
    });

    if !Path::new(&path).is_file() {
        return None;
    }
    Some(path)
}

fn load_pem_into_store(path: &str, roots: &mut RootCertStore) -> AppResult<()> {
    let file = std::fs::File::open(path).map_err(|e| {
        AppError::Internal(anyhow::anyhow!("open DATABASE_SSL_ROOT_CERT {path}: {e}"))
    })?;
    let mut reader = BufReader::new(file);
    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| {
            AppError::Internal(anyhow::anyhow!("parse DATABASE_SSL_ROOT_CERT {path}: {e}"))
        })?;
    if certs.is_empty() {
        return Err(AppError::Internal(anyhow::anyhow!(
            "DATABASE_SSL_ROOT_CERT {path} contains no certificates"
        )));
    }
    for cert in certs {
        roots
            .add(cert)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("invalid cert in {path}: {e}")))?;
    }
    Ok(())
}

fn make_rustls_connector(database_url: &str) -> AppResult<MakeRustlsConnect> {
    let mut roots = RootCertStore::empty();
    roots.extend(TLS_SERVER_ROOTS.iter().cloned());

    if is_supabase_host(database_url) {
        let path = supabase_ca_path().ok_or_else(|| {
            AppError::Internal(anyhow::anyhow!(
                "Supabase DATABASE_SSL_ROOT_CERT missing or not readable (set in /etc/forge/*.env)"
            ))
        })?;
        load_pem_into_store(&path, &mut roots)?;
    }

    for cert in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(cert);
    }

    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(MakeRustlsConnect::new(config))
}

pub fn connect_pool(database_url: &str) -> AppResult<Pool> {
    validate_database_url(database_url)?;
    let mut cfg = Config::new();
    cfg.url = Some(database_url.to_string());
    cfg.pool = Some(PoolConfig {
        max_size: std::env::var("POSTGRES_POOL_MAX_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10),
        ..PoolConfig::default()
    });
    let pool = if uses_tls(database_url) {
        let tls = make_rustls_connector(database_url)?;
        cfg.create_pool(Some(Runtime::Tokio1), tls)
    } else {
        cfg.create_pool(Some(Runtime::Tokio1), NoTls)
    };
    pool.map_err(|e| AppError::Internal(anyhow::anyhow!("postgres pool: {e}")))
}

pub async fn migrate(pool: &Pool) -> AppResult<()> {
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(
            "postgres conn: {e:#} (Supabase: DATABASE_SSL_ROOT_CERT must point at the project CA PEM)"
        )))?;
    client
        .batch_execute(SCHEMA)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres migrate: {e}")))?;
    client
        .batch_execute(SCHEMA_002)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres migrate 002: {e}")))?;
    client
        .batch_execute(SCHEMA_003)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres migrate 003: {e}")))?;
    client
        .batch_execute(SCHEMA_004)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres migrate 004: {e}")))?;
    Ok(())
}

pub async fn health_check(pool: &Pool) -> bool {
    let pool = pool.clone();
    tokio::time::timeout(Duration::from_secs(2), async move {
        let client = pool.get().await.ok()?;
        client.query_one("SELECT 1", &[]).await.ok()?;
        Some(())
    })
    .await
    .ok()
    .flatten()
    .is_some()
}

pub async fn insert_listing(pool: &Pool, row: &ListingRow) -> AppResult<()> {
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;
    client
        .execute(
            r#"
            INSERT INTO listings (
                id, seller_wallet, display_name, title, description, category,
                price_micro_usdc, preview_key, preview_content_type, asset_key, content_type, byte_size,
                agent_friendly, delivery_scheme, status, tags, license, content_hash,
                moderation_status, moderation_labels, created_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21)
            "#,
            &[
                &row.id,
                &row.seller_wallet,
                &row.display_name,
                &row.title,
                &row.description,
                &row.category,
                &row.price_micro_usdc,
                &row.preview_key,
                &row.preview_content_type,
                &row.asset_key,
                &row.content_type,
                &row.byte_size,
                &row.agent_friendly,
                &row.delivery_scheme,
                &row.status,
                &row.tags,
                &row.license,
                &row.content_hash,
                &row.moderation_status,
                &row.moderation_labels,
                &row.created_at,
            ],
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("insert listing: {e}")))?;
    Ok(())
}

pub async fn get_listing(pool: &Pool, id: Uuid) -> AppResult<ListingRow> {
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;
    let row = client
        .query_opt(
            "SELECT * FROM listings WHERE id = $1 AND status = 'active'",
            &[&id],
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("get listing: {e}")))?
        .ok_or(AppError::NotFound)?;
    Ok(map_listing(&row))
}

pub async fn get_listing_any(pool: &Pool, id: Uuid) -> AppResult<ListingRow> {
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;
    let row = client
        .query_opt("SELECT * FROM listings WHERE id = $1", &[&id])
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("get listing: {e}")))?
        .ok_or(AppError::NotFound)?;
    Ok(map_listing(&row))
}

pub async fn soft_delist_listing(pool: &Pool, id: Uuid, seller_wallet: &str) -> AppResult<bool> {
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;
    let n = client
        .execute(
            "UPDATE listings SET status = 'removed' WHERE id = $1 AND seller_wallet = $2 AND status = 'active'",
            &[&id, &seller_wallet],
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("soft delist: {e}")))?;
    Ok(n > 0)
}

pub async fn count_listings(pool: &Pool, binds: &ListingFilterBinds) -> AppResult<i64> {
    let (suffix, _) = listing_filter_suffix(binds, 1, false);
    let sql = format!("SELECT COUNT(*) FROM listings WHERE status = 'active'{suffix}");
    let mut params: Vec<&(dyn ToSql + Sync)> = Vec::new();
    if let Some(ref c) = binds.category {
        params.push(c);
    }
    if let Some(af) = binds.agent_friendly.as_ref() {
        params.push(af);
    }
    if let Some(ref w) = binds.seller_wallet {
        params.push(w);
    }
    if let Some(ref p) = binds.search_pattern {
        params.push(p);
    }
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;
    let count: i64 = client
        .query_one(&sql, &params)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("count listings: {e}")))?
        .get(0);
    Ok(count)
}

pub async fn list_listings(
    pool: &Pool,
    binds: &ListingFilterBinds,
    sort: &str,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<ListingRow>> {
    let order = match sort {
        "price_asc" => "price_micro_usdc ASC, created_at DESC",
        "price_desc" => "price_micro_usdc DESC, created_at DESC",
        "newest" => "created_at DESC",
        "trending" => "(SELECT COUNT(*) FROM sales s WHERE s.listing_id = listings.id AND s.settled_at >= NOW() - INTERVAL '24 hours') DESC, created_at DESC",
        "quality" => "(SELECT CASE WHEN COUNT(*) >= 2 THEN AVG(CASE sf.outcome WHEN 'as_described' THEN 100 WHEN 'hash_mismatch' THEN 0 WHEN 'corrupt' THEN 25 WHEN 'misleading' THEN 35 ELSE 50 END) ELSE NULL END FROM sale_feedback sf WHERE sf.listing_id = listings.id) DESC NULLS LAST, created_at DESC",
        _ => "(SELECT COUNT(*) FROM sales s WHERE s.listing_id = listings.id AND s.settled_at >= NOW() - INTERVAL '24 hours') DESC, created_at DESC",
    };
    let (suffix, next_idx) = listing_filter_suffix(binds, 1, false);
    let sql = format!(
        "SELECT * FROM listings WHERE status = 'active'{suffix} ORDER BY {order} LIMIT ${next_idx} OFFSET ${}",
        next_idx + 1
    );
    let mut params: Vec<&(dyn ToSql + Sync)> = Vec::new();
    if let Some(ref c) = binds.category {
        params.push(c);
    }
    if let Some(af) = binds.agent_friendly.as_ref() {
        params.push(af);
    }
    if let Some(ref w) = binds.seller_wallet {
        params.push(w);
    }
    if let Some(ref p) = binds.search_pattern {
        params.push(p);
    }
    params.push(&limit);
    params.push(&offset);
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;
    let rows = client
        .query(&sql, &params)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("list listings: {e}")))?;

    Ok(rows.iter().map(map_listing).collect())
}

pub async fn find_by_idempotency(pool: &Pool, key: &str) -> AppResult<Option<PaymentRow>> {
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;
    let row = client
        .query_opt(
            "SELECT buyer_wallet, tx_signature FROM payments WHERE idempotency_key = $1",
            &[&key],
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("find payment: {e}")))?;
    Ok(row.as_ref().map(map_payment))
}

pub async fn insert_payment(
    pool: &Pool,
    key: &str,
    listing_id: Uuid,
    buyer_wallet: &str,
    tx_signature: &str,
) -> AppResult<()> {
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;
    client
        .execute(
            r#"
            INSERT INTO payments (idempotency_key, listing_id, buyer_wallet, tx_signature)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (idempotency_key) DO NOTHING
            "#,
            &[&key, &listing_id, &buyer_wallet, &tx_signature],
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("insert payment: {e}")))?;
    Ok(())
}

pub async fn insert_sale(
    pool: &Pool,
    listing_id: Uuid,
    seller_wallet: &str,
    buyer_wallet: &str,
    amount_micro_usdc: i64,
    tx_signature: &str,
) -> AppResult<SaleRow> {
    let id = Uuid::new_v4();
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;
    client
        .execute(
            r#"
            INSERT INTO sales (id, listing_id, seller_wallet, buyer_wallet, amount_micro_usdc, tx_signature)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
            &[
                &id,
                &listing_id,
                &seller_wallet,
                &buyer_wallet,
                &amount_micro_usdc,
                &tx_signature,
            ],
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("insert sale: {e}")))?;
    let row = client
        .query_one("SELECT * FROM sales WHERE id = $1", &[&id])
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("fetch sale: {e}")))?;
    Ok(map_sale(&row))
}

pub async fn top_earners_24h(pool: &Pool, limit: u32) -> AppResult<Vec<LeaderboardWalletRow>> {
    query_leaderboard_wallets(
        pool,
        "SELECT * FROM leaderboard_earners_24h LIMIT $1",
        i64::from(limit),
    )
    .await
}

pub async fn top_payers_24h(pool: &Pool, limit: u32) -> AppResult<Vec<LeaderboardWalletRow>> {
    query_leaderboard_wallets(
        pool,
        "SELECT * FROM leaderboard_payers_24h LIMIT $1",
        i64::from(limit),
    )
    .await
}

pub async fn hottest_listings_24h(
    pool: &Pool,
    limit: u32,
) -> AppResult<Vec<LeaderboardListingRow>> {
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;
    let rows = client
        .query(
            "SELECT * FROM leaderboard_hottest_24h LIMIT $1",
            &[&i64::from(limit)],
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("hottest listings: {e}")))?;
    Ok(rows
        .iter()
        .map(|row| LeaderboardListingRow {
            listing_id: row.get("listing_id"),
            title: row.get("title"),
            sales_count: row.get("sales_count"),
            volume_micro_usdc: row.get("volume_micro_usdc"),
        })
        .collect())
}

async fn query_leaderboard_wallets(
    pool: &Pool,
    sql: &str,
    limit: i64,
) -> AppResult<Vec<LeaderboardWalletRow>> {
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;
    let rows = client
        .query(sql, &[&limit])
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("leaderboard: {e}")))?;
    Ok(rows
        .iter()
        .map(|row| LeaderboardWalletRow {
            wallet: row.get("wallet"),
            amount_micro_usdc: row.get("amount_micro_usdc"),
            sales_count: row.get("sales_count"),
        })
        .collect())
}

fn map_listing(row: &Row) -> ListingRow {
    ListingRow {
        id: row.get("id"),
        seller_wallet: row.get("seller_wallet"),
        display_name: row.get("display_name"),
        title: row.get("title"),
        description: row.get("description"),
        category: row.get("category"),
        price_micro_usdc: row.get("price_micro_usdc"),
        preview_key: row.get("preview_key"),
        preview_content_type: row.get("preview_content_type"),
        asset_key: row.get("asset_key"),
        content_type: row.get("content_type"),
        byte_size: row.get("byte_size"),
        agent_friendly: row.get("agent_friendly"),
        delivery_scheme: row.get("delivery_scheme"),
        status: row.get("status"),
        tags: row.get("tags"),
        license: row.get("license"),
        content_hash: row.get("content_hash"),
        moderation_status: row.get("moderation_status"),
        moderation_labels: row.get("moderation_labels"),
        created_at: row.get("created_at"),
    }
}

fn map_payment(row: &Row) -> PaymentRow {
    PaymentRow {
        buyer_wallet: row.get("buyer_wallet"),
        tx_signature: row.get("tx_signature"),
    }
}

fn map_sale(row: &Row) -> SaleRow {
    SaleRow {
        id: row.get("id"),
        listing_id: row.get("listing_id"),
        seller_wallet: row.get("seller_wallet"),
        buyer_wallet: row.get("buyer_wallet"),
        amount_micro_usdc: row.get("amount_micro_usdc"),
        tx_signature: row.get("tx_signature"),
        settled_at: row.get("settled_at"),
    }
}

pub async fn listings_missing_preview_content_type(pool: &Pool) -> AppResult<Vec<(Uuid, String)>> {
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;
    let rows = client
        .query(
            "SELECT id, preview_key FROM listings WHERE preview_content_type = ''",
            &[],
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("list preview content types: {e}")))?;
    Ok(rows
        .iter()
        .map(|row| (row.get("id"), row.get("preview_key")))
        .collect())
}

pub async fn set_preview_content_type(
    pool: &Pool,
    id: Uuid,
    preview_content_type: &str,
) -> AppResult<()> {
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;
    client
        .execute(
            "UPDATE listings SET preview_content_type = $2 WHERE id = $1",
            &[&id, &preview_content_type],
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("set preview content type: {e}")))?;
    Ok(())
}

pub async fn is_content_hash_blocked(pool: &Pool, content_hash: &str) -> AppResult<bool> {
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;
    let row = client
        .query_opt(
            "SELECT 1 FROM blocked_content_hashes WHERE content_hash = $1",
            &[&content_hash],
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("hash blocklist: {e}")))?;
    Ok(row.is_some())
}

pub async fn get_sale(pool: &Pool, sale_id: Uuid) -> AppResult<SaleRow> {
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;
    let row = client
        .query_opt("SELECT * FROM sales WHERE id = $1", &[&sale_id])
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("get sale: {e}")))?
        .ok_or(AppError::NotFound)?;
    Ok(map_sale(&row))
}

pub async fn find_sale_by_payment(
    pool: &Pool,
    listing_id: Uuid,
    buyer_wallet: &str,
    tx_signature: &str,
) -> AppResult<Option<SaleRow>> {
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;
    let row = client
        .query_opt(
            r#"
            SELECT * FROM sales
            WHERE listing_id = $1 AND buyer_wallet = $2 AND tx_signature = $3
            ORDER BY settled_at DESC
            LIMIT 1
            "#,
            &[&listing_id, &buyer_wallet, &tx_signature],
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("find sale: {e}")))?;
    Ok(row.as_ref().map(map_sale))
}

pub async fn find_buyer_sale_for_listing(
    pool: &Pool,
    listing_id: Uuid,
    buyer_wallet: &str,
) -> AppResult<Option<SaleRow>> {
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;
    let row = client
        .query_opt(
            r#"
            SELECT * FROM sales
            WHERE listing_id = $1 AND buyer_wallet = $2
            ORDER BY settled_at DESC
            LIMIT 1
            "#,
            &[&listing_id, &buyer_wallet],
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("find buyer sale: {e}")))?;
    Ok(row.as_ref().map(map_sale))
}

pub async fn insert_sale_feedback(
    pool: &Pool,
    sale_id: Uuid,
    listing_id: Uuid,
    buyer_wallet: &str,
    outcome: &str,
    score: Option<i16>,
    note: Option<&str>,
) -> AppResult<SaleFeedbackRow> {
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;
    client
        .execute(
            r#"
            INSERT INTO sale_feedback (sale_id, listing_id, buyer_wallet, outcome, score, note)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
            &[
                &sale_id,
                &listing_id,
                &buyer_wallet,
                &outcome,
                &score,
                &note,
            ],
        )
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("duplicate key") || msg.contains("unique constraint") {
                AppError::Conflict("feedback already submitted for this sale".into())
            } else {
                AppError::Internal(anyhow::anyhow!("insert sale feedback: {e}"))
            }
        })?;
    let row = client
        .query_one(
            "SELECT * FROM sale_feedback WHERE sale_id = $1",
            &[&sale_id],
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("fetch sale feedback: {e}")))?;
    Ok(map_sale_feedback(&row))
}

pub async fn listing_quality_stats(
    pool: &Pool,
    listing_ids: &[Uuid],
) -> AppResult<std::collections::HashMap<Uuid, ListingQualityStats>> {
    if listing_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;
    let rows = client
        .query(
            r#"
            SELECT listing_id,
                   COUNT(*)::BIGINT AS feedback_count,
                   AVG(CASE outcome
                     WHEN 'as_described' THEN 100
                     WHEN 'hash_mismatch' THEN 0
                     WHEN 'corrupt' THEN 25
                     WHEN 'misleading' THEN 35
                     ELSE 50
                   END)::INT AS quality_score
            FROM sale_feedback
            WHERE listing_id = ANY($1)
            GROUP BY listing_id
            "#,
            &[&listing_ids],
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("listing quality stats: {e}")))?;
    Ok(rows
        .iter()
        .map(|row| {
            (
                row.get("listing_id"),
                ListingQualityStats {
                    quality_score: row.get("quality_score"),
                    verified_feedback_count: row.get("feedback_count"),
                },
            )
        })
        .collect())
}

fn map_sale_feedback(row: &Row) -> SaleFeedbackRow {
    SaleFeedbackRow {
        sale_id: row.get("sale_id"),
        listing_id: row.get("listing_id"),
        buyer_wallet: row.get("buyer_wallet"),
        outcome: row.get("outcome"),
        score: row.get("score"),
        note: row.get("note"),
        created_at: row.get("created_at"),
    }
}
