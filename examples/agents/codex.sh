#!/usr/bin/env bash
set -euo pipefail

# Read the prompt from stdin once so we can document what is being sent to Codex.
prompt=$(cat)
if [ -z "$prompt" ]; then
  printf '[codex wrapper] prompt: <empty>\n' >&2
else
  first_line=${prompt%%$'\n'*}
  printf '[codex wrapper] prompt (first line preview): %s\n' "$first_line" >&2
fi

printf '%s' "$prompt" | codex exec --json - \
  | tee >(jq -r '
      if .type == "item.completed" and .item.type == "reasoning" then
        "[codex] reasoning: \(.item.text)"
      elif .type == "item.started" and .item.type == "command_execution" then
        "[codex] cmd start: \(.item.command)"
      elif .type == "item.completed" and .item.type == "command_execution" then
        "[codex] cmd done (\(.item.exit_code // "n/a")): \(.item.command)"
      elif .type == "turn.completed" then
        "[codex] turn completed (input=\(.usage.input_tokens // 0) cached=\(.usage.cached_input_tokens // 0) output=\(.usage.output_tokens // 0))"
      elif .type == "error" then
        "[codex] error: \(.message)"
      else empty end
    ' >&2) \
  | jq -r 'select(.type == "item.completed" and .item.type == "agent_message") | .item.text' \
  | tail -n 1
