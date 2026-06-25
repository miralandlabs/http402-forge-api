use std::time::Duration;

use chrono::{DateTime, Utc};
use deadpool_sqlite::{Config, Hook, HookError, Pool, PoolConfig, Runtime};
use rusqlite::{params, Row};
use uuid::Uuid;

use super::{LeaderboardListingRow, LeaderboardWalletRow, ListingRow, PaymentRow, SaleRow};
use crate::db::listing_filters::search_like_pattern;
use crate::error::{AppError, AppResult};

const SCHEMA: &str = include_str!("../../migrations/sqlite/001_init.sql");

fn configure_sqlite_connection(conn: &mut rusqlite::Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(Duration::from_millis(10_000))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 10000;",
    )?;
    Ok(())
}

fn sqlite_path(database_url: &str) -> AppResult<String> {
    database_url
        .strip_prefix("sqlite://")
        .or_else(|| database_url.strip_prefix("sqlite:"))
        .map(str::to_string)
        .ok_or_else(|| AppError::BadRequest("sqlite DATABASE_URL must start with sqlite:".into()))
}

pub async fn connect_pool(database_url: &str) -> AppResult<Pool> {
    let path = sqlite_path(database_url)?;
    if let Some(parent) = std::path::Path::new(&path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("create data dir: {e}")))?;
        }
    }

    let mut cfg = Config::new(&path);
    cfg.pool = Some(PoolConfig {
        max_size: std::env::var("SQLITE_POOL_MAX_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(6),
        ..PoolConfig::default()
    });

    cfg.builder(Runtime::Tokio1)
        .expect("sqlite pool builder")
        .runtime(Runtime::Tokio1)
        .post_create(Hook::async_fn(|conn, _| {
            Box::pin(async move {
                conn.interact(configure_sqlite_connection)
                    .await
                    .map_err(|e| HookError::message(e.to_string()))?
                    .map_err(HookError::Backend)
            })
        }))
        .build()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite pool: {e}")))
}

pub async fn migrate(pool: &Pool) -> AppResult<()> {
    let sql = SCHEMA.to_string();
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| conn.execute_batch(&sql))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite interact: {e}")))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite migrate: {e}")))?;
    Ok(())
}

pub async fn health_check(pool: &Pool) -> bool {
    let pool = pool.clone();
    tokio::time::timeout(Duration::from_secs(2), async move {
        pool.get()
            .await
            .ok()?
            .interact(|conn| conn.query_row("SELECT 1", [], |_| Ok(())))
            .await
            .ok()?
            .ok()
    })
    .await
    .ok()
    .flatten()
    .is_some()
}

pub async fn insert_listing(pool: &Pool, row: &ListingRow) -> AppResult<()> {
    let row = row.clone();
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| {
            conn.execute(
                r#"
                INSERT INTO listings (
                    id, seller_wallet, display_name, title, description, category,
                    price_micro_usdc, preview_key, asset_key, content_type, byte_size,
                    agent_friendly, delivery_scheme, status, created_at
                ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
                "#,
                params![
                    row.id.to_string(),
                    row.seller_wallet,
                    row.display_name,
                    row.title,
                    row.description,
                    row.category,
                    row.price_micro_usdc,
                    row.preview_key,
                    row.asset_key,
                    row.content_type,
                    row.byte_size,
                    i32::from(row.agent_friendly),
                    row.delivery_scheme,
                    row.status,
                    row.created_at.to_rfc3339(),
                ],
            )
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite interact: {e}")))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("insert listing: {e}")))?;
    Ok(())
}

pub async fn get_listing(pool: &Pool, id: Uuid) -> AppResult<ListingRow> {
    let id = id.to_string();
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| -> rusqlite::Result<Option<ListingRow>> {
            let mut stmt = conn.prepare(
                "SELECT id, seller_wallet, display_name, title, description, category,
                        price_micro_usdc, preview_key, asset_key, content_type, byte_size,
                        agent_friendly, delivery_scheme, status, created_at
                 FROM listings WHERE id = ?1 AND status = 'active'",
            )?;
            let mut rows = stmt.query(params![id])?;
            if let Some(row) = rows.next()? {
                return Ok(Some(map_listing(row)?));
            }
            Ok(None)
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite interact: {e}")))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("get listing: {e}")))?
        .ok_or(AppError::NotFound)
}

pub async fn count_listings(
    pool: &Pool,
    category: Option<&str>,
    agent_friendly: Option<bool>,
    search: Option<&str>,
) -> AppResult<i64> {
    let category = category.map(str::to_string);
    let search_pat = search.map(search_like_pattern);
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| -> rusqlite::Result<i64> {
            match (
                category.as_deref(),
                agent_friendly,
                search_pat.as_deref(),
            ) {
                (Some(cat), Some(af), Some(pat)) => conn.query_row(
                    "SELECT COUNT(*) FROM listings WHERE status = 'active' AND category = ?1 AND agent_friendly = ?2 AND (title LIKE ?3 ESCAPE '\\' OR description LIKE ?3 ESCAPE '\\')",
                    params![cat, i32::from(af), pat],
                    |row| row.get(0),
                ),
                (Some(cat), Some(af), None) => conn.query_row(
                    "SELECT COUNT(*) FROM listings WHERE status = 'active' AND category = ?1 AND agent_friendly = ?2",
                    params![cat, i32::from(af)],
                    |row| row.get(0),
                ),
                (Some(cat), None, Some(pat)) => conn.query_row(
                    "SELECT COUNT(*) FROM listings WHERE status = 'active' AND category = ?1 AND (title LIKE ?2 ESCAPE '\\' OR description LIKE ?2 ESCAPE '\\')",
                    params![cat, pat],
                    |row| row.get(0),
                ),
                (Some(cat), None, None) => conn.query_row(
                    "SELECT COUNT(*) FROM listings WHERE status = 'active' AND category = ?1",
                    params![cat],
                    |row| row.get(0),
                ),
                (None, Some(af), Some(pat)) => conn.query_row(
                    "SELECT COUNT(*) FROM listings WHERE status = 'active' AND agent_friendly = ?1 AND (title LIKE ?2 ESCAPE '\\' OR description LIKE ?2 ESCAPE '\\')",
                    params![i32::from(af), pat],
                    |row| row.get(0),
                ),
                (None, Some(af), None) => conn.query_row(
                    "SELECT COUNT(*) FROM listings WHERE status = 'active' AND agent_friendly = ?1",
                    params![i32::from(af)],
                    |row| row.get(0),
                ),
                (None, None, Some(pat)) => conn.query_row(
                    "SELECT COUNT(*) FROM listings WHERE status = 'active' AND (title LIKE ?1 ESCAPE '\\' OR description LIKE ?1 ESCAPE '\\')",
                    params![pat],
                    |row| row.get(0),
                ),
                (None, None, None) => conn.query_row(
                    "SELECT COUNT(*) FROM listings WHERE status = 'active'",
                    [],
                    |row| row.get(0),
                ),
            }
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite interact: {e}")))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("count listings: {e}")))
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
    let category = category.map(str::to_string);
    let search_pat = search.map(search_like_pattern);
    let sort = sort.to_string();
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| -> rusqlite::Result<Vec<ListingRow>> {
            let order = match sort.as_str() {
                "price_asc" => "price_micro_usdc ASC, created_at DESC",
                "price_desc" => "price_micro_usdc DESC, created_at DESC",
                _ => "created_at DESC",
            };
            let select = "SELECT id, seller_wallet, display_name, title, description, category,
                            price_micro_usdc, preview_key, asset_key, content_type, byte_size,
                            agent_friendly, delivery_scheme, status, created_at
                     FROM listings";
            match (
                category.as_deref(),
                agent_friendly,
                search_pat.as_deref(),
            ) {
                (Some(cat), Some(af), Some(pat)) => {
                    let sql = format!(
                        "{select} WHERE status = 'active' AND category = ?1 AND agent_friendly = ?2 \
                         AND (title LIKE ?3 ESCAPE '\\' OR description LIKE ?3 ESCAPE '\\') \
                         ORDER BY {order} LIMIT ?4 OFFSET ?5"
                    );
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map(
                        params![cat, i32::from(af), pat, limit, offset],
                        map_listing,
                    )?;
                    rows.collect()
                }
                (Some(cat), Some(af), None) => {
                    let sql = format!(
                        "{select} WHERE status = 'active' AND category = ?1 AND agent_friendly = ?2
                         ORDER BY {order} LIMIT ?3 OFFSET ?4"
                    );
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map(params![cat, i32::from(af), limit, offset], map_listing)?;
                    rows.collect()
                }
                (Some(cat), None, Some(pat)) => {
                    let sql = format!(
                        "{select} WHERE status = 'active' AND category = ?1 \
                         AND (title LIKE ?2 ESCAPE '\\' OR description LIKE ?2 ESCAPE '\\') \
                         ORDER BY {order} LIMIT ?3 OFFSET ?4"
                    );
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map(params![cat, pat, limit, offset], map_listing)?;
                    rows.collect()
                }
                (Some(cat), None, None) => {
                    let sql = format!(
                        "{select} WHERE status = 'active' AND category = ?1
                         ORDER BY {order} LIMIT ?2 OFFSET ?3"
                    );
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map(params![cat, limit, offset], map_listing)?;
                    rows.collect()
                }
                (None, Some(af), Some(pat)) => {
                    let sql = format!(
                        "{select} WHERE status = 'active' AND agent_friendly = ?1 \
                         AND (title LIKE ?2 ESCAPE '\\' OR description LIKE ?2 ESCAPE '\\') \
                         ORDER BY {order} LIMIT ?3 OFFSET ?4"
                    );
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map(params![i32::from(af), pat, limit, offset], map_listing)?;
                    rows.collect()
                }
                (None, Some(af), None) => {
                    let sql = format!(
                        "{select} WHERE status = 'active' AND agent_friendly = ?1
                         ORDER BY {order} LIMIT ?2 OFFSET ?3"
                    );
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map(params![i32::from(af), limit, offset], map_listing)?;
                    rows.collect()
                }
                (None, None, Some(pat)) => {
                    let sql = format!(
                        "{select} WHERE status = 'active' \
                         AND (title LIKE ?1 ESCAPE '\\' OR description LIKE ?1 ESCAPE '\\') \
                         ORDER BY {order} LIMIT ?2 OFFSET ?3"
                    );
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map(params![pat, limit, offset], map_listing)?;
                    rows.collect()
                }
                (None, None, None) => {
                    let sql = format!(
                        "{select} WHERE status = 'active' ORDER BY {order} LIMIT ?1 OFFSET ?2"
                    );
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map(params![limit, offset], map_listing)?;
                    rows.collect()
                }
            }
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite interact: {e}")))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("list listings: {e}")))
}

pub async fn find_by_idempotency(pool: &Pool, key: &str) -> AppResult<Option<PaymentRow>> {
    let key = key.to_string();
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| -> rusqlite::Result<Option<PaymentRow>> {
            let mut stmt = conn.prepare(
                "SELECT buyer_wallet, tx_signature FROM payments WHERE idempotency_key = ?1",
            )?;
            let mut rows = stmt.query(params![key])?;
            if let Some(row) = rows.next()? {
                return Ok(Some(map_payment(row)?));
            }
            Ok(None)
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite interact: {e}")))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("find payment: {e}")))
}

pub async fn insert_payment(
    pool: &Pool,
    key: &str,
    listing_id: Uuid,
    buyer_wallet: &str,
    tx_signature: &str,
) -> AppResult<()> {
    let key = key.to_string();
    let listing_id = listing_id.to_string();
    let buyer_wallet = buyer_wallet.to_string();
    let tx_signature = tx_signature.to_string();
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| {
            conn.execute(
                r#"
                INSERT INTO payments (idempotency_key, listing_id, buyer_wallet, tx_signature)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT (idempotency_key) DO NOTHING
                "#,
                params![key, listing_id, buyer_wallet, tx_signature],
            )
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite interact: {e}")))?
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
    let listing_id_s = listing_id.to_string();
    let seller_wallet = seller_wallet.to_string();
    let buyer_wallet = buyer_wallet.to_string();
    let tx_signature = tx_signature.to_string();
    let id_s = id.to_string();
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| -> rusqlite::Result<SaleRow> {
            conn.execute(
                r#"
                INSERT INTO sales (id, listing_id, seller_wallet, buyer_wallet, amount_micro_usdc, tx_signature)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
                params![
                    id_s,
                    listing_id_s,
                    seller_wallet,
                    buyer_wallet,
                    amount_micro_usdc,
                    tx_signature
                ],
            )?;
            let mut stmt = conn.prepare(
                "SELECT id, listing_id, seller_wallet, buyer_wallet, amount_micro_usdc, tx_signature, settled_at
                 FROM sales WHERE id = ?1",
            )?;
            let mut rows = stmt.query(params![id_s])?;
            let row = rows.next()?.expect("sale row");
            map_sale(row)
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite interact: {e}")))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("insert sale: {e}")))
}

pub async fn top_earners_24h(pool: &Pool) -> AppResult<Vec<LeaderboardWalletRow>> {
    query_leaderboard_wallets(
        pool,
        "SELECT wallet, amount_micro_usdc, sales_count FROM leaderboard_earners_24h",
    )
    .await
}

pub async fn top_payers_24h(pool: &Pool) -> AppResult<Vec<LeaderboardWalletRow>> {
    query_leaderboard_wallets(
        pool,
        "SELECT wallet, amount_micro_usdc, sales_count FROM leaderboard_payers_24h",
    )
    .await
}

pub async fn hottest_listings_24h(pool: &Pool) -> AppResult<Vec<LeaderboardListingRow>> {
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(|conn| -> rusqlite::Result<Vec<LeaderboardListingRow>> {
            let mut stmt = conn.prepare(
                "SELECT listing_id, title, sales_count, volume_micro_usdc FROM leaderboard_hottest_24h",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(LeaderboardListingRow {
                    listing_id: parse_uuid(row.get::<_, String>(0)?)?,
                    title: row.get(1)?,
                    sales_count: row.get(2)?,
                    volume_micro_usdc: row.get(3)?,
                })
            })?;
            rows.collect()
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite interact: {e}")))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("hottest listings: {e}")))
}

async fn query_leaderboard_wallets(pool: &Pool, sql: &str) -> AppResult<Vec<LeaderboardWalletRow>> {
    let sql = sql.to_string();
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| -> rusqlite::Result<Vec<LeaderboardWalletRow>> {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| {
                Ok(LeaderboardWalletRow {
                    wallet: row.get(0)?,
                    amount_micro_usdc: row.get(1)?,
                    sales_count: row.get(2)?,
                })
            })?;
            rows.collect()
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite interact: {e}")))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("leaderboard: {e}")))
}

fn map_listing(row: &Row<'_>) -> rusqlite::Result<ListingRow> {
    Ok(ListingRow {
        id: parse_uuid(row.get::<_, String>(0)?)?,
        seller_wallet: row.get(1)?,
        display_name: row.get(2)?,
        title: row.get(3)?,
        description: row.get(4)?,
        category: row.get(5)?,
        price_micro_usdc: row.get(6)?,
        preview_key: row.get(7)?,
        asset_key: row.get(8)?,
        content_type: row.get(9)?,
        byte_size: row.get(10)?,
        agent_friendly: row.get::<_, i32>(11)? != 0,
        delivery_scheme: row.get(12)?,
        status: row.get(13)?,
        created_at: parse_datetime(row.get::<_, String>(14)?)?,
    })
}

fn map_payment(row: &Row<'_>) -> rusqlite::Result<PaymentRow> {
    Ok(PaymentRow {
        buyer_wallet: row.get(0)?,
        tx_signature: row.get(1)?,
    })
}

fn map_sale(row: &Row<'_>) -> rusqlite::Result<SaleRow> {
    Ok(SaleRow {
        id: parse_uuid(row.get::<_, String>(0)?)?,
        listing_id: parse_uuid(row.get::<_, String>(1)?)?,
        seller_wallet: row.get(2)?,
        buyer_wallet: row.get(3)?,
        amount_micro_usdc: row.get(4)?,
        tx_signature: row.get(5)?,
        settled_at: parse_datetime(row.get::<_, String>(6)?)?,
    })
}

fn parse_uuid(raw: String) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(&raw).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}

fn parse_datetime(raw: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&raw)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(&raw, "%Y-%m-%d %H:%M:%S")
                .map(|ndt| ndt.and_utc())
        })
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}
