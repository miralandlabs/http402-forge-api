-- Escrow fund binds: one row per funded sla-escrow payment bound to a listing.
-- Recorded by the Forge API when the pr402 facilitator reports a successful
-- FundPayment. The verdict door reads this row to decide which oracle key may
-- stream a listing's artifact.

CREATE TABLE IF NOT EXISTS escrow_fund_binds (
    listing_id UUID NOT NULL REFERENCES listings(id),
    payment_uid TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    oracle_authority TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (listing_id, payment_uid)
);

CREATE INDEX IF NOT EXISTS idx_escrow_fund_binds_listing ON escrow_fund_binds (listing_id);
CREATE INDEX IF NOT EXISTS idx_escrow_fund_binds_oracle ON escrow_fund_binds (oracle_authority);
