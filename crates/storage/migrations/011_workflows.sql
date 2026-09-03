-- Current Day1 Workflow catalog and immutable version reachability.

CREATE TABLE workflow_bindings (
  id TEXT PRIMARY KEY,
  version_id TEXT NOT NULL REFERENCES worker_versions(id),
  name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 64),
  definition_id TEXT NOT NULL REFERENCES workflow_definitions(id),
  definition_lifecycle_generation INTEGER NOT NULL CHECK(definition_lifecycle_generation >= 1),
  capability_version INTEGER NOT NULL CHECK(capability_version = 1),
  schedules_json BLOB NOT NULL CHECK(length(schedules_json) BETWEEN 2 AND 32768),
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms INTEGER NOT NULL,
  UNIQUE(version_id,name)
) STRICT;

CREATE TABLE workflow_binding_operations (
  operation_id TEXT PRIMARY KEY,
  binding_id TEXT NOT NULL REFERENCES workflow_bindings(id),
  kind TEXT NOT NULL,
  fingerprint BLOB NOT NULL CHECK(length(fingerprint)=32),
  request_json BLOB NOT NULL CHECK(length(request_json)<=2097152),
  state TEXT NOT NULL CHECK(state IN ('prepared','applied')),
  response_json BLOB,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  CHECK((state='applied')=(response_json IS NOT NULL))
) STRICT;

CREATE TABLE workflow_binding_operation_locks (
  binding_id TEXT NOT NULL REFERENCES workflow_bindings(id),
  operation_id TEXT PRIMARY KEY REFERENCES workflow_binding_operations(operation_id),
  created_at_ms INTEGER NOT NULL
) STRICT;

CREATE TRIGGER workflow_binding_operation_immutable BEFORE UPDATE OF operation_id,binding_id,kind,
  fingerprint,request_json,created_at_ms ON workflow_binding_operations
BEGIN SELECT RAISE(ABORT,'workflow binding operation identity is immutable'); END;

CREATE TRIGGER workflow_binding_operation_transition BEFORE UPDATE ON workflow_binding_operations
WHEN OLD.state!='prepared' OR NEW.state!='applied' OR OLD.response_json IS NOT NULL OR NEW.response_json IS NULL
  OR NOT EXISTS(SELECT 1 FROM workflow_binding_operation_locks l
    WHERE l.binding_id=OLD.binding_id AND l.operation_id=OLD.operation_id)
BEGIN SELECT RAISE(ABORT,'workflow binding operation transition'); END;

CREATE TRIGGER workflow_binding_operation_lock_guard BEFORE INSERT ON workflow_binding_operation_locks
WHEN NOT EXISTS(SELECT 1 FROM workflow_binding_operations o WHERE o.operation_id=NEW.operation_id
  AND o.binding_id=NEW.binding_id AND o.state='prepared' AND o.created_at_ms=NEW.created_at_ms)
BEGIN SELECT RAISE(ABORT,'workflow binding operation lock authority'); END;

CREATE TRIGGER workflow_binding_operation_unlock_guard BEFORE DELETE ON workflow_binding_operation_locks
WHEN NOT EXISTS(SELECT 1 FROM workflow_binding_operations o WHERE o.operation_id=OLD.operation_id
  AND o.binding_id=OLD.binding_id AND o.state='applied')
BEGIN SELECT RAISE(ABORT,'workflow binding operation is unfinished'); END;

CREATE TABLE workflow_definitions (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 64),
  state TEXT NOT NULL CHECK(state IN ('creating','ready','deleting','tombstoned')),
  availability TEXT NOT NULL CHECK(availability IN ('healthy','degraded','unavailable')),
  availability_code TEXT,
  lifecycle_generation INTEGER NOT NULL CHECK(lifecycle_generation >= 1),
  current_version_id TEXT REFERENCES workflow_versions(id) DEFERRABLE INITIALLY DEFERRED,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  deleted_at_ms INTEGER,
  CHECK((state = 'tombstoned') = (deleted_at_ms IS NOT NULL)),
  CHECK((availability = 'healthy') = (availability_code IS NULL))
) STRICT;

CREATE TABLE workflow_instance_operations (
  operation_id TEXT PRIMARY KEY,
  instance_id TEXT NOT NULL UNIQUE REFERENCES workflow_instance_referrers(instance_id) ON DELETE CASCADE,
  creation_nonce BLOB NOT NULL CHECK(length(creation_nonce)=32),
  expected_generation INTEGER NOT NULL CHECK(expected_generation>=1),
  target_generation INTEGER NOT NULL CHECK(target_generation>=1),
  kind TEXT NOT NULL CHECK(kind IN ('restart','purge')),
  restart_from_name TEXT CHECK(restart_from_name IS NULL OR length(CAST(restart_from_name AS BLOB)) BETWEEN 1 AND 256),
  restart_from_count INTEGER CHECK(restart_from_count IS NULL OR restart_from_count BETWEEN 1 AND 1024),
  restart_from_kind TEXT CHECK(restart_from_kind IS NULL OR restart_from_kind IN ('do','sleep','waitForEvent')),
  prior_ref_state TEXT NOT NULL CHECK(prior_ref_state IN ('live','retained')),
  applied INTEGER NOT NULL DEFAULT 0 CHECK(applied IN (0,1)),
  created_at_ms INTEGER NOT NULL, operation_sequence INTEGER NOT NULL DEFAULT 1 CHECK(operation_sequence>=1),
  CHECK((kind='restart' AND expected_generation<9223372036854775807 AND target_generation=expected_generation+1)
     OR (kind='purge' AND target_generation=expected_generation AND prior_ref_state='retained')),
  CHECK((kind='purge' AND restart_from_name IS NULL AND restart_from_count IS NULL AND restart_from_kind IS NULL)
     OR (kind='restart' AND ((restart_from_name IS NULL AND restart_from_count IS NULL AND restart_from_kind IS NULL)
       OR (restart_from_name IS NOT NULL AND restart_from_count IS NOT NULL))))
) STRICT;

CREATE TABLE workflow_instance_referrers (
  instance_id TEXT PRIMARY KEY,
  definition_id TEXT NOT NULL REFERENCES workflow_definitions(id),
  definition_name TEXT NOT NULL,
  external_instance_id TEXT NOT NULL CHECK(length(external_instance_id) BETWEEN 1 AND 100),
  workflow_version_id TEXT NOT NULL REFERENCES workflow_versions(id),
  worker_version_id TEXT NOT NULL REFERENCES worker_versions(id),
  instance_generation INTEGER NOT NULL CHECK(instance_generation >= 1),
  creation_nonce BLOB NOT NULL CHECK(length(creation_nonce) = 32),
  creation_operation_id TEXT NOT NULL UNIQUE,
  creation_batch_id TEXT NOT NULL,
  state TEXT NOT NULL CHECK(state IN ('creating','live','retained','restarting','releasing','released')),
  trigger_cron TEXT CHECK(trigger_cron IS NULL OR length(trigger_cron) BETWEEN 1 AND 256),
  trigger_scheduled_time_ms INTEGER CHECK(trigger_scheduled_time_ms IS NULL OR trigger_scheduled_time_ms >= 0),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  released_at_ms INTEGER, operation_sequence INTEGER NOT NULL DEFAULT 0 CHECK(operation_sequence>=0),
  UNIQUE(definition_id,external_instance_id),
  CHECK((state = 'released') = (released_at_ms IS NOT NULL)),
  CHECK((trigger_cron IS NULL) = (trigger_scheduled_time_ms IS NULL))
) STRICT;

CREATE INDEX workflow_instance_creation_batch ON workflow_instance_referrers(creation_batch_id);

CREATE TABLE workflow_referrers (
  definition_id TEXT NOT NULL REFERENCES workflow_definitions(id),
  referrer_kind TEXT NOT NULL CHECK(referrer_kind IN ('binding','instance')),
  referrer_id TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  PRIMARY KEY(definition_id,referrer_kind,referrer_id)
) WITHOUT ROWID, STRICT;

CREATE TABLE workflow_versions (
  id TEXT PRIMARY KEY,
  definition_id TEXT NOT NULL REFERENCES workflow_definitions(id),
  version_number INTEGER NOT NULL CHECK(version_number > 0),
  state TEXT NOT NULL CHECK(state IN ('staging','validating','ready','rejected','deleting','tombstoned')),
  worker_id TEXT NOT NULL REFERENCES workers(id),
  worker_version_id TEXT NOT NULL REFERENCES worker_versions(id),
  class_name TEXT NOT NULL CHECK(length(class_name) BETWEEN 1 AND 128),
  worker_code_sha256 BLOB NOT NULL CHECK(length(worker_code_sha256) = 32),
  loader_schema_version INTEGER NOT NULL CHECK(loader_schema_version > 0),
  capability_version INTEGER NOT NULL CHECK(capability_version = 1),
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms INTEGER NOT NULL,
  ready_at_ms INTEGER,
  rejected_at_ms INTEGER,
  rejection_code TEXT,
  deleted_at_ms INTEGER,
  UNIQUE(definition_id,version_number),
  CHECK((state = 'tombstoned') = (deleted_at_ms IS NOT NULL)),
  CHECK(state != 'ready' OR ready_at_ms IS NOT NULL)
) STRICT;

CREATE UNIQUE INDEX workflow_definitions_live_name ON workflow_definitions(account_id,name)
WHERE state != 'tombstoned';

CREATE INDEX workflow_instance_operations_reconcile ON workflow_instance_operations(created_at_ms,operation_id);

CREATE INDEX workflow_instance_referrers_reconcile ON workflow_instance_referrers(state,updated_at_ms,instance_id);

CREATE TRIGGER workflow_binding_add_ref AFTER INSERT ON workflow_bindings
BEGIN INSERT INTO workflow_referrers VALUES(NEW.definition_id,'binding',NEW.id,NEW.created_at_ms); END;

CREATE TRIGGER workflow_binding_delete_guard BEFORE DELETE ON workflow_bindings
WHEN NOT EXISTS (SELECT 1 FROM worker_versions WHERE id = OLD.version_id AND state IN ('staging','rejected','deleting'))
BEGIN SELECT RAISE(ABORT,'workflow binding version is immutable'); END;

CREATE TRIGGER workflow_binding_immutable BEFORE UPDATE ON workflow_bindings
BEGIN SELECT RAISE(ABORT,'workflow binding is immutable'); END;

CREATE TRIGGER workflow_binding_insert_guard BEFORE INSERT ON workflow_bindings
BEGIN
  SELECT CASE WHEN NEW.name GLOB '*[^A-Za-z0-9_$]*' OR NEW.name GLOB '[0-9]*'
    OR NEW.name GLOB 'OPEN_COMPUTE_*' OR NEW.name GLOB '__*'
    THEN RAISE(ABORT,'workflow binding name') END;
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM worker_versions d JOIN workers w ON w.id = d.worker_id
    JOIN workflow_definitions f ON f.account_id = w.account_id
    WHERE d.id = NEW.version_id AND d.state = 'staging' AND f.id = NEW.definition_id
      AND f.state = 'ready' AND f.availability = 'healthy'
      AND f.lifecycle_generation = NEW.definition_lifecycle_generation
  ) THEN RAISE(ABORT,'workflow binding authority') END;
  SELECT CASE WHEN EXISTS (
    SELECT 1 FROM version_vars WHERE version_id = NEW.version_id AND name = NEW.name
    UNION ALL SELECT 1 FROM version_secrets WHERE version_id = NEW.version_id AND name = NEW.name
    UNION ALL SELECT 1 FROM version_bindings WHERE version_id = NEW.version_id AND name = NEW.name
    UNION ALL SELECT 1 FROM queue_producer_bindings WHERE version_id = NEW.version_id AND name = NEW.name
  ) THEN RAISE(ABORT,'workflow binding name conflict') END;
END;

CREATE TRIGGER workflow_binding_remove_ref AFTER DELETE ON workflow_bindings
BEGIN DELETE FROM workflow_referrers WHERE definition_id = OLD.definition_id AND referrer_kind = 'binding' AND referrer_id = OLD.id; END;

CREATE TRIGGER workflow_definition_current_guard BEFORE UPDATE OF current_version_id,state ON workflow_definitions
WHEN NEW.current_version_id IS NOT NULL OR NEW.state = 'ready'
BEGIN
  SELECT CASE WHEN NOT EXISTS (SELECT 1 FROM workflow_versions v
    WHERE v.id = NEW.current_version_id AND v.definition_id = NEW.id AND v.state = 'ready')
    THEN RAISE(ABORT,'workflow current version is not ready') END;
END;

CREATE TRIGGER workflow_definition_delete_guard BEFORE UPDATE OF state ON workflow_definitions
WHEN NEW.state IN ('deleting','tombstoned') AND (
  EXISTS (SELECT 1 FROM workflow_referrers WHERE definition_id = OLD.id) OR
  EXISTS (SELECT 1 FROM workflow_versions WHERE definition_id = OLD.id AND state IN ('staging','validating'))
) BEGIN SELECT RAISE(ABORT,'workflow is referenced'); END;

CREATE TRIGGER workflow_definition_identity_guard BEFORE UPDATE OF id,account_id,lifecycle_generation,created_at_ms
ON workflow_definitions BEGIN SELECT RAISE(ABORT,'workflow identity is immutable'); END;

CREATE TRIGGER workflow_definition_insert_guard BEFORE INSERT ON workflow_definitions
WHEN NEW.state != 'creating' OR NEW.current_version_id IS NOT NULL OR NEW.lifecycle_generation != 1
  OR NOT EXISTS(SELECT 1 FROM accounts WHERE id=NEW.account_id AND deleted_at_ms IS NULL)
BEGIN SELECT RAISE(ABORT,'workflow definition initial authority'); END;

CREATE TRIGGER workflow_definition_no_delete BEFORE DELETE ON workflow_definitions
BEGIN SELECT RAISE(ABORT,'workflow history cannot be deleted'); END;

CREATE TRIGGER workflow_definition_state_guard BEFORE UPDATE OF state ON workflow_definitions
WHEN NEW.state != OLD.state AND NOT (
  (OLD.state = 'creating' AND NEW.state IN ('ready','deleting')) OR
  (OLD.state = 'ready' AND NEW.state = 'deleting') OR
  (OLD.state = 'deleting' AND NEW.state = 'tombstoned')
) BEGIN SELECT RAISE(ABORT,'workflow state transition'); END;

CREATE TRIGGER workflow_definition_terminal_guard BEFORE UPDATE ON workflow_definitions
WHEN OLD.state = 'tombstoned' BEGIN SELECT RAISE(ABORT,'workflow tombstone is immutable'); END;

CREATE TRIGGER workflow_version_referrer_guard BEFORE DELETE ON version_referrers
WHEN (OLD.kind = 'workflow_version' AND EXISTS (SELECT 1 FROM workflow_versions
       WHERE id = OLD.ref_id AND state NOT IN ('deleting','tombstoned')))
  OR (OLD.kind = 'workflow_instance' AND EXISTS (SELECT 1 FROM workflow_instance_referrers
       WHERE instance_id = OLD.ref_id AND state != 'released'))
BEGIN SELECT RAISE(ABORT,'workflow version is referenced'); END;

CREATE TRIGGER workflow_instance_add_ref AFTER INSERT ON workflow_instance_referrers
BEGIN
  INSERT INTO version_referrers VALUES(NEW.worker_version_id,'workflow_instance',NEW.instance_id,NEW.created_at_ms);
  INSERT INTO workflow_referrers VALUES(NEW.definition_id,'instance',NEW.instance_id,NEW.created_at_ms);
END;

CREATE TRIGGER workflow_instance_generation_guard BEFORE UPDATE OF instance_generation ON workflow_instance_referrers
WHEN NOT (OLD.state='restarting' AND NEW.state='live' AND OLD.instance_generation<9223372036854775807
  AND NEW.instance_generation=OLD.instance_generation+1 AND EXISTS(
    SELECT 1 FROM workflow_instance_operations o WHERE o.instance_id=OLD.instance_id AND o.kind='restart'
      AND o.applied=1 AND o.creation_nonce=OLD.creation_nonce AND o.expected_generation=OLD.instance_generation
      AND o.target_generation=NEW.instance_generation))
BEGIN SELECT RAISE(ABORT,'workflow restart generation requires exact intent'); END;

CREATE TRIGGER workflow_instance_ref_identity_guard BEFORE UPDATE OF instance_id,definition_id,
  definition_name,external_instance_id,workflow_version_id,worker_version_id,creation_nonce,creation_operation_id,creation_batch_id,created_at_ms
ON workflow_instance_referrers BEGIN SELECT RAISE(ABORT,'workflow instance identity is immutable'); END;

CREATE TRIGGER workflow_instance_ref_insert_guard BEFORE INSERT ON workflow_instance_referrers
BEGIN
  SELECT CASE WHEN NEW.state != 'creating' OR NEW.instance_generation != 1 OR NOT EXISTS (
    SELECT 1 FROM workflow_definitions f JOIN workflow_versions v ON v.id = f.current_version_id
    WHERE f.id = NEW.definition_id AND f.state = 'ready' AND f.availability = 'healthy'
      AND v.id = NEW.workflow_version_id AND v.state = 'ready' AND v.worker_version_id = NEW.worker_version_id
  ) THEN RAISE(ABORT,'workflow creation authority') END;
END;

CREATE TRIGGER workflow_instance_ref_state_guard BEFORE UPDATE OF state ON workflow_instance_referrers
WHEN NEW.state != OLD.state AND NOT (
  (OLD.state='creating' AND NEW.state='live') OR
  (OLD.state='live' AND NEW.state='retained') OR
  (OLD.state IN ('live','retained') AND NEW.state='restarting' AND EXISTS(
    SELECT 1 FROM workflow_instance_operations o WHERE o.instance_id=OLD.instance_id AND o.kind='restart'
      AND o.creation_nonce=OLD.creation_nonce AND o.expected_generation=OLD.instance_generation AND o.prior_ref_state=OLD.state AND o.applied=0)) OR
  (OLD.state='restarting' AND EXISTS(SELECT 1 FROM workflow_instance_operations o WHERE o.instance_id=OLD.instance_id
    AND o.kind='restart' AND o.creation_nonce=OLD.creation_nonce AND o.expected_generation=OLD.instance_generation
    AND ((o.applied=0 AND NEW.state=o.prior_ref_state AND NEW.instance_generation=o.expected_generation)
      OR (o.applied=1 AND NEW.state='live' AND NEW.instance_generation=o.target_generation)))) OR
  (OLD.state='retained' AND NEW.state='releasing' AND EXISTS(SELECT 1 FROM workflow_instance_operations o
    WHERE o.instance_id=OLD.instance_id AND o.kind='purge' AND o.applied=1 AND o.creation_nonce=OLD.creation_nonce
      AND o.expected_generation=OLD.instance_generation)) OR
  (OLD.state='releasing' AND NEW.state='released')
) BEGIN SELECT RAISE(ABORT,'workflow referrer transition'); END;

CREATE TRIGGER workflow_instance_ref_terminal_guard BEFORE UPDATE ON workflow_instance_referrers
WHEN OLD.state = 'released' BEGIN SELECT RAISE(ABORT,'workflow released history is immutable'); END;

CREATE TRIGGER workflow_instance_release_ref AFTER UPDATE OF state ON workflow_instance_referrers
WHEN NEW.state = 'released'
BEGIN
  DELETE FROM version_referrers WHERE version_id = NEW.worker_version_id AND kind = 'workflow_instance' AND ref_id = NEW.instance_id;
  DELETE FROM workflow_referrers WHERE definition_id = NEW.definition_id AND referrer_kind = 'instance' AND referrer_id = NEW.instance_id;
END;

CREATE TRIGGER workflow_instance_reservation_delete_guard BEFORE DELETE ON workflow_instance_referrers
WHEN OLD.state!='creating' AND NOT (OLD.state='released' AND EXISTS(
  SELECT 1 FROM workflow_instance_operations o WHERE o.instance_id=OLD.instance_id AND o.kind='purge'
    AND o.applied=1 AND o.creation_nonce=OLD.creation_nonce AND o.expected_generation=OLD.instance_generation))
BEGIN SELECT RAISE(ABORT,'workflow history requires a proven purge'); END;

CREATE TRIGGER workflow_instance_reservation_remove_ref AFTER DELETE ON workflow_instance_referrers
BEGIN
  DELETE FROM version_referrers WHERE version_id = OLD.worker_version_id AND kind = 'workflow_instance' AND ref_id = OLD.instance_id;
  DELETE FROM workflow_referrers WHERE definition_id = OLD.definition_id AND referrer_kind = 'instance' AND referrer_id = OLD.instance_id;
END;

CREATE TRIGGER workflow_operation_apply_guard BEFORE UPDATE OF applied ON workflow_instance_operations
WHEN OLD.applied!=0 OR NEW.applied!=1
BEGIN SELECT RAISE(ABORT,'workflow operation proof is monotonic'); END;

CREATE TRIGGER workflow_operation_delete_guard BEFORE DELETE ON workflow_instance_operations
WHEN NOT (
  (OLD.applied=0 AND EXISTS(SELECT 1 FROM workflow_instance_referrers r WHERE r.instance_id=OLD.instance_id
    AND r.instance_generation=OLD.expected_generation AND r.state=OLD.prior_ref_state)) OR
  (OLD.kind='restart' AND OLD.applied=1 AND EXISTS(SELECT 1 FROM workflow_instance_referrers r
    WHERE r.instance_id=OLD.instance_id AND r.instance_generation=OLD.target_generation AND r.state='live')) OR
  (OLD.kind='purge' AND OLD.applied=1 AND NOT EXISTS(SELECT 1 FROM workflow_instance_referrers WHERE instance_id=OLD.instance_id))
) BEGIN SELECT RAISE(ABORT,'workflow operation is unfinished'); END;

CREATE TRIGGER workflow_operation_identity_guard BEFORE UPDATE OF operation_id,instance_id,creation_nonce,
  expected_generation,target_generation,kind,restart_from_name,restart_from_count,restart_from_kind,
  prior_ref_state,created_at_ms ON workflow_instance_operations
BEGIN SELECT RAISE(ABORT,'workflow operation is immutable'); END;

CREATE TRIGGER workflow_operation_insert_guard BEFORE INSERT ON workflow_instance_operations
WHEN NEW.applied!=0 OR NOT EXISTS(
  SELECT 1 FROM workflow_instance_referrers r JOIN workflow_versions v ON v.id=r.workflow_version_id
  WHERE r.instance_id=NEW.instance_id AND r.creation_nonce=NEW.creation_nonce
    AND r.instance_generation=NEW.expected_generation AND r.state=NEW.prior_ref_state
    AND v.capability_version=1 AND (NEW.kind='purge' OR (v.state='ready' AND EXISTS(
      SELECT 1 FROM workflow_definitions f JOIN worker_versions d ON d.id=r.worker_version_id
      JOIN workers w ON w.id=d.worker_id WHERE f.id=r.definition_id AND f.state='ready' AND f.availability='healthy'
        AND d.state='ready' AND w.deleted_at_ms IS NULL))))
BEGIN SELECT RAISE(ABORT,'workflow operation identity'); END;

CREATE TRIGGER workflow_operation_sequence_immutable BEFORE UPDATE OF operation_sequence ON workflow_instance_operations
BEGIN SELECT RAISE(ABORT,'workflow operation sequence is immutable'); END;

CREATE TRIGGER workflow_operation_sequence_insert_guard BEFORE INSERT ON workflow_instance_operations
WHEN NOT EXISTS(SELECT 1 FROM workflow_instance_referrers r WHERE r.instance_id=NEW.instance_id
  AND r.operation_sequence=NEW.operation_sequence AND r.operation_sequence>=1)
BEGIN SELECT RAISE(ABORT,'workflow operation sequence does not match its reservation'); END;

CREATE TRIGGER workflow_operation_sequence_reservation_guard BEFORE UPDATE OF operation_sequence ON workflow_instance_referrers
WHEN NEW.operation_sequence!=OLD.operation_sequence+1 OR OLD.operation_sequence=9223372036854775807
  OR OLD.state NOT IN ('live','retained') OR EXISTS(SELECT 1 FROM workflow_instance_operations WHERE instance_id=OLD.instance_id)
BEGIN SELECT RAISE(ABORT,'workflow operation sequence requires a free intent slot'); END;

CREATE TRIGGER workflow_queue_conflict BEFORE INSERT ON queue_producer_bindings
WHEN EXISTS(SELECT 1 FROM workflow_bindings WHERE version_id = NEW.version_id AND name = NEW.name)
BEGIN SELECT RAISE(ABORT,'workflow queue name conflict'); END;

CREATE TRIGGER workflow_referrer_guard BEFORE DELETE ON workflow_referrers
WHEN (OLD.referrer_kind = 'binding' AND EXISTS (
      SELECT 1 FROM workflow_bindings b
      JOIN worker_versions d ON d.id=b.version_id
      JOIN workers w ON w.id=d.worker_id
      WHERE b.id=OLD.referrer_id AND w.deleted_at_ms IS NULL
    ))
  OR (OLD.referrer_kind = 'instance' AND EXISTS (SELECT 1 FROM workflow_instance_referrers
      WHERE instance_id = OLD.referrer_id AND state != 'released'))
BEGIN SELECT RAISE(ABORT,'workflow is referenced'); END;

CREATE TRIGGER workflow_resource_conflict BEFORE INSERT ON version_bindings
WHEN EXISTS(SELECT 1 FROM workflow_bindings WHERE version_id = NEW.version_id AND name = NEW.name)
BEGIN SELECT RAISE(ABORT,'workflow resource name conflict'); END;

CREATE TRIGGER workflow_secret_conflict BEFORE INSERT ON version_secrets
WHEN EXISTS(SELECT 1 FROM workflow_bindings WHERE version_id = NEW.version_id AND name = NEW.name)
BEGIN SELECT RAISE(ABORT,'workflow secret name conflict'); END;

CREATE TRIGGER workflow_var_conflict BEFORE INSERT ON version_vars
WHEN EXISTS(SELECT 1 FROM workflow_bindings WHERE version_id = NEW.version_id AND name = NEW.name)
BEGIN SELECT RAISE(ABORT,'workflow variable name conflict'); END;

CREATE TRIGGER workflow_version_add_ref AFTER INSERT ON workflow_versions
BEGIN INSERT INTO version_referrers VALUES(NEW.worker_version_id,'workflow_version',NEW.id,NEW.created_at_ms); END;

CREATE TRIGGER workflow_version_delete_guard BEFORE UPDATE OF state ON workflow_versions
WHEN NEW.state IN ('deleting','tombstoned') AND (
  EXISTS (SELECT 1 FROM workflow_definitions WHERE current_version_id = OLD.id) OR
  EXISTS (SELECT 1 FROM workflow_instance_referrers WHERE workflow_version_id = OLD.id AND state != 'released')
) BEGIN SELECT RAISE(ABORT,'workflow version is referenced'); END;

CREATE TRIGGER workflow_version_identity_guard BEFORE UPDATE OF id,definition_id,version_number,worker_id,
  worker_version_id,class_name,worker_code_sha256,loader_schema_version,capability_version,descriptor_sha256,created_at_ms
ON workflow_versions BEGIN SELECT RAISE(ABORT,'workflow frozen version is immutable'); END;

CREATE TRIGGER workflow_version_insert_guard BEFORE INSERT ON workflow_versions
BEGIN
  SELECT CASE WHEN NEW.state != 'staging' OR NOT EXISTS (
    SELECT 1 FROM workflow_definitions f JOIN workers w ON w.account_id = f.account_id
    JOIN worker_versions d ON d.worker_id = w.id
    WHERE f.id = NEW.definition_id AND f.state IN ('creating','ready')
      AND w.id = NEW.worker_id AND w.deleted_at_ms IS NULL
      AND d.id = NEW.worker_version_id AND d.state = 'ready'
      AND d.worker_code_sha256 = NEW.worker_code_sha256 AND d.loader_schema_version = NEW.loader_schema_version
  ) THEN RAISE(ABORT,'workflow version authority') END;
END;

CREATE TRIGGER workflow_version_no_delete BEFORE DELETE ON workflow_versions
BEGIN SELECT RAISE(ABORT,'workflow version history cannot be deleted'); END;

CREATE TRIGGER workflow_version_release_ref AFTER UPDATE OF state ON workflow_versions
WHEN NEW.state = 'deleting'
BEGIN DELETE FROM version_referrers WHERE version_id = OLD.worker_version_id AND kind = 'workflow_version' AND ref_id = OLD.id; END;

CREATE TRIGGER workflow_version_state_guard BEFORE UPDATE OF state ON workflow_versions
WHEN NEW.state != OLD.state AND NOT (
  (OLD.state = 'staging' AND NEW.state IN ('validating','rejected')) OR
  (OLD.state = 'validating' AND NEW.state IN ('ready','rejected')) OR
  (OLD.state IN ('ready','rejected') AND NEW.state = 'deleting') OR
  (OLD.state = 'deleting' AND NEW.state = 'tombstoned')
) BEGIN SELECT RAISE(ABORT,'workflow version transition'); END;

CREATE TRIGGER workflow_version_terminal_guard BEFORE UPDATE ON workflow_versions
WHEN OLD.state = 'tombstoned' BEGIN SELECT RAISE(ABORT,'workflow version tombstone is immutable'); END;
