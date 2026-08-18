-- Escrow fund bind rows: payment_uid ↔ listing ↔ content_hash ↔ on-chain oracle_authority.

CREATE TABLE IF NOT EXISTS escrow_fund_binds (
    listing_id TEXT NOT NULL REFERENCES listings(id),
    payment_uid TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    oracle_authority TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (listing_id, payment_uid)
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_escrow_fund_binds_payment_uid
    ON escrow_fund_binds (payment_uid);
