# Surface Checklist

Read only the sections for domains changed by the reviewed branch. This is a risk checklist, not an API catalog;
enumerate exact members from the fixed upstream stable declarations instead of copying signatures here.

## Workers runtime

- globals, module imports, handlers, `ExecutionContext`, `waitUntil`, `passThroughOnException`
- Request/Response/Headers, Fetch, Streams, WebSocket, Crypto, HTMLRewriter, Cache API, scheduled handlers, TCP sockets
- RPC capability lifetimes, entrypoint selection, structured-clone values, stream cancellation, error sanitization
- current default Node.js surface under the single effective compatibility date
- outbound HTTP(S) behavior without weakening open-compute's public-network boundary
- Service Binding, Static Assets, Version Metadata, and generated `Env` behavior where touched

## KV

- every single-key and batch overload of `get` and `getWithMetadata`
- text/json/arrayBuffer/stream values, metadata and `cacheStatus`
- expiration versus expiration TTL, list cursor/completion/prefix/limit ordering, key and value limits
- strong-local-consistency deviation without changing legal inputs, return shapes, or errors

## R2

- head/get/put/delete/list overloads, body/bodyUsed and conversion methods
- range and conditional requests, HTTP/custom metadata, checksums, storage class, SSE-C
- multipart create/resume/upload/complete/abort, part validation and failure cleanup
- cursor/delimiter/startAfter behavior, stream interruption, immutable bytes, restart and orphan cleanup

## D1

- prepare/bind/run/all/first/raw/batch/exec result and metadata shapes
- session constraints, bookmark production/consumption and monotonic single-node semantics
- parameter and SQL rejection, transaction visibility, failure atomicity, SQLite error mapping
- tenant database isolation, ATTACH/file access denial, restart and corruption behavior

## Durable Objects

- namespace and ID creation/parsing/equality, location-hint acceptance, stub fetch/RPC
- request serialization, class/facet identity, props/exports and deployment pinning
- async storage overloads, transactions/rollback, alarms and options
- SQL, sync KV, `transactionSync`, bookmarks, session restore behavior
- hibernatable WebSocket accept/list/tags/auto-response/timeouts/events and restart restoration
- delete/recreate identity, stale generation fencing and storage isolation

## Queues

- send/sendBatch results, metrics, delay and content types including structured-clone `v8`
- push consumer message/batch fields, attempts/timestamps, ack/retry and batch variants
- serialization failures, batch partial failure behavior, DLQ/retry limits where exposed
- committed-message durability, at-least-once delivery, crash recovery and stale claim fencing

## Workflows

- binding get/create/createBatch/deleteBatch and per-instance delete/status
- params and output structured clone, retention/location options and batch limits/errors
- step identity/config/context, parallel steps, sleep/sleepUntil/waitForEvent and event delivery
- pause/resume/terminate, restart-from-step, rollback registration/execution and cached step preservation
- output gate, stale-run fencing, retries, crash replay, version retention and external side-effect semantics

## Cross-cutting evidence

- apply the `SKILL.md` self-host exclusion test before treating edge placement, replication, global cache, fleet scale,
  or hosted control-plane differences as findings
- upstream type AST and compile fixture cover the changed member and all overloads
- capability/catalog/generated `Env`/config/docs agree in both directions
- positive, legal-boundary, rejection, stream/RPC, restart/crash and tenant-isolation cases exist as applicable
- product Gate reaches public `platformd`, verified stock workerd, real SQLite/S3 and real processes
- any accepted deviation has one owner, one stable ID, direct official evidence and regression coverage
