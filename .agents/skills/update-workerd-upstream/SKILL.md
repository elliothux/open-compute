---
name: update-workerd-upstream
description: Update open-compute's workerd fork onto current Cloudflare upstream, minimize fork-owned code, regroup fork commits by capability, and coordinate the submodule, pin, tests, and docs. Invoke only when the user explicitly requests `$update-workerd-upstream`.
---

# Update workerd upstream

Update `third_party/workerd` without preserving obsolete fork implementations. Keep upstream behavior wherever it now satisfies open-compute, and keep every remaining fork seam narrow and reviewable.

## Establish the exact state

Read the root and applicable workerd `AGENTS.md` files, `docs/workerd/README.md`, the active workerd design, and `packages/runtime/workerd.lock.json`. Confirm the parent and submodule worktrees are safe before rewriting history.

Fetch Cloudflare `main` into a dedicated local upstream ref and refresh the fork remote. Record the old fork head, upstream head, merge base, divergence, fork-only commits, changed paths, and parent gitlink. Do not treat commit subjects or `git cherry` alone as proof that upstream covers a fork feature: trace the upstream source and tests for the same behavior.

## Minimize and regroup the fork

Rebase the current Day1 implementation onto the fetched upstream head. For each fork capability:

1. Drop code fully covered by upstream and use the upstream API directly.
2. Adapt the smallest remaining open-compute seam to current upstream ownership and conventions.
3. Keep fork-specific files separate where doing so avoids recurring merge conflicts; touch upstream files only for the narrow registration or call site that must connect them.
4. Remove superseded helpers, flags, tests, and compatibility branches in the same rewrite.
5. Preserve security, capability, lifecycle, and resource-limit behavior; never resolve conflicts by weakening validation or silently ignoring configuration.

Produce one compileable commit per coherent open-compute capability. Fold tests into the capability they prove. Avoid merge, fixup, formatting-only, and historical compatibility commits. Rewrite the fork branch only when the user explicitly authorized it, and publish with `--force-with-lease`, never unguarded `--force`.

## Validate and coordinate

Run focused formatting/build/tests while resolving each capability. Do not download runtimes implicitly. Before final acceptance, update the parent gitlink and the workerd source identity in docs. Change the formal multi-platform binary pin only as a coordinated pin update with verified archives, digests, version output, runtime config, and required product evidence.

Open Compute's formal workerd builds and FD-passing tests use `--//:io_backend=cxx`. Keep
`--@rules_rust//:extra_exec_rustc_flag=-Cstrip=none` on macOS release builds: stripping the exec-configuration Rust
proc-macro dylibs can produce a malformed Mach-O before C++ compilation begins. Treat these as recorded build inputs,
not as generic upstream defaults, and do not use them to mask an unrelated compiler or linker failure.

Before the product Gate, explicitly build `//src/workerd/server:host-extension-test-provider` with the same C++ I/O
backend and set `OPEN_COMPUTE_TEST_HOST_EXTENSION_PROVIDER` to its absolute executable path. The fixture is test-only
and must not be added to the release pin or production artifact.

Run the repository's required static checks, coverage, and final Gate once after source freeze. Use `$cf-compatibility-check` after implementation and fix its actionable findings before the final Gate. Report any platform build or formal-pin evidence that remains unavailable rather than substituting a development binary or a stock runtime.
