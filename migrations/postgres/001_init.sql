-- http402-forge-api — complete schema (fresh install).
-- Incremental upgrades: numbered files after 001 (e.g. 002_agent_metadata.sql).
-- When adding schema: update this file AND add a delta migration for existing DBs.

CREATE TABLE IF NOT EXISTS listings (
    id UUID PRIMARY KEY,
    seller_wallet TEXT NOT NULL,
    display_name TEXT,
    title TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    category TEXT NOT NULL,
    price_micro_usdc BIGINT NOT NULL,
    preview_key TEXT NOT NULL,
    asset_key TEXT NOT NULL,
    content_type TEXT NOT NULL,
    byte_size BIGINT NOT NULL,
    agent_friendly BOOLEAN NOT NULL DEFAULT FALSE,
    delivery_scheme TEXT NOT NULL DEFAULT 'exact',
    status TEXT NOT NULL DEFAULT 'active',
    tags TEXT NOT NULL DEFAULT '[]',
    license TEXT,
    content_hash TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_listings_status_created ON listings (status, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_listings_category ON listings (category);
CREATE INDEX IF NOT EXISTS idx_listings_seller ON listings (seller_wallet);

CREATE TABLE IF NOT EXISTS sales (
    id UUID PRIMARY KEY,
    listing_id UUID NOT NULL REFERENCES listings(id),
    seller_wallet TEXT NOT NULL,
    buyer_wallet TEXT NOT NULL,
    amount_micro_usdc BIGINT NOT NULL,
    tx_signature TEXT NOT NULL DEFAULT '',
    settled_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_sales_settled ON sales (settled_at DESC);
CREATE INDEX IF NOT EXISTS idx_sales_seller ON sales (seller_wallet, settled_at DESC);
CREATE INDEX IF NOT EXISTS idx_sales_buyer ON sales (buyer_wallet, settled_at DESC);
CREATE INDEX IF NOT EXISTS idx_sales_listing ON sales (listing_id, settled_at DESC);

CREATE TABLE IF NOT EXISTS payments (
    idempotency_key TEXT PRIMARY KEY,
    listing_id UUID NOT NULL REFERENCES listings(id),
    buyer_wallet TEXT NOT NULL,
    tx_signature TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE OR REPLACE VIEW leaderboard_earners_24h AS
SELECT seller_wallet AS wallet,
       SUM(amount_micro_usdc)::BIGINT AS amount_micro_usdc,
       COUNT(*)::BIGINT AS sales_count
FROM sales
WHERE settled_at >= NOW() - INTERVAL '24 hours'
GROUP BY seller_wallet
ORDER BY amount_micro_usdc DESC
LIMIT 20;

CREATE OR REPLACE VIEW leaderboard_payers_24h AS
SELECT buyer_wallet AS wallet,
       SUM(amount_micro_usdc)::BIGINT AS amount_micro_usdc,
       COUNT(*)::BIGINT AS sales_count
FROM sales
WHERE settled_at >= NOW() - INTERVAL '24 hours'
GROUP BY buyer_wallet
ORDER BY amount_micro_usdc DESC
LIMIT 20;

CREATE OR REPLACE VIEW leaderboard_hottest_24h AS
SELECT s.listing_id,
       l.title,
       COUNT(*)::BIGINT AS sales_count,
       SUM(s.amount_micro_usdc)::BIGINT AS volume_micro_usdc
FROM sales s
JOIN listings l ON l.id = s.listing_id
WHERE s.settled_at >= NOW() - INTERVAL '24 hours'
GROUP BY s.listing_id, l.title
ORDER BY sales_count DESC
LIMIT 20;
