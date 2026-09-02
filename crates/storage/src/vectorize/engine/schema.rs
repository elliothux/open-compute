//! Authoritative per-index SQLite schema.

pub(super) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS index_meta (
  singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
  resource_id TEXT NOT NULL,
  schema_version INTEGER NOT NULL CHECK(schema_version = 1),
  dimensions INTEGER NOT NULL CHECK(dimensions BETWEEN 32 AND 1536),
  metric TEXT NOT NULL CHECK(metric IN ('cosine', 'euclidean', 'dot-product')),
  quota_vectors INTEGER NOT NULL CHECK(quota_vectors > 0),
  quota_bytes INTEGER NOT NULL CHECK(quota_bytes >= 1048576),
  vector_count INTEGER NOT NULL DEFAULT 0 CHECK(vector_count >= 0),
  next_sequence INTEGER NOT NULL DEFAULT 1 CHECK(next_sequence >= 1),
  processed_sequence INTEGER NOT NULL DEFAULT 0 CHECK(processed_sequence >= 0),
  metadata_generation INTEGER NOT NULL DEFAULT 0 CHECK(metadata_generation >= 0)
) STRICT;

CREATE TABLE IF NOT EXISTS vectors (
  vector_rowid INTEGER PRIMARY KEY,
  vector_id TEXT NOT NULL UNIQUE,
  namespace TEXT,
  values_f32le BLOB NOT NULL,
  metadata_json BLOB,
  norm REAL,
  updated_sequence INTEGER NOT NULL CHECK(updated_sequence >= 1),
  CHECK(length(CAST(vector_id AS BLOB)) BETWEEN 1 AND 64),
  CHECK(namespace IS NULL OR length(CAST(namespace AS BLOB)) BETWEEN 1 AND 64),
  CHECK(metadata_json IS NULL OR length(metadata_json) <= 10240)
) STRICT;
CREATE INDEX IF NOT EXISTS vectors_namespace ON vectors(namespace, vector_rowid);

CREATE TABLE IF NOT EXISTS metadata_indexes (
  property_name TEXT PRIMARY KEY,
  property_type TEXT NOT NULL CHECK(property_type IN ('string', 'number', 'boolean')),
  created_at_ms INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS metadata_terms (
  property_name TEXT NOT NULL REFERENCES metadata_indexes(property_name) ON DELETE CASCADE,
  vector_rowid INTEGER NOT NULL REFERENCES vectors(vector_rowid) ON DELETE CASCADE,
  ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
  string_value TEXT,
  number_value REAL,
  boolean_value INTEGER CHECK(boolean_value IN (0, 1)),
  PRIMARY KEY(property_name, vector_rowid, ordinal),
  CHECK((string_value IS NOT NULL) + (number_value IS NOT NULL) + (boolean_value IS NOT NULL) = 1)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS metadata_terms_string
  ON metadata_terms(property_name, string_value, vector_rowid);
CREATE INDEX IF NOT EXISTS metadata_terms_number
  ON metadata_terms(property_name, number_value, vector_rowid);
CREATE INDEX IF NOT EXISTS metadata_terms_boolean
  ON metadata_terms(property_name, boolean_value, vector_rowid);

CREATE TABLE IF NOT EXISTS vector_mutations (
  mutation_id TEXT PRIMARY KEY,
  sequence INTEGER NOT NULL UNIQUE CHECK(sequence >= 1),
  kind TEXT NOT NULL CHECK(kind IN ('insert', 'upsert', 'delete')),
  state TEXT NOT NULL CHECK(state IN ('queued', 'claimed', 'applied', 'failed')),
  claim_token BLOB,
  claim_until_ms INTEGER,
  attempt INTEGER NOT NULL DEFAULT 0 CHECK(attempt >= 0),
  next_attempt_at_ms INTEGER NOT NULL,
  item_count INTEGER NOT NULL CHECK(item_count BETWEEN 1 AND 1000),
  payload_bytes INTEGER NOT NULL CHECK(payload_bytes >= 0),
  error_code TEXT,
  created_at_ms INTEGER NOT NULL,
  completed_at_ms INTEGER,
  CHECK((state IN ('queued', 'claimed')) = (completed_at_ms IS NULL)),
  CHECK((state = 'claimed') = (claim_token IS NOT NULL AND claim_until_ms IS NOT NULL)),
  CHECK((state = 'failed') = (error_code IS NOT NULL))
) STRICT;

CREATE TABLE IF NOT EXISTS vector_mutation_items (
  mutation_id TEXT NOT NULL REFERENCES vector_mutations(mutation_id) ON DELETE CASCADE,
  ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
  vector_id TEXT NOT NULL,
  namespace TEXT,
  values_f32le BLOB,
  metadata_json BLOB,
  PRIMARY KEY(mutation_id, ordinal)
) STRICT, WITHOUT ROWID;
"#;
