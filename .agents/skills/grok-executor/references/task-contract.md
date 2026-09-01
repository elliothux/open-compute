# Planner-to-executor task contract

Give Grok a self-contained task. Replace every placeholder and omit empty sections.

```markdown
# Goal

<One concrete outcome.>

# Context

- Repository: <absolute path>
- Relevant code path and observed current behavior: <facts>
- Existing user changes that must be preserved: <paths or "none observed">
- Codex's chosen design: <the implementation direction; do not ask Grok to redesign it>

# Required changes

1. <Specific change>
2. <Specific change>

# Constraints

- Stay within the stated files and ownership boundaries unless a directly required call site must change.
- Follow repository instructions and reuse existing implementations before adding abstractions.
- Preserve unrelated worktree changes.
- Do not commit, push, deploy, publish, access secrets, modify user configuration, or change remote state.
- Do not create subagents or a new plan; execute this task directly.

# Acceptance criteria

- <Observable condition>
- <Regression condition>

# Validation

- Run: `<exact command>`
- Run: `<exact command>`

# Stop conditions

Stop and report `BLOCKED` without guessing if completion requires a secret, external authority, a material product choice, destructive cleanup, or scope beyond this contract.
```

Require Grok's final response to use this compact shape:

```text
STATUS: COMPLETED | BLOCKED
SUMMARY: <what was implemented>
CHANGED: <paths and purpose>
VALIDATION: <commands and exact outcomes>
REMAINING: <none, or a concrete blocker/manual check>
```

Treat this response only as a handoff. Verify the worktree and tests independently.
