#!/usr/bin/env bash
set -euo pipefail

prompt=$(cat)
if [ -z "$prompt" ]; then
  printf '[gemini shim] prompt: <empty>\n' >&2
else
  first_line=${prompt%%$'\n'*}
  printf '[gemini shim] prompt (first line preview): %s\n' "$first_line" >&2
fi

printf '%s' "$prompt" | gemini --output-format stream-json
