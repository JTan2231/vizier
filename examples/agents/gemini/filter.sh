#!/bin/sh
set -euo pipefail

# gemini/filter.sh
# ----------------
# Bridges the Gemini CLI JSONL stream and Vizier. Gemini writes newline-delimited
# JSON events to stdout; this filter:
#   - Parses each event with jq and renders human-friendly progress to stderr.
#   - Tracks the final assistant reply (or a reasonable fallback) and prints it
#     once at the end to stdout.
#
# NOTE: Vizier only consumes stdout for the agent reply. Ensure this script
#       emits the final assistant text there.
#
# ANSI color is optional: it enables when stderr is a TTY and both NO_COLOR and
# VIZIER_NO_ANSI are unset; otherwise output stays plain for logs/pipes.
USE_COLOR=0
if [ -t 2 ] && [ -z "${NO_COLOR:-}" ] && [ -z "${VIZIER_NO_ANSI:-}" ]; then
  USE_COLOR=1
fi

if ! command -v jq >/dev/null 2>&1; then
  printf '[gemini-filter] jq is required on PATH to render progress\n' >&2
  exit 1
fi

# Collapse whitespace so multi-line JSON/text shows up as single-line progress.
collapse() {
  printf '%s' "$1" | tr '\n' ' ' | sed 's/[[:space:]]\+/ /g; s/^ //; s/ $//'
}

# Wrap text in ANSI color codes when allowed. Pass standard SGR codes (e.g.,
# "32" for green, "33" for yellow). Falls back to raw text when color is off.
paint() {
  code="$1"
  shift
  text="$*"
  if [ "$USE_COLOR" -eq 1 ]; then
    # shellcheck disable=SC2059
    printf '\033[%sm%s\033[0m' "$code" "$text"
  else
    printf '%s' "$text"
  fi
}

# Format usage stats from a result event when present.
format_stats() {
  line="$1"
  total=$(printf '%s' "$line" | jq -r 'try .stats.total_tokens // ""' 2>/dev/null)
  input=$(printf '%s' "$line" | jq -r 'try .stats.input_tokens // ""' 2>/dev/null)
  output=$(printf '%s' "$line" | jq -r 'try .stats.output_tokens // ""' 2>/dev/null)
  duration=$(printf '%s' "$line" | jq -r 'try .stats.duration_ms // ""' 2>/dev/null)
  tools=$(printf '%s' "$line" | jq -r 'try .stats.tool_calls // ""' 2>/dev/null)

  parts=""
  if [ -n "$total" ] && [ "$total" != "null" ]; then
    parts="tokens=$total"
  fi

  if [ -n "$input" ] && [ "$input" != "null" ]; then
    [ -n "$parts" ] && parts="$parts "
    parts="${parts}input=$input"
  fi

  if [ -n "$output" ] && [ "$output" != "null" ]; then
    [ -n "$parts" ] && parts="$parts "
    parts="${parts}output=$output"
  fi

  if [ -n "$tools" ] && [ "$tools" != "null" ]; then
    [ -n "$parts" ] && parts="$parts "
    parts="${parts}tools=$tools"
  fi

  if [ -n "$duration" ] && [ "$duration" != "null" ]; then
    [ -n "$parts" ] && parts="$parts "
    parts="${parts}duration=${duration}ms"
  fi

  printf '%s' "$parts"
}

# Small helper to keep stderr writes consistent.
progress() {
  msg="$1"
  [ -n "$msg" ] && printf '%s\n' "$msg" >&2
}

# Track assistant output so we can emit a single reply on stdout at the end.
assistant_buffer=""
final_text=""
fallback_text=""
last_event_type=""
last_role=""

# Main loop: consume JSONL, emit progress, remember final output.
while IFS= read -r line; do
  # Ignore accidental blank lines so they do not spam stderr.
  [ -z "$line" ] && continue

  role=""
  type=$(printf '%s' "$line" | jq -r 'try .type // ""' 2>/dev/null)
  if [ -z "$type" ] || [ "$type" = "null" ]; then
    progress "$(collapse "$line")"
    last_event_type=""
    last_role=""
    continue
  fi

  case "$type" in
    init)
      model=$(printf '%s' "$line" | jq -r 'try .model // ""' 2>/dev/null)
      session=$(printf '%s' "$line" | jq -r 'try .session_id // ""' 2>/dev/null)
      summary="session start"
      [ -n "$model" ] && summary="$summary (model=$model)"
      [ -n "$session" ] && summary="$summary id=$session"
      progress "$(paint 36 "$summary")"
      ;;

    message)
      role=$(printf '%s' "$line" | jq -r 'try .role // ""' 2>/dev/null)
      content=$(printf '%s' "$line" | jq -r 'try .content // ""' 2>/dev/null)
      [ "$content" = "null" ] && content=""
      delta=$(printf '%s' "$line" | jq -r 'try .delta // false' 2>/dev/null)
      summary=$(collapse "$content")

      if [ "$role" = "assistant" ]; then
        new_message=1
        if [ "$last_event_type" = "message" ] && [ "$last_role" = "assistant" ]; then
          new_message=0
        fi

        if [ "$new_message" -eq 1 ]; then
          assistant_buffer=""
        fi

        assistant_buffer="${assistant_buffer}${content}"
        if [ -n "$assistant_buffer" ]; then
          final_text="$assistant_buffer"
          fallback_text="$assistant_buffer"
        fi

        label="assistant"
        if [ "$delta" = "true" ]; then
          label="$label (stream)"
        fi

        if [ -n "$summary" ]; then
          progress "$(paint 32 "$label: $summary")"
        else
          progress "$(paint 32 "$label update")"
        fi
      elif [ "$role" = "user" ]; then
        [ -n "$summary" ] && progress "$(paint 36 "user: $summary")"
      else
        [ -n "$summary" ] && progress "$(paint 36 "message: $summary")"
      fi
      ;;

    tool_use)
      tool_name=$(printf '%s' "$line" | jq -r 'try .tool_name // ""' 2>/dev/null)
      tool_id=$(printf '%s' "$line" | jq -r 'try .tool_id // ""' 2>/dev/null)
      params=$(printf '%s' "$line" | jq -c 'try .parameters // {}' 2>/dev/null)
      [ "$params" = "null" ] && params=""
      param_summary=$(collapse "$params")

      msg="tool"
      [ -n "$tool_name" ] && msg="$msg $tool_name"
      [ -n "$tool_id" ] && [ "$tool_id" != "null" ] && msg="$msg ($tool_id)"
      [ -n "$param_summary" ] && msg="$msg — $param_summary"

      progress "$(paint 34 "$msg")"
      ;;

    tool_result)
      tool_id=$(printf '%s' "$line" | jq -r 'try .tool_id // ""' 2>/dev/null)
      status=$(printf '%s' "$line" | jq -r 'try .status // ""' 2>/dev/null)
      output=$(printf '%s' "$line" | jq -r 'try .output // ""' 2>/dev/null)
      [ "$output" = "null" ] && output=""
      summary=$(collapse "$output")

      msg="tool result"
      [ -n "$tool_id" ] && [ "$tool_id" != "null" ] && msg="$msg ($tool_id)"
      [ -n "$status" ] && [ "$status" != "null" ] && msg="$msg [$status]"
      [ -n "$summary" ] && msg="$msg — $summary"

      progress "$(paint 34 "$msg")"
      if [ -z "$final_text" ] && [ -n "$summary" ]; then
        fallback_text="$summary"
      fi
      ;;

    result)
      status=$(printf '%s' "$line" | jq -r 'try .status // ""' 2>/dev/null)
      result_text=$(printf '%s' "$line" | jq -r 'try .result // ""' 2>/dev/null)
      [ "$result_text" = "null" ] && result_text=""
      usage=$(format_stats "$line")
      color=32
      if [ "$status" != "success" ] && [ -n "$status" ]; then
        color=31
      fi

      msg="run"
      [ -n "$status" ] && msg="$msg $status"
      [ -n "$usage" ] && msg="$msg — $usage"

      progress "$(paint "$color" "$msg")"
      if [ -z "$final_text" ] && [ -n "$result_text" ]; then
        fallback_text="$result_text"
      fi
      ;;

    *)
      message=$(printf '%s' "$line" | jq -r 'try .message // .detail // ""' 2>/dev/null)
      [ "$message" = "null" ] && message=""
      summary=$(collapse "$message")
      [ -z "$summary" ] && summary=$(collapse "$line")
      progress "$(paint 36 "$type: $summary")"
      ;;
  esac

  last_event_type="$type"
  last_role="$role"
done

# Emit the assistant reply once. If Gemini never sent an assistant message, use
# the best fallback we saw (tool output or result summary) so callers still get output.
if [ -n "$final_text" ]; then
  printf '%s\n' "$final_text"
elif [ -n "$fallback_text" ]; then
  printf '%s\n' "$fallback_text"
fi
