CREATE TABLE IF NOT EXISTS file_metadata (
    id          TEXT PRIMARY KEY NOT NULL,
    file_name   TEXT NOT NULL,
    size_bytes  INTEGER NOT NULL,
    mime_type   TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
