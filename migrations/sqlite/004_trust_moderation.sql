-- Trust signals (purchase-linked feedback) and upload moderation metadata.

CREATE TABLE IF NOT EXISTS sale_feedback (
    sale_id TEXT PRIMARY KEY NOT NULL REFERENCES sales(id),
    listing_id TEXT NOT NULL REFERENCES listings(id),
    buyer_wallet TEXT NOT NULL,
    outcome TEXT NOT NULL,
    score INTEGER CHECK (score IS NULL OR (score >= 1 AND score <= 5)),
    note TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_sale_feedback_listing ON sale_feedback (listing_id);

CREATE TABLE IF NOT EXISTS blocked_content_hashes (
    content_hash TEXT PRIMARY KEY NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

ALTER TABLE listings ADD COLUMN moderation_status TEXT NOT NULL DEFAULT 'approved';
ALTER TABLE listings ADD COLUMN moderation_labels TEXT NOT NULL DEFAULT '[]';
