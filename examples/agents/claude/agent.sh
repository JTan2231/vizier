#!/usr/bin/env bash
set -euo pipefail

prompt=$(cat)
printf '%s' "$prompt" | claude --output-format stream-json
