#!/usr/bin/env bash
set -euo pipefail

codex exec --json - \
  1> >(jq -r 'select(.type == "turn.completed") | .message // empty') \
  2> >(cat >&2)
