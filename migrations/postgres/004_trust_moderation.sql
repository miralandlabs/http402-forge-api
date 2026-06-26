-- Trust signals (purchase-linked feedback) and upload moderation metadata.

CREATE TABLE IF NOT EXISTS sale_feedback (
    sale_id UUID PRIMARY KEY REFERENCES sales(id),
    listing_id UUID NOT NULL REFERENCES listings(id),
    buyer_wallet TEXT NOT NULL,
    outcome TEXT NOT NULL,
    score SMALLINT CHECK (score IS NULL OR (score >= 1 AND score <= 5)),
    note TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_sale_feedback_listing ON sale_feedback (listing_id);

CREATE TABLE IF NOT EXISTS blocked_content_hashes (
    content_hash TEXT PRIMARY KEY,
    reason TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

ALTER TABLE listings ADD COLUMN IF NOT EXISTS moderation_status TEXT NOT NULL DEFAULT 'approved';
ALTER TABLE listings ADD COLUMN IF NOT EXISTS moderation_labels TEXT NOT NULL DEFAULT '[]';
