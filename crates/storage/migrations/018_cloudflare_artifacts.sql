CREATE TABLE artifact_namespaces (
  id TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  account_id TEXT NOT NULL REFERENCES accounts(id),
  name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 64),
  jurisdiction TEXT CHECK(jurisdiction IS NULL OR length(jurisdiction) BETWEEN 1 AND 32),
  max_repositories INTEGER NOT NULL CHECK(max_repositories BETWEEN 1 AND 10000),
  max_tokens_per_repository INTEGER NOT NULL CHECK(max_tokens_per_repository BETWEEN 1 AND 1000),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  UNIQUE(account_id, name)
) STRICT;

CREATE TABLE artifact_repositories (
  id TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  namespace_id TEXT NOT NULL REFERENCES artifact_namespaces(id),
  name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 128),
  description TEXT NOT NULL DEFAULT '' CHECK(length(description) <= 2048),
  default_branch TEXT NOT NULL CHECK(length(default_branch) BETWEEN 1 AND 255),
  state TEXT NOT NULL CHECK(state IN (
    'creating', 'importing', 'forking', 'ready', 'deleting', 'failed', 'tombstoned'
  )),
  read_only INTEGER NOT NULL DEFAULT 0 CHECK(read_only IN (0, 1)),
  source TEXT,
  generation INTEGER NOT NULL DEFAULT 1 CHECK(generation >= 1),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  last_push_at_ms INTEGER,
  deleted_at_ms INTEGER,
  CHECK((state = 'tombstoned') = (deleted_at_ms IS NOT NULL))
) STRICT;

CREATE UNIQUE INDEX artifact_repositories_live_name
ON artifact_repositories(namespace_id, name)
WHERE state != 'tombstoned';

CREATE INDEX artifact_repositories_state
ON artifact_repositories(state, updated_at_ms, id);

CREATE TABLE artifact_repo_tokens (
  id TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  repository_id TEXT NOT NULL REFERENCES artifact_repositories(id),
  token_digest BLOB NOT NULL CHECK(length(token_digest) = 32),
  scope TEXT NOT NULL CHECK(scope IN ('read', 'write')),
  expires_at_ms INTEGER NOT NULL,
  created_at_ms INTEGER NOT NULL,
  revoked_at_ms INTEGER,
  UNIQUE(repository_id, token_digest)
) STRICT;

CREATE INDEX artifact_repo_tokens_active
ON artifact_repo_tokens(repository_id, expires_at_ms, id)
WHERE revoked_at_ms IS NULL;

CREATE TABLE version_artifact_bindings (
  id TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  version_id TEXT NOT NULL REFERENCES worker_versions(id),
  name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 64),
  namespace_id TEXT NOT NULL REFERENCES artifact_namespaces(id),
  namespace_generation INTEGER NOT NULL CHECK(namespace_generation >= 1),
  capability_version INTEGER NOT NULL CHECK(capability_version = 1),
  permissions_json BLOB NOT NULL,
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms INTEGER NOT NULL,
  UNIQUE(version_id, name)
) STRICT;

CREATE INDEX version_artifact_bindings_namespace
ON version_artifact_bindings(namespace_id, version_id, id);

CREATE TRIGGER artifact_namespace_identity_immutable
BEFORE UPDATE OF id, account_id, name, created_at_ms ON artifact_namespaces
BEGIN
  SELECT RAISE(ABORT, 'artifact namespace identity is immutable');
END;

CREATE TRIGGER artifact_repository_identity_immutable
BEFORE UPDATE OF id, namespace_id, name, created_at_ms ON artifact_repositories
BEGIN
  SELECT RAISE(ABORT, 'artifact repository identity is immutable');
END;

CREATE TRIGGER artifact_repository_transition_guard
BEFORE UPDATE OF state ON artifact_repositories
WHEN OLD.state != NEW.state AND NOT (
  (OLD.state IN ('creating', 'importing', 'forking') AND NEW.state IN ('ready', 'failed', 'deleting')) OR
  (OLD.state = 'ready' AND NEW.state IN ('deleting', 'failed')) OR
  (OLD.state = 'failed' AND NEW.state = 'deleting') OR
  (OLD.state = 'deleting' AND NEW.state = 'ready') OR
  (OLD.state = 'deleting' AND NEW.state = 'tombstoned')
)
BEGIN
  SELECT RAISE(ABORT, 'invalid artifact repository transition');
END;
