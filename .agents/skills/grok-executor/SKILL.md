---
name: grok-executor
description: Delegate a concrete, locally authorized implementation task from Codex as planner and reviewer to the official Grok Build CLI as executor. Use when the user explicitly asks Codex to have Grok, SuperGrok, or Grok CLI implement, fix, test, or modify code, or asks for a Codex-planner/Grok-executor workflow. Do not trigger for ordinary coding tasks that do not request Grok, or for external commits, pushes, deployments, publication, or production mutations.
---

# Grok Executor

Keep Codex responsible for scope, planning, authorization, and final verification. Use the official `grok` CLI only as the implementation worker.

## Preserve the role boundary

- Derive the plan from the user's request, repository instructions, current code, and current diff before invoking Grok.
- Resolve material design choices yourself. Ask the user only when a missing choice would materially change the result.
- Delegate one bounded task with explicit acceptance criteria. Do not ask Grok to choose the product direction or broaden scope.
- Do not make implementation edits in parallel with Grok. If its result is incomplete, give it a focused correction turn in the same task session or report the blocker.
- Never delegate secrets, credentials, production data, or actions outside the local workspace.
- Never let Grok commit, push, deploy, publish, change remote systems, or edit user-level configuration. Perform any separately authorized external step yourself after reviewing the result.

## Prepare the task

1. Read all applicable repository instructions and inspect `git status --short` plus the relevant diff. Preserve unrelated user changes.
2. Form a concrete implementation plan. Do not wait for plan approval unless the request is ambiguous or high-impact enough to require a user choice.
3. Read [references/task-contract.md](references/task-contract.md) and write a complete task brief. Include exact scope, constraints, acceptance criteria, validation, and known dirty-worktree context.
4. Avoid putting sensitive values in the brief. The wrapper removes its temporary task session after Codex closes it, but task content and follow-ups are still sent to Grok for inference.

## Delegate

Pass the task brief through a file or stdin; never interpolate a multiline task into shell syntax. Use a fresh session for every bounded task—do not add `--continue` or `--resume` across tasks. Within that bounded task, keep the wrapper's ACP process alive and use the same Grok session for progress updates, steering, corrections, and validation follow-ups.

For implementation:

```bash
.codex/skills/grok-executor/scripts/run-grok-executor.sh \
  --execute \
  --cwd /absolute/path/to/repo \
  --prompt-file /absolute/path/to/task-brief.md
```

For a Grok read-only investigation explicitly requested by the user:

```bash
.codex/skills/grok-executor/scripts/run-grok-executor.sh \
  --inspect \
  --cwd /absolute/path/to/repo \
  --prompt-file /absolute/path/to/task-brief.md
```

Allocate a PTY when Codex may need to steer the task. The wrapper prints `GROK_ACP_SESSION` once the fresh session exists, `GROK_ACP_TURN_STARTED` for each turn, and `GROK_ACP_IDLE` after a turn completes. It remains alive at idle and accepts one JSON control object per input line:

```json
{"type":"interject","text":"Stop the single-crate approach; keep the requested workspace boundaries."}
{"type":"prompt","text":"The clippy run failed with this exact error. Fix it and rerun the scoped checks."}
{"type":"prompt_file","path":"/absolute/path/to/follow-up.md"}
{"type":"status"}
{"type":"cancel"}
{"type":"close"}
```

- Use `interject` to steer an active turn. The controller uses Grok's native interjection when available and otherwise cancels safely, then sends the text as the next turn in the same session.
- Use `prompt` or `prompt_file` for an ordinary same-session follow-up. If a turn is active, the controller queues it.
- Use `cancel` to stop the active turn without abandoning the task session.
- After Codex accepts the implementation, send `close`; only then is the isolated session removed. On non-interactive stdin EOF, the wrapper waits for the initial turn and closes automatically for one-shot compatibility.

The default `summary` output is the context-efficient interface. It suppresses streamed reasoning, individual tool-call events, and Grok stderr. Each turn emits one `GROK_ACP_RESULT` containing the final assistant handoff capped at 4 KiB, elapsed time, tool-call count, and last tool, followed by `GROK_ACP_IDLE`. `status` is likewise compact. Keep Codex tool-output budgets small and do not replay unchanged polls into the conversation.

`GROK_ACP_SESSION` includes a `diagnosticFile` path. Full ACP traffic, prompts, tool events, and stderr are recorded there with mode `0600` and a 16 MiB cap. The file is inside the task's temporary Grok home and disappears on `close`; it may contain sensitive task text. Read only a targeted tail or matching lines when a result fails or is ambiguous, and do so before closing. Use `--output-format plain` or `--output-format streaming-json` only for an explicitly diagnosed transport problem; these debug modes can consume substantial context. `json` keeps controller events structured without streaming raw ACP traffic.

The script defaults to `--inspect`; require the explicit `--execute` flag for writes. Let the script own Grok's ACP lifecycle, permission, sandbox, update, plan, subagent, memory, web-search, and destructive-command controls. Do not bypass or weaken them. The wrapper isolates each task home but atomically preserves an OAuth auth file refreshed by the official CLI, because refresh-token rotation would otherwise invalidate the next fresh session.

Read [references/grok-build.md](references/grok-build.md) only when CLI flags drift or sandbox behavior needs troubleshooting.

## Verify independently

When Grok reports `GROK_ACP_RESULT` and `GROK_ACP_IDLE`:

1. Inspect `git status --short` and the complete diff yourself.
2. Check every changed file against the plan, repository rules, and acceptance criteria.
3. Confirm unrelated changes were preserved and no external action occurred.
4. Run the relevant validation yourself when safe and practical; do not treat Grok's claim that tests passed as proof.
5. If the result is wrong but the task is still well-scoped, send one focused `prompt` follow-up in the same Grok task session. Include the observed diff or error, not a vague request to retry.
6. If Grok needs secrets, external authority, a material user choice, or broader scope, stop and report the blocker.
7. Send `close` only after the task is accepted, blocked, or deliberately abandoned; then confirm the controller exits and perform the final status/diff check.

## Report the outcome

State separately:

- the plan Codex delegated;
- what Grok actually changed;
- what Codex independently verified;
- any remaining manual QA or blocker.

Do not imply the task succeeded merely because the Grok process exited successfully.
