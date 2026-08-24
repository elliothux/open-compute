---
name: anti-cheating
description: Audit the current Lynx branch and working tree for test-, fixture-, demo-, page-, domain-, or scenario-tuned production logic. Use for anti-cheating reviews of code, prompts, skills, tool descriptions, or e2e-related changes; do not use for general code review.
---

# Anti-Cheating Audit

Audit committed branch changes, tracked working-tree changes, and untracked files by default. Expand only to call sites needed to prove or disprove a candidate. Report findings; do not edit unless the user also asks for fixes.

## Invariant

Production behavior must follow runtime inputs, schemas, observed UI state, persisted data, generic algorithms, or real product/platform contracts. Test and example literals belong only in tests, fixtures, or explicit user input.

## Exclusions

Do not flag these without evidence of production coupling:

- `e2e/**`, specs, tests, fixtures, snapshots, and `e2e/output/**`
- generated files
- platform/API contracts, connector manifests, bundle IDs, and regional product domains
- legitimate specialization contained inside its owning domain skill or connector
- clearly illustrative documentation examples
- literals supplied or persisted at runtime

## Workflow

1. Resolve the branch base from `@{upstream}` and its merge base with `HEAD`. Inventory three scopes separately:
   - committed changes from the upstream merge base through `HEAD`
   - staged and unstaged tracked changes against `HEAD`
   - untracked files from `git ls-files --others --exclude-standard`
     If no upstream exists, do not guess a base. Audit the working tree immediately; ask for the intended base before judging committed changes. If the user requested only a working-tree audit, state that committed history is excluded and continue.
2. Identify changed tests, prompts, skills, tool descriptions, routing/policy, workflows, and production branches.
3. Extract distinctive scenario literals from changed tests or examples: prompts, labels, selectors, URLs, app/page names, expected response fragments, and ordered steps.
4. Cross-search those literals against production surfaces, then reverse-search new production guidance against tests.
5. Inspect conditionals and constants for behavior keyed to scenario identity rather than runtime contracts.
6. Classify every candidate with the decision tests below.
7. Return only evidence-backed findings. If none remain, state that the audit is clean and name the scope reviewed.

Use [patterns.md](patterns.md) for repo paths, smell patterns, approved exceptions, and grep recipes. Treat grep hits only as candidates.

## Decision Tests

Apply all four:

1. **Removal:** If the triggering spec/example vanished, would the production behavior still belong?
2. **Duplication:** Is the same scenario rule repeated across a test, global prompt, domain skill, tool description, or production branch?
3. **Generalization:** Could the requirement be expressed from runtime state or a generic contract without scenario vocabulary?
4. **Input:** Does behavior depend on a literal copied from a test/example instead of runtime input, schema, state, or a product contract?

A suspicious match is not a finding until the evidence shows scenario tuning.

## Severity

- **Blocker:** Production logic or global guidance encodes a specific test, fixture, e2e prompt, or scenario identity.
- **Major:** A global prompt, skill, or tool description contains a scenario playbook that belongs in an owning domain surface or only in tests.
- **Minor:** Duplicated or misleading scenario guidance creates drift risk but does not currently control production behavior.

## Output

Lead with the verdict and scope. Sort findings by severity and include:

- `path:line`
- exact evidence
- the matched test/example or scenario source
- why it violates a decision test
- the smallest root-cause fix

Include a compact cross-reference table when multiple literals or surfaces are involved. Do not include empty severity sections. Preserve raw evidence and distinguish facts from inference.

## Fix Direction

When fixes are requested:

- delete scenario shaping from global prompts and tool descriptions
- keep domain behavior only in its owning skill or connector
- replace literal branches with schema-, input-, state-, or contract-driven logic
- keep qualitative assertions limited to stable lifecycle signals
- never add test-only hooks, bridges, fake tools, or mock agent surfaces
