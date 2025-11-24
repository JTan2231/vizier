#!/usr/bin/env bash
set -euo pipefail

# Read the prompt from stdin once so we can document what is being sent to Codex.
prompt=$(cat)
if [ -z "$prompt" ]; then
  printf '[codex shim] prompt: <empty>\n' >&2
else
  first_line=${prompt%%$'\n'*}
  printf '[codex shim] prompt (first line preview): %s\n' "$first_line" >&2
fi

printf '%s' "$prompt" | codex exec --json -
