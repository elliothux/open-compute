CREATE TABLE dual_origin_guard (
  valid INTEGER NOT NULL CHECK(valid = 1)
) STRICT;

INSERT INTO dual_origin_guard(valid)
SELECT CASE WHEN NOT EXISTS (
  SELECT 1 FROM hostname_claims c
  LEFT JOIN worker_host_routes r ON r.claim_id = c.id
  WHERE c.namespace != 'worker' OR c.exposure != 'local'
    OR r.id IS NULL OR r.account_id != c.account_id OR r.state != c.state
) AND NOT EXISTS (
  SELECT 1 FROM worker_host_routes r
  LEFT JOIN hostname_claims c ON c.id = r.claim_id
  WHERE c.id IS NULL OR c.account_id != r.account_id
) THEN 1 ELSE 0 END;

DROP TABLE dual_origin_guard;

CREATE TABLE public_gateway_domains (
  id INTEGER PRIMARY KEY CHECK(id = 1),
  base_domain_ascii TEXT NOT NULL,
  state TEXT NOT NULL CHECK(state IN ('provisioning', 'active', 'degraded', 'disabling', 'disabled')),
  generation INTEGER NOT NULL CHECK(generation > 0),
  updated_at_ms INTEGER NOT NULL,
  CHECK(length(base_domain_ascii) BETWEEN 4 AND 253),
  CHECK(base_domain_ascii = lower(base_domain_ascii)),
  CHECK(base_domain_ascii NOT GLOB '*[^a-z0-9.-]*'),
  CHECK(instr(base_domain_ascii, '..') = 0)
) STRICT;

CREATE TABLE public_gateway_namespaces (
  name TEXT PRIMARY KEY CHECK(name IN ('worker', 'r2')),
  domain_id INTEGER NOT NULL DEFAULT 1 REFERENCES public_gateway_domains(id),
  state TEXT NOT NULL CHECK(state IN ('provisioning', 'active', 'degraded', 'disabling', 'disabled')),
  generation INTEGER NOT NULL CHECK(generation > 0),
  qualified_at_ms INTEGER,
  updated_at_ms INTEGER NOT NULL,
  CHECK((state = 'active' AND qualified_at_ms IS NOT NULL) OR state != 'active')
) STRICT;

CREATE TABLE hostname_claims_next (
  id TEXT PRIMARY KEY,
  hostname_ascii TEXT NOT NULL,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  namespace TEXT NOT NULL CHECK(namespace = 'worker'),
  exposure TEXT NOT NULL CHECK(exposure IN ('local', 'public')),
  state TEXT NOT NULL CHECK(state IN ('active', 'tombstoned')),
  generation INTEGER NOT NULL CHECK(generation > 0),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  deleted_at_ms INTEGER,
  CHECK(length(hostname_ascii) BETWEEN 4 AND 253),
  CHECK(hostname_ascii = lower(hostname_ascii)),
  CHECK(hostname_ascii NOT GLOB '*[^a-z0-9.-]*'),
  CHECK(instr(hostname_ascii, '..') = 0),
  CHECK(instr(hostname_ascii, ':') = 0),
  CHECK(instr(hostname_ascii, '/') = 0),
  CHECK(exposure != 'public' OR
        (instr(hostname_ascii, '.') BETWEEN 2 AND 64
         AND substr(hostname_ascii, 1, 1) GLOB '[a-z0-9]'
         AND substr(hostname_ascii, instr(hostname_ascii, '.') - 1, 1) GLOB '[a-z0-9]')),
  CHECK((exposure = 'local' AND substr(hostname_ascii, -10) = '.localhost') OR
        (exposure = 'public' AND substr(hostname_ascii, -10) != '.localhost')),
  CHECK((state = 'active' AND deleted_at_ms IS NULL) OR
        (state = 'tombstoned' AND deleted_at_ms IS NOT NULL))
) STRICT;

CREATE UNIQUE INDEX hostname_claim_authority
ON hostname_claims_next(id, account_id, namespace, exposure, state);

CREATE TABLE worker_host_routes_next (
  id TEXT PRIMARY KEY,
  claim_id TEXT NOT NULL UNIQUE,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  worker_id TEXT NOT NULL,
  namespace TEXT NOT NULL CHECK(namespace = 'worker'),
  exposure TEXT NOT NULL CHECK(exposure IN ('local', 'public')),
  path_prefix TEXT NOT NULL CHECK(path_prefix = '/'),
  entrypoint TEXT,
  state TEXT NOT NULL CHECK(state IN ('active', 'tombstoned')),
  generation INTEGER NOT NULL CHECK(generation > 0),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  deleted_at_ms INTEGER,
  CHECK((state = 'active' AND deleted_at_ms IS NULL) OR
        (state = 'tombstoned' AND deleted_at_ms IS NOT NULL)),
  FOREIGN KEY(claim_id, account_id, namespace, exposure, state)
    REFERENCES hostname_claims_next(id, account_id, namespace, exposure, state)
    DEFERRABLE INITIALLY DEFERRED,
  FOREIGN KEY(worker_id, account_id) REFERENCES workers(id, account_id)
) STRICT;

INSERT INTO hostname_claims_next
  (id, hostname_ascii, account_id, namespace, exposure, state, generation,
   created_at_ms, updated_at_ms, deleted_at_ms)
SELECT id, hostname_ascii, account_id, namespace, exposure, state, generation,
       created_at_ms, updated_at_ms, deleted_at_ms
FROM hostname_claims;

INSERT INTO worker_host_routes_next
  (id, claim_id, account_id, worker_id, namespace, exposure, path_prefix,
   entrypoint, state, generation, created_at_ms, updated_at_ms, deleted_at_ms)
SELECT r.id, r.claim_id, r.account_id, r.worker_id, c.namespace, c.exposure,
       r.path_prefix, r.entrypoint, r.state, r.generation,
       r.created_at_ms, r.updated_at_ms, r.deleted_at_ms
FROM worker_host_routes r
JOIN hostname_claims c ON c.id = r.claim_id;

DROP TABLE worker_host_routes;
DROP TABLE hostname_claims;
ALTER TABLE hostname_claims_next RENAME TO hostname_claims;
ALTER TABLE worker_host_routes_next RENAME TO worker_host_routes;

CREATE UNIQUE INDEX active_hostname_claims
ON hostname_claims(hostname_ascii)
WHERE state = 'active';

CREATE UNIQUE INDEX active_worker_origin
ON worker_host_routes(worker_id, exposure)
WHERE state = 'active';

CREATE TRIGGER public_worker_claim_insert_guard
BEFORE INSERT ON hostname_claims
WHEN NEW.exposure = 'public'
BEGIN
  SELECT CASE WHEN substr(NEW.hostname_ascii, 1, instr(NEW.hostname_ascii, '.') - 1)
    IN ('ingress', 'ns1', 'r2', 'kv', 'api', 'admin', 'health', 'operator', 'probe')
    THEN RAISE(ABORT, 'public worker name is reserved') END;
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM public_gateway_domains d
    JOIN public_gateway_namespaces n ON n.domain_id = d.id
    WHERE d.id = 1 AND d.state = 'active'
      AND n.name = 'worker' AND n.state = 'active'
      AND substr(NEW.hostname_ascii, -length(d.base_domain_ascii) - 1)
          = '.' || d.base_domain_ascii
      AND length(NEW.hostname_ascii) - length(d.base_domain_ascii) - 1 BETWEEN 1 AND 63
      AND instr(substr(NEW.hostname_ascii, 1,
                       length(NEW.hostname_ascii) - length(d.base_domain_ascii) - 1), '.') = 0
  ) THEN RAISE(ABORT, 'public worker namespace is not active') END;
END;

CREATE TRIGGER hostname_claim_identity_immutable
BEFORE UPDATE OF hostname_ascii, account_id, namespace, exposure ON hostname_claims
BEGIN
  SELECT RAISE(ABORT, 'hostname claim identity is immutable');
END;

CREATE TRIGGER hostname_claim_transition_guard
BEFORE UPDATE OF state ON hostname_claims
WHEN NOT (OLD.state = 'active' AND NEW.state = 'tombstoned')
BEGIN
  SELECT RAISE(ABORT, 'hostname claim state transition is invalid');
END;

CREATE TRIGGER worker_host_route_transition_guard
BEFORE UPDATE OF state ON worker_host_routes
WHEN NOT (OLD.state = 'active' AND NEW.state = 'tombstoned')
BEGIN
  SELECT RAISE(ABORT, 'worker host route state transition is invalid');
END;

CREATE TRIGGER public_gateway_domain_change_guard
BEFORE UPDATE OF base_domain_ascii ON public_gateway_domains
WHEN EXISTS (SELECT 1 FROM hostname_claims WHERE exposure = 'public' AND state = 'active')
BEGIN
  SELECT RAISE(ABORT, 'public claims still use the current base domain');
END;
