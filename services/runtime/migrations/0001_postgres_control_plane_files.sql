CREATE TABLE IF NOT EXISTS runtime_control_plane_files (
    file_path TEXT PRIMARY KEY,
    content BYTEA NOT NULL,
    content_sha256 CHAR(64) NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CHECK (file_path <> '' AND file_path !~ '(^|/)\.\.(/|$)'),
    CHECK (content_sha256 ~ '^[0-9a-f]{64}$')
);

CREATE INDEX IF NOT EXISTS runtime_control_plane_files_updated_at_idx
    ON runtime_control_plane_files (updated_at);
