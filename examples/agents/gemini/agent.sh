#!/usr/bin/env bash
set -euo pipefail

prompt=$(cat)
printf '%s' "$prompt" | gemini --output-format stream-json
