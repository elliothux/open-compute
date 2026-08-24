---
name: simplify
description: Simplify recently modified Lynx business code while preserving behavior. Use to remove unnecessary wrappers, hooks, DI, transforms, duplicate types, and stale wiring; prefer direct imports, plain functions, existing APIs, root-cause fixes, and one source of truth. Do not trigger for test-only refactors.
---

# Simplify

Simplify recently modified business code. Keep tests, fixtures, snapshots, and test harnesses out of scope unless the user explicitly includes them.

## Target Architecture

Treat Lynx as a monolith with real frontend/server and external-adapter boundaries. Within one process, prefer:

1. Direct import
2. Context or state only for genuinely runtime-, user-, UI-, or provider-scoped values
3. Dependency injection only for a real boundary or implementation that must vary outside the module

The result should be direct, local, explicit, and owned in one place.

## Required Review

1. Read the current diff and list every new or materially changed file, method, type, component, hook, service, utility, wrapper, and module.
2. Search the repository for existing equivalents before keeping any new symbol or file.
3. Classify each change as a root-cause fix or a patch-style workaround.
4. Identify duplicated ownership, alternate data shapes, forwarding layers, and stale wiring.
5. Rewrite only where the simpler form preserves requested behavior.
6. Remove superseded code, types, fields, parameters, call sites, imports, and files in the same pass.
7. Re-read the final diff and run the repository-required validation for code changes.

## Simplification Rules

- Reuse an existing implementation or source type before creating another.
- Prefer plain functions. Use hooks only when they call hooks or own React lifecycle; use classes only for stateful clients or specialized data structures.
- Delete pass-through helpers, renaming wrappers, no-op adapters, forwarding modules, and same-process message layers.
- Keep one-off logic local when clear. Extract only for real reuse, meaningful ownership, or long-file navigation.
- Use inline literals for one-off values and module constants for shared static data. Do not hide static data behind functions, hooks, factories, providers, or lazy builders.
- Prefer language, runtime, browser, and existing framework APIs over local reimplementations.
- Consume raw source data and source types when they fit. Transform or normalize only at a real boundary, once, in the owning layer.
- Compute synchronous UI values directly; do not mirror them into React state or effects.
- Prefer direct imports over threading static config, constants, helpers, or services through props, context, state, factories, or DI.
- Do not preserve a fallback, compatibility path, or abstraction merely because it already exists when no active contract requires it.

## Root-Cause Gate

Do not keep a workaround that masks broken ownership or data flow, including:

- UI compensation for invalid source data
- normalization repeated on both sides of a boundary
- guards or fallbacks that suppress a deeper inconsistency
- one-off transforms near the symptom
- parallel implementations with slightly different names or shapes

If replacing the workaround requires a material behavior or scope change not requested by the user, report the required root-cause change instead of silently widening scope.

## Scope

Focus on the current-session diff. Expand only to reuse an existing implementation, restore one source of truth, or remove wiring superseded by the change.

## Completion Criteria

- behavior is preserved unless the request changes it
- fewer layers, wrappers, helpers, and duplicate concepts remain
- ownership and runtime boundaries are clearer
- new symbols were checked against the repository
- no dead or orphaned implementation remains
- required validation passes

If the diff already meets this bar, leave it unchanged and say so.
