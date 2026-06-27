use std::time::Duration;

use chrono::{DateTime, Utc};
use deadpool_sqlite::{Config, Hook, HookError, Pool, PoolConfig, Runtime};
use rusqlite::{params, Row};
use uuid::Uuid;

use super::{LeaderboardListingRow, LeaderboardWalletRow, ListingRow, PaymentRow, SaleRow};
use super::trust::{ListingQualityStats, SaleFeedbackRow};
use crate::db::listing_filters::{listing_filter_suffix, ListingFilterBinds};
use crate::error::{AppError, AppResult};

const SCHEMA: &str = include_str!("../../migrations/sqlite/001_init.sql");
const SCHEMA_002: &str = include_str!("../../migrations/sqlite/002_agent_metadata.sql");
const SCHEMA_003: &str = include_str!("../../migrations/sqlite/003_preview_content_type.sql");
const SCHEMA_004: &str = include_str!("../../migrations/sqlite/004_trust_moderation.sql");

const LISTING_COLUMNS: &str = "id, seller_wallet, display_name, title, description, category,
                        price_micro_usdc, preview_key, preview_content_type, asset_key, content_type, byte_size,
                        agent_friendly, delivery_scheme, status, tags, license, content_hash,
                        moderation_status, moderation_labels, created_at";

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

    let sql2 = SCHEMA_002.to_string();
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| {
            for stmt in sql2.split(';').map(str::trim).filter(|s| !s.is_empty()) {
                if let Err(e) = conn.execute(stmt, []) {
                    let msg = e.to_string();
                    if !msg.contains("duplicate column") {
                        return Err(e);
                    }
                }
            }
            Ok(())
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite interact: {e}")))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite migrate 002: {e}")))?;

    let sql3 = SCHEMA_003.to_string();
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| {
            for stmt in sql3.split(';').map(str::trim).filter(|s| !s.is_empty()) {
                if let Err(e) = conn.execute(stmt, []) {
                    let msg = e.to_string();
                    if !msg.contains("duplicate column") {
                        return Err(e);
                    }
                }
            }
            Ok(())
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite interact: {e}")))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite migrate 003: {e}")))?;

    let sql4 = SCHEMA_004.to_string();
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| {
            for stmt in sql4.split(';').map(str::trim).filter(|s| !s.is_empty()) {
                if let Err(e) = conn.execute(stmt, []) {
                    let msg = e.to_string();
                    if !msg.contains("duplicate column") {
                        return Err(e);
                    }
                }
            }
            Ok(())
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite interact: {e}")))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite migrate 004: {e}")))?;
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
                    price_micro_usdc, preview_key, preview_content_type, asset_key, content_type, byte_size,
                    agent_friendly, delivery_scheme, status, tags, license, content_hash,
                    moderation_status, moderation_labels, created_at
                ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)
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
                    row.preview_content_type,
                    row.asset_key,
                    row.content_type,
                    row.byte_size,
                    i32::from(row.agent_friendly),
                    row.delivery_scheme,
                    row.status,
                    row.tags,
                    row.license,
                    row.content_hash,
                    row.moderation_status,
                    row.moderation_labels,
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
            let mut stmt = conn.prepare(&format!(
                "SELECT {LISTING_COLUMNS} FROM listings WHERE id = ?1 AND status = 'active'",
            ))?;
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

pub async fn get_listing_any(pool: &Pool, id: Uuid) -> AppResult<ListingRow> {
    let id = id.to_string();
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| -> rusqlite::Result<Option<ListingRow>> {
            let mut stmt = conn.prepare(&format!(
                "SELECT {LISTING_COLUMNS} FROM listings WHERE id = ?1",
            ))?;
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

pub async fn soft_delist_listing(pool: &Pool, id: Uuid, seller_wallet: &str) -> AppResult<bool> {
    let id = id.to_string();
    let seller_wallet = seller_wallet.to_string();
    let n = pool
        .get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| {
            conn.execute(
                "UPDATE listings SET status = 'removed' WHERE id = ?1 AND seller_wallet = ?2 AND status = 'active'",
                params![id, seller_wallet],
            )
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite interact: {e}")))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("soft delist: {e}")))?;
    Ok(n > 0)
}

fn listing_filter_values(
    binds: &ListingFilterBinds,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Vec<rusqlite::types::Value> {
    let mut values = Vec::new();
    if let Some(ref c) = binds.category {
        values.push(c.clone().into());
    }
    if let Some(af) = binds.agent_friendly {
        values.push(i32::from(af).into());
    }
    if let Some(ref w) = binds.seller_wallet {
        values.push(w.clone().into());
    }
    if let Some(ref p) = binds.search_pattern {
        values.push(p.clone().into());
    }
    if let Some(l) = limit {
        values.push(l.into());
    }
    if let Some(o) = offset {
        values.push(o.into());
    }
    values
}

pub async fn count_listings(pool: &Pool, binds: &ListingFilterBinds) -> AppResult<i64> {
    let (suffix, _) = listing_filter_suffix(binds, 1, true);
    let sql = format!("SELECT COUNT(*) FROM listings WHERE status = 'active'{suffix}");
    let values = listing_filter_values(binds, None, None);
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| -> rusqlite::Result<i64> {
            conn.query_row(&sql, rusqlite::params_from_iter(values), |row| row.get(0))
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite interact: {e}")))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("count listings: {e}")))
}

pub async fn list_listings(
    pool: &Pool,
    binds: &ListingFilterBinds,
    sort: &str,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<ListingRow>> {
    let sort = sort.to_string();
    let (suffix, next_idx) = listing_filter_suffix(binds, 1, true);
    let order = match sort.as_str() {
        "price_asc" => "price_micro_usdc ASC, created_at DESC",
        "price_desc" => "price_micro_usdc DESC, created_at DESC",
        "newest" => "created_at DESC",
        "trending" => "(SELECT COUNT(*) FROM sales s WHERE s.listing_id = listings.id AND s.settled_at >= datetime('now', '-24 hours')) DESC, created_at DESC",
        "quality" => "(SELECT CASE WHEN COUNT(*) >= 2 THEN AVG(CASE sf.outcome WHEN 'as_described' THEN 100 WHEN 'hash_mismatch' THEN 0 WHEN 'corrupt' THEN 25 WHEN 'misleading' THEN 35 ELSE 50 END) ELSE NULL END FROM sale_feedback sf WHERE sf.listing_id = listings.id) DESC, created_at DESC",
        _ => "(SELECT COUNT(*) FROM sales s WHERE s.listing_id = listings.id AND s.settled_at >= datetime('now', '-24 hours')) DESC, created_at DESC",
    };
    let sql = format!(
        "SELECT {LISTING_COLUMNS} FROM listings WHERE status = 'active'{suffix} ORDER BY {order} LIMIT ?{next_idx} OFFSET ?{}",
        next_idx + 1
    );
    let values = listing_filter_values(binds, Some(limit), Some(offset));
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| -> rusqlite::Result<Vec<ListingRow>> {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(values), map_listing)?;
            rows.collect()
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
        preview_content_type: row.get(8)?,
        asset_key: row.get(9)?,
        content_type: row.get(10)?,
        byte_size: row.get(11)?,
        agent_friendly: row.get::<_, i32>(12)? != 0,
        delivery_scheme: row.get(13)?,
        status: row.get(14)?,
        tags: row.get(15)?,
        license: row.get(16)?,
        content_hash: row.get(17)?,
        moderation_status: row.get(18)?,
        moderation_labels: row.get(19)?,
        created_at: parse_datetime(row.get::<_, String>(20)?)?,
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

pub async fn listings_missing_preview_content_type(
    pool: &Pool,
) -> AppResult<Vec<(Uuid, String)>> {
    let sql = "SELECT id, preview_key FROM listings WHERE preview_content_type = ''".to_string();
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| -> rusqlite::Result<Vec<(Uuid, String)>> {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    parse_uuid(row.get::<_, String>(0)?)?,
                    row.get::<_, String>(1)?,
                ))
            })?;
            rows.collect()
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite interact: {e}")))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("list preview content types: {e}")))
}

pub async fn set_preview_content_type(
    pool: &Pool,
    id: Uuid,
    preview_content_type: &str,
) -> AppResult<()> {
    let id = id.to_string();
    let preview_content_type = preview_content_type.to_string();
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| {
            conn.execute(
                "UPDATE listings SET preview_content_type = ?2 WHERE id = ?1",
                params![id, preview_content_type],
            )
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite interact: {e}")))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("set preview content type: {e}")))?;
    Ok(())
}

pub async fn is_content_hash_blocked(pool: &Pool, content_hash: &str) -> AppResult<bool> {
    let content_hash = content_hash.to_string();
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| -> rusqlite::Result<bool> {
            let mut stmt =
                conn.prepare("SELECT 1 FROM blocked_content_hashes WHERE content_hash = ?1")?;
            let mut rows = stmt.query(params![content_hash])?;
            Ok(rows.next()?.is_some())
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite interact: {e}")))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("hash blocklist: {e}")))
}

pub async fn get_sale(pool: &Pool, sale_id: Uuid) -> AppResult<SaleRow> {
    let sale_id = sale_id.to_string();
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| -> rusqlite::Result<Option<SaleRow>> {
            let mut stmt = conn.prepare(
                "SELECT id, listing_id, seller_wallet, buyer_wallet, amount_micro_usdc, tx_signature, settled_at
                 FROM sales WHERE id = ?1",
            )?;
            let mut rows = stmt.query(params![sale_id])?;
            if let Some(row) = rows.next()? {
                return Ok(Some(map_sale(row)?));
            }
            Ok(None)
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite interact: {e}")))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("get sale: {e}")))?
        .ok_or(AppError::NotFound)
}

pub async fn find_sale_by_payment(
    pool: &Pool,
    listing_id: Uuid,
    buyer_wallet: &str,
    tx_signature: &str,
) -> AppResult<Option<SaleRow>> {
    let listing_id = listing_id.to_string();
    let buyer_wallet = buyer_wallet.to_string();
    let tx_signature = tx_signature.to_string();
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| -> rusqlite::Result<Option<SaleRow>> {
            let mut stmt = conn.prepare(
                "SELECT id, listing_id, seller_wallet, buyer_wallet, amount_micro_usdc, tx_signature, settled_at
                 FROM sales
                 WHERE listing_id = ?1 AND buyer_wallet = ?2 AND tx_signature = ?3
                 ORDER BY settled_at DESC
                 LIMIT 1",
            )?;
            let mut rows = stmt.query(params![listing_id, buyer_wallet, tx_signature])?;
            if let Some(row) = rows.next()? {
                return Ok(Some(map_sale(row)?));
            }
            Ok(None)
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite interact: {e}")))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("find sale: {e}")))
}

pub async fn find_buyer_sale_for_listing(
    pool: &Pool,
    listing_id: Uuid,
    buyer_wallet: &str,
) -> AppResult<Option<SaleRow>> {
    let listing_id = listing_id.to_string();
    let buyer_wallet = buyer_wallet.to_string();
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| -> rusqlite::Result<Option<SaleRow>> {
            let mut stmt = conn.prepare(
                "SELECT id, listing_id, seller_wallet, buyer_wallet, amount_micro_usdc, tx_signature, settled_at
                 FROM sales
                 WHERE listing_id = ?1 AND buyer_wallet = ?2
                 ORDER BY settled_at DESC
                 LIMIT 1",
            )?;
            let mut rows = stmt.query(params![listing_id, buyer_wallet])?;
            if let Some(row) = rows.next()? {
                return Ok(Some(map_sale(row)?));
            }
            Ok(None)
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite interact: {e}")))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("find buyer sale: {e}")))
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
    let sale_id_s = sale_id.to_string();
    let sale_id_query = sale_id_s.clone();
    let listing_id_s = listing_id.to_string();
    let buyer_wallet = buyer_wallet.to_string();
    let outcome = outcome.to_string();
    let note = note.map(str::to_string);
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| -> rusqlite::Result<SaleFeedbackRow> {
            conn.execute(
                r#"
                INSERT INTO sale_feedback (sale_id, listing_id, buyer_wallet, outcome, score, note)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
                params![sale_id_s, listing_id_s, buyer_wallet, outcome, score, note],
            )?;
            let mut stmt = conn.prepare(
                "SELECT sale_id, listing_id, buyer_wallet, outcome, score, note, created_at
                 FROM sale_feedback WHERE sale_id = ?1",
            )?;
            let mut rows = stmt.query(params![sale_id_query])?;
            let row = rows.next()?.expect("sale feedback row");
            map_sale_feedback(row)
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite interact: {e}")))?
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("UNIQUE constraint") {
                AppError::Conflict("feedback already submitted for this sale".into())
            } else {
                AppError::Internal(anyhow::anyhow!("insert sale feedback: {e}"))
            }
        })
}

pub async fn listing_quality_stats(
    pool: &Pool,
    listing_ids: &[Uuid],
) -> AppResult<std::collections::HashMap<Uuid, ListingQualityStats>> {
    if listing_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let placeholders: String = (1..=listing_ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT listing_id,
                COUNT(*) AS feedback_count,
                CAST(AVG(CASE outcome
                  WHEN 'as_described' THEN 100
                  WHEN 'hash_mismatch' THEN 0
                  WHEN 'corrupt' THEN 25
                  WHEN 'misleading' THEN 35
                  ELSE 50
                END) AS INTEGER) AS quality_score
         FROM sale_feedback
         WHERE listing_id IN ({placeholders})
         GROUP BY listing_id"
    );
    let ids: Vec<String> = listing_ids.iter().map(Uuid::to_string).collect();
    pool.get()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite conn: {e}")))?
        .interact(move |conn| -> rusqlite::Result<std::collections::HashMap<Uuid, ListingQualityStats>> {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(ids), |row| {
                Ok((
                    parse_uuid(row.get::<_, String>(0)?)?,
                    ListingQualityStats {
                        verified_feedback_count: row.get(1)?,
                        quality_score: row.get(2)?,
                    },
                ))
            })?;
            rows.collect::<Result<std::collections::HashMap<_, _>, _>>()
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sqlite interact: {e}")))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("listing quality stats: {e}")))
}

fn map_sale_feedback(row: &Row<'_>) -> rusqlite::Result<SaleFeedbackRow> {
    Ok(SaleFeedbackRow {
        sale_id: parse_uuid(row.get::<_, String>(0)?)?,
        listing_id: parse_uuid(row.get::<_, String>(1)?)?,
        buyer_wallet: row.get(2)?,
        outcome: row.get(3)?,
        score: row.get(4)?,
        note: row.get(5)?,
        created_at: parse_datetime(row.get::<_, String>(6)?)?,
    })
}
