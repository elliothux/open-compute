-- P24: a public Worker origin is served by exactly one transport. Existing public
-- routes were created by the managed Gateway; local routes use the local listener.
CREATE TABLE worker_host_routes_next (
  id TEXT PRIMARY KEY,
  claim_id TEXT NOT NULL UNIQUE,
  worker_id TEXT NOT NULL REFERENCES workers(id),
  namespace TEXT NOT NULL CHECK(namespace = 'worker'),
  exposure TEXT NOT NULL CHECK(exposure IN ('local', 'public')),
  transport TEXT NOT NULL
    CHECK(transport IN ('local_http', 'managed_gateway', 'external_proxy')),
  external_scheme TEXT,
  external_port INTEGER,
  path_prefix TEXT NOT NULL CHECK(path_prefix = '/'),
  entrypoint TEXT,
  state TEXT NOT NULL CHECK(state IN ('active', 'tombstoned')),
  generation INTEGER NOT NULL CHECK(generation > 0),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  deleted_at_ms INTEGER,
  CHECK((exposure = 'local' AND transport = 'local_http'
         AND external_scheme IS NULL AND external_port IS NULL) OR
        (exposure = 'public' AND transport = 'managed_gateway'
         AND external_scheme IS NULL AND external_port IS NULL) OR
        (exposure = 'public' AND transport = 'external_proxy'
         AND external_scheme IN ('http', 'https')
         AND external_port BETWEEN 1 AND 65535)),
  CHECK((state = 'active' AND deleted_at_ms IS NULL) OR
        (state = 'tombstoned' AND deleted_at_ms IS NOT NULL)),
  FOREIGN KEY(claim_id, namespace, exposure, state)
    REFERENCES hostname_claims(id, namespace, exposure, state)
    DEFERRABLE INITIALLY DEFERRED
) STRICT;

INSERT INTO worker_host_routes_next
  (id, claim_id, worker_id, namespace, exposure, transport, external_scheme,
   external_port, path_prefix, entrypoint, state, generation, created_at_ms,
   updated_at_ms, deleted_at_ms)
SELECT id, claim_id, worker_id, namespace, exposure,
       CASE exposure WHEN 'local' THEN 'local_http' ELSE 'managed_gateway' END,
       NULL, NULL, path_prefix, entrypoint, state, generation, created_at_ms,
       updated_at_ms, deleted_at_ms
FROM worker_host_routes;

DROP TABLE worker_host_routes;
ALTER TABLE worker_host_routes_next RENAME TO worker_host_routes;

CREATE UNIQUE INDEX active_worker_origin
ON worker_host_routes(worker_id, exposure) WHERE state = 'active';

CREATE TRIGGER worker_host_route_transition_guard
BEFORE UPDATE OF state ON worker_host_routes
WHEN NOT (OLD.state = 'active' AND NEW.state = 'tombstoned')
BEGIN SELECT RAISE(ABORT, 'worker host route state transition is invalid'); END;

CREATE TRIGGER worker_host_route_identity_immutable
BEFORE UPDATE OF claim_id, worker_id, exposure, transport, external_scheme,
                 external_port ON worker_host_routes
BEGIN SELECT RAISE(ABORT, 'worker host route identity is immutable'); END;

-- The managed namespace guard moves from the claim to the route, which knows the transport.
DROP TRIGGER public_worker_claim_insert_guard;

CREATE TRIGGER managed_worker_route_insert_guard
BEFORE INSERT ON worker_host_routes
WHEN NEW.transport = 'managed_gateway'
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM hostname_claims c
    JOIN public_gateway_domains d ON d.id = 1
    JOIN public_gateway_namespaces n ON n.domain_id = d.id
    WHERE c.id = NEW.claim_id
      AND d.state = 'active' AND n.name = 'worker' AND n.state = 'active'
      AND substr(c.hostname_ascii, 1, instr(c.hostname_ascii, '.') - 1)
          NOT IN ('ingress', 'ns1', 'r2', 'kv', 'api', 'admin', 'health', 'operator', 'probe')
      AND substr(c.hostname_ascii, -length(d.base_domain_ascii) - 1)
          = '.' || d.base_domain_ascii
      AND length(c.hostname_ascii) - length(d.base_domain_ascii) - 1 BETWEEN 1 AND 63
      AND instr(substr(c.hostname_ascii, 1,
                       length(c.hostname_ascii) - length(d.base_domain_ascii) - 1), '.') = 0
  ) THEN RAISE(ABORT, 'public worker namespace is not active') END;
END;

CREATE TRIGGER external_worker_route_insert_guard
BEFORE INSERT ON worker_host_routes
WHEN NEW.transport = 'external_proxy'
BEGIN
  SELECT CASE WHEN EXISTS (
    SELECT 1 FROM hostname_claims c
    JOIN public_gateway_domains d ON d.id = 1
    WHERE c.id = NEW.claim_id
      AND (c.hostname_ascii = d.base_domain_ascii
           OR substr(c.hostname_ascii, -length(d.base_domain_ascii) - 1)
              = '.' || d.base_domain_ascii)
  ) THEN RAISE(ABORT, 'external hostname uses the managed base domain') END;
END;

DROP TRIGGER public_gateway_domain_change_guard;

CREATE TRIGGER public_gateway_domain_change_guard
BEFORE UPDATE OF base_domain_ascii ON public_gateway_domains
WHEN EXISTS (
  SELECT 1 FROM worker_host_routes
  WHERE transport = 'managed_gateway' AND state = 'active'
)
BEGIN
  SELECT RAISE(ABORT, 'public claims still use the current base domain');
END;
