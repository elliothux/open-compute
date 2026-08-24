CREATE TABLE schema_migrations (
  version INTEGER NOT NULL PRIMARY KEY,
  name TEXT NOT NULL,
  checksum_sha256 BLOB NOT NULL CHECK (length(checksum_sha256) = 32),
  applied_at_ms INTEGER NOT NULL,
  app_version TEXT NOT NULL
) STRICT;

CREATE TABLE platform_meta (
  key TEXT NOT NULL PRIMARY KEY,
  value BLOB NOT NULL,
  updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE accounts (
  id TEXT NOT NULL PRIMARY KEY,
  name TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  deleted_at_ms INTEGER
) STRICT;

CREATE UNIQUE INDEX accounts_live_name ON accounts(name) WHERE deleted_at_ms IS NULL;
