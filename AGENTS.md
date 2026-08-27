# AGENTS – open-compute

## Operating Contract

- Do not send optional commentary.
- For answer, explanation, review, diagnosis, or planning requests, inspect the relevant evidence and report the result; do not implement changes unless requested.
- For change, build, or fix requests, make the requested in-scope local changes and run relevant non-destructive validation without asking first.
- Reading files, inspecting logs and Git state, editing requested local files, and running non-destructive checks are authorized local actions.
- Ask before external writes, destructive actions, privileged commands, runtime downloads, release packaging, or a material scope expansion.

## Repository Scope

- This file applies to the entire repository. Do not add nested `AGENTS.md` files.
- Treat this repository as the source of truth for `open-compute`; do not edit the parent Lynx OS project unless the user explicitly includes it in scope.
- `crates/**`, `runtime/**`, and `share/**` are production sources and assets; `scripts/**` and `examples/**` are operator/release surfaces. `poc/**` is disposable G0 black-box evidence, not a production implementation or reusable product layer.
- `docs/g0-results.md` is generated atomically by `./poc/g0 test all`; do not hand-edit it.
- Do not edit generated build output under `target/**`, runtime caches under `poc/.runtime-cache/**`, successful-run artifacts, coverage output, or `*.profraw` files.

## Commands

- Format: `cargo fmt --all --check`
- Lint: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Test: `cargo test --workspace --all-targets --all-features -- --test-threads=1`
- No-default-features check: `RUSTFLAGS='-D warnings' cargo check --workspace --no-default-features`
- MSRV check: `cargo +1.98.0 check --workspace --all-targets`
- Metadata: `cargo metadata --no-deps --format-version 1`
- Dependency boundaries: `./scripts/check-boundaries.sh`
- Coverage setup (macOS): `brew install cargo-llvm-cov`
- Coverage setup (portable): `cargo install cargo-llvm-cov --locked`
- Coverage run (from the repository root): `./scripts/coverage.sh`
- Coverage reports: `target/llvm-cov/html/index.html`, `target/llvm-cov/lcov.info`, and `target/llvm-cov/summary.json`
- Development Gate: one relevant round per iteration; commands and legacy runner limits are documented in `docs/testing.md`.
- Final G0 stock-workerd regression: `./poc/g0 test all`
- Final P0.1 process Gate: `OPEN_COMPUTE_TEST_WORKERD=/abs/path/to/workerd ./scripts/test-p0-1.sh`
- Final P0.2 Worker Gate: `OPEN_COMPUTE_TEST_WORKERD=/abs/path/to/workerd ./scripts/test-p0-2.sh`

## Architecture and Ownership

- Preserve the single-process model: one `platformd` owns config, the data-dir lock, SQLite authority, master-key lifecycle, S3 artifacts/cache, HTTP control/data planes, and one supervised pinned `workerd` child.
- Keep each concern in its owning crate:
  - `core`: dependency foundation for config, errors, IDs, secrets, health, and clocks;
  - `storage`: data directory, locks, SQLite, migrations, identity, and secret crypto;
  - `artifacts`: S3-compatible artifact storage, preflight, and verified cache;
  - `runtime`: workerd pinning, verification, config compilation, process ownership, and supervision;
  - `workers`: immutable bundles/deployments, routing pins, and runtime-source snapshots;
  - `service`: CLI and composition of the production control/data planes.
- Enforce the dependency direction checked by `scripts/check-boundaries.sh`: `core`, `storage`, `artifacts`, and `runtime` remain lower-level siblings; `workers` may build on `core`, `storage`, and `artifacts` but not `runtime`; `service` is the composition root.
- Default to the direct architecture this repository should have now. Preserve compatibility or legacy paths only when required by an API/protocol contract, persisted data, the current workerd pin, or a release boundary.
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

## Runtime and Supply Chain

- Production startup is offline. It must never fetch, auto-upgrade, or search `PATH` for `workerd`.
- `runtime/workerd.lock.json` is the formal multi-platform release pin. Verify target, archive checksum, binary checksum, version output, compatibility date/flags, and process flags before spawn or packaging.
- Treat a workerd pin change as a coordinated migration: update the formal lock and runtime assets, rerun stock-workerd G0 and P0 Gates, refresh generated evidence/API compatibility docs, and verify packaged layouts on supported hosts.
- Keep upstream `workerd` unmodified. Compile the checked-in Cap'n Proto configuration with the verified binary; never interpolate tenant input into Cap'n Proto source.
- Runtime/internal listeners stay loopback-only and capability-scoped. Per-generation internal tokens must never appear in argv, environment variables, logs, status, metrics, errors, or tenant-visible responses, and old-generation tokens must become invalid after restart.
- `platformd` owns the complete child lifecycle: readiness, process group, bounded stdout/stderr capture, graceful stop, forced stop, reaping, restart backoff, and secret-free orphan recovery. Never signal a PID without validating its start identity and binary digest.
- Preserve readiness as both successful control-fd listen evidence and an HTTP probe. `/health/live` is process liveness; `/health/ready` is admission state and must not become a restart signal.

## Persistence and Filesystem

- `control.sqlite` is the authority for platform, Worker, deployment, route, and secret metadata. Runtime memory and workerd loader caches are disposable acceleration only.
- Migrations are forward-only, contiguous, checksummed, and transactional. Never modify an applied migration or add runtime schema self-healing/down-migrations; add the next numbered SQL migration, build-time checksum wiring, migration dispatch, invariants, and fault/restart coverage together.
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
- Do not treat client disconnect as a guaranteed cancellation primitive. For the current pin, the only accepted G0 limitation is the exact `D-abort` observation documented by the generated report; never broaden that allowlist or accept a different failure shape.

## Testing

- While implementation, review, or fixes are still in progress, run one relevant Gate round per iteration. Do not repeatedly run three-round or recursively chained historical aggregates during development. Use a supported single-round option or the relevant test target directly; never assume an environment variable changes a runner that does not read it. See `docs/testing.md`.
- Run the required three-round final Gates only after implementation and review/fix work are complete and the source is frozen for acceptance. If a failure requires code changes, return to focused single-round validation, then rerun the affected final checks once the fix is ready. A one-round development pass is not final aggregate evidence.
- Put focused unit tests beside their owning module and cross-crate/process behavior in crate integration tests. Test public behavior and invariants, not private implementation details.
- Add focused success and failure-path tests with behavior changes. Dedicated `tests/**`, `src/tests.rs`, nested `src/**/*_tests.rs`, `src/mock_s3.rs`, and the supervisor fixture are excluded from line coverage because they contain test or mock code; never place production logic in an excluded file, exclude any production source, add coverage-only branches, or weaken an assertion merely to make a metric or Gate pass.
- Security, persistence, protocol, and process-lifecycle changes require regression coverage for success and failure paths, including restart/crash behavior when relevant.
- G0 and P0 Gates must exercise the verified stock `workerd`, real processes, real SQLite, and the documented SigV4/network fixtures. Do not replace Gate evidence with mocks, Miniflare, an in-memory substitute, or a skipped test.
- A missing or checksum-mismatched workerd binary is a test failure, not a reason to skip. Use `OPEN_COMPUTE_TEST_WORKERD` to select an already available verified binary.
- `./poc/g0 test all` runs three fresh-process rounds and regenerates `docs/g0-results.md`. Standalone loader currently exits nonzero for the documented `D-abort`; aggregate acceptance must remain fail-closed and match the exact allowlisted observation.
- Preserve failure diagnostics under the ignored run directories' `failed/` subtrees, sanitize generated reports, and check that tests leave no workerd process, listener, temp file, or secret behind.
- `scripts/test-p0-2-egress-linux.sh` is Linux-only and mutates loopback addresses plus `/etc/hosts` through `sudo`; run it only with explicit user authorization and `OPEN_COMPUTE_EGRESS_FIXTURE_ALLOW_SUDO=1`.

## Release and Operations

- `scripts/package-release.sh` may download only the formally pinned upstream archive during packaging. It requires an explicit absolute destination, refuses checksum/version mismatch and overwrite, and must never be treated as a normal local validation command.
- When requesting the Operating Contract's confirmation for packaging, publishing, or deployment, state the source revision, target platform, exact workerd pin, destination, network/privilege effects, and excluded unrelated changes.
- Release layouts contain the freshly built `platformd`, verified `workerd`, matching runtime lock/config/system workers, license, and default config. Verify version, size, SHA-256, and offline startup from the packaged layout.
- Keep container, systemd, and launchd examples on the same binary/config/data-dir contract. Never embed credentials in images, service units, examples, or release archives.

## Verification and Git

- After implementation and review/fix work are complete, remove dead code and run final acceptance: format, clippy, the workspace test suite, no-default-features check, metadata, dependency-boundary check, and `./scripts/coverage.sh`. Workspace Rust line coverage must remain at or above 90.00%; never lower the threshold. Coverage includes real-runtime P0 tests but does not replace the relevant final three-round P0/G0 Gate for runtime, process, persistence, Worker, routing, egress, packaging, or security changes. Do not repeat this full acceptance loop for each intermediate edit.
- Documentation-only and policy-only edits do not require Rust checks; run `git diff --check` and verify commands, paths, and generated-file claims against the repository.
- If a required check cannot run, report the exact reason and the next best evidence. Never report a command as passing before it exits successfully.
- Keep diffs focused and preserve unrelated user changes. Do not rewrite Git history, delete retained failure evidence, or clean the workspace unless explicitly requested.
- Before handoff, verify that ownership is direct, assumptions are proven, no unnecessary abstraction/fallback/compatibility path remains, and the same behavior cannot be expressed more simply.

## Documentation and Communication

- Keep implementation docs evidence-backed and distinguish planned behavior, implemented behavior, verified behavior, and accepted limitations.
- Preserve the language of the surrounding document: Rust API docs and code comments are English; the existing architecture/design documentation may remain Chinese.
- Lead with the result. Include the evidence needed to support it, any material caveat, and the next action; omit repetition, generic reassurance, and optional background first.
- Conversational responses are Chinese.
