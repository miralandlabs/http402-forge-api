CREATE TABLE IF NOT EXISTS listings (
    id TEXT PRIMARY KEY NOT NULL,
    seller_wallet TEXT NOT NULL,
    display_name TEXT,
    title TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    category TEXT NOT NULL,
    price_micro_usdc INTEGER NOT NULL,
    preview_key TEXT NOT NULL,
    asset_key TEXT NOT NULL,
    content_type TEXT NOT NULL,
    byte_size INTEGER NOT NULL,
    agent_friendly INTEGER NOT NULL DEFAULT 0,
    delivery_scheme TEXT NOT NULL DEFAULT 'exact',
    status TEXT NOT NULL DEFAULT 'active',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_listings_status_created ON listings (status, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_listings_category ON listings (category);
CREATE INDEX IF NOT EXISTS idx_listings_seller ON listings (seller_wallet);

CREATE TABLE IF NOT EXISTS sales (
    id TEXT PRIMARY KEY NOT NULL,
    listing_id TEXT NOT NULL REFERENCES listings(id),
    seller_wallet TEXT NOT NULL,
    buyer_wallet TEXT NOT NULL,
    amount_micro_usdc INTEGER NOT NULL,
    tx_signature TEXT NOT NULL DEFAULT '',
    settled_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_sales_settled ON sales (settled_at DESC);
CREATE INDEX IF NOT EXISTS idx_sales_seller ON sales (seller_wallet, settled_at DESC);
CREATE INDEX IF NOT EXISTS idx_sales_buyer ON sales (buyer_wallet, settled_at DESC);
CREATE INDEX IF NOT EXISTS idx_sales_listing ON sales (listing_id, settled_at DESC);

CREATE TABLE IF NOT EXISTS payments (
    idempotency_key TEXT PRIMARY KEY,
    listing_id TEXT NOT NULL REFERENCES listings(id),
    buyer_wallet TEXT NOT NULL,
    tx_signature TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

DROP VIEW IF EXISTS leaderboard_earners_24h;
CREATE VIEW leaderboard_earners_24h AS
SELECT seller_wallet AS wallet,
       CAST(SUM(amount_micro_usdc) AS INTEGER) AS amount_micro_usdc,
       CAST(COUNT(*) AS INTEGER) AS sales_count
FROM sales
WHERE settled_at >= datetime('now', '-24 hours')
GROUP BY seller_wallet
ORDER BY amount_micro_usdc DESC
LIMIT 20;

DROP VIEW IF EXISTS leaderboard_payers_24h;
CREATE VIEW leaderboard_payers_24h AS
SELECT buyer_wallet AS wallet,
       CAST(SUM(amount_micro_usdc) AS INTEGER) AS amount_micro_usdc,
       CAST(COUNT(*) AS INTEGER) AS sales_count
FROM sales
WHERE settled_at >= datetime('now', '-24 hours')
GROUP BY buyer_wallet
ORDER BY amount_micro_usdc DESC
LIMIT 20;

DROP VIEW IF EXISTS leaderboard_hottest_24h;
CREATE VIEW leaderboard_hottest_24h AS
SELECT s.listing_id,
       l.title,
       CAST(COUNT(*) AS INTEGER) AS sales_count,
       CAST(SUM(s.amount_micro_usdc) AS INTEGER) AS volume_micro_usdc
FROM sales s
JOIN listings l ON l.id = s.listing_id
WHERE s.settled_at >= datetime('now', '-24 hours')
GROUP BY s.listing_id, l.title
ORDER BY sales_count DESC
LIMIT 20;
