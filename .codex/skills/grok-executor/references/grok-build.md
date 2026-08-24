# Grok Build operational notes

These notes were verified against the official documentation and local Grok Build `1.0.4` on 2026-08-23. The CLI changes quickly; prefer `grok --help` when local behavior differs.

## Headless execution

- `grok -p "..."` and the CLI's `--prompt-file <path>` are single-turn interfaces. They cannot accept a true follow-up while that process is running.
- The executor wrapper defaults to its own `summary` format: lifecycle sentinels plus one bounded final result per turn. `plain` streams assistant/tool output, `json` emits structured controller events, and `streaming-json` also exposes raw ACP traffic; use the verbose modes only for diagnosis.
- Use `--no-auto-update` so an execution task does not mutate the CLI binary or stall on an update.
- The executor wrapper uses `grok agent stdio`, which exposes newline-delimited ACP JSON-RPC. Its lifecycle is `initialize`, `authenticate`, `session/new`, one or more `session/prompt` turns, streamed `session/update` notifications, and `session/cancel` when needed.
- The wrapper gives every bounded task one isolated temporary Grok home and one fresh ACP session. Personal hooks, plugins, MCP credentials, skills, config, memories, and prior sessions are not loaded. The task session remains alive for Codex follow-ups and is removed only after `close` or non-interactive EOF.
- Raw ACP traffic and stderr go to a mode-`0600`, 16 MiB-capped diagnostic JSONL file in that temporary home. Its path is announced with the session and it is deleted with the task session.
- Grok's current `x.ai/interject` extension can steer a running turn at a safe point. The controller probes the older `_x.ai/interject` spelling only for compatibility; if neither exists, it cancels the active turn and sends the interjection as the next prompt in the same session.

## Permissions and sandbox

Permissions decide which tool calls run; the sandbox limits what an approved process can access. They are separate controls.

The wrapper starts global controls before `agent`, then `agent --always-approve --no-leader stdio`. It uses:

- always-approve mode to prevent an unattended ACP task from hanging on a prompt;
- explicit deny rules for destructive Git, filesystem, privilege, publish, and deployment commands;
- `--sandbox workspace` for execution, which permits writes only in the current working directory, `~/.grok`, and temporary storage;
- `--sandbox read-only` for investigations;
- `--no-plan`, `--no-subagents`, `--no-memory`, and `--disable-web-search` to keep Grok in the executor role and reduce hidden scope.
- disabled Cursor and Claude compatibility discovery and background workflows, plus a deny rule for all MCP calls. Grok may still advertise a remote built-in MCP during startup, but the isolated home has no copied MCP credential and the permission rule forbids its tools.
- a `shell_environment_policy` that gives Grok-run commands only the core process environment and applies Grok's default `*KEY*`, `*SECRET*`, and `*TOKEN*` exclusions.

Important limits:

- `workspace` can read outside the repository and allows child-process network access. Never pass secrets, and retain prompt rules that forbid credential access.
- The isolated Grok runtime directory and temporary storage remain writable for task state.
- On macOS, the built-in child-network restriction for `read-only` and `strict` is not enforced. Do not treat those profiles as network isolation.
- Deny rules override allow/always-approve behavior, but they do not replace independent diff review.

Official sources:

- [Agent mode and ACP](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/15-agent-mode.md)
- [Session management](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/17-sessions.md)
- [Headless and scripting](https://docs.x.ai/build/cli/headless-scripting)
- [CLI reference](https://docs.x.ai/build/cli/reference)
- [Permissions](https://docs.x.ai/build/features/permissions)
- [Sandbox profiles and limitations](https://docs.x.ai/build/features/sandbox)
- [Shell environment policy](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/18-sandbox.md#shell-environment-policy)
