use std::time::Duration;

use deadpool_postgres::{Config, Pool, PoolConfig, Runtime};
use native_tls::TlsConnector;
use postgres_native_tls::MakeTlsConnector;
use tokio_postgres::{NoTls, Row};
use uuid::Uuid;

use super::{LeaderboardListingRow, LeaderboardWalletRow, ListingRow, PaymentRow, SaleRow};
use crate::db::listing_filters::search_like_pattern;
use crate::error::{AppError, AppResult};

const SCHEMA: &str = include_str!("../../migrations/postgres/001_init.sql");

fn uses_tls(database_url: &str) -> bool {
    let lower = database_url.to_ascii_lowercase();
    lower.contains("sslmode=require")
        || lower.contains("sslmode=verify-full")
        || lower.contains("sslmode=verify-ca")
}

pub fn connect_pool(database_url: &str) -> AppResult<Pool> {
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
        let connector = TlsConnector::builder()
            .build()
            .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres tls: {e}")))?;
        let tls = MakeTlsConnector::new(connector);
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
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;
    client
        .batch_execute(SCHEMA)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres migrate: {e}")))?;
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
                price_micro_usdc, preview_key, asset_key, content_type, byte_size,
                agent_friendly, delivery_scheme, status, created_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
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
                &row.asset_key,
                &row.content_type,
                &row.byte_size,
                &row.agent_friendly,
                &row.delivery_scheme,
                &row.status,
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

pub async fn count_listings(
    pool: &Pool,
    category: Option<&str>,
    agent_friendly: Option<bool>,
    search: Option<&str>,
) -> AppResult<i64> {
    let search_pat = search.map(search_like_pattern);
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;
    let count: i64 = match (category, agent_friendly, search_pat.as_deref()) {
        (Some(cat), Some(af), Some(pat)) => client
            .query_one(
                "SELECT COUNT(*) FROM listings WHERE status = 'active' AND category = $1 AND agent_friendly = $2 AND (title ILIKE $3 OR description ILIKE $3)",
                &[&cat, &af, &pat],
            )
            .await,
        (Some(cat), Some(af), None) => client
            .query_one(
                "SELECT COUNT(*) FROM listings WHERE status = 'active' AND category = $1 AND agent_friendly = $2",
                &[&cat, &af],
            )
            .await,
        (Some(cat), None, Some(pat)) => client
            .query_one(
                "SELECT COUNT(*) FROM listings WHERE status = 'active' AND category = $1 AND (title ILIKE $2 OR description ILIKE $2)",
                &[&cat, &pat],
            )
            .await,
        (Some(cat), None, None) => client
            .query_one(
                "SELECT COUNT(*) FROM listings WHERE status = 'active' AND category = $1",
                &[&cat],
            )
            .await,
        (None, Some(af), Some(pat)) => client
            .query_one(
                "SELECT COUNT(*) FROM listings WHERE status = 'active' AND agent_friendly = $1 AND (title ILIKE $2 OR description ILIKE $2)",
                &[&af, &pat],
            )
            .await,
        (None, Some(af), None) => client
            .query_one(
                "SELECT COUNT(*) FROM listings WHERE status = 'active' AND agent_friendly = $1",
                &[&af],
            )
            .await,
        (None, None, Some(pat)) => client
            .query_one(
                "SELECT COUNT(*) FROM listings WHERE status = 'active' AND (title ILIKE $1 OR description ILIKE $1)",
                &[&pat],
            )
            .await,
        (None, None, None) => client
            .query_one("SELECT COUNT(*) FROM listings WHERE status = 'active'", &[])
            .await,
    }
    .map_err(|e| AppError::Internal(anyhow::anyhow!("count listings: {e}")))?
    .get(0);
    Ok(count)
}

pub async fn list_listings(
    pool: &Pool,
    category: Option<&str>,
    agent_friendly: Option<bool>,
    search: Option<&str>,
    sort: &str,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<ListingRow>> {
    let order = match sort {
        "price_asc" => "price_micro_usdc ASC, created_at DESC",
        "price_desc" => "price_micro_usdc DESC, created_at DESC",
        _ => "created_at DESC",
    };
    let search_pat = search.map(search_like_pattern);
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;

    let rows = match (category, agent_friendly, search_pat.as_deref()) {
        (Some(cat), Some(af), Some(pat)) => {
            let sql = format!(
                "SELECT * FROM listings WHERE status = 'active' AND category = $1 AND agent_friendly = $2 AND (title ILIKE $3 OR description ILIKE $3) ORDER BY {order} LIMIT $4 OFFSET $5"
            );
            client.query(&sql, &[&cat, &af, &pat, &limit, &offset]).await
        }
        (Some(cat), Some(af), None) => {
            let sql = format!(
                "SELECT * FROM listings WHERE status = 'active' AND category = $1 AND agent_friendly = $2 ORDER BY {order} LIMIT $3 OFFSET $4"
            );
            client.query(&sql, &[&cat, &af, &limit, &offset]).await
        }
        (Some(cat), None, Some(pat)) => {
            let sql = format!(
                "SELECT * FROM listings WHERE status = 'active' AND category = $1 AND (title ILIKE $2 OR description ILIKE $2) ORDER BY {order} LIMIT $3 OFFSET $4"
            );
            client.query(&sql, &[&cat, &pat, &limit, &offset]).await
        }
        (Some(cat), None, None) => {
            let sql = format!(
                "SELECT * FROM listings WHERE status = 'active' AND category = $1 ORDER BY {order} LIMIT $2 OFFSET $3"
            );
            client.query(&sql, &[&cat, &limit, &offset]).await
        }
        (None, Some(af), Some(pat)) => {
            let sql = format!(
                "SELECT * FROM listings WHERE status = 'active' AND agent_friendly = $1 AND (title ILIKE $2 OR description ILIKE $2) ORDER BY {order} LIMIT $3 OFFSET $4"
            );
            client.query(&sql, &[&af, &pat, &limit, &offset]).await
        }
        (None, Some(af), None) => {
            let sql = format!(
                "SELECT * FROM listings WHERE status = 'active' AND agent_friendly = $1 ORDER BY {order} LIMIT $2 OFFSET $3"
            );
            client.query(&sql, &[&af, &limit, &offset]).await
        }
        (None, None, Some(pat)) => {
            let sql = format!(
                "SELECT * FROM listings WHERE status = 'active' AND (title ILIKE $1 OR description ILIKE $1) ORDER BY {order} LIMIT $2 OFFSET $3"
            );
            client.query(&sql, &[&pat, &limit, &offset]).await
        }
        (None, None, None) => {
            let sql = format!(
                "SELECT * FROM listings WHERE status = 'active' ORDER BY {order} LIMIT $1 OFFSET $2"
            );
            client.query(&sql, &[&limit, &offset]).await
        }
    }
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

pub async fn top_earners_24h(pool: &Pool) -> AppResult<Vec<LeaderboardWalletRow>> {
    query_leaderboard_wallets(pool, "SELECT * FROM leaderboard_earners_24h").await
}

pub async fn top_payers_24h(pool: &Pool) -> AppResult<Vec<LeaderboardWalletRow>> {
    query_leaderboard_wallets(pool, "SELECT * FROM leaderboard_payers_24h").await
}

pub async fn hottest_listings_24h(pool: &Pool) -> AppResult<Vec<LeaderboardListingRow>> {
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;
    let rows = client
        .query("SELECT * FROM leaderboard_hottest_24h", &[])
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

async fn query_leaderboard_wallets(pool: &Pool, sql: &str) -> AppResult<Vec<LeaderboardWalletRow>> {
    let client = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("postgres conn: {e}")))?;
    let rows = client
        .query(sql, &[])
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
        asset_key: row.get("asset_key"),
        content_type: row.get("content_type"),
        byte_size: row.get("byte_size"),
        agent_friendly: row.get("agent_friendly"),
        delivery_scheme: row.get("delivery_scheme"),
        status: row.get("status"),
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
