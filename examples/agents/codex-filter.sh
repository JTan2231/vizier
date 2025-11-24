#!/bin/sh
set -euo pipefail

# Stream progress lines to stderr and emit the final assistant text on stdout.
last=""
while IFS= read -r line; do
  # Mirror all agent output to progress stderr for terminal rendering.
  printf 'progress:%s\n' "$line" >&2
  last="$line"
done

# Return the final assistant text (last line) on stdout.
printf '%s' "$last"
