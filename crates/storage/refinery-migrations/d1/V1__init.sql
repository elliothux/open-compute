CREATE TABLE __open_compute_meta (
  key TEXT PRIMARY KEY,
  value BLOB NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE __open_compute_migrations (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  sha256 BLOB NOT NULL CHECK(length(sha256) = 32),
  applied_at_ms INTEGER NOT NULL,
  UNIQUE(name, sha256)
) STRICT;
