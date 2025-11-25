#!/bin/sh
set -euo pipefail

# claude/filter.sh
# ----------------
# Bridges the Claude CLI JSONL stream and Vizier. Claude writes newline-delimited
# JSON events to stdout; this filter:
#   - Parses each event with jq and renders human-friendly progress to stderr.
#   - Tracks the final assistant reply (or fallback) and prints it once to stdout.
#
# NOTE: Vizier only consumes stdout for the agent reply; keep progress on stderr.
#
# ANSI color is optional: it enables when stderr is a TTY and both NO_COLOR and
# VIZIER_NO_ANSI are unset; otherwise output stays plain for logs/pipes.
USE_COLOR=0
if [ -t 2 ] && [ -z "${NO_COLOR:-}" ] && [ -z "${VIZIER_NO_ANSI:-}" ]; then
  USE_COLOR=1
fi

if ! command -v jq >/dev/null 2>&1; then
  printf '[claude-filter] jq is required on PATH to render progress\n' >&2
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

# Human-friendly usage formatter that tolerates missing fields.
format_usage() {
  line="$1"
  total=$(printf '%s' "$line" | jq -r 'try (.usage.total_tokens // .usage.total) // ""' 2>/dev/null)
  input=$(printf '%s' "$line" | jq -r 'try (.usage.input_tokens // .usage.prompt_tokens) // ""' 2>/dev/null)
  cached=$(printf '%s' "$line" | jq -r 'try (.usage.cache_read_input_tokens // .usage.cached_input_tokens // .usage.cache_creation_input_tokens) // ""' 2>/dev/null)
  output=$(printf '%s' "$line" | jq -r 'try (.usage.output_tokens // .usage.completion_tokens) // ""' 2>/dev/null)
  reasoning=$(printf '%s' "$line" | jq -r 'try .usage.reasoning_output_tokens // ""' 2>/dev/null)
  cost=$(printf '%s' "$line" | jq -r 'try (.total_cost_usd // .usage.cost) // ""' 2>/dev/null)

  parts=""
  if [ -n "$total" ] && [ "$total" != "null" ]; then
    parts="total=$total"
  fi

  if [ -n "$input" ] && [ "$input" != "null" ]; then
    [ -n "$parts" ] && parts="$parts "
    parts="${parts}input=$input"
  fi

  if [ -n "$cached" ] && [ "$cached" != "null" ] && [ "$cached" != "0" ]; then
    [ -n "$parts" ] && parts="$parts "
    parts="${parts}cached=$cached"
  fi

  if [ -n "$output" ] && [ "$output" != "null" ]; then
    [ -n "$parts" ] && parts="$parts "
    parts="${parts}output=$output"
  fi

  if [ -n "$reasoning" ] && [ "$reasoning" != "null" ]; then
    [ -n "$parts" ] && parts="$parts "
    parts="${parts}reasoning=$reasoning"
  fi

  if [ -n "$cost" ] && [ "$cost" != "null" ]; then
    [ -n "$parts" ] && parts="$parts "
    parts="${parts}cost=$cost"
  fi

  printf '%s' "$parts"
}

# Small helper to keep stderr writes consistent.
progress() {
  msg="$1"
  [ -n "$msg" ] && printf '%s\n' "$msg" >&2
}

assistant_buffer=""
last_message_id=""
final_text=""
fallback_text=""

# Main loop: consume JSONL, emit progress, remember final output.
while IFS= read -r line; do
  # Ignore accidental blank lines so they do not spam stderr.
  [ -z "$line" ] && continue

  type=$(printf '%s' "$line" | jq -r 'try .type // ""' 2>/dev/null)
  if [ -z "$type" ] || [ "$type" = "null" ]; then
    progress "$(collapse "$line")"
    continue
  fi

  case "$type" in
    system)
      subtype=$(printf '%s' "$line" | jq -r 'try .subtype // ""' 2>/dev/null)
      model=$(printf '%s' "$line" | jq -r 'try .model // ""' 2>/dev/null)
      session=$(printf '%s' "$line" | jq -r 'try .session_id // ""' 2>/dev/null)

      summary="system"
      [ -n "$subtype" ] && [ "$subtype" != "null" ] && summary="$summary $subtype"
      [ -n "$model" ] && [ "$model" != "null" ] && summary="$summary (model=$model)"
      [ -n "$session" ] && [ "$session" != "null" ] && summary="$summary id=$session"

      progress "$(paint 36 "$summary")"
      ;;

    assistant)
      msg_id=$(printf '%s' "$line" | jq -r 'try .message.id // ""' 2>/dev/null)
      [ "$msg_id" = "null" ] && msg_id=""
      if [ -n "$msg_id" ] && [ "$msg_id" != "$last_message_id" ]; then
        assistant_buffer=""
      fi
      [ -n "$msg_id" ] && last_message_id="$msg_id"

      tools=$(printf '%s' "$line" | jq -c 'try [.message.content[]? | select(.type=="tool_use")][] // empty' 2>/dev/null)
      if [ -n "$tools" ]; then
        while IFS= read -r tool_line; do
          [ -z "$tool_line" ] && continue
          tool_name=$(printf '%s' "$tool_line" | jq -r 'try .name // ""' 2>/dev/null)
          [ "$tool_name" = "null" ] && tool_name=""
          tool_id=$(printf '%s' "$tool_line" | jq -r 'try .id // .tool_use_id // ""' 2>/dev/null)
          [ "$tool_id" = "null" ] && tool_id=""
          input=$(printf '%s' "$tool_line" | jq -c 'try .input // {}' 2>/dev/null)
          [ "$input" = "null" ] && input=""
          input_summary=$(collapse "$input")

          msg="tool"
          [ -n "$tool_name" ] && msg="$msg $tool_name"
          [ -n "$tool_id" ] && msg="$msg ($tool_id)"
          [ -n "$input_summary" ] && msg="$msg — $input_summary"

          progress "$(paint 34 "$msg")"
          if [ -z "$final_text" ] && [ -n "$input_summary" ]; then
            fallback_text="$input_summary"
          fi
        done <<EOF
$tools
EOF
      fi

      text=$(printf '%s' "$line" | jq -r 'try [.message.content[]? | select(.type=="text") | .text // ""] | map(select(. != "")) | join("\n\n") // ""' 2>/dev/null)
      [ "$text" = "null" ] && text=""
      usage=$(format_usage "$line")
      delta=$(printf '%s' "$line" | jq -r 'try .message.delta // false' 2>/dev/null)
      [ "$delta" = "null" ] && delta="false"

      label="assistant"
      [ "$delta" = "true" ] && label="$label (stream)"

      if [ -n "$text" ]; then
        if [ -n "$assistant_buffer" ]; then
          assistant_buffer="${assistant_buffer}\n\n${text}"
        else
          assistant_buffer="$text"
        fi
        final_text="$assistant_buffer"
        fallback_text="$assistant_buffer"

        summary=$(collapse "$text")
        msg="$label"
        [ -n "$summary" ] && msg="$msg: $summary"
        [ -n "$usage" ] && msg="$msg — $usage"

        progress "$(paint 32 "$msg")"
      elif [ -n "$usage" ]; then
        progress "$(paint 32 "$label — $usage")"
      fi
      ;;

    user)
      results=$(printf '%s' "$line" | jq -c 'try [.message.content[]? | select(.type=="tool_result")][] // empty' 2>/dev/null)
      if [ -n "$results" ]; then
        while IFS= read -r result_line; do
          [ -z "$result_line" ] && continue
          rid=$(printf '%s' "$result_line" | jq -r 'try .tool_use_id // ""' 2>/dev/null)
          [ "$rid" = "null" ] && rid=""
          is_error=$(printf '%s' "$result_line" | jq -r 'try .is_error // false' 2>/dev/null)
          [ "$is_error" = "null" ] && is_error="false"
          content=$(printf '%s' "$result_line" | jq -r 'try .content // ""' 2>/dev/null)
          [ "$content" = "null" ] && content=""
          summary=$(collapse "$content")

          color=36
          [ "$is_error" = "true" ] && color=31

          msg="tool result"
          [ -n "$rid" ] && msg="$msg ($rid)"
          [ -n "$summary" ] && msg="$msg — $summary"

          progress "$(paint "$color" "$msg")"
          if [ -z "$final_text" ] && [ -n "$summary" ]; then
            fallback_text="$summary"
          fi
        done <<EOF
$results
EOF
      else
        user_text=$(printf '%s' "$line" | jq -r 'try [.message.content[]? | select(.type=="text") | .text // ""] | map(select(. != "")) | join("\n\n") // ""' 2>/dev/null)
        [ "$user_text" = "null" ] && user_text=""
        summary=$(collapse "$user_text")
        [ -n "$summary" ] && progress "$(paint 36 "user: $summary")"
      fi
      ;;

    result)
      status=$(printf '%s' "$line" | jq -r 'try .status // .subtype // ""' 2>/dev/null)
      [ "$status" = "null" ] && status=""
      result_text=$(printf '%s' "$line" | jq -r 'try .result // ""' 2>/dev/null)
      [ "$result_text" = "null" ] && result_text=""
      usage=$(format_usage "$line")
      duration=$(printf '%s' "$line" | jq -r 'try .duration_ms // ""' 2>/dev/null)
      [ "$duration" = "null" ] && duration=""
      is_error=$(printf '%s' "$line" | jq -r 'try .is_error // false' 2>/dev/null)

      msg="run"
      [ -n "$status" ] && msg="$msg $status"
      [ -n "$usage" ] && msg="$msg — $usage"
      [ -n "$duration" ] && msg="$msg duration=${duration}ms"

      color=32
      [ "$is_error" = "true" ] && color=31
      progress "$(paint "$color" "$msg")"

      if [ -z "$final_text" ] && [ -n "$result_text" ]; then
        fallback_text="$result_text"
      fi
      ;;

    *)
      message=$(printf '%s' "$line" | jq -r 'try .message // .detail // .subtype // ""' 2>/dev/null)
      [ "$message" = "null" ] && message=""
      summary=$(collapse "$message")
      [ -z "$summary" ] && summary=$(collapse "$line")
      progress "$(paint 36 "$type: $summary")"
      if [ -z "$final_text" ] && [ -n "$summary" ]; then
        fallback_text="$summary"
      fi
      ;;
  esac
done

# Emit the assistant reply once. If Claude never sent an assistant message, use
# the best fallback we saw (tool output or result summary) so callers still get output.
if [ -n "$final_text" ]; then
  printf '%s\n' "$final_text"
elif [ -n "$fallback_text" ]; then
  printf '%s\n' "$fallback_text"
fi
