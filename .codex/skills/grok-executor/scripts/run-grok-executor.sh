#!/usr/bin/env bash
set -euo pipefail

usage() {
  command cat <<'EOF'
Usage:
  run-grok-executor.sh (--inspect | --execute) [options]

Options:
  --inspect               Run with a read-only sandbox (default).
  --execute               Allow writes inside the working directory.
  --cwd PATH              Grok working directory (default: current directory).
  --prompt-file PATH      Read the initial task contract from PATH (default: stdin).
  --output-format FORMAT  summary, plain, json, or streaming-json (default: summary).
  --model MODEL           Override the subscription-selected default model.
  --effort LEVEL          Override reasoning effort.
  --max-turns N           Limit agent turns.
  -h, --help              Show this help.

With a TTY, the process remains open after the initial turn. Send one JSON
control object per line: prompt, prompt_file, interject, cancel, status, close.
Without a TTY, EOF closes the controller after the initial turn completes.
EOF
}

mode="inspect"
cwd="$PWD"
prompt_file=""
output_format="summary"
model=""
effort=""
max_turns=""

while (($#)); do
  case "$1" in
    --inspect) mode="inspect"; shift ;;
    --execute) mode="execute"; shift ;;
    --cwd)
      (($# >= 2)) || { printf '%s\n' 'error: --cwd requires a path' >&2; exit 2; }
      cwd="$2"; shift 2 ;;
    --prompt-file)
      (($# >= 2)) || { printf '%s\n' 'error: --prompt-file requires a path' >&2; exit 2; }
      prompt_file="$2"; shift 2 ;;
    --output-format)
      (($# >= 2)) || { printf '%s\n' 'error: --output-format requires a value' >&2; exit 2; }
      output_format="$2"; shift 2 ;;
    --model)
      (($# >= 2)) || { printf '%s\n' 'error: --model requires a value' >&2; exit 2; }
      model="$2"; shift 2 ;;
    --effort)
      (($# >= 2)) || { printf '%s\n' 'error: --effort requires a value' >&2; exit 2; }
      effort="$2"; shift 2 ;;
    --max-turns)
      (($# >= 2)) || { printf '%s\n' 'error: --max-turns requires a value' >&2; exit 2; }
      max_turns="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *)
      printf 'error: unknown argument: %s\n' "$1" >&2
      usage >&2
      exit 2 ;;
  esac
done

grok_bin="${GROK_EXECUTOR_GROK_BIN:-}"
if [[ -z "$grok_bin" ]]; then
  grok_bin="$(command -v grok || true)"
fi
if [[ -z "$grok_bin" || ! -x "$grok_bin" ]]; then
  printf '%s\n' 'error: official Grok Build CLI not found on PATH' >&2
  exit 127
fi

case "$output_format" in
  summary|plain|json|streaming-json) ;;
  *) printf 'error: unsupported output format: %s\n' "$output_format" >&2; exit 2 ;;
esac

if [[ -n "$max_turns" && ! "$max_turns" =~ ^[1-9][0-9]*$ ]]; then
  printf '%s\n' 'error: --max-turns must be a positive integer' >&2
  exit 2
fi
if [[ ! -d "$cwd" ]]; then
  printf 'error: working directory does not exist: %s\n' "$cwd" >&2
  exit 2
fi
cwd="$(cd "$cwd" && pwd -P)"
if [[ -n "$prompt_file" && ! -r "$prompt_file" ]]; then
  printf 'error: prompt file is not readable: %s\n' "$prompt_file" >&2
  exit 2
fi
if [[ -z "$prompt_file" && -t 0 ]]; then
  printf '%s\n' 'error: provide --prompt-file PATH or pipe a task contract on stdin' >&2
  exit 2
fi

source_grok_home="${GROK_HOME:-$HOME/.grok}"
executor_home="$(mktemp -d "${TMPDIR:-/tmp}/grok-executor-home.XXXXXX")"
chmod 700 "$executor_home"

cleanup() { find "$executor_home" -depth -delete; }
trap cleanup EXIT INT TERM HUP

sync_refreshed_auth() {
  local refreshed="$executor_home/auth.json"
  local destination="$source_grok_home/auth.json"
  local destination_dir staged
  [[ -f "$refreshed" ]] || return 0
  if [[ -f "$destination" ]] && cmp -s "$refreshed" "$destination"; then return 0; fi
  destination_dir="$(dirname "$destination")"
  mkdir -p "$destination_dir"
  chmod 700 "$destination_dir"
  staged="$(mktemp "$destination_dir/.auth.json.grok-executor.XXXXXX")"
  cp -p "$refreshed" "$staged"
  chmod 600 "$staged"
  mv -f "$staged" "$destination"
}

for state_file in auth.json agent_id models_cache.json; do
  if [[ -f "$source_grok_home/$state_file" ]]; then
    cp -p "$source_grok_home/$state_file" "$executor_home/$state_file"
  fi
done
if [[ -d "$source_grok_home/bundled" ]]; then
  ln -s "$source_grok_home/bundled" "$executor_home/bundled"
fi

task_file="$executor_home/task.md"
diagnostic_file="$executor_home/acp-diagnostics.jsonl"
if [[ -n "$prompt_file" ]]; then
  command cat "$prompt_file" >"$task_file"
else
  command cat >"$task_file"
fi
if [[ ! -s "$task_file" ]]; then
  printf '%s\n' 'error: task contract is empty' >&2
  exit 2
fi

rules='Act only as the implementation executor for Codex. Codex has already selected the design and scope. Execute the supplied task directly; do not create a new plan, broaden scope, or delegate to subagents. Follow repository instructions and preserve unrelated changes. Work only inside the current working directory. Never access secrets or credentials; modify user configuration; commit, push, deploy, publish, or change remote state. Run the task validation when possible. If a secret, external authority, destructive cleanup, material design choice, or broader scope is required, stop and report BLOCKED. Treat later prompts and interjections in this session as Codex steering for this same bounded task. End completed handoffs with STATUS, SUMMARY, CHANGED, VALIDATION, and REMAINING.'

sandbox="read-only"
if [[ "$mode" == "execute" ]]; then sandbox="workspace"; fi

grok_args=(
  --no-auto-update
  --cwd "$cwd"
  --sandbox "$sandbox"
  --no-plan
  --no-subagents
  --no-memory
  --disable-web-search
  --rules "$rules"
  --deny 'Bash(*rm -rf*)'
  --deny 'Bash(*sudo *)'
  --deny 'Bash(*git reset --hard*)'
  --deny 'Bash(*git clean*)'
  --deny 'Bash(*git commit*)'
  --deny 'Bash(*git push*)'
  --deny 'Bash(*gh *)'
  --deny 'Bash(*docker push*)'
  --deny 'Bash(*npm publish*)'
  --deny 'Bash(*pnpm publish*)'
  --deny 'Bash(*yarn npm publish*)'
  --deny 'Bash(*wrangler deploy*)'
  --deny 'Bash(*vercel deploy*)'
  --deny 'MCPTool(*)'
)
if [[ -n "$max_turns" ]]; then grok_args+=(--max-turns "$max_turns"); fi
grok_args+=(agent --always-approve --no-leader)
if [[ -n "$model" ]]; then grok_args+=(--model "$model"); fi
if [[ -n "$effort" ]]; then grok_args+=(--reasoning-effort "$effort"); fi
grok_args+=(stdio)

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../scripts" && pwd -P)"
printf 'Grok executor mode=%s transport=acp cwd=%s output=%s\n' "$mode" "$cwd" "$output_format" >&2
export GROK_HOME="$executor_home"
export GROK_CURSOR_SKILLS_ENABLED=0
export GROK_CURSOR_RULES_ENABLED=0
export GROK_CURSOR_AGENTS_ENABLED=0
export GROK_CURSOR_MCPS_ENABLED=0
export GROK_CURSOR_HOOKS_ENABLED=0
export GROK_CURSOR_SESSIONS_ENABLED=0
export GROK_CLAUDE_SKILLS_ENABLED=0
export GROK_CLAUDE_RULES_ENABLED=0
export GROK_CLAUDE_AGENTS_ENABLED=0
export GROK_CLAUDE_MCPS_ENABLED=0
export GROK_CLAUDE_HOOKS_ENABLED=0
export GROK_CLAUDE_SESSIONS_ENABLED=0
export GROK_MANAGED_MCPS_ENABLED=0
export GROK_MANAGED_MCP_GATEWAY_TOOLS_ENABLED=0
export GROK_WORKFLOWS=0
export GROK_CONFIG='{"shell_environment_policy":{"inherit":"core","ignore_default_excludes":false}}'
unset GROK_CONFIG_PATH GROK_AGENT XAI_API_KEY

set +e
python3 "$script_dir/grok-acp-executor.py" \
  --cwd "$cwd" \
  --prompt-file "$task_file" \
  --diagnostic-file "$diagnostic_file" \
  --output-format "$output_format" \
  -- "$grok_bin" "${grok_args[@]}"
grok_status=$?
set -e
sync_refreshed_auth
exit "$grok_status"
