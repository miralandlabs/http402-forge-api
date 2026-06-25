mod listing;
mod listing_filters;
mod payment;
mod postgres;
mod sales;
mod sqlite;

pub use listing::ListingRow;
pub use listing_filters::{normalize_search, ListingFilterBinds};
pub use payment::PaymentRow;
pub use sales::{LeaderboardListingRow, LeaderboardWalletRow, SaleRow};

use deadpool_postgres::Pool as PgPool;
use deadpool_sqlite::Pool as SqlitePool;

use crate::error::AppResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseKind {
    Postgres,
    Sqlite,
}

impl DatabaseKind {
    pub fn detect(database_url: &str) -> Self {
        let lower = database_url.to_ascii_lowercase();
        if lower.starts_with("sqlite:") {
            Self::Sqlite
        } else {
            Self::Postgres
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Sqlite => "sqlite",
        }
    }
}

enum DbBackend {
    Postgres(PgPool),
    Sqlite(SqlitePool),
}

pub struct Database {
    backend: DbBackend,
    kind: DatabaseKind,
}

impl Database {
    pub async fn connect(database_url: &str) -> AppResult<Self> {
        let kind = DatabaseKind::detect(database_url);
        let db = match kind {
            DatabaseKind::Postgres => Self {
                backend: DbBackend::Postgres(postgres::connect_pool(database_url)?),
                kind,
            },
            DatabaseKind::Sqlite => Self {
                backend: DbBackend::Sqlite(sqlite::connect_pool(database_url).await?),
                kind,
            },
        };
        db.migrate().await?;
        Ok(db)
    }

    pub async fn migrate(&self) -> AppResult<()> {
        match &self.backend {
            DbBackend::Postgres(pool) => postgres::migrate(pool).await,
            DbBackend::Sqlite(pool) => sqlite::migrate(pool).await,
        }
    }

    pub fn kind(&self) -> DatabaseKind {
        self.kind
    }

    pub async fn health_check(&self) -> bool {
        match &self.backend {
            DbBackend::Postgres(pool) => postgres::health_check(pool).await,
            DbBackend::Sqlite(pool) => sqlite::health_check(pool).await,
        }
    }

    pub async fn insert_listing(&self, row: &ListingRow) -> AppResult<()> {
        match &self.backend {
            DbBackend::Postgres(pool) => postgres::insert_listing(pool, row).await,
            DbBackend::Sqlite(pool) => sqlite::insert_listing(pool, row).await,
        }
    }

    pub async fn get_listing(&self, id: uuid::Uuid) -> AppResult<ListingRow> {
        match &self.backend {
            DbBackend::Postgres(pool) => postgres::get_listing(pool, id).await,
            DbBackend::Sqlite(pool) => sqlite::get_listing(pool, id).await,
        }
    }

    pub async fn count_listings(&self, filters: &ListingFilterBinds) -> AppResult<i64> {
        match &self.backend {
            DbBackend::Postgres(pool) => postgres::count_listings(pool, filters).await,
            DbBackend::Sqlite(pool) => sqlite::count_listings(pool, filters).await,
        }
    }

    pub async fn list_listings(
        &self,
        filters: &ListingFilterBinds,
        sort: &str,
        limit: i64,
        offset: i64,
    ) -> AppResult<Vec<ListingRow>> {
        match &self.backend {
            DbBackend::Postgres(pool) => {
                postgres::list_listings(pool, filters, sort, limit, offset).await
            }
            DbBackend::Sqlite(pool) => {
                sqlite::list_listings(pool, filters, sort, limit, offset).await
            }
        }
    }

    pub async fn find_by_idempotency(&self, key: &str) -> AppResult<Option<PaymentRow>> {
        match &self.backend {
            DbBackend::Postgres(pool) => postgres::find_by_idempotency(pool, key).await,
            DbBackend::Sqlite(pool) => sqlite::find_by_idempotency(pool, key).await,
        }
    }

    pub async fn insert_payment(
        &self,
        key: &str,
        listing_id: uuid::Uuid,
        buyer_wallet: &str,
        tx_signature: &str,
    ) -> AppResult<()> {
        match &self.backend {
            DbBackend::Postgres(pool) => {
                postgres::insert_payment(pool, key, listing_id, buyer_wallet, tx_signature).await
            }
            DbBackend::Sqlite(pool) => {
                sqlite::insert_payment(pool, key, listing_id, buyer_wallet, tx_signature).await
            }
        }
    }

    pub async fn insert_sale(
        &self,
        listing_id: uuid::Uuid,
        seller_wallet: &str,
        buyer_wallet: &str,
        amount_micro_usdc: i64,
        tx_signature: &str,
    ) -> AppResult<SaleRow> {
        match &self.backend {
            DbBackend::Postgres(pool) => {
                postgres::insert_sale(
                    pool,
                    listing_id,
                    seller_wallet,
                    buyer_wallet,
                    amount_micro_usdc,
                    tx_signature,
                )
                .await
            }
            DbBackend::Sqlite(pool) => {
                sqlite::insert_sale(
                    pool,
                    listing_id,
                    seller_wallet,
                    buyer_wallet,
                    amount_micro_usdc,
                    tx_signature,
                )
                .await
            }
        }
    }

    pub async fn top_earners_24h(&self) -> AppResult<Vec<LeaderboardWalletRow>> {
        match &self.backend {
            DbBackend::Postgres(pool) => postgres::top_earners_24h(pool).await,
            DbBackend::Sqlite(pool) => sqlite::top_earners_24h(pool).await,
        }
    }

    pub async fn top_payers_24h(&self) -> AppResult<Vec<LeaderboardWalletRow>> {
        match &self.backend {
            DbBackend::Postgres(pool) => postgres::top_payers_24h(pool).await,
            DbBackend::Sqlite(pool) => sqlite::top_payers_24h(pool).await,
        }
    }

    pub async fn hottest_listings_24h(&self) -> AppResult<Vec<LeaderboardListingRow>> {
        match &self.backend {
            DbBackend::Postgres(pool) => postgres::hottest_listings_24h(pool).await,
            DbBackend::Sqlite(pool) => sqlite::hottest_listings_24h(pool).await,
        }
    }
}
