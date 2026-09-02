CREATE TABLE IF NOT EXISTS instance_meta (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    schema_version INTEGER NOT NULL CHECK(schema_version = 1),
    resource_id TEXT NOT NULL,
    model_contract_sha256 BLOB NOT NULL CHECK(length(model_contract_sha256) = 32),
    previous_model_contract_sha256 BLOB CHECK(previous_model_contract_sha256 IS NULL OR length(previous_model_contract_sha256) = 32),
    transition_model_contract_sha256 BLOB CHECK(transition_model_contract_sha256 IS NULL OR length(transition_model_contract_sha256) = 32),
    previous_model_contract_json BLOB,
    previous_public_config_json BLOB,
    previous_dimensions INTEGER CHECK(previous_dimensions IS NULL OR previous_dimensions >= 0),
    previous_vector_enabled INTEGER CHECK(previous_vector_enabled IS NULL OR previous_vector_enabled IN (0, 1)),
    previous_keyword_enabled INTEGER CHECK(previous_keyword_enabled IS NULL OR previous_keyword_enabled IN (0, 1)),
    model_contract_json BLOB NOT NULL,
    public_config_json BLOB NOT NULL,
    dimensions INTEGER NOT NULL CHECK(dimensions >= 0),
    vector_enabled INTEGER NOT NULL CHECK(vector_enabled IN (0, 1)),
    keyword_enabled INTEGER NOT NULL CHECK(keyword_enabled IN (0, 1)),
    active_index_generation INTEGER NOT NULL CHECK(active_index_generation > 0),
    active_epoch INTEGER NOT NULL CHECK(active_epoch > 0),
    config_generation INTEGER NOT NULL CHECK(config_generation > 0),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    CHECK((vector_enabled = 1 AND dimensions > 0) OR (vector_enabled = 0 AND dimensions = 0)),
    CHECK(vector_enabled = 1 OR keyword_enabled = 1),
    CHECK((previous_model_contract_sha256 IS NULL) = (previous_model_contract_json IS NULL)),
    CHECK((previous_model_contract_sha256 IS NULL) = (previous_public_config_json IS NULL)),
    CHECK((previous_model_contract_sha256 IS NULL) = (previous_dimensions IS NULL)),
    CHECK((previous_model_contract_sha256 IS NULL) = (previous_vector_enabled IS NULL)),
    CHECK((previous_model_contract_sha256 IS NULL) = (previous_keyword_enabled IS NULL))
) STRICT;

CREATE TABLE IF NOT EXISTS items (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    key TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('queued','running','completed','error','skipped','outdated')),
    active_generation INTEGER CHECK(active_generation > 0),
    desired_generation INTEGER NOT NULL CHECK(desired_generation > 0),
    metadata_json BLOB NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    UNIQUE(source, key)
) STRICT;

CREATE TABLE IF NOT EXISTS item_generations (
    item_id TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK(generation > 0),
    index_generation INTEGER NOT NULL CHECK(index_generation > 0),
    state TEXT NOT NULL CHECK(state IN ('queued','claimed','chunked','completed','error','outdated','cancelled')),
    object_key TEXT NOT NULL,
    object_sha256 BLOB NOT NULL CHECK(length(object_sha256) = 32),
    object_size INTEGER NOT NULL CHECK(object_size >= 0),
    content_type TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER,
    PRIMARY KEY(item_id, generation),
    FOREIGN KEY(item_id) REFERENCES items(id) ON DELETE CASCADE
) STRICT;

CREATE TABLE IF NOT EXISTS chunks (
    id TEXT PRIMARY KEY,
    item_id TEXT NOT NULL,
    item_generation INTEGER NOT NULL,
    index_generation INTEGER NOT NULL CHECK(index_generation > 0),
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    start_byte INTEGER NOT NULL CHECK(start_byte >= 0),
    end_byte INTEGER NOT NULL CHECK(end_byte >= start_byte),
    text TEXT NOT NULL,
    embedding_f32le BLOB CHECK(embedding_f32le IS NULL OR length(embedding_f32le) % 4 = 0),
    vector_norm REAL CHECK(vector_norm IS NULL OR vector_norm > 0.0),
    metadata_json BLOB NOT NULL,
    UNIQUE(item_id, item_generation, ordinal),
    FOREIGN KEY(item_id, item_generation)
      REFERENCES item_generations(item_id, generation) ON DELETE CASCADE
) STRICT;

CREATE INDEX IF NOT EXISTS chunks_by_active_item
ON chunks(item_id, item_generation, ordinal);

CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts_porter USING fts5(
    chunk_id UNINDEXED, text, tokenize='porter unicode61'
);

CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts_trigram USING fts5(
    chunk_id UNINDEXED, text, tokenize='trigram'
);

CREATE TABLE IF NOT EXISTS index_jobs (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL CHECK(source IN ('user','schedule')),
    description TEXT,
    state TEXT NOT NULL CHECK(state IN ('queued','claimed','retry_wait','completed','error','cancelling','cancelled','outdated')),
    config_generation INTEGER NOT NULL CHECK(config_generation > 0),
    index_generation INTEGER NOT NULL CHECK(index_generation > 0),
    claim_token BLOB CHECK(claim_token IS NULL OR length(claim_token) = 32),
    claim_until_ms INTEGER,
    attempt INTEGER NOT NULL CHECK(attempt >= 0),
    next_attempt_at_ms INTEGER NOT NULL,
    cancel_requested INTEGER NOT NULL CHECK(cancel_requested IN (0, 1)),
    created_at_ms INTEGER NOT NULL,
    started_at_ms INTEGER,
    ended_at_ms INTEGER,
    updated_at_ms INTEGER NOT NULL,
    CHECK((state IN ('claimed','cancelling')) =
          (claim_token IS NOT NULL AND claim_until_ms IS NOT NULL))
) STRICT;

CREATE INDEX IF NOT EXISTS index_jobs_due
ON index_jobs(state, next_attempt_at_ms, created_at_ms);

CREATE TABLE IF NOT EXISTS index_job_items (
    job_id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    item_generation INTEGER NOT NULL,
    index_generation INTEGER NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('queued','claimed','chunked','completed','error','outdated','cancelled')),
    next_batch_ordinal INTEGER NOT NULL CHECK(next_batch_ordinal >= 0),
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY(job_id, item_id),
    FOREIGN KEY(job_id) REFERENCES index_jobs(id) ON DELETE CASCADE,
    FOREIGN KEY(item_id, item_generation)
      REFERENCES item_generations(item_id, generation) ON DELETE CASCADE
) STRICT;

CREATE TABLE IF NOT EXISTS item_logs (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id TEXT NOT NULL,
    action TEXT NOT NULL,
    message_code TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    FOREIGN KEY(item_id) REFERENCES items(id) ON DELETE CASCADE
) STRICT;

CREATE TABLE IF NOT EXISTS job_logs (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id TEXT NOT NULL,
    message_code TEXT NOT NULL,
    message_type INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL,
    FOREIGN KEY(job_id) REFERENCES index_jobs(id) ON DELETE CASCADE
) STRICT;

CREATE TABLE IF NOT EXISTS ingest_intents (
    id TEXT PRIMARY KEY,
    item_id TEXT NOT NULL,
    object_key TEXT,
    object_sha256 BLOB CHECK(object_sha256 IS NULL OR length(object_sha256) = 32),
    object_size INTEGER CHECK(object_size IS NULL OR (object_size > 0 AND object_size <= 4194304)),
    state TEXT NOT NULL CHECK(state IN ('uploading','uploaded','committed','abandoned')),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    CHECK(
      (state IN ('uploading','uploaded','committed') AND object_key IS NOT NULL
          AND object_sha256 IS NOT NULL AND object_size IS NOT NULL)
      OR state = 'abandoned'
    )
) STRICT;

CREATE UNIQUE INDEX IF NOT EXISTS ingest_intents_active_item
ON ingest_intents(item_id) WHERE state IN ('uploading','uploaded');

CREATE TABLE IF NOT EXISTS object_gc (
    object_key TEXT PRIMARY KEY,
    object_sha256 BLOB NOT NULL CHECK(length(object_sha256) = 32),
    object_size INTEGER NOT NULL CHECK(object_size > 0 AND object_size <= 4194304),
    state TEXT NOT NULL CHECK(state IN ('queued','claimed','retry_wait')),
    claim_token BLOB CHECK(claim_token IS NULL OR length(claim_token) = 32),
    claim_until_ms INTEGER,
    attempt INTEGER NOT NULL CHECK(attempt >= 0),
    next_attempt_at_ms INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    CHECK((state = 'claimed') = (claim_token IS NOT NULL AND claim_until_ms IS NOT NULL))
) STRICT;
