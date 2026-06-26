-- Agent listing metadata: tags (JSON array string), license, content_hash.
--
-- Fresh installs: columns included in migrations/sqlite/001_init.sql.
-- Existing deployments: apply incrementally after 001_init.sql baseline.

ALTER TABLE listings ADD COLUMN tags TEXT NOT NULL DEFAULT '[]';
ALTER TABLE listings ADD COLUMN license TEXT;
ALTER TABLE listings ADD COLUMN content_hash TEXT;
