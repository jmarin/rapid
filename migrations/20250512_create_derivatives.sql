CREATE TABLE IF NOT EXISTS derivatives (
    id          TEXT PRIMARY KEY NOT NULL,
    parent_id   TEXT NOT NULL REFERENCES file_metadata(id) ON DELETE CASCADE,
    size_label  TEXT NOT NULL,
    s3_key      TEXT NOT NULL,
    width       INTEGER NOT NULL,
    height      INTEGER NOT NULL,
    status      TEXT NOT NULL DEFAULT 'pending',
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(parent_id, size_label)
);

CREATE INDEX IF NOT EXISTS idx_derivatives_parent_id ON derivatives(parent_id);
