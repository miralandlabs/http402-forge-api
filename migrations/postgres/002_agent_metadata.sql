-- Agent listing metadata: tags (JSON array string), license, content_hash.
--
-- Fresh installs: columns included in migrations/postgres/001_init.sql.
-- Existing deployments: apply incrementally after 001_init.sql baseline.

ALTER TABLE listings ADD COLUMN IF NOT EXISTS tags TEXT NOT NULL DEFAULT '[]';
ALTER TABLE listings ADD COLUMN IF NOT EXISTS license TEXT;
ALTER TABLE listings ADD COLUMN IF NOT EXISTS content_hash TEXT;
