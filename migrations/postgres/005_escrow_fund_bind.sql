-- Escrow fund bind rows: payment_uid ↔ listing ↔ content_hash ↔ oracle_authority at fund time.
CREATE TABLE IF NOT EXISTS escrow_fund_binds (
    listing_id UUID NOT NULL REFERENCES listings(id),
    payment_uid TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    oracle_authority TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (listing_id, payment_uid)
);

CREATE INDEX IF NOT EXISTS idx_escrow_fund_binds_payment_uid ON escrow_fund_binds (payment_uid);
