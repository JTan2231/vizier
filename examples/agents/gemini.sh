#!/usr/bin/env bash
set -euo pipefail

gemini --output-format stream-json \
  1> >(jq -r 'select(.type == "message" and .message.role == "assistant") | .message.content // empty') \
  2> >(cat >&2)
