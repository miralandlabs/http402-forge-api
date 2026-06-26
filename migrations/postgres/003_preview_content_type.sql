-- Preview MIME type stored separately from asset content_type.
--
-- Fresh installs: column included in migrations/postgres/001_init.sql.
-- Existing deployments: apply incrementally after 002_agent_metadata.sql.

ALTER TABLE listings ADD COLUMN IF NOT EXISTS preview_content_type TEXT NOT NULL DEFAULT '';
