CREATE TABLE kv_meta (
  key TEXT PRIMARY KEY,
  value BLOB NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE kv_entries (
  id INTEGER PRIMARY KEY,
  key BLOB NOT NULL UNIQUE CHECK(length(key) BETWEEN 1 AND 512),
  value BLOB NOT NULL CHECK(length(value) <= 26214400),
  metadata_json BLOB CHECK(metadata_json IS NULL OR length(metadata_json) <= 1024),
  expires_at_ms INTEGER CHECK(expires_at_ms IS NULL OR expires_at_ms > 0),
  updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= 0)
) STRICT;

CREATE INDEX kv_entries_expiration ON kv_entries(expires_at_ms, id)
WHERE expires_at_ms IS NOT NULL;
