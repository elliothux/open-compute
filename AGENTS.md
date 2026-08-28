# AGENTS – open-compute

## Operating Contract

- Do not send optional commentary.
- For answer, explanation, review, diagnosis, or planning requests, inspect the relevant evidence and report the result; do not implement changes unless requested.
- For change, build, or fix requests, make the requested in-scope local changes and run relevant non-destructive validation without asking first.
- Reading files, inspecting logs and Git state, editing requested local files, and running non-destructive checks are authorized local actions.
- Ask before external writes, destructive actions, privileged commands, runtime downloads, release packaging, or a material scope expansion.

## Day1 Architecture

- **open-compute is still in development and has not been deployed to production. All logic and architecture must follow Day1 design for current requirements.** This applies to Rust, TypeScript, runtime assets, persistence, APIs, CLI, configuration, and tooling.
- Do not design for compatibility with earlier open-compute revisions, historical development data, old APIs/configuration, schemas, snapshots, artifacts, module paths, or byte/hash formats. Do not introduce or retain compatibility shims, legacy implementations, version-selection branches, dual read/write paths, aliases, defaults, or backfills to preserve those earlier states.
- The only compatibility exception is observable behavior required by an official Cloudflare API contract within this project's declared support scope, including applicable compatibility dates and flags. Before retaining a compatibility branch, document the official source, the exact affected behavior, its supported date/flag or version range, the pinned workerd constraints where relevant, and regression coverage. Implement that contract directly; it does not justify preserving obsolete open-compute engines, private protocols, schemas, configurations, snapshots, or byte/hash formats.
- Change the current model directly and keep one authoritative implementation. Update affected producers, consumers, schema definitions, fixtures, tests, build paths, and documentation together within the requested scope; remove superseded code in the same change. Existing code, version labels, tests, and historical design documents do not create compatibility obligations.
- Continue to enforce current dependency/protocol contracts, the pinned workerd requirements, security boundaries, data integrity, immutable deployment semantics, and restart/crash recovery within the current implementation. Development status does not permit weaker validation, skipped Gates, or silent repair of corrupt state.
- Day1 design does not authorize deleting or resetting existing local data, rewriting Git history, or removing retained evidence. The Operating Contract's approval requirements and evidence-preservation rules still apply.
- Track known removals and their acceptance criteria in [Day1 architecture cleanup](docs/day1-architecture-cleanup.md). A recorded cleanup item is not evidence that its implementation or verification is complete.

## Repository Scope

- This file applies to the entire repository. Do not add nested `AGENTS.md` files.
- Treat this repository as the source of truth for `open-compute`; do not edit the parent Lynx OS project unless the user explicitly includes it in scope.
- `crates/**`, `packages/**`, and `share/**` own production sources, tooling, and assets; `test/**` owns repository-level test/Gate scripts, fixtures, and fuzz tooling; `scripts/**` and `examples/**` are operator/release surfaces. Keep crate-local and package-local tests beside their owning code. The current root `runtime/` is pending migration to `packages/runtime/`; see [Runtime and test layout](docs/runtime-and-test-layout.md) for the target layout and acceptance criteria, not evidence of a completed move.
- Completed POC upstream-capability probes are disposable one-time code, not recurring acceptance Gates. Delete them in the scoped cleanup; move surviving product regression tests and their required harness/fixtures from `poc/` into `test/`, consolidating equivalent coverage instead of retaining a second prototype implementation. Do not keep an old `poc/g0` compatibility entry point.
- Preserve `docs/implemented/g0-results.md` and other historical evidence as records of their actual runs. Do not hand-edit them, rewrite them to claim new acceptance, or require their old generator to survive the POC cleanup. Deleting probe code does not authorize deleting retained run evidence or data.
- Put all repository-local disposable caches, temporary run directories, diagnostic logs, and retained failure evidence under the root `.temp/<purpose>/` directory. Examples: `.temp/ruff-cache/`, `.temp/cargo-home/`, `.temp/runtime-cache/`, `.temp/g0-run/`, and `.temp/single-binary-run/`. Configure each tool or test producer to use that location; do not add a new `.gitignore` entry for each cache or run directory. The single `/.temp/` rule covers them all.
- Keep standard build/dependency output in its established location (`target/`, `node_modules/`, and generated package `dist/`); persistent development data in `.data/` is not a disposable cache. Short-lived OS temporary files may use standard temporary-file APIs, but repository-local or retained run artifacts belong under `.temp/`.
- Do not hand-edit generated build output under `target/**`, runtime caches under `.temp/runtime-cache/**`, successful-run artifacts, coverage output, or `*.profraw` files. Relocating caches or evidence must be explicitly in scope, preserve their contents, and refuse overwrite; never delete retained failure evidence as cleanup.

## Commands

- Cargo builds and checks require `OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE` to name an absolute, formally pinned archive for the target platform; real-runtime tests also require the verified `OPEN_COMPUTE_TEST_WORKERD` binary. Prepare these inputs explicitly as documented in `docs/references/single-binary.md`; never download a runtime as an implicit validation step.
- Format: `cargo fmt --all --check`
- Lint: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Test: `cargo test --workspace --all-targets --all-features -- --test-threads=1`
- No-default-features check: `RUSTFLAGS='-D warnings' cargo check --workspace --no-default-features`
- MSRV check: `cargo +1.98.0 check --workspace --all-targets`
- Metadata: `cargo metadata --no-deps --format-version 1`
- Dependency boundaries: `./test/check-boundaries.sh`
- Coverage setup (macOS): `brew install cargo-llvm-cov`
- Coverage setup (portable): `cargo install cargo-llvm-cov --locked`
- Coverage run (from the repository root): `./test/coverage.sh`
- Coverage reports: `target/llvm-cov/html/index.html`, `target/llvm-cov/lcov.info`, and `target/llvm-cov/summary.json`
- Development Gate: exactly one round of the relevant targets per iteration; use the supported single-round commands in `docs/references/testing.md`. Never invoke an internally three-round or recursively chained entry point as a development check.
- Final Gate: three fresh-process rounds only as the last acceptance step after implementation, review, and fixes are complete. The retired G0 capability investigation is not a required recurring Gate; current runner limitations and the planned default-one/explicit-three contract are documented in `docs/references/testing.md` and `docs/runtime-and-test-layout.md`.
- Final P0.1 process Gate: `OPEN_COMPUTE_TEST_WORKERD=/abs/path/to/workerd ./test/test-p0-1.sh`
- Final P0.2 Worker Gate: `OPEN_COMPUTE_TEST_WORKERD=/abs/path/to/workerd ./test/test-p0-2.sh`

## Architecture and Ownership

- Preserve the single-process model: one `platformd` owns config, the data-dir lock, SQLite authority, master-key lifecycle, S3 artifacts/cache, HTTP control/data planes, and one supervised pinned `workerd` child.
- Keep each concern in its owning crate:
  - `core`: dependency foundation for config, errors, IDs, secrets, health, and clocks;
  - `storage`: data directory, locks, SQLite, migrations, identity, and secret crypto;
  - `artifacts`: S3-compatible artifact storage, preflight, and verified cache;
  - `runtime`: workerd pinning, verification, config compilation, process ownership, and supervision;
  - `workers`: immutable bundles/deployments, routing pins, and runtime-source snapshots;
  - `service`: CLI and composition of the production control/data planes.
- Enforce the dependency direction checked by `test/check-boundaries.sh`: `core`, `storage`, `artifacts`, and `runtime` remain lower-level siblings; `workers` may build on `core`, `storage`, and `artifacts` but not `runtime`; `service` is the composition root.
- Apply the Day1 policy to every architectural decision. Prior implementations, persisted development state, and past release layouts must not justify retaining obsolete paths.
- Keep transport handlers thin. Validate and route at HTTP/CLI boundaries; put storage, deployment, artifact, and supervisor workflows in their owning crates.
- Normalize and validate data once at the authority boundary, then pass structured values forward. Do not repair persisted or untrusted values during reads or presentation.
- Make useful architectural assumptions, then verify them against source, tests, logs, real runtime behavior, or upstream documentation before relying on them.
- Add abstractions only when they establish ownership, remove real duplication, enforce a security boundary, or materially reduce complexity. Forbid no-op wrappers and pass-through helpers.
- Remove obsolete parameters, branches, helpers, fields, types, call sites, and files in the same refactor. Do not leave placeholder wiring, dead compatibility shims, or unused future extension points.
- Fix root causes and fail closed. Do not add fallbacks that silently download a runtime, weaken verification, use in-memory authority, skip a Gate, or mask corrupt persisted state.
- Keep code direct and small. Prefer every source and test file to stay below 800 lines and split by ownership before crossing that size. When touching an existing oversized file, avoid growing it and extract the changed concern when that produces a clearer boundary; document the reason when a cohesive protocol/test matrix must remain larger.

## Anti-Cheating

- Treat this section as a highest-priority repository invariant.
- Never put G0/P0 case IDs, fixture account/Worker/deployment names, test URLs, seeded outcomes, expected report counts, fault endpoints, or scenario-specific branches into production Rust, system workers, config, prompts, or operator scripts to satisfy a test.
- Production behavior must derive scope, identity, capabilities, routing, and lifecycle decisions from validated runtime input and persisted authority, using generic protocol and security rules.
- Keep fake S3 services, network fixtures, fault injection, mock clocks, deterministic IDs, and scenario data in tests or explicit `test-support` code. A passing Gate must not weaken or bypass the production path it claims to verify.

## Rust and Dependency Rules

- Preserve Rust 1.98 MSRV, edition 2024, workspace lints, `#![deny(missing_docs)]`, and `unsafe_code = "forbid"`.
- Public APIs require useful English rustdoc. Keep Rust identifiers, code comments, error-code names, and log field names in English.
- Prefer direct, explicit types and ownership over speculative generics. Reuse canonical config, ID, descriptor, persistence, and error types; do not redeclare local variants or add mapping layers without a real boundary.
- Avoid `unwrap`/`expect` outside tests and compile-time invariants. Propagate errors with `?`; transform them only at a semantic, cleanup, process, or external-response boundary, and return stable `PlatformError` values from fallible production paths.
- Use ordinary static module imports. Dynamic module assembly is restricted to the owned tenant-bundle/`workerLoader` boundary and must remain data-driven rather than selecting hardcoded implementations.
- Use `#[cfg(any(test, feature = "test-support"))]` for test-only hooks. Never make fault injection, fake services, or test credentials reachable in a production build.
- The root `Cargo.toml` owns shared dependency versions and workspace policy. Each crate must declare every dependency it imports, normally through `workspace = true`.
- Keep default features empty unless a production capability requires otherwise, and preserve `--no-default-features` builds.
- Commit intentional `Cargo.lock` changes, but never hand-edit the lockfile.

## TypeScript and JavaScript Sources

- Organize runtime TypeScript sources and behavioral tests by domain: gateway, loader, KV, D1, R2, Queues, Durable Objects, and Workflows. Keep each domain's protocol types beside its implementation; reserve shared binding types for capabilities actually used across domains. Preserve the source directory structure in generated assets instead of flattening filenames.
- All maintained functional JavaScript-family code must be TypeScript (`.ts` or `.tsx`) and pass the pinned TypeScript 7 compiler in strict mode. This includes system Workers, developer tools, build scripts, and functional examples. JavaScript is allowed only in tests, fixtures, mocks, disposable G0 evidence, and reproducibly generated runtime/build artifacts; never hand-edit generated JavaScript or hide functional source in an excluded path.
- Manage JavaScript-family packages in the root Bun workspace with one committed `bun.lock`. Use the root dependency catalog for shared versions and `workspace:*` for local packages. Use Bun as the package manager; do not introduce npm, pnpm, or Yarn lockfiles.
- Rolldown owns TypeScript/JSX transformation and project bundling; TypeScript 7 owns type checking. Both must succeed before a new compiled Worker is admitted for deployment. Compilation and dependency installation belong to developer/build tooling, never the production request path or daemon startup.
- Do not use `any`, `@ts-nocheck`, `@ts-ignore`, unchecked double assertions, ambient catch-all modules, or relaxed compiler flags to make migration/type checks pass. Model private protocols and product APIs explicitly; use `unknown` plus validation at untrusted boundaries. Type declarations must not advertise unsupported platform capabilities or expose internal credentials.
- The runtime package migration must rename `system-workers/` to `packages/runtime/dist/` and remove generated assets from Git tracking. Commit sources, build configuration, and the formal pin, never generated `dist/` files. CI must build from a checkout without `dist/`, verify source/manifest consistency and reproducibility, and prepare assets before Rust consumes them; do not rely on a committed generated baseline or an old-path fallback. Production startup remains offline and consumes built runtime assets without Bun, Node.js, or a TypeScript compiler.

## Runtime and Supply Chain

- Production startup is offline. It must never fetch, auto-upgrade, or search `PATH` for `workerd`.
- The formal multi-platform release pin is currently `runtime/workerd.lock.json`; move it to `packages/runtime/workerd.lock.json` with the runtime package and update every consumer in the same change. Keep one authoritative pin, not a separate POC copy. Verify target, archive checksum, binary checksum, version output, compatibility date/flags, and process flags before spawn or packaging.
- Treat a workerd pin change as a coordinated dependency update: update the formal lock and runtime assets, validate affected upstream behavior and maintained product Gates against stock workerd, record fresh evidence/API compatibility findings, and verify packaged layouts on supported hosts. Historical G0 results do not prove a new pin, and targeted investigation does not require retaining or restoring the retired POC suite.
- Keep upstream `workerd` unmodified. Compile the checked-in Cap'n Proto configuration with the verified binary; never interpolate tenant input into Cap'n Proto source.
- Runtime/internal listeners stay loopback-only and capability-scoped. Per-generation internal tokens must never appear in argv, environment variables, logs, status, metrics, errors, or tenant-visible responses, and old-generation tokens must become invalid after restart.
- `platformd` owns the complete child lifecycle: readiness, process group, bounded stdout/stderr capture, graceful stop, forced stop, reaping, restart backoff, and secret-free orphan recovery. Never signal a PID without validating its start identity and binary digest.
- Preserve readiness as both successful control-fd listen evidence and an HTTP probe. `/health/live` is process liveness; `/health/ready` is admission state and must not become a restart signal.

## Persistence and Filesystem

- `control.sqlite` is the authority for platform, Worker, deployment, route, and secret metadata. Runtime memory and workerd loader caches are disposable acceleration only.
- Platform-owned SQL schema files define the current Day1 schema, not a production upgrade history. Revise or consolidate existing schema/migration definitions directly when the design changes; do not require append-only migrations or backfills to preserve old development databases. User-facing D1 database migrations are a separate product capability, not a reason to retain historical platform schema upgrades.
- Keep the current SQL sequence contiguous, checksummed, and transactional. Update SQL, build-time checksum wiring, schema versions/dispatch, invariants, fixtures, and fault/restart coverage together. Reject unsupported or corrupt persisted state; never add runtime schema self-healing or downgrade paths. Resetting existing local databases still requires user authorization.
- Keep SQLite foreign keys enabled and transaction callbacks synchronous. Perform filesystem, S3, process, and other async I/O outside database transactions.
- Preserve one `platformd` owner per data directory and existing atomic-write, fsync, permission, symlink, and path-containment guarantees. Do not replace security-sensitive filesystem helpers with unchecked convenience APIs.
- Artifacts and ready deployments are immutable and content-addressed. Verify digests before cache admission or execution; promotion/rollback changes an active pointer rather than mutating deployment content.
- Store secrets only as validated env/file references or encrypted values with their existing AEAD context. Never persist or expose plaintext secrets through GET APIs, artifacts, caches, diagnostics, logs, metrics, argv, or errors.

## Worker Security Boundaries

- `platformd` is the only public listener. It generates trusted request/deployment identity and overwrites or strips conflicting external internal headers.
- Tenant loader keys are immutable deployment identities. Resolve modules, vars, secrets, compatibility metadata, and future bindings from the persisted authority, not request-supplied scope or host-memory registries.
- Tenant `env` exposes only explicitly declared vars, secrets, and supported product bindings. Never leak RuntimeSource, S3, SQLite, internal fetchers/tokens, control APIs, or platform services into an isolate.
- Tenant outbound remains HTTP(S)-only and `Network(allow = ["public"])`-backed. Reject private, loopback, link-local, metadata, Unix, DNS-to-private, and redirect-to-private targets; do not replace address-layer enforcement with hostname string checks.
- Return stable, sanitized tenant/control-plane errors. Raw upstream exceptions, loader keys, paths, module source, authorization/cookies, signed URLs, secrets/ciphertext, and internal topology belong in neither responses nor platform logs.
- Do not treat client disconnect as a guaranteed cancellation primitive. The historical G0 `D-abort` observation remains evidence of that limitation, not permission to accept unrelated failures. Preserve current product cancellation, stream-interruption, and recovery coverage without requiring the retired probe or its allowlist runner to remain.

## Testing

- While implementation, review, or fixes are still in progress, execute each relevant Gate target exactly once per iteration. Do not run three rounds, recursive historical aggregates, or full coverage during development. Use a supported single-round option or a single-round test target directly; never assume an environment variable changes a runner that does not read it. If a complete single-round entry does not exist, report that gap and use focused tests without claiming a full Gate pass. See `docs/references/testing.md`.
- Run the required three fresh-process rounds only as the last acceptance step after implementation, review, fixes, and the other required checks are complete and the source is frozen. Keep each round isolated; preserve persistent state only within a scenario that tests restart/crash recovery. If a failure requires code changes, return to single-round development validation, then rerun the affected final checks after the fix. A one-round pass is never final aggregate evidence.
- Refactored Gate entry points must default to one round and require explicit `OPEN_COMPUTE_GATE_ROUNDS=3` for final acceptance; accept only 1 or 3 and reject other values before execution. One top-level runner owns repetition and target selection. Leaf tests run one round, never recursively invoke other Gates, and never hide their own three-round loop. This is the required runner contract, not a claim that existing scripts already implement it.
- Remove duplicate cases and duplicate scheduling using explicit invariant/coverage ownership. Each selected target runs once per round; do not rerun unchanged inputs through differently named aggregates. Stop after a failed round, preserve diagnostics, and do not retry until green. One-time completed capability probes are not mandatory product regression tests.
- Reduce avoidable test cost: reuse correctly keyed build/dependency caches and verified immutable inputs, build/typecheck once per required configuration, replace arbitrary sleeps with bounded condition waits, and parallelize only after proving isolation. Never trade away fresh-process coverage, security/integrity assertions, failure diagnostics, real-runtime paths, or timeout correctness for speed. Long soak/load/fuzz runs and release packaging are not routine development prerequisites.
- Put focused unit tests beside their owning module and cross-crate/process behavior in crate integration tests. Test public behavior and invariants, not private implementation details.
- Test the current Day1 model, clean initialization, and restart/crash recovery. Update or remove obsolete compatibility, upgrade, and historical byte-identity assertions with the model change while preserving security, integrity, and failure-path coverage. Retain regression coverage for the documented official Cloudflare API compatibility exception. Historical reports record past runs; they do not require retaining old implementations.
- Add focused success and failure-path tests with behavior changes. Dedicated `tests/**`, `src/tests.rs`, nested `src/**/*_tests.rs`, `src/mock_s3.rs`, and the supervisor fixture are excluded from line coverage because they contain test or mock code; never place production logic in an excluded file, exclude any production source, add coverage-only branches, or weaken an assertion merely to make a metric or Gate pass.
- Security, persistence, protocol, and process-lifecycle changes require regression coverage for success and failure paths, including restart/crash behavior when relevant.
- Maintained runtime/product Gates must exercise the verified stock `workerd`, real processes, real SQLite, and the documented SigV4/network fixtures where required by the behavior under test. Do not replace Gate evidence with mocks, Miniflare, an in-memory substitute, or a skipped test.
- A missing or checksum-mismatched workerd binary is a test failure, not a reason to skip. Use `OPEN_COMPUTE_TEST_WORKERD` to select an already available verified binary.
- Preserve required product invariants when removing POC code: migrate uncovered current behavior to maintained tests and delete equivalent duplicates. Record whether a removal is a completed investigation, equivalent coverage elsewhere, or an obsolete model. Do not preserve a historical report's case count as a product requirement or broaden accepted failure shapes.
- Preserve failure diagnostics under the ignored run directories' `failed/` subtrees, sanitize generated reports, and check that tests leave no workerd process, listener, temp file, or secret behind.
- `test/test-p0-2-egress-linux.sh` is Linux-only and mutates loopback addresses plus `/etc/hosts` through `sudo`; run it only with explicit user authorization and `OPEN_COMPUTE_EGRESS_FIXTURE_ALLOW_SUDO=1`.

## Release and Operations

- `scripts/package-release.sh` may download only the formally pinned upstream archive during packaging. It requires an explicit absolute destination, refuses checksum/version mismatch and overwrite, and must never be treated as a normal local validation command.
- When requesting the Operating Contract's confirmation for packaging, publishing, or deployment, state the source revision, target platform, exact workerd pin, destination, network/privilege effects, and excluded unrelated changes.
- The only production release artifact is one native `platformd` executable embedding the verified workerd archive, matching runtime lock/config/system Workers, licenses, default config, and operator docs. Do not retain sidecar layouts, external runtime overrides, or startup downloads. Verify version, size, SHA-256, and isolated offline startup of that single executable; its persistent runtime cache belongs to its exclusively owned data directory, not the distribution.
- Keep container, systemd, and launchd examples on the same binary/config/data-dir contract. Never embed credentials in images, service units, examples, or release archives.

## Verification and Git

- After implementation and review/fix work are complete, remove dead code and run final acceptance: format, clippy, the workspace test suite, no-default-features check, metadata, dependency-boundary check, and `./test/coverage.sh`, then the relevant final three-round product Gates for runtime, process, persistence, Worker, routing, egress, packaging, or security changes. Workspace Rust line coverage must remain at or above 90.00%; never lower the threshold. Run static checks, ordinary workspace tests, and coverage once for their required configurations, not once per Gate round. Coverage's instrumented execution does not replace final fresh-process acceptance. Do not repeat this full acceptance loop for intermediate edits or require the retired G0 investigation.
- Documentation-only and policy-only edits do not require Rust checks; run `git diff --check` and verify commands, paths, and generated-file claims against the repository.
- If a required check cannot run, report the exact reason and the next best evidence. Never report a command as passing before it exits successfully.
- Keep diffs focused and preserve unrelated user changes. Do not rewrite Git history, delete retained failure evidence, or clean the workspace unless explicitly requested.
- Before handoff, verify that ownership is direct, assumptions are proven, no unnecessary abstraction/fallback/compatibility path remains, and the same behavior cannot be expressed more simply.

## Documentation and Communication

- Keep implementation docs evidence-backed and distinguish planned behavior, implemented behavior, verified behavior, and accepted limitations.
- Keep active implementation and acceptance plans directly under `docs/`; put continuously maintained references, API/deviation matrices, testing policy, deployment guides, and runbooks under `docs/references/`. Put completed stage designs, stage-specific matrices, and completed investigation/verification records under `docs/implemented/`, and maintain both directory indexes.
- Before marking an implementation goal complete or handing it off as finished, move its completed design/plan documents and associated completion reports into `docs/implemented/`. Complete the agreed implementation and verification first; writing a plan, partial implementation, a submitted command, or a blocked/unverified result does not qualify. If a document spans unfinished goals, keep it active or separate the unfinished scope before archiving the completed part.
- Separate outstanding release qualification or long-running verification from completed core implementation into an explicit active acceptance plan, then archive the completed implementation with its actual evidence and limitations. A concluded investigation may be archived with a No-Go or Conditional Go verdict; that does not mean its unsupported capability has been implemented or its unrun checks passed.
- Maintain `docs/implemented/README.md` with the completed scope, supporting evidence, and accepted limitations. Correct stale plan-status labels when evidence proves completion; preserve historical commands, dates, results, hashes, and failure evidence. Moving a generated report must not rewrite its contents or imply a new test run.
- In the same archive or reference reorganization, update inbound links, the moved documents' relative links, and any report producer or code/test consumer of their paths; verify those references and refuse destination overwrite. Do not leave duplicate documents or old-path compatibility stubs. An archived design does not override current code, maintained references, or Day1 policy.
- Preserve the language of the surrounding document: Rust API docs and code comments are English; the existing architecture/design documentation may remain Chinese.
- Lead with the result. Include the evidence needed to support it, any material caveat, and the next action; omit repetition, generic reassurance, and optional background first.
- Conversational responses are Chinese.
