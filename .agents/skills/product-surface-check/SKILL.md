---
name: product-surface-check
description: Manually review changed open-compute behavior for drift across maintained user, operator, API, SDK, configuration, deployment, capability, CLI, Dashboard, and release surfaces. Use only when explicitly invoked for this cross-surface check.
---

# Product Surface Check

Perform a read-only consistency review. Report required updates; do not edit files unless the user separately asks for fixes.

## Resolve the review scope

Use a user-specified revision or range when provided. Otherwise:

1. Inspect staged, unstaged, and untracked files. If any exist, review only those uncommitted changes against `HEAD`.
2. If the worktree is clean, compare `HEAD` with the newest reachable annotated open-compute release tag matching exact SemVer `vX.Y.Z`.
3. Do not fetch merely to find a base. Do not use `git describe` without filtering: the repository also contains workerd-style `v1.YYYYMMDD.N` tags that are not open-compute releases.
4. If no reachable annotated SemVer release exists, report the missing base and review the available change scope without inventing one.

Inventory untracked files with `git ls-files --others --exclude-standard`; ordinary `git diff` does not include them. For a release comparison, use the exact tag-to-`HEAD` range rather than a merge-base range because the tag must be an ancestor of `HEAD`.

## Identify externally meaningful changes

Trace changed behavior before judging presentation files. Look for changes to product topology, ownership, supported capabilities, configuration, defaults, commands, flags, output, installation, lifecycle, security boundaries, limits, recovery, and operator or developer workflows. Ignore internal refactors that preserve all of those observable contracts.

Inspect changed producers and their consumers. A presentation file appearing in the diff is not proof that it is current; compare its content with the implemented behavior.

## Check the maintained product surfaces

### README and product architecture diagram

Check `README.md`, `README.zh.md`, and `share/open-compute-architecture.svg` / `.png` when the change affects product positioning, setup commands, supported products, headline counts, deployment topology, component ownership, trust boundaries, persistent authorities, or important data flows.

Require a diagram update only for architectural information a diagram should communicate. Do not request diagram churn for local implementation details. Treat the SVG as inspectable source and the PNG as the README-rendered artifact; if one needs changing, verify that both remain synchronized.

### Product website and docs site

Check the marketing copy in `apps/website/src/i18n/` and the maintained documentation in both:

- `apps/website/src/content/docs/docs/`
- `apps/website/src/content/docs/zh/docs/`

Inspect root `docs/` only when the changed contract is represented there. Require English and Chinese documentation to describe the same behavior. Flag stale commands, configuration, defaults, supported-surface claims, operational procedures, links, and architecture explanations.

Also enforce the documentation lifecycle defined by `docs/references/README.md` whenever root documentation changes or the reviewed implementation completes an active plan:

- `docs/*.md` and `docs/workerd/*.md` contain only work that still requires implementation;
- completed implementation summaries live in `docs/implemented/`;
- completed implementations with only external, cross-platform, long-running, or release qualification remaining keep the implementation summary in `docs/implemented/` and the remaining work in `docs/acceptance/`;
- genuinely externally blocked work lives in `docs/blocked/`;
- maintained current contracts and runbooks live in `docs/references/`.

Check that document moves update `docs/README.md`, the destination index, generators, and repository links without leaving redirects, stubs, duplicate copies, or completed plans in the active root. Flag broken relative links, stale worktree/branch language, obsolete TODOs, unnumbered lifecycle documents, and PASS/verified claims that are not backed by a recorded successful check. Do not ask to archive a maintained current contract merely because its original implementation is complete.

### Default configuration and deployment artifacts

Check `share/default-config.toml`, `scripts/install.sh`, and the systemd, launchd, and container examples under `examples/` when configuration shape, defaults, paths, permissions, installation, service scope, process ownership, startup, shutdown, or upgrade behavior changed. These are shipped operator inputs, not illustrative snippets that may drift independently.

Check developer examples and Wrangler configuration only when their supported workflow or generated types changed. Do not request unrelated example churn.

### Public API, SDK, and generated developer contracts

Check `openapi/**`, `packages/sdk/**`, and affected generated types or developer examples when HTTP routes, request or response fields, errors, pagination, authentication, supported operations, SDK methods, or public identifiers changed. Verify the authoritative OpenAPI/capability inputs and generated SDK surface agree; do not treat documentation prose or a passing route test as a substitute for the machine-readable contract.

Keep this separate from `ocd capabilities`: the API/SDK contract describes callable management surfaces, while capabilities describe advertised support and runtime/product qualification.

### Dashboard

Check `apps/dashboard/**` when authentication/session behavior, instance selection, terminology, navigation, resource operations, capability presentation, API fields, errors, or destructive confirmations changed. The Dashboard must use the same current public contract and names as the API and docs; Cloudflare wire names may remain at the SDK boundary without becoming stale product terminology in the UI.

### `ocd capabilities`

Check whether the live capability contract needs changing when support status, public members, deviations, limits, pins, compatibility dates or flags, management support, Wrangler support, or observability support changed. Trace the authority through:

- `share/cloudflare-capabilities.json`;
- `crates/service/src/capabilities.rs`;
- the capability/deviation catalog and conformance inputs under `test/conformance/`;
- `docs/references/cloudflare-compatibility.md` and `docs/references/p1-deviations.md`.

Distinguish tenant runtime compatibility from management API support. Do not request capability changes for an internal refactor or for behavior outside the declared support scope.

### `ocd` CLI help

Check Clap declarations in `crates/service/src/cli/model.rs` and related subcommand models against the actual dispatch and validation paths. Flag stale or missing descriptions for commands, flags, positional arguments, defaults, conflicts, scope selection, output modes, side effects, destructive behavior, and hidden/internal commands accidentally exposed.

Compare CLI help with README and website command examples. Source review is sufficient by default; run help from an already built current binary only when it resolves uncertainty. Do not trigger a full build or Gate for this review alone.

### Embedded operator docs and agent entry points

Check `docs/references/runbooks/**` when recovery, backup, installation, failure handling, support collection, paths, permissions, or operator commands changed. These files are embedded in the executable and exposed through `ocd docs`, so they are a shipped product surface.

Check `apps/website/public/llms.txt` when setup, command selection, supported workflows, or the canonical documentation entry points changed. Keep it small and link to detailed docs rather than duplicating them.

### Release-only surfaces

Apply this section only when the user asks for release preparation, the reviewed changes include version/release metadata, or a release note for the next version already exists. Check:

- `docs/releases/` and its index;
- workspace and SDK package versions;
- formal runtime/tool pins and their documented identities;
- installer/download asset names and upgrade instructions.

Do not require release-note or version churn for an ordinary implementation review. When applicable, require release notes to describe user-visible changes, breaking configuration/data implications, operator actions, and actual verification without overstating unrun qualification.

## Report

List actionable mismatches first, ordered by user impact. Each finding must include:

- the implemented change and evidence path;
- the stale or missing surface and evidence path;
- the smallest accurate update required.

Finish with exactly one row for each applicable surface using `update required`, `no update needed`, `not applicable`, or `unverified`:

| Surface                               | Verdict | Evidence |
| ------------------------------------- | ------- | -------- |
| README + architecture diagram         |         |          |
| Product website + docs site           |         |          |
| Default config + deployment artifacts |         |          |
| Public API + SDK contracts            |         |          |
| Dashboard                             |         |          |
| `ocd capabilities`                    |         |          |
| `ocd` CLI help                        |         |          |
| Embedded runbooks + `llms.txt`        |         |          |
| Release-only surfaces                 |         |          |

If no mismatch exists, say so directly. Do not turn a clean scoped review into a claim that all repository documentation or product behavior is correct.
