# AGENTS – open-compute

open-compute runs Cloudflare Workers on one machine for small teams. `ocd` (Rust) supervises a pinned workerd fork, keeps metadata in SQLite and object bytes on local disk or S3. Nested `AGENTS.md` files add local rules.

## Working here

- Code, commits, PRs, issues: use English.
- A change request authorizes local edits and non-destructive checks. Ask first for external writes, destructive actions, downloads, runtime pin changes and releases.
- Work and commit on the current branch. Create a branch only when asked.
- Do not rewrite Git history or force-push. Keep unrelated changes.
- Scratch: `.temp/<purpose>/`. Research credentials: root `.env`, `cf` login.

## Build and test

```sh
bun install && bun run build  # inputs: docs/references/single-binary.md
./test/gate.py runtime contract kv --keep-going  # --list shows domain targets and cases
cargo fmt --all --check && ./test/check-rust-clippy.sh
```

Run the tests for what you changed. [Testing](docs/references/testing.md) lists targets and full checks. Run the full workspace Gate only if the user asks or for a release.

## Five criteria for every change

1. **Match current Cloudflare only.** Current docs and what cf and the official SDKs send define the surface. Skip deprecated APIs, legacy compatibility behaviors and old-client paths (e.g. wrangler) when they add complexity.
2. **Upstream first.** Use stock workerd configuration before a fork patch. Each patch names its removal condition in [workerd](docs/workerd/README.md).
3. **Libraries first.** A mature crate that replaces more code than it adds is not a speculative dependency.
4. **Defend by the [threat model](docs/security.md).** A new defense names the adversary it stops.
5. **Tiered tests.** PRs run deterministic tests for the changed products. Full qualification runs only before a release.

## Invariants

1. Secrets never reach GET APIs, logs, errors, argv or artifacts.
2. A Worker sees only its declared bindings. It cannot reach another Worker's data or platform internals.
3. Validate external input once at the boundary, then trust the types.
4. Deployments are immutable and content-addressed. Promotion and rollback move a pointer.
5. Production startup is offline: no downloads, no `PATH` lookup, no JS toolchain.
6. Released platform SQL migrations never change; append the next one. Pre-1.0 breaks fail explicitly, with no shims, aliases or dual paths.
7. Every retry and wait ends. Cached failures expire. A restart recovers without manual database edits.
8. Runtime pins live only in `packages/runtime/{workerd,caddy}.lock.json`. Never commit native binaries.
9. Production code holds no test case IDs, fixture data or test-only branches.

## Style

Delete what you can. Rust limits: 1500-line files, 600-line functions (tests 2000/800). Add no code without a caller, no single-implementation trait, no unread state. A new trait, state or table needs one sentence on why simpler fails. Write docs by the [documentation rules](docs/references/README.md). Report only checks that ran and passed.
