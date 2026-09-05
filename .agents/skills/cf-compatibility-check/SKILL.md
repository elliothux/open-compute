---
name: cf-compatibility-check
description: Review branch and working-tree implementation changes for conformance with open-compute's Cloudflare Workers runtime target under explicit single-machine self-host exclusions. Use when checking new Workers, Durable Objects, Queues, Workflows, R2, D1, KV, binding, type, or runtime behavior against current Cloudflare; report evidence-backed findings without editing. Do not use for Cloudflare management API parity or general code review.
---

# Cloudflare Compatibility Check

Review only the changed implementation and the context needed to prove its behavior. Report findings; do not edit unless the user also requests fixes.

## Load the contract first

Before judging code, read completely:

1. the repository `AGENTS.md`;
2. `docs/implemented/cloudflare-runtime-compatibility.md`;
3. `docs/references/cloudflare-compatibility.md`;
4. `docs/references/p1-deviations.md`;
5. `packages/runtime/workerd.lock.json` and the root package manifest when runtime or public types changed.

The active target is the stable tenant Worker programming surface for Workers runtime, Durable Objects, Queues,
Workflows, R2, D1, and KV. Cloudflare management APIs, global edge topology, and other products are outside this
review unless the user explicitly changes the scope.

## Resolve the review scope

1. Use a user-specified base when provided.
2. Otherwise resolve the repository's default branch from a local `refs/remotes/<remote>/HEAD` symbolic ref and use
   its merge base with `HEAD`. Do not fetch or pull merely to establish a base.
3. If no default-branch ref exists, use a configured upstream only when it clearly represents the integration base.
   Do not guess `main`, `master`, or another branch name. Review the working tree immediately and state that committed
   branch history remains unreviewed until the base is known.
4. Inventory these scopes separately:
   - committed changes from the merge base through `HEAD`;
   - staged and unstaged tracked changes against `HEAD`;
   - untracked files from `git ls-files --others --exclude-standard`.
5. Inspect deleted code and changed consumers, not only added lines. Expand beyond the diff only to resolve call paths,
   persisted authority, generated assets, and tests needed to prove a candidate.

Do not report unrelated pre-existing incompatibilities. Report an older defect only when changed code activates,
widens, or relies on it.

## Use current primary sources

For every changed Cloudflare-visible behavior, retrieve current evidence instead of relying on memory:

1. fixed stable `@cloudflare/workers-types` declarations and the matching pinned workerd generated declarations;
2. official Cloudflare runtime or product documentation for semantics that types cannot express;
3. official compatibility-flags documentation and Workers changelog for current default behavior;
4. matching upstream workerd source and tests, then a verified stock workerd binary for executable behavior;
5. a portable real-Cloudflare differential only when the user explicitly authorizes its external writes and supplies
   the required account and credentials.

Use the `third_party/workerd` submodule, the workerd pin, and installed or locked npm artifacts before considering a
network download. The fork checkout may differ from the formal pin: read `types/generated-snapshot/index.d.ts` and
runtime source from the matching revision with `git show`, and report missing objects instead of substituting HEAD.
Browse only official Cloudflare documentation and primary upstream sources for
technical claims. Treat workers-sdk, Wrangler, Miniflare, WDL, and other reference projects as integration evidence,
not as authority over Cloudflare docs, stable types, or stock workerd behavior.

When sources conflict, record the exact versions and block the compatibility claim. Do not invent a compromise API.

## Review each changed surface

Map every changed tenant-visible behavior to a product, upstream declaration, runtime source, and local test. Read only
the relevant sections of [the surface checklist](references/surface-checklist.md) after identifying the changed
domains.

Apply these gates:

### Type gate

- Public Cloudflare runtime declarations must come directly from fixed `@cloudflare/workers-types` stable or from a
  reproducible matching workerd type generation.
- Generated per-deployment `Env`, Service RPC, and Durable Object stub types may compose upstream declarations from
  validated binding configuration.
- Flag handwritten, copied, narrowed, widened, or partially re-declared Cloudflare interfaces, including missing
  overloads, changed generics, optionality, readonly fields, return types, module declarations, and error types.
- The full upstream package may contain types for non-target products. Actual `Env`, runtime availability,
  capability/catalog state, and configuration rejection establish the product boundary; deleting upstream type names
  does not.
- Stable target members without runtime support are `blocked`, not `unsupported`. Experimental declarations are not
  part of the target unless explicitly added.

### Runtime gate

- Trace the public entry point through validation, canonical descriptor, persisted authority, WorkerLoader/system
  Worker, stock workerd primitive, storage, and response/error mapping.
- Compare success, failure, overload dispatch, default values, limits, metadata, pagination, conditional operations,
  streams, structured clone, RPC, cancellation, retry, transaction, lifecycle, and restart behavior where applicable.
- A matching method name or a passing smoke test is not runtime compatibility. A stock workerd primitive is not enough
  when an open-compute facade changes inputs, outputs, errors, visibility, or durability.
- Accept a deviation only when it is caused directly by single-machine topology, preserves the API and security/data
  contract, has a stable deviation ID and official source, and is covered by positive, negative, and recovery tests.

### Single-latest gate

- Tenant configuration must not select historical `compatibility_date`, date ranges, or compatibility-flag sets.
- The platform must use one reproducibly pinned effective compatibility date and only the internal flags required for
  the current stable contract.
- Flag old/new behavior branches, historical fallbacks, date-dependent persisted contracts, or claims based on a
  newer npm type surface than the pinned workerd/runtime can execute.

### Scope and authority gate

- Do not require Cloudflare `/client/v4`, Dashboard, billing, account, or Wrangler management parity.
- Reject non-target binding configuration at the authority boundary, but do not confuse an external data-plane
  protocol with a management API.
- Preserve tenant isolation, immutable deployment identity, secret boundaries, transaction integrity, restart/crash
  recovery, and fail-closed validation. Self-host topology does not excuse violations of these properties.
- Reject scenario-specific production branches, test fixture literals, mock-only behavior, or a framework adapter used
  as proof of the generic platform contract.

### What does not count as incompatibility

Do not file a finding merely because open-compute lacks a Cloudflare property that only exists to operate a global,
multi-region edge network or hosted control plane. Apply the boundary explicitly:

| Cloudflare property | Not a finding for this single-machine self-host target | Still a finding when changed code does this |
| --- | --- | --- |
| Anycast, colo selection, smart placement, regional traffic steering, edge deployment | Runs one local `platformd` and one supervised workerd generation; a placement hint has no physical placement effect | Removes a stable API/type, rejects an otherwise legal hint without the documented local contract, leaks internal topology, or claims real regional placement |
| Cross-region replication, geographic failover, multi-site HA, concurrent multi-writer storage | Uses one authoritative local SQLite/data directory and configured object store; no remote replica or automatic regional failover | Loses committed local state on restart, permits split authority, breaks atomicity/isolation, or advertises replication/failover it does not implement |
| Globally distributed KV/R2/D1 consistency and edge caching | Provides a documented stronger local consistency model and no global replica/cache propagation | Changes legal API inputs, return/error shapes, transaction behavior, D1 session/bookmark semantics, object bytes, or tenant isolation |
| Durable Object global placement and cross-colo migration | Keeps an object on the local runtime; location hints need not move it between regions | Breaks single-node uniqueness, ID/stub behavior, storage, alarms, RPC, hibernation, restart recovery, or accepts stale-generation commits |
| Queue/Workflow global autoscaling, placement and hosted orchestration | Uses the local durable scheduler; does not promise global scaling, strict FIFO, or exactly-once delivery | Loses committed work, violates the declared at-least-once/retry contract, reruns committed Workflow steps, breaks lifecycle APIs, or skips stale-token fencing |
| Global CDN, tiered cache, Cache Reserve and worldwide purge propagation | Implements only the local Workers Cache/Cache API authority and local invalidation | Changes Cache API shape/conditions, serves stale bytes after a completed local purge, or crosses account/Worker/deployment boundaries |
| Physical edge metadata | Returns documented, stable, sanitized local semantics where a stable Worker API exposes edge metadata; values need not represent a real colo | Omits the target API member, changes its type/error contract, invents remote topology, or exposes host/internal network details |
| Cloudflare plan tiers, billing quotas and fleet-scale resource limits | Uses explicit local resource budgets appropriate to one host instead of matching paid-plan or fleet quotas | Silently ignores a legal option, removes a stable method, becomes unbounded/unsafe, or reports a limit as Cloudflare-compatible without evidence |
| Dashboard, account/organization, billing, token/permission, `/client/v4`, hosted analytics and global observability | Omits Cloudflare's hosted management and control-plane behavior | Leaks those concepts into the tenant runtime contract, or a local management path violates open-compute's own security, integrity, and lifecycle rules |
| R2 S3 endpoint, Queue pull HTTP endpoint and other external data-plane protocols not defined by tenant Worker types | Leaves them outside this review unless a separate protocol-compatibility target exists | Mislabels them as management APIs, claims protocol compatibility, or changes an explicitly in-scope protocol implementation incorrectly |

Classify a difference as an excluded self-host property only when all of these are true:

1. it follows directly from single-host topology or the excluded hosted control plane;
2. it removes no stable target API member, overload, legal input, return field, or required failure behavior;
3. it preserves local durability, transaction, security, isolation, lifecycle, and restart/crash guarantees;
4. the local behavior is explicit in the compatibility target or a stable deviation, with no false Cloudflare claim.

If any condition fails, review the difference normally. “Single machine” is never a blanket excuse for an incomplete
API facade, an in-memory fallback, lost work, corrupt state, weak tenant isolation, or missing recovery.

## Validate proportionally

Run existing non-destructive focused checks needed to validate a finding. Follow the repository's one-round development
Gate rules and build runtime TypeScript assets before Cargo consumes them. Use real stock workerd/product paths when
the claim concerns runtime behavior; mocks can narrow a diagnosis but cannot close it.

Do not download a runtime, run a final three-round Gate, invoke privileged egress fixtures, or create Cloudflare
resources during an ordinary review. If a required pinned archive or `OPEN_COMPUTE_TEST_WORKERD` is unavailable, mark
the affected behavior `unverified` and name the exact missing evidence. An unrun differential is a qualification gap,
not proof of incompatibility.

## Findings

Return only actionable, evidence-backed findings caused by the reviewed change. Sort by severity:

- **P0:** tenant escape, secret disclosure, cross-account access, corruption, or irreversible loss relative to the
  Cloudflare contract;
- **P1:** stable target API/type/runtime mismatch, falsely advertised support, silently ignored legal input, wrong
  single-latest behavior, or broken durability/transaction semantics;
- **P2:** observable error, limit, ordering, retry, metadata, stream, or deviation mismatch with material compatibility
  impact;
- **P3:** capability/catalog/docs or regression-evidence drift that can cause a false compatibility claim but does not
  itself prove wrong runtime behavior.

For each finding include:

- changed `path:line`;
- the exact open-compute behavior and reachable call path;
- the expected Cloudflare behavior with a direct official URL and pinned type/workerd revision where relevant;
- a minimal reproducer or concrete input;
- impact and why the severity fits;
- the smallest root-cause fix and the regression Gate that should prove it.

Separate defects from evidence gaps. Finish with a compact coverage table mapping each changed Cloudflare surface to
`aligned`, `mismatch`, or `unverified`, with the evidence used. If no finding remains, say the reviewed scope is clean,
list the surfaces and checks covered, and retain any unverified items. Never generalize a clean branch review into a
claim that the whole platform is Cloudflare-compatible.

When the reviewed branch touches an excluded distributed property, list it separately as `excluded self-host scope`
with the applicable row from the table above. Do not count it as a finding or as an unverified target capability unless
the branch claims to implement it.
